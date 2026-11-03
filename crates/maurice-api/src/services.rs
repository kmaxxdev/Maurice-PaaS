//! Handlers. State is PostgreSQL; side effects on the host go through the
//! agent. A plan is stored with a typed payload (`PlanPayload`) that `apply`
//! executes step by step while streaming events.

use crate::crypto::MasterKey;
use crate::model_map;
use crate::model_map::{env_from_proto, env_to_proto, podman_secret_name};
use crate::proto::maurice::v1::*;
use connectrpc::{
    ConnectError, ErrorCode, RequestContext, Response, ServiceRequest, ServiceResult, ServiceStream,
};
use futures::StreamExt;
use maurice_agent::{Agent, NewUser, UserRef};
use maurice_core::Env as CoreEnv;
use maurice_db::{DeploymentRow, EnvironmentRow, PlanRow, Store, WorkspaceRow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

pub struct State {
    pub agent: Arc<dyn Agent>,
    pub store: Store,
    pub master_key: MasterKey,
    /// Where workspace homes live.
    pub home_root: String,
    /// Host ports workspace blocks are carved from, inclusive.
    pub port_range: (u16, u16),
}

impl State {
    pub fn new(agent: Arc<dyn Agent>, store: Store, master_key: MasterKey) -> Self {
        Self {
            agent,
            store,
            master_key,
            home_root: "/srv/ws".into(),
            port_range: (20000, 39999),
        }
    }
}

/// Parse a `start-end` port range, both inclusive.
pub fn parse_port_range(s: &str) -> anyhow::Result<(u16, u16)> {
    let (a, b) = s
        .trim()
        .split_once('-')
        .ok_or_else(|| anyhow::anyhow!("port range must look like 20000-39999"))?;
    let (a, b): (u16, u16) = (a.trim().parse()?, b.trim().parse()?);
    anyhow::ensure!(
        a >= 1024 && b > a && (b - a + 1) >= 100,
        "port range must start at 1024 or above and hold at least 100 ports"
    );
    Ok((a, b))
}

/// What a plan does when applied. Stored as JSON in `plans.changes` next to the
/// human-readable `PlannedChange` list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanPayload {
    CreateWorkspace {
        name: String,
        /// Environments to create with the workspace, by name.
        environments: Vec<String>,
        unix_user: String,
        home: String,
        subuid_start: i64,
        subuid_len: i32,
        port_block_start: i32,
        port_block_size: i32,
    },
    /// Add an environment and its network to an existing workspace.
    AddEnvironment {
        workspace_id: Uuid,
        environment: String,
    },
    DeleteWorkspace {
        workspace_id: Uuid,
        unix_user: String,
        uid: i32,
        purge: bool,
    },
    /// Write rows and units; does not start anything.
    UpsertService {
        workspace_id: Uuid,
        environment_id: Uuid,
        environment: String,
        project_id: Uuid,
        project_name: String,
        service: maurice_db::ServiceRow,
        ports: Vec<maurice_db::PortRow>,
        env: Vec<maurice_db::EnvVarRow>,
        units: Vec<UnitText>,
        /// Unit of this service to restart if it is already running.
        running_unit: Option<String>,
    },
    /// Start or restart from the current units; for build sources, clone and build first.
    DeployService {
        workspace_id: Uuid,
        service_id: Uuid,
        unit: String,
        image: String,
        build: Option<StoredBuild>,
    },
    DeleteService {
        workspace_id: Uuid,
        service_id: Uuid,
        unit: String,
        unit_files: Vec<String>,
        purge_volumes: bool,
    },
}

/// Build inputs stored in the plan; the deploy key is looked up at apply time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredBuild {
    pub repo: String,
    pub git_ref: String,
    pub key_secret_name: Option<String>,
    pub dockerfile: String,
    pub context: String,
    /// Image name without tag.
    pub image: String,
    /// Scratch checkout name, unique per environment and service.
    pub checkout: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitText {
    pub file_name: String,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredChanges {
    payload: PlanPayload,
    changes: Vec<StoredChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredChange {
    kind: String,
    target: String,
    diff: String,
    destructive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredFinding {
    severity: String,
    code: String,
    message: String,
    target: String,
}

impl From<&maurice_core::Finding> for StoredFinding {
    fn from(f: &maurice_core::Finding) -> Self {
        Self {
            severity: match f.severity {
                maurice_core::Severity::Error => "error",
                maurice_core::Severity::Warning => "warning",
                maurice_core::Severity::Info => "info",
            }
            .into(),
            code: f.code.into(),
            message: f.message.clone(),
            target: f.target.clone(),
        }
    }
}

fn internal(e: impl std::fmt::Display) -> ConnectError {
    tracing::error!(error = %e, "internal");
    ConnectError::new(ErrorCode::Internal, e.to_string())
}

fn invalid(msg: impl Into<String>) -> ConnectError {
    ConnectError::new(ErrorCode::InvalidArgument, msg)
}

fn not_found(what: &str, id: &str) -> ConnectError {
    ConnectError::new(ErrorCode::NotFound, format!("{what} {id} not found"))
}

fn parse_uuid(s: &str, what: &str) -> Result<Uuid, ConnectError> {
    Uuid::parse_str(s).map_err(|_| invalid(format!("{what} id must be a uuid")))
}

fn env_required(e: buffa::EnumValue<Env>) -> Result<&'static str, ConnectError> {
    env_from_proto(e).ok_or_else(|| invalid("environment must be ENV_DEV, ENV_STAGING or ENV_PROD"))
}

fn core_env(name: &str) -> Result<CoreEnv, ConnectError> {
    CoreEnv::parse(name).ok_or_else(|| internal(format!("unknown environment '{name}'")))
}

fn workspace_to_proto(w: &WorkspaceRow, environments: &[EnvironmentRow]) -> Workspace {
    Workspace {
        id: w.id.to_string(),
        name: w.name.clone(),
        unix_user: w.unix_user.clone(),
        uid: w.uid as u32,
        subuid_start: w.subuid_start as u32,
        subuid_len: w.subuid_len as u32,
        home: w.home.clone(),
        environments: environments
            .iter()
            .map(|e| env_to_proto(&e.name).into())
            .collect(),
        cpu_slice: w.cpu_slice.clone().unwrap_or_default(),
        port_block_start: w.port_block_start as u32,
        port_block_size: w.port_block_size as u32,
        ..Default::default()
    }
}

fn change_kind(kind: &str) -> planned_change::Kind {
    match kind {
        "create_user" => planned_change::Kind::KIND_CREATE_USER,
        "delete_user" => planned_change::Kind::KIND_DELETE_USER,
        "write_unit" => planned_change::Kind::KIND_WRITE_UNIT,
        "remove_unit" => planned_change::Kind::KIND_REMOVE_UNIT,
        "start_unit" => planned_change::Kind::KIND_START_UNIT,
        "stop_unit" => planned_change::Kind::KIND_STOP_UNIT,
        "restart_unit" => planned_change::Kind::KIND_RESTART_UNIT,
        "publish_port" => planned_change::Kind::KIND_PUBLISH_PORT,
        "unpublish_port" => planned_change::Kind::KIND_UNPUBLISH_PORT,
        "set_secret" => planned_change::Kind::KIND_SET_SECRET,
        "build_image" => planned_change::Kind::KIND_BUILD_IMAGE,
        _ => planned_change::Kind::KIND_UNSPECIFIED,
    }
}

fn severity(s: &str) -> Severity {
    match s {
        "error" => Severity::SEVERITY_ERROR,
        "warning" => Severity::SEVERITY_WARNING,
        "info" => Severity::SEVERITY_INFO,
        _ => Severity::SEVERITY_UNSPECIFIED,
    }
}

fn plan_to_proto(p: &PlanRow) -> Result<Plan, ConnectError> {
    let stored: StoredChanges = serde_json::from_value(p.changes.clone()).map_err(internal)?;
    let findings: Vec<StoredFinding> =
        serde_json::from_value(p.findings.clone()).map_err(internal)?;
    Ok(Plan {
        id: p.id.to_string(),
        workspace_id: if p.workspace_id.is_nil() {
            String::new()
        } else {
            p.workspace_id.to_string()
        },
        changes: stored
            .changes
            .iter()
            .map(|c| PlannedChange {
                kind: change_kind(&c.kind).into(),
                target: c.target.clone(),
                diff: c.diff.clone(),
                destructive: c.destructive,
                ..Default::default()
            })
            .collect(),
        findings: findings
            .iter()
            .map(|f| Finding {
                severity: severity(&f.severity).into(),
                code: f.code.clone(),
                message: f.message.clone(),
                target: f.target.clone(),
                ..Default::default()
            })
            .collect(),
        destructive: p.destructive,
        created_by: p.created_by.clone(),
        ..Default::default()
    })
}

async fn store_plan(
    state: &State,
    workspace_id: Uuid,
    payload: PlanPayload,
    changes: Vec<StoredChange>,
    findings: Vec<StoredFinding>,
) -> Result<Plan, ConnectError> {
    let destructive = changes.iter().any(|c| c.destructive);
    let now = OffsetDateTime::now_utc();
    let row = PlanRow {
        id: Uuid::now_v7(),
        workspace_id,
        changes: serde_json::to_value(StoredChanges { payload, changes }).map_err(internal)?,
        findings: serde_json::to_value(findings).map_err(internal)?,
        destructive,
        created_by: "anonymous".into(),
        created_at: now,
        expires_at: now + Duration::hours(1),
        applied_at: None,
        discarded_at: None,
    };
    state.store.insert_plan(&row).await.map_err(internal)?;
    plan_to_proto(&row)
}

/// Line-based diff for unit files: enough to show what changes on disk.
fn unit_diff(old: Option<&str>, new: &str) -> String {
    let Some(old) = old else {
        return new
            .lines()
            .map(|l| format!("+{l}"))
            .collect::<Vec<_>>()
            .join("\n");
    };
    if old == new {
        return String::new();
    }
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut out = Vec::new();
    for l in &old_lines {
        if !new_lines.contains(l) {
            out.push(format!("-{l}"));
        }
    }
    for l in &new_lines {
        if !old_lines.contains(l) {
            out.push(format!("+{l}"));
        }
    }
    out.join("\n")
}

/// A secret as the handlers see it: its scope (environment name, `None` = shared) and name.
#[derive(Debug, Clone)]
struct SecretRef {
    environment: Option<String>,
    name: String,
}

/// Everything the handlers need about one workspace, loaded in one go.
struct Loaded {
    ws: WorkspaceRow,
    user: UserRef,
    environments: Vec<EnvironmentRow>,
    projects: Vec<maurice_db::ProjectRow>,
    services: Vec<maurice_db::ServiceFull>,
    /// Every secret id referenced anywhere → scope and name.
    secret_refs: HashMap<Uuid, SecretRef>,
    /// Latest version of each secret by (environment id, name); `None` = shared.
    secret_ids: HashMap<(Option<Uuid>, String), Uuid>,
}

impl Loaded {
    fn environment(&self, name: &str) -> Option<&EnvironmentRow> {
        self.environments.iter().find(|e| e.name == name)
    }
    fn environment_name(&self, id: Uuid) -> Option<&str> {
        self.environments
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.name.as_str())
    }
    /// Plain secret name, for display.
    fn secret_name(&self) -> impl Fn(Uuid) -> Option<String> + '_ {
        move |id| self.secret_refs.get(&id).map(|r| r.name.clone())
    }
    /// Podman secret name, for units.
    fn secret_podman_name(&self) -> impl Fn(Uuid) -> Option<String> + '_ {
        move |id| {
            self.secret_refs
                .get(&id)
                .map(|r| podman_secret_name(r.environment.as_deref(), &r.name))
        }
    }
    /// Resolve a secret name from inside an environment: that environment first, then shared.
    fn secret_id(&self, environment_id: Uuid, name: &str) -> Option<Uuid> {
        self.secret_ids
            .get(&(Some(environment_id), name.to_string()))
            .or_else(|| self.secret_ids.get(&(None, name.to_string())))
            .copied()
    }
    fn shared_secret_id(&self, name: &str) -> Option<Uuid> {
        self.secret_ids.get(&(None, name.to_string())).copied()
    }
    fn core(&self) -> maurice_core::Workspace {
        model_map::workspace_to_core(
            &self.ws,
            &self.environments,
            &self.projects,
            &self.services,
            &self.secret_podman_name(),
        )
    }
}

async fn load(state: &State, workspace_id: Uuid) -> Result<Loaded, ConnectError> {
    let Some(ws) = state
        .store
        .get_workspace(workspace_id)
        .await
        .map_err(internal)?
    else {
        return Err(not_found("workspace", &workspace_id.to_string()));
    };
    let environments = state
        .store
        .list_environments(ws.id)
        .await
        .map_err(internal)?;
    let projects = state.store.list_projects(ws.id).await.map_err(internal)?;
    let services = state.store.list_services(ws.id).await.map_err(internal)?;
    let secrets = state.store.list_secrets(ws.id).await.map_err(internal)?;
    let env_name = |id: Option<Uuid>| {
        id.and_then(|id| environments.iter().find(|e| e.id == id))
            .map(|e| e.name.clone())
    };
    let mut secret_refs = HashMap::new();
    let mut secret_ids = HashMap::new();
    for s in &secrets {
        secret_refs.insert(
            s.id,
            SecretRef {
                environment: env_name(s.environment_id),
                name: s.name.clone(),
            },
        );
        secret_ids.insert((s.environment_id, s.name.clone()), s.id);
    }
    // Env rows may point at older versions of a secret; map those ids too.
    for svc in &services {
        for e in &svc.env {
            if let Some(sid) = e.secret_id
                && !secret_refs.contains_key(&sid)
                && let Some(row) = state.store.get_secret(sid).await.map_err(internal)?
            {
                secret_refs.insert(
                    sid,
                    SecretRef {
                        environment: env_name(row.environment_id),
                        name: row.name,
                    },
                );
            }
        }
    }
    let user = UserRef {
        name: ws.unix_user.clone(),
        uid: ws.uid as u32,
    };
    Ok(Loaded {
        ws,
        user,
        environments,
        projects,
        services,
        secret_refs,
        secret_ids,
    })
}

/// Render every unit of a service against the full workspace model.
fn render_units(
    ws: &maurice_core::Workspace,
    env: CoreEnv,
    svc: &maurice_core::Service,
    image: &str,
    version: &str,
    enabled: bool,
) -> Result<Vec<UnitText>, ConnectError> {
    let mut units = Vec::new();
    for v in maurice_quadlet::render_volumes(env, svc) {
        units.push(UnitText {
            file_name: v.file_name,
            content: v.content,
        });
    }
    let unit = maurice_quadlet::render_container(&maurice_quadlet::RenderInput {
        workspace: ws,
        env,
        service: svc,
        image,
        version,
        resolve_ref: &|r| resolve_ref(ws, env, r),
        enabled,
    })
    .map_err(|e| invalid(e.to_string()))?;
    units.push(UnitText {
        file_name: unit.file_name,
        content: unit.content,
    });
    Ok(units)
}

/// `{{svc.host}}`, `{{svc.port}}`, and `{{svc.url}}` for well-known images, resolved
/// inside one environment.
fn resolve_ref(ws: &maurice_core::Workspace, env: CoreEnv, r: &str) -> Option<String> {
    let (svc_name, field) = maurice_core::validate::parse_ref(r)?;
    let svc = ws
        .environment(env)?
        .services()
        .find(|s| s.name == svc_name)?;
    let port = svc.ports.first().map(|p| p.container_port);
    match field {
        "host" => Some(svc.name.clone()),
        "port" => port.map(|p| p.to_string()),
        "url" => {
            let image = match &svc.source {
                maurice_core::Source::Image { reference } => reference.as_str(),
                _ => "",
            };
            if image.contains("postgres") {
                Some(format!(
                    "postgres://postgres@{}:{}/postgres",
                    svc.name,
                    port.unwrap_or(5432)
                ))
            } else if image.contains("dragonfly")
                || image.contains("valkey")
                || image.contains("redis")
            {
                Some(format!("redis://{}:{}", svc.name, port.unwrap_or(6379)))
            } else {
                port.map(|p| format!("http://{}:{p}", svc.name))
            }
        }
        _ => None,
    }
}

/// Image a service runs: its reference, or a placeholder tag for build sources that
/// the first deploy replaces with the built image.
fn image_of(svc: &maurice_core::Service, ws_name: &str, env: CoreEnv) -> String {
    match &svc.source {
        maurice_core::Source::Image { reference } => reference.clone(),
        maurice_core::Source::Build { .. } => {
            format!("{}:pending", build_image_name(ws_name, env, &svc.name))
        }
    }
}

/// Image name (without tag) for a service built from a Dockerfile.
fn build_image_name(ws_name: &str, env: CoreEnv, service: &str) -> String {
    format!("localhost/{ws_name}/{env}/{service}")
}

/// Workspace without its services: enough to render network units.
fn core_workspace_shell(ws: &WorkspaceRow) -> maurice_core::Workspace {
    maurice_core::Workspace {
        name: ws.name.clone(),
        unix_user: ws.unix_user.clone(),
        uid: ws.uid as u32,
        subuid_start: ws.subuid_start as u32,
        subuid_len: ws.subuid_len as u32,
        home: ws.home.clone(),
        cpu_slice: ws.cpu_slice.clone(),
        port_block: model_map::port_block_of(ws),
        environments: vec![],
    }
}

/// Parse and de-duplicate requested environments; empty means dev and prod.
fn requested_environments(envs: &[buffa::EnumValue<Env>]) -> Result<Vec<CoreEnv>, ConnectError> {
    if envs.is_empty() {
        return Ok(vec![CoreEnv::Dev, CoreEnv::Prod]);
    }
    let mut out: Vec<CoreEnv> = Vec::new();
    for e in envs {
        let name = env_required(*e)?;
        let env = core_env(name)?;
        if !out.contains(&env) {
            out.push(env);
        }
    }
    out.sort();
    Ok(out)
}

/// Health checks for well-known images so `Notify=healthy` works out of the box.
fn default_health_cmd(image: Option<&str>) -> Option<String> {
    let i = image?;
    if i.contains("postgres") {
        Some("pg_isready -U postgres".into())
    } else if i.contains("dragonfly") || i.contains("valkey") || i.contains("redis") {
        Some("redis-cli ping".into())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Workspaces
// ---------------------------------------------------------------------------

pub struct WorkspacesSvc {
    pub state: Arc<State>,
}

#[allow(refining_impl_trait)]
impl Workspaces for WorkspacesSvc {
    async fn list(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, ListWorkspacesRequest>,
    ) -> ServiceResult<ListWorkspacesResponse> {
        let rows = self.state.store.list_workspaces().await.map_err(internal)?;
        let mut out = Vec::with_capacity(rows.len());
        for w in &rows {
            let envs = self
                .state
                .store
                .list_environments(w.id)
                .await
                .map_err(internal)?;
            out.push(workspace_to_proto(w, &envs));
        }
        Response::ok(ListWorkspacesResponse {
            workspaces: out,
            ..Default::default()
        })
    }

    async fn get(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, GetWorkspaceRequest>,
    ) -> ServiceResult<Workspace> {
        let id = parse_uuid(req.id, "workspace")?;
        match self.state.store.get_workspace(id).await.map_err(internal)? {
            Some(w) => {
                let envs = self
                    .state
                    .store
                    .list_environments(w.id)
                    .await
                    .map_err(internal)?;
                Response::ok(workspace_to_proto(&w, &envs))
            }
            None => Err(not_found("workspace", req.id)),
        }
    }

    async fn get_ports(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, GetPortsRequest>,
    ) -> ServiceResult<WorkspacePorts> {
        let wid = parse_uuid(req.workspace_id, "workspace")?;
        let loaded = load(&self.state, wid).await?;
        let block = model_map::port_block_of(&loaded.ws);
        let mut used: Vec<UsedPort> = Vec::new();
        for s in &loaded.services {
            for p in s.ports.iter().filter(|p| p.published) {
                used.push(UsedPort {
                    host_ip: p.host_ip.clone().unwrap_or_default(),
                    host_port: p.host_port.unwrap_or(0) as u32,
                    environment: env_to_proto(&s.environment).into(),
                    service: s.service.name.clone(),
                    ..Default::default()
                });
            }
        }
        used.sort_by_key(|u| u.host_port);
        let taken: std::collections::HashSet<u32> = used.iter().map(|u| u.host_port).collect();
        let next_free = (block.start..=block.end())
            .map(u32::from)
            .find(|p| !taken.contains(p))
            .unwrap_or(0);
        Response::ok(WorkspacePorts {
            block_start: block.start as u32,
            block_size: block.size as u32,
            used,
            next_free,
            ..Default::default()
        })
    }

    async fn plan_add_environment(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, PlanAddEnvironmentRequest>,
    ) -> ServiceResult<Plan> {
        let wid = parse_uuid(req.workspace_id, "workspace")?;
        let loaded = load(&self.state, wid).await?;
        let name = env_required(req.environment)?;
        let env = core_env(name)?;
        let mut findings = Vec::new();
        if loaded.environment(name).is_some() {
            findings.push(StoredFinding {
                severity: "error".into(),
                code: "environment.exists".into(),
                message: format!(
                    "workspace '{}' already has a {name} environment",
                    loaded.ws.name
                ),
                target: format!("environment:{name}"),
            });
        }
        let net = maurice_quadlet::render_network(&core_workspace_shell(&loaded.ws), env);
        let changes = vec![StoredChange {
            kind: "write_unit".into(),
            target: format!("{}/{}", loaded.ws.unix_user, net.file_name),
            diff: unit_diff(None, &net.content),
            destructive: false,
        }];
        let payload = PlanPayload::AddEnvironment {
            workspace_id: wid,
            environment: name.into(),
        };
        Response::ok(store_plan(&self.state, wid, payload, changes, findings).await?)
    }

    async fn plan_create(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, PlanCreateWorkspaceRequest>,
    ) -> ServiceResult<Plan> {
        let name = req.name.to_string();
        let environments = requested_environments(&req.environments)?;
        let mut findings = Vec::new();
        if !maurice_core::validate::is_dns_label(&name) || name.len() > 28 {
            findings.push(StoredFinding {
                severity: "error".into(),
                code: "workspace.invalid_name".into(),
                message:
                    "workspace name must be lowercase letters, digits and dashes, at most 28 chars"
                        .into(),
                target: format!("workspace:{name}"),
            });
        }
        if self
            .state
            .store
            .get_workspace_by_name(&name)
            .await
            .map_err(internal)?
            .is_some()
        {
            findings.push(StoredFinding {
                severity: "error".into(),
                code: "workspace.exists".into(),
                message: format!("workspace '{name}' already exists"),
                target: format!("workspace:{name}"),
            });
        }
        let subuid_len = 65536;
        let subuid_start = self
            .state
            .store
            .next_subuid_start(subuid_len as i64)
            .await
            .map_err(internal)?;
        let unix_user = format!("ws-{name}");
        let home = format!("{}/{name}", self.state.home_root);
        // Port block: the requested size from the host range, first free aligned slot.
        let block_size = if req.port_block_size == 0 {
            maurice_core::PortBlock::DEFAULT_SIZE
        } else {
            u16::try_from(req.port_block_size)
                .ok()
                .filter(|s| maurice_core::PortBlock::SIZES.contains(s))
                .ok_or_else(|| invalid("port_block_size must be 100, 500 or 1000"))?
        };
        let held = self
            .state
            .store
            .list_port_blocks()
            .await
            .map_err(internal)?;
        let (range_start, range_end) = self.state.port_range;
        let block_start = maurice_db::next_port_block(
            &held,
            (range_start as i32, range_end as i32),
            block_size as i32,
        );
        if block_start.is_none() {
            findings.push(StoredFinding {
                severity: "error".into(),
                code: "ports.exhausted".into(),
                message: format!(
                    "no free block of {block_size} ports left in {range_start}-{range_end}; free a workspace or widen MAURICE_PORT_RANGE"
                ),
                target: format!("workspace:{name}"),
            });
        }
        let port_block = maurice_core::PortBlock {
            start: block_start.unwrap_or(0) as u16,
            size: block_size,
        };
        let mut changes = vec![StoredChange {
            kind: "create_user".into(),
            target: format!(
                "{unix_user} (uid auto, subuid {subuid_start}+{subuid_len}, ports {port_block}, home {home})"
            ),
            diff: String::new(),
            destructive: false,
        }];
        let shell = maurice_core::Workspace {
            name: name.clone(),
            unix_user: unix_user.clone(),
            uid: 0,
            subuid_start: subuid_start as u32,
            subuid_len: subuid_len as u32,
            home: home.clone(),
            cpu_slice: None,
            port_block,
            environments: vec![],
        };
        for env in &environments {
            let net = maurice_quadlet::render_network(&shell, *env);
            changes.push(StoredChange {
                kind: "write_unit".into(),
                target: format!("{unix_user}/{}", net.file_name),
                diff: unit_diff(None, &net.content),
                destructive: false,
            });
        }
        let payload = PlanPayload::CreateWorkspace {
            name,
            environments: environments.iter().map(|e| e.to_string()).collect(),
            unix_user,
            home,
            subuid_start,
            subuid_len,
            port_block_start: port_block.start as i32,
            port_block_size: port_block.size as i32,
        };
        Response::ok(store_plan(&self.state, Uuid::nil(), payload, changes, findings).await?)
    }

    async fn plan_delete(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, PlanDeleteWorkspaceRequest>,
    ) -> ServiceResult<Plan> {
        let id = parse_uuid(req.id, "workspace")?;
        let Some(w) = self.state.store.get_workspace(id).await.map_err(internal)? else {
            return Err(not_found("workspace", req.id));
        };
        let changes = vec![StoredChange {
            kind: "delete_user".into(),
            target: format!(
                "{}{}",
                w.unix_user,
                if req.purge {
                    " (purge home, containers, volumes)"
                } else {
                    ""
                }
            ),
            diff: String::new(),
            destructive: true,
        }];
        let payload = PlanPayload::DeleteWorkspace {
            workspace_id: w.id,
            unix_user: w.unix_user.clone(),
            uid: w.uid,
            purge: req.purge,
        };
        Response::ok(store_plan(&self.state, w.id, payload, changes, vec![]).await?)
    }
}

// ---------------------------------------------------------------------------
// Services
// ---------------------------------------------------------------------------

pub struct ServicesSvc {
    pub state: Arc<State>,
}

/// A service row with the names the handlers address it by on the host.
struct Target {
    full: maurice_db::ServiceFull,
    env: CoreEnv,
    /// `prod-web`
    stem: String,
    /// `prod-web.service`
    unit: String,
}

impl ServicesSvc {
    async fn service_and_workspace(&self, id: &str) -> Result<(Target, Loaded), ConnectError> {
        let sid = parse_uuid(id, "service")?;
        let Some(full) = self.state.store.get_service(sid).await.map_err(internal)? else {
            return Err(not_found("service", id));
        };
        let loaded = load(&self.state, full.project.workspace_id).await?;
        let env = core_env(&full.environment)?;
        let stem = maurice_quadlet::unit_stem(env, &full.service.name);
        let unit = format!("{stem}.service");
        Ok((
            Target {
                full,
                env,
                stem,
                unit,
            },
            loaded,
        ))
    }
}

#[allow(refining_impl_trait)]
impl Services for ServicesSvc {
    async fn get_status(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, GetStatusRequest>,
    ) -> ServiceResult<ServiceStatus> {
        let (t, loaded) = self.service_and_workspace(req.service_id).await?;
        let st = self
            .state
            .agent
            .unit_status(&loaded.user, &t.unit)
            .await
            .map_err(internal)?;
        let dep = self
            .state
            .store
            .latest_deployment(t.full.service.id)
            .await
            .map_err(internal)?;
        Response::ok(ServiceStatus {
            service_id: t.full.service.id.to_string(),
            active_state: st.active_state,
            sub_state: st.sub_state,
            since: match st.since_unix {
                Some(t) => buffa_types::Timestamp {
                    seconds: t as i64,
                    nanos: 0,
                    ..Default::default()
                }
                .into(),
                None => buffa::MessageField::none(),
            },
            deployment_id: dep.as_ref().map(|d| d.id.to_string()).unwrap_or_default(),
            image: dep.map(|d| d.image_digest).unwrap_or_default(),
            ..Default::default()
        })
    }

    async fn list(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, ListServicesRequest>,
    ) -> ServiceResult<ListServicesResponse> {
        let wid = parse_uuid(req.workspace_id, "workspace")?;
        let loaded = load(&self.state, wid).await?;
        let name = loaded.secret_name();
        let only = env_from_proto(req.environment);
        Response::ok(ListServicesResponse {
            services: loaded
                .services
                .iter()
                .filter(|s| only.is_none_or(|e| s.environment == e))
                .map(|s| model_map::service_to_proto(s, &name))
                .collect(),
            ..Default::default()
        })
    }

    async fn get(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, GetServiceRequest>,
    ) -> ServiceResult<Service> {
        let (t, loaded) = self.service_and_workspace(req.id).await?;
        Response::ok(model_map::service_to_proto(&t.full, &loaded.secret_name()))
    }

    async fn get_unit(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, GetUnitRequest>,
    ) -> ServiceResult<UnitFile> {
        let (t, loaded) = self.service_and_workspace(req.service_id).await?;
        let file_name = format!("{}.container", t.stem);
        let content = self
            .state
            .agent
            .read_unit(&loaded.user, &file_name)
            .await
            .map_err(internal)?
            .unwrap_or_default();
        let deployment_id = self
            .state
            .store
            .latest_deployment(t.full.service.id)
            .await
            .map_err(internal)?
            .map(|d| d.id.to_string())
            .unwrap_or_default();
        Response::ok(UnitFile {
            path: format!("{}/.config/containers/systemd/{file_name}", loaded.ws.home),
            content,
            deployment_id,
            ..Default::default()
        })
    }

    async fn logs(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, LogsRequest>,
    ) -> ServiceResult<ServiceStream<LogLine>> {
        let (t, loaded) = self.service_and_workspace(req.service_id).await?;
        let unit = t.unit;
        let tail = if req.tail == 0 { 200 } else { req.tail };
        let since = Some(loaded.ws.created_at.unix_timestamp() as u64);
        let agent = self.state.agent.clone();
        let user = loaded.user.clone();
        let follow = req.follow;
        let first = agent
            .read_journal(&user, &unit, tail, since, None)
            .await
            .map_err(internal)?;
        let to_line = {
            let unit = unit.clone();
            move |l: maurice_agent::JournalLine| LogLine {
                time: buffa_types::Timestamp {
                    seconds: (l.realtime_usec / 1_000_000) as i64,
                    nanos: ((l.realtime_usec % 1_000_000) * 1000) as i32,
                    ..Default::default()
                }
                .into(),
                unit: unit.clone(),
                message: l.message,
                priority: l.priority as u32,
                ..Default::default()
            }
        };
        let last_cursor = first.last().map(|l| l.cursor.clone());
        let initial: Vec<LogLine> = first.into_iter().map(&to_line).collect();
        if !follow {
            return Response::stream_ok(futures::stream::iter(initial.into_iter().map(Ok)));
        }
        // Follow: poll the journal after the last cursor until the client goes away
        // (dropping the stream stops the loop).
        let polled = futures::stream::unfold(
            (last_cursor, agent, user, unit, to_line),
            |(cursor, agent, user, unit, to_line)| async move {
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                let lines = match agent
                    .read_journal(&user, &unit, 500, None, cursor.as_deref())
                    .await
                {
                    Ok(l) => l,
                    Err(e) => {
                        return Some((
                            vec![Err(ConnectError::new(ErrorCode::Internal, e.to_string()))],
                            (cursor, agent, user, unit, to_line),
                        ));
                    }
                };
                let next = lines.last().map(|l| l.cursor.clone()).or(cursor);
                let out: Vec<Result<LogLine, ConnectError>> =
                    lines.into_iter().map(|l| Ok(to_line(l))).collect();
                Some((out, (next, agent, user, unit, to_line)))
            },
        )
        .flat_map(futures::stream::iter);
        Response::stream_ok(futures::stream::iter(initial.into_iter().map(Ok)).chain(polled))
    }

    async fn stats(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, StatsRequest>,
    ) -> ServiceResult<ServiceStream<StatsSample>> {
        let (t, loaded) = self.service_and_workspace(req.service_id).await?;
        let agent = self.state.agent.clone();
        let user = loaded.user.clone();
        let name = t.stem;
        // One sample every two seconds until the client disconnects.
        // CPU is derived from consecutive cpu_time samples: podman's one-shot percentage is
        // averaged since the container started and would draw a flat line.
        let stream = futures::stream::unfold(
            (agent, user, name, true, None::<(f64, std::time::Instant)>),
            |(agent, user, name, first, prev)| async move {
                if !first {
                    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
                }
                let now = std::time::Instant::now();
                let mut next_prev = None;
                let sample = match agent.container_stats(&user, &name).await {
                    Ok(Some(st)) => {
                        let cores = match prev {
                            Some((t0, i0)) if now > i0 && st.cpu_time_secs >= t0 => {
                                (st.cpu_time_secs - t0) / (now - i0).as_secs_f64()
                            }
                            _ => st.cpu_percent / 100.0,
                        };
                        next_prev = Some((st.cpu_time_secs, now));
                        Ok(StatsSample {
                            time: buffa_types::Timestamp {
                                seconds: OffsetDateTime::now_utc().unix_timestamp(),
                                nanos: 0,
                                ..Default::default()
                            }
                            .into(),
                            cpu_cores: cores,
                            memory_bytes: st.mem_bytes,
                            net_rx_bytes: st.net_rx_bytes,
                            net_tx_bytes: st.net_tx_bytes,
                            ..Default::default()
                        })
                    }
                    // Not running: an empty sample keeps the stream alive so the UI can show "stopped".
                    Ok(None) => Ok(StatsSample {
                        time: buffa_types::Timestamp {
                            seconds: OffsetDateTime::now_utc().unix_timestamp(),
                            nanos: 0,
                            ..Default::default()
                        }
                        .into(),
                        ..Default::default()
                    }),
                    Err(e) => Err(ConnectError::new(ErrorCode::Internal, e.to_string())),
                };
                Some((sample, (agent, user, name, false, next_prev)))
            },
        );
        Response::stream_ok(stream)
    }

    async fn plan_upsert(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, PlanUpsertServiceRequest>,
    ) -> ServiceResult<Plan> {
        let wid = parse_uuid(req.workspace_id, "workspace")?;
        let loaded = load(&self.state, wid).await?;
        let owned: PlanUpsertServiceRequest = req.to_owned_message();
        let Some(svc) = owned.service.into_option() else {
            return Err(invalid("service is required"));
        };
        let project_name = if req.project.is_empty() {
            "default".to_string()
        } else {
            req.project.to_string()
        };
        if !maurice_core::validate::is_dns_label(&project_name) {
            return Err(invalid("project must be a DNS label"));
        }
        // Environment: the request, the definition, or the one an existing service of
        // this name lives in when that is unambiguous.
        let env_name = match env_from_proto(req.environment).or(env_from_proto(svc.environment)) {
            Some(e) => e.to_string(),
            None => {
                let mut homes: Vec<&str> = loaded
                    .services
                    .iter()
                    .filter(|s| s.service.name == svc.name)
                    .map(|s| s.environment.as_str())
                    .collect();
                homes.dedup();
                match homes.as_slice() {
                    [one] => one.to_string(),
                    [] => return Err(invalid("environment is required for a new service")),
                    _ => {
                        return Err(invalid(format!(
                            "service '{}' exists in several environments; say which one",
                            svc.name
                        )));
                    }
                }
            }
        };
        let env = core_env(&env_name)?;
        let Some(env_row) = loaded.environment(&env_name) else {
            return Err(invalid(format!(
                "workspace '{}' has no {env_name} environment; add it first",
                loaded.ws.name
            )));
        };
        let environment_id = env_row.id;
        // An existing service with this name in this environment keeps its id so the
        // upsert replaces it.
        let existing = loaded
            .services
            .iter()
            .find(|s| s.service.name == svc.name && s.service.environment_id == environment_id);
        let service_id = existing.map(|s| s.service.id).unwrap_or_else(Uuid::now_v7);
        let project_id = match existing {
            Some(s) => s.project.id,
            None => loaded
                .projects
                .iter()
                .find(|p| p.name == project_name)
                .map(|p| p.id)
                .unwrap_or_else(Uuid::now_v7),
        };
        let secret_ids = |n: &str| loaded.secret_id(environment_id, n);
        let (mut row, ports, env_vars) = model_map::service_from_proto(
            &svc,
            service_id,
            environment_id,
            project_id,
            &secret_ids,
        )
        .map_err(|e| invalid(e.to_string()))?;
        if let Some(e) = existing {
            row.created_at = e.service.created_at;
            row.health_cmd = e.service.health_cmd.clone();
        }
        if row.health_cmd.is_none() {
            row.health_cmd = default_health_cmd(row.image_ref.as_deref());
        }

        // Validate the workspace as it would be after the change.
        let mut core = loaded.core();
        let new_full = maurice_db::ServiceFull {
            service: row.clone(),
            project: maurice_db::ProjectRow {
                id: project_id,
                workspace_id: wid,
                name: project_name.clone(),
                git_repo: None,
                git_ref: "main".into(),
                git_key_secret_id: None,
            },
            environment: env_name.clone(),
            ports: ports.clone(),
            env: env_vars.clone(),
        };
        let core_svc = model_map::service_to_core(&new_full, &loaded.secret_podman_name());
        let core_env_model = match core.environments.iter_mut().find(|e| e.env == env) {
            Some(e) => e,
            None => {
                core.environments.push(maurice_core::Environment {
                    env,
                    projects: vec![],
                });
                core.environments.last_mut().expect("just pushed")
            }
        };
        for p in &mut core_env_model.projects {
            p.services.retain(|s| s.name != core_svc.name);
        }
        match core_env_model
            .projects
            .iter_mut()
            .find(|p| p.name == project_name)
        {
            Some(p) => p.services.push(core_svc.clone()),
            None => core_env_model.projects.push(maurice_core::Project {
                name: project_name.clone(),
                git_repo: None,
                services: vec![core_svc.clone()],
            }),
        }
        // Only findings about this service (or host ports) block its plan; problems of
        // other services are theirs to fix and must not wedge the whole workspace.
        let svc_tag = format!(":{}", row.name);
        let who = format!("'{env}/{}'", row.name);
        let findings: Vec<StoredFinding> = maurice_core::validate_workspace(&core)
            .iter()
            .filter(|f| {
                f.target == format!("service:{}", row.name)
                    || f.target.starts_with(&format!("env:{}/", row.name))
                    || f.target.starts_with(&format!("port:{}/", row.name))
                    || f.code == "port.conflict"
                    || (f.code.starts_with("port.") && f.message.contains(&who))
                    || f.target.ends_with(&svc_tag)
            })
            .map(StoredFinding::from)
            .collect();

        // Host ports are global: refuse bindings already published by another service anywhere.
        let mut findings = findings;
        for pr in self
            .state
            .store
            .list_published_ports()
            .await
            .map_err(internal)?
        {
            // Conflicts inside this workspace are the validator's findings above.
            if pr.workspace == loaded.ws.name {
                continue;
            }
            for mine in ports.iter().filter(|p| p.published) {
                if mine.host_ip == pr.host_ip && mine.host_port == pr.host_port {
                    let bind = format!(
                        "{}:{}",
                        pr.host_ip.clone().unwrap_or_default(),
                        pr.host_port.unwrap_or(0)
                    );
                    findings.push(StoredFinding {
                        severity: "error".into(),
                        code: "port.conflict".into(),
                        message: format!(
                            "{bind} is already published by {}/{}/{}",
                            pr.workspace, pr.environment, pr.service
                        ),
                        target: format!("port:{bind}"),
                    });
                }
            }
        }

        // Render and diff against disk.
        let image = image_of(&core_svc, &core.name, env);
        let stem = maurice_quadlet::unit_stem(env, &row.name);
        let unit = format!("{stem}.service");
        let deployed = match existing {
            Some(e) => {
                let active = self
                    .state
                    .agent
                    .unit_status(&loaded.user, &unit)
                    .await
                    .map(|s| s.active_state == "active")
                    .unwrap_or(false);
                active
                    || self
                        .state
                        .store
                        .latest_deployment(e.service.id)
                        .await
                        .map_err(internal)?
                        .is_some()
            }
            None => false,
        };
        // With validation errors the plan cannot be applied anyway; an unresolved
        // reference is then reported as a finding, not as a request error.
        let units = match render_units(
            &core,
            env,
            &core_svc,
            &image,
            &maurice_quadlet::image_version(&image),
            deployed,
        ) {
            Ok(units) => units,
            Err(_) if findings.iter().any(|f| f.severity == "error") => vec![],
            Err(e) => return Err(e),
        };
        let mut changes = Vec::new();
        for u in &units {
            let on_disk = self
                .state
                .agent
                .read_unit(&loaded.user, &u.file_name)
                .await
                .map_err(internal)?;
            let diff = unit_diff(on_disk.as_deref(), &u.content);
            if !diff.is_empty() {
                changes.push(StoredChange {
                    kind: "write_unit".into(),
                    target: format!("{}/{}", loaded.ws.unix_user, u.file_name),
                    diff,
                    destructive: false,
                });
            }
        }
        let running = existing.is_some()
            && self
                .state
                .agent
                .unit_status(&loaded.user, &unit)
                .await
                .map(|s| s.active_state == "active")
                .unwrap_or(false);
        if running && !changes.is_empty() {
            changes.push(StoredChange {
                kind: "restart_unit".into(),
                target: unit.clone(),
                diff: String::new(),
                destructive: false,
            });
        }
        if changes.is_empty() {
            changes.push(StoredChange {
                kind: "write_unit".into(),
                target: format!("{} (definition only, units unchanged)", row.name),
                diff: String::new(),
                destructive: false,
            });
        }
        let payload = PlanPayload::UpsertService {
            workspace_id: wid,
            environment_id,
            environment: env_name,
            project_id,
            project_name,
            service: row,
            ports,
            env: env_vars,
            units,
            running_unit: running.then_some(unit),
        };
        Response::ok(store_plan(&self.state, wid, payload, changes, findings).await?)
    }

    async fn plan_deploy(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, PlanDeployRequest>,
    ) -> ServiceResult<Plan> {
        let (t, loaded) = self.service_and_workspace(req.service_id).await?;
        let full = &t.full;
        let core = loaded.core();
        let core_svc = core
            .environment(t.env)
            .and_then(|e| e.services().find(|s| s.name == full.service.name))
            .ok_or_else(|| internal("service missing from model"))?;
        let mut findings: Vec<StoredFinding> = maurice_core::validate_workspace(&core)
            .iter()
            .map(StoredFinding::from)
            .collect();
        let unit = t.unit.clone();
        let running = self
            .state
            .agent
            .unit_status(&loaded.user, &unit)
            .await
            .map(|s| s.active_state == "active")
            .unwrap_or(false);
        let mut changes = Vec::new();
        let mut build = None;
        let image = match &core_svc.source {
            maurice_core::Source::Image { reference } => reference.clone(),
            maurice_core::Source::Build {
                dockerfile,
                context,
            } => {
                let image = build_image_name(&core.name, t.env, &core_svc.name);
                match full.project.git_repo.as_deref().filter(|r| !r.is_empty()) {
                    None => findings.push(StoredFinding {
                        severity: "error".into(),
                        code: "deploy.no_repository".into(),
                        message: format!(
                            "project '{}' has no git repository; set one with `marik project set`",
                            full.project.name
                        ),
                        target: format!("project:{}", full.project.name),
                    }),
                    Some(repo) => {
                        let key_secret_name = full
                            .project
                            .git_key_secret_id
                            .map(|id| loaded.secret_name()(id).unwrap_or_default());
                        changes.push(StoredChange {
                            kind: "build_image".into(),
                            target: format!("{repo}@{} → {image}:<sha>", full.project.git_ref),
                            diff: String::new(),
                            destructive: false,
                        });
                        changes.push(StoredChange {
                            kind: "write_unit".into(),
                            target: format!(
                                "{}/{}.container (Image= and version)",
                                loaded.ws.unix_user, t.stem
                            ),
                            diff: String::new(),
                            destructive: false,
                        });
                        build = Some(StoredBuild {
                            repo: repo.to_string(),
                            git_ref: full.project.git_ref.clone(),
                            key_secret_name,
                            dockerfile: dockerfile.clone(),
                            context: context.clone(),
                            image: image.clone(),
                            checkout: t.stem.clone(),
                        });
                    }
                }
                image
            }
        };
        changes.push(StoredChange {
            kind: if running {
                "restart_unit"
            } else {
                "start_unit"
            }
            .into(),
            target: format!("{unit} ← {image}"),
            diff: String::new(),
            destructive: false,
        });
        let payload = PlanPayload::DeployService {
            workspace_id: loaded.ws.id,
            service_id: full.service.id,
            unit,
            image,
            build,
        };
        Response::ok(store_plan(&self.state, loaded.ws.id, payload, changes, findings).await?)
    }

    async fn plan_env_change(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, PlanEnvChangeRequest>,
    ) -> ServiceResult<Plan> {
        Err(ConnectError::new(
            ErrorCode::Unimplemented,
            "use PlanUpsert with the full definition for now",
        ))
    }

    async fn plan_publish_port(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, PlanPublishPortRequest>,
    ) -> ServiceResult<Plan> {
        Err(ConnectError::new(
            ErrorCode::Unimplemented,
            "use PlanUpsert with the full definition for now",
        ))
    }

    async fn plan_delete(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, PlanDeleteServiceRequest>,
    ) -> ServiceResult<Plan> {
        let (t, loaded) = self.service_and_workspace(req.id).await?;
        let full = &t.full;
        let core = loaded.core();
        let core_svc = core
            .environment(t.env)
            .and_then(|e| e.services().find(|s| s.name == full.service.name))
            .ok_or_else(|| internal("service missing from model"))?;
        let unit = t.unit.clone();
        let mut unit_files = vec![format!("{}.container", t.stem)];
        let mut changes = vec![
            StoredChange {
                kind: "stop_unit".into(),
                target: unit.clone(),
                diff: String::new(),
                destructive: false,
            },
            StoredChange {
                kind: "remove_unit".into(),
                target: format!("{}/{}.container", loaded.ws.unix_user, t.stem),
                diff: String::new(),
                destructive: true,
            },
        ];
        if req.purge_volumes {
            for v in maurice_quadlet::render_volumes(t.env, core_svc) {
                changes.push(StoredChange {
                    kind: "remove_unit".into(),
                    target: format!(
                        "{}/{} (volume data stays on disk)",
                        loaded.ws.unix_user, v.file_name
                    ),
                    diff: String::new(),
                    destructive: true,
                });
                unit_files.push(v.file_name);
            }
        }
        let payload = PlanPayload::DeleteService {
            workspace_id: loaded.ws.id,
            service_id: full.service.id,
            unit,
            unit_files,
            purge_volumes: req.purge_volumes,
        };
        Response::ok(store_plan(&self.state, loaded.ws.id, payload, changes, vec![]).await?)
    }

    async fn restart(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, RestartRequest>,
    ) -> ServiceResult<Empty> {
        let (t, loaded) = self.service_and_workspace(req.service_id).await?;
        self.state
            .agent
            .restart_unit(&loaded.user, &t.unit)
            .await
            .map_err(internal)?;
        Response::ok(Empty::default())
    }

    async fn stop(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, StopRequest>,
    ) -> ServiceResult<Empty> {
        let (t, loaded) = self.service_and_workspace(req.service_id).await?;
        self.state
            .agent
            .stop_unit(&loaded.user, &t.unit)
            .await
            .map_err(internal)?;
        Response::ok(Empty::default())
    }
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

pub struct ProjectsSvc {
    pub state: Arc<State>,
}

fn project_to_proto(
    p: &maurice_db::ProjectRow,
    secret_name: &dyn Fn(Uuid) -> Option<String>,
) -> Project {
    Project {
        id: p.id.to_string(),
        workspace_id: p.workspace_id.to_string(),
        name: p.name.clone(),
        git_repo: p.git_repo.clone().unwrap_or_default(),
        git_ref: p.git_ref.clone(),
        git_key_secret: p
            .git_key_secret_id
            .and_then(secret_name)
            .unwrap_or_default(),
        ..Default::default()
    }
}

#[allow(refining_impl_trait)]
impl Projects for ProjectsSvc {
    async fn list(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, ListProjectsRequest>,
    ) -> ServiceResult<ListProjectsResponse> {
        let wid = parse_uuid(req.workspace_id, "workspace")?;
        let loaded = load(&self.state, wid).await?;
        let name = loaded.secret_name();
        Response::ok(ListProjectsResponse {
            projects: loaded
                .projects
                .iter()
                .map(|p| project_to_proto(p, &name))
                .collect(),
            ..Default::default()
        })
    }

    async fn upsert(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, UpsertProjectRequest>,
    ) -> ServiceResult<Project> {
        let wid = parse_uuid(req.workspace_id, "workspace")?;
        let loaded = load(&self.state, wid).await?;
        if !maurice_core::validate::is_dns_label(req.name) {
            return Err(invalid("project name must be a DNS label"));
        }
        let git_repo = (!req.git_repo.is_empty()).then(|| req.git_repo.to_string());
        if let Some(r) = &git_repo
            && (r.starts_with('-') || r.chars().any(char::is_whitespace))
        {
            return Err(invalid("git repository url is not valid"));
        }
        let git_ref = if req.git_ref.is_empty() {
            "main"
        } else {
            req.git_ref
        };
        let key_id = if req.git_key_secret.is_empty() {
            None
        } else {
            Some(loaded.shared_secret_id(req.git_key_secret).ok_or_else(|| {
                invalid(format!(
                    "shared secret '{}' does not exist in this workspace (deploy keys are workspace-wide)",
                    req.git_key_secret
                ))
            })?)
        };
        let row = self
            .state
            .store
            .upsert_project(wid, req.name, git_repo.as_deref(), git_ref, key_id)
            .await
            .map_err(internal)?;
        Response::ok(project_to_proto(&row, &loaded.secret_name()))
    }
}

// ---------------------------------------------------------------------------
// Secrets
// ---------------------------------------------------------------------------

pub struct SecretsSvc {
    pub state: Arc<State>,
}

fn secret_to_proto(s: &maurice_db::SecretRow, environment: Option<&str>) -> SecretInfo {
    SecretInfo {
        id: s.id.to_string(),
        workspace_id: s.workspace_id.to_string(),
        name: s.name.clone(),
        environment: environment
            .map(env_to_proto)
            .unwrap_or(Env::ENV_UNSPECIFIED)
            .into(),
        version: s.version as u32,
        created_at: buffa_types::Timestamp {
            seconds: s.created_at.unix_timestamp(),
            nanos: 0,
            ..Default::default()
        }
        .into(),
        ..Default::default()
    }
}

#[allow(refining_impl_trait)]
impl Secrets for SecretsSvc {
    async fn list(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, ListSecretsRequest>,
    ) -> ServiceResult<ListSecretsResponse> {
        let wid = parse_uuid(req.workspace_id, "workspace")?;
        let loaded = load(&self.state, wid).await?;
        let rows = self.state.store.list_secrets(wid).await.map_err(internal)?;
        Response::ok(ListSecretsResponse {
            secrets: rows
                .iter()
                .map(|s| {
                    secret_to_proto(
                        s,
                        s.environment_id.and_then(|id| loaded.environment_name(id)),
                    )
                })
                .collect(),
            ..Default::default()
        })
    }

    async fn set(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, SetSecretRequest>,
    ) -> ServiceResult<SecretInfo> {
        let wid = parse_uuid(req.workspace_id, "workspace")?;
        let loaded = load(&self.state, wid).await?;
        maurice_agent::host::validate_secret_name(req.name).map_err(|e| invalid(e.to_string()))?;
        if req.value.is_empty() {
            return Err(invalid("secret value must not be empty"));
        }
        let environment = env_from_proto(req.environment);
        let environment_id = match environment {
            Some(name) => Some(
                loaded
                    .environment(name)
                    .ok_or_else(|| {
                        invalid(format!(
                            "workspace '{}' has no {name} environment",
                            loaded.ws.name
                        ))
                    })?
                    .id,
            ),
            None => None,
        };
        let sealed = self.state.master_key.seal(req.value).map_err(internal)?;
        // Push to the host first: if podman refuses, nothing is recorded.
        self.state
            .agent
            .set_secret(
                &loaded.user,
                &podman_secret_name(environment, req.name),
                req.value,
            )
            .await
            .map_err(internal)?;
        let row = self
            .state
            .store
            .insert_secret_version(wid, environment_id, req.name, &sealed)
            .await
            .map_err(internal)?;
        Response::ok(secret_to_proto(&row, environment))
    }
}

// ---------------------------------------------------------------------------
// Plans
// ---------------------------------------------------------------------------

pub struct PlansSvc {
    pub state: Arc<State>,
}

fn event(i: usize, status: apply_event::Status, detail: impl Into<String>) -> ApplyEvent {
    ApplyEvent {
        change_index: i as u32,
        status: status.into(),
        detail: detail.into(),
        ..Default::default()
    }
}

async fn execute(
    state: &State,
    plan_id: Uuid,
    payload: PlanPayload,
    events: &mut Vec<ApplyEvent>,
) -> anyhow::Result<()> {
    use apply_event::Status::{STATUS_DONE as DONE, STATUS_STARTED as STARTED};
    match payload {
        PlanPayload::CreateWorkspace {
            name,
            environments,
            unix_user,
            home,
            subuid_start,
            subuid_len,
            port_block_start,
            port_block_size,
        } => {
            events.push(event(0, STARTED, format!("creating {unix_user}")));
            let user = state
                .agent
                .create_user(&NewUser {
                    name: unix_user.clone(),
                    home: home.clone(),
                    subuid_start: subuid_start as u32,
                    subuid_len: subuid_len as u32,
                })
                .await?;
            let wrow = WorkspaceRow {
                id: Uuid::now_v7(),
                name,
                unix_user,
                uid: user.uid as i32,
                subuid_start,
                subuid_len,
                home,
                cpu_slice: None,
                port_block_start,
                port_block_size,
                created_at: OffsetDateTime::now_utc(),
            };
            state.store.insert_workspace(&wrow).await?;
            events.push(event(
                0,
                DONE,
                format!("workspace {} uid {}", wrow.id, user.uid),
            ));
            let shell = core_workspace_shell(&wrow);
            for (i, name) in environments.iter().enumerate() {
                let env = CoreEnv::parse(name)
                    .ok_or_else(|| anyhow::anyhow!("unknown environment '{name}'"))?;
                let net = maurice_quadlet::render_network(&shell, env);
                events.push(event(i + 1, STARTED, format!("writing {}", net.file_name)));
                state
                    .store
                    .insert_environment(&EnvironmentRow {
                        id: Uuid::now_v7(),
                        workspace_id: wrow.id,
                        name: name.clone(),
                        created_at: OffsetDateTime::now_utc(),
                    })
                    .await?;
                state
                    .agent
                    .write_unit(&user, &net.file_name, &net.content)
                    .await?;
                events.push(event(i + 1, DONE, format!("{name} environment ready")));
            }
            state.agent.daemon_reload(&user).await?;
        }
        PlanPayload::AddEnvironment {
            workspace_id,
            environment,
        } => {
            let ws = state
                .store
                .get_workspace(workspace_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("workspace vanished"))?;
            let user = UserRef {
                name: ws.unix_user.clone(),
                uid: ws.uid as u32,
            };
            let env = CoreEnv::parse(&environment)
                .ok_or_else(|| anyhow::anyhow!("unknown environment '{environment}'"))?;
            let net = maurice_quadlet::render_network(&core_workspace_shell(&ws), env);
            events.push(event(0, STARTED, format!("writing {}", net.file_name)));
            state
                .store
                .insert_environment(&EnvironmentRow {
                    id: Uuid::now_v7(),
                    workspace_id: ws.id,
                    name: environment.clone(),
                    created_at: OffsetDateTime::now_utc(),
                })
                .await?;
            state
                .agent
                .write_unit(&user, &net.file_name, &net.content)
                .await?;
            state.agent.daemon_reload(&user).await?;
            events.push(event(0, DONE, format!("{environment} environment ready")));
        }
        PlanPayload::DeleteWorkspace {
            workspace_id,
            unix_user,
            uid,
            purge,
        } => {
            events.push(event(0, STARTED, format!("deleting {unix_user}")));
            // Idempotent: a retry after a partial failure must still clean the database.
            match state
                .agent
                .delete_user(
                    &UserRef {
                        name: unix_user.clone(),
                        uid: uid as u32,
                    },
                    purge,
                )
                .await
            {
                Ok(()) => {}
                Err(maurice_agent::AgentError::NoSuchUser(_)) => {
                    events.push(event(
                        0,
                        STARTED,
                        format!("{unix_user} already absent on the host"),
                    ));
                }
                Err(e) => return Err(e.into()),
            }
            state.store.delete_workspace(workspace_id).await?;
            events.push(event(0, DONE, format!("{unix_user} removed")));
        }
        PlanPayload::UpsertService {
            workspace_id,
            environment_id,
            environment,
            project_id,
            project_name,
            service,
            ports,
            env,
            units,
            running_unit,
        } => {
            let ws = state
                .store
                .get_workspace(workspace_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("workspace vanished"))?;
            let user = UserRef {
                name: ws.unix_user.clone(),
                uid: ws.uid as u32,
            };
            let project = match state
                .store
                .list_projects(workspace_id)
                .await?
                .into_iter()
                .find(|p| p.id == project_id)
            {
                Some(p) => p,
                None => {
                    state
                        .store
                        .ensure_project(workspace_id, &project_name)
                        .await?
                }
            };
            let mut service = service;
            service.project_id = project.id;
            service.environment_id = environment_id;
            events.push(event(
                0,
                STARTED,
                format!("saving {environment}/{}", service.name),
            ));
            state.store.upsert_service(&service, &ports, &env).await?;
            // The network unit is shared by the environment; writing it is idempotent.
            let env_kind = CoreEnv::parse(&environment)
                .ok_or_else(|| anyhow::anyhow!("unknown environment '{environment}'"))?;
            let net = maurice_quadlet::render_network(&core_workspace_shell(&ws), env_kind);
            state
                .agent
                .write_unit(&user, &net.file_name, &net.content)
                .await?;
            for (i, u) in units.iter().enumerate() {
                events.push(event(i + 1, STARTED, format!("writing {}", u.file_name)));
                state
                    .agent
                    .write_unit(&user, &u.file_name, &u.content)
                    .await?;
                events.push(event(i + 1, DONE, u.file_name.clone()));
            }
            state.agent.daemon_reload(&user).await?;
            if let Some(unit) = running_unit {
                let i = units.len() + 1;
                events.push(event(i, STARTED, format!("restarting {unit}")));
                state.agent.restart_unit(&user, &unit).await?;
                events.push(event(i, DONE, format!("{unit} restarted")));
            }
            events.push(event(0, DONE, format!("service {} saved", service.id)));
        }
        PlanPayload::DeployService {
            workspace_id,
            service_id,
            unit,
            image,
            build,
        } => {
            let ws = state
                .store
                .get_workspace(workspace_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("workspace vanished"))?;
            let user = UserRef {
                name: ws.unix_user.clone(),
                uid: ws.uid as u32,
            };
            let mut image = image;
            let mut git_sha = None;
            let mut idx = 0;
            if let Some(b) = build {
                events.push(event(
                    idx,
                    STARTED,
                    format!("cloning {}@{} and building {}", b.repo, b.git_ref, b.image),
                ));
                let ssh_key = match &b.key_secret_name {
                    Some(name) if !name.is_empty() => {
                        let row = state
                            .store
                            .latest_secret(workspace_id, None, name)
                            .await?
                            .ok_or_else(|| {
                                anyhow::anyhow!("deploy key secret '{name}' not found")
                            })?;
                        Some(state.master_key.open(&row.encrypted_value)?)
                    }
                    _ => None,
                };
                let result = state
                    .agent
                    .build_image(
                        &user,
                        &maurice_agent::BuildSpec {
                            repo: b.repo.clone(),
                            git_ref: b.git_ref.clone(),
                            ssh_key,
                            dockerfile: b.dockerfile.clone(),
                            context: b.context.clone(),
                            image: b.image.clone(),
                            checkout: b.checkout.clone(),
                        },
                    )
                    .await?;
                let short = result
                    .git_sha
                    .get(..12)
                    .unwrap_or(&result.git_sha)
                    .to_string();
                events.push(event(
                    idx,
                    DONE,
                    format!("built {} from {short}", result.image_ref),
                ));
                image = result.image_ref.clone();
                git_sha = Some(result.git_sha.clone());
                idx += 1;
            }
            // Re-render the container unit with the image to run and mark it enabled, so it
            // comes back after a reboot. Units of never-deployed services carry no [Install].
            {
                let loaded = load(state, workspace_id)
                    .await
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                let core = loaded.core();
                let row = loaded
                    .services
                    .iter()
                    .find(|s| s.service.id == service_id)
                    .ok_or_else(|| anyhow::anyhow!("service vanished"))?;
                let env = CoreEnv::parse(&row.environment)
                    .ok_or_else(|| anyhow::anyhow!("unknown environment '{}'", row.environment))?;
                let svc = core
                    .environment(env)
                    .and_then(|e| e.services().find(|s| s.name == row.service.name))
                    .ok_or_else(|| anyhow::anyhow!("service vanished"))?;
                let version = git_sha
                    .as_deref()
                    .map(|s| s.get(..12).unwrap_or(s).to_string())
                    .unwrap_or_else(|| maurice_quadlet::image_version(&image));
                let units = render_units(&core, env, svc, &image, &version, true)
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                for u in &units {
                    events.push(event(idx, STARTED, format!("writing {}", u.file_name)));
                    state
                        .agent
                        .write_unit(&user, &u.file_name, &u.content)
                        .await?;
                    events.push(event(idx, DONE, u.file_name.clone()));
                }
                state.agent.daemon_reload(&user).await?;
                idx += 1;
            }
            let dep_id = Uuid::now_v7();
            let started = OffsetDateTime::now_utc();
            events.push(event(
                idx,
                STARTED,
                format!("starting {unit} (waits for healthy)"),
            ));
            let before = state.agent.unit_status(&user, &unit).await?;
            let result = if before.active_state == "active" {
                state.agent.restart_unit(&user, &unit).await
            } else {
                state.agent.start_unit(&user, &unit).await
            };
            let after = state.agent.unit_status(&user, &unit).await?;
            let ok = result.is_ok() && after.active_state == "active";
            state
                .store
                .insert_deployment(&DeploymentRow {
                    id: dep_id,
                    service_id,
                    plan_id,
                    image_digest: image.clone(),
                    git_sha,
                    status: if ok { "healthy" } else { "failed" }.into(),
                    started_at: started,
                    finished_at: Some(OffsetDateTime::now_utc()),
                })
                .await?;
            result?;
            anyhow::ensure!(
                ok,
                "{unit} is {} ({}) after start",
                after.active_state,
                after.sub_state
            );
            events.push(event(
                idx,
                DONE,
                format!(
                    "deployment {dep_id}: {unit} {} ({})",
                    after.active_state, after.sub_state
                ),
            ));
        }
        PlanPayload::DeleteService {
            workspace_id,
            service_id,
            unit,
            unit_files,
            purge_volumes: _,
        } => {
            let ws = state
                .store
                .get_workspace(workspace_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("workspace vanished"))?;
            let user = UserRef {
                name: ws.unix_user.clone(),
                uid: ws.uid as u32,
            };
            events.push(event(0, STARTED, format!("stopping {unit}")));
            let _ = state.agent.stop_unit(&user, &unit).await;
            events.push(event(0, DONE, format!("{unit} stopped")));
            for (i, f) in unit_files.iter().enumerate() {
                events.push(event(i + 1, STARTED, format!("removing {f}")));
                match state.agent.remove_unit(&user, f).await {
                    Ok(()) | Err(maurice_agent::AgentError::NoSuchUnit(_)) => {}
                    Err(e) => return Err(e.into()),
                }
                events.push(event(i + 1, DONE, f.clone()));
            }
            state.agent.daemon_reload(&user).await?;
            state.store.delete_service(service_id).await?;
        }
    }
    Ok(())
}

#[allow(refining_impl_trait)]
impl Plans for PlansSvc {
    async fn get(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, GetPlanRequest>,
    ) -> ServiceResult<Plan> {
        let id = parse_uuid(req.id, "plan")?;
        match self.state.store.get_plan(id).await.map_err(internal)? {
            Some(p) => Response::ok(plan_to_proto(&p)?),
            None => Err(not_found("plan", req.id)),
        }
    }

    async fn apply(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, ApplyPlanRequest>,
    ) -> ServiceResult<ServiceStream<ApplyEvent>> {
        let id = parse_uuid(req.plan_id, "plan")?;
        let Some(row) = self.state.store.get_plan(id).await.map_err(internal)? else {
            return Err(not_found("plan", req.plan_id));
        };
        if row.applied_at.is_some() {
            return Err(ConnectError::new(
                ErrorCode::FailedPrecondition,
                "plan was already applied",
            ));
        }
        if row.discarded_at.is_some() {
            return Err(ConnectError::new(
                ErrorCode::FailedPrecondition,
                "plan was discarded",
            ));
        }
        if row.expires_at < OffsetDateTime::now_utc() {
            return Err(ConnectError::new(
                ErrorCode::FailedPrecondition,
                "plan has expired; create it again",
            ));
        }
        let findings: Vec<StoredFinding> =
            serde_json::from_value(row.findings.clone()).map_err(internal)?;
        if findings.iter().any(|f| f.severity == "error") {
            return Err(ConnectError::new(
                ErrorCode::FailedPrecondition,
                "plan has validation errors",
            ));
        }
        if row.destructive && !req.confirmed {
            return Err(ConnectError::new(
                ErrorCode::FailedPrecondition,
                "plan is destructive; set confirmed=true to apply",
            ));
        }
        let stored: StoredChanges =
            serde_json::from_value(row.changes.clone()).map_err(internal)?;

        // Alpha executes synchronously and streams the recorded events; a live
        // channel replaces this once applies take more than a few seconds.
        let mut events = Vec::new();
        match execute(&self.state, row.id, stored.payload, &mut events).await {
            Ok(()) => {
                self.state
                    .store
                    .mark_plan_applied(row.id)
                    .await
                    .map_err(internal)?;
            }
            Err(e) => {
                tracing::error!(plan = %row.id, error = %e, "apply failed");
                events.push(event(0, apply_event::Status::STATUS_FAILED, e.to_string()));
            }
        }
        Response::stream_ok(futures::stream::iter(events.into_iter().map(Ok)))
    }

    async fn discard(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, DiscardPlanRequest>,
    ) -> ServiceResult<Empty> {
        let id = parse_uuid(req.id, "plan")?;
        if self
            .state
            .store
            .mark_plan_discarded(id)
            .await
            .map_err(internal)?
        {
            Response::ok(Empty::default())
        } else {
            Err(not_found("plan", req.id))
        }
    }
}
