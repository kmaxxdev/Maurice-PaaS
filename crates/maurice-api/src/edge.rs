//! Routes and domains (ADR-009): "expose" on the workspace edge, domains on the
//! host edge. Plans here change rows and then bring both edges in line with
//! what the database says; deploys and deletes call the same sync.

use crate::auth::{self, Need};
use crate::model_map::{self, env_to_proto};
use crate::proto::maurice::v1::*;
use crate::services::{
    PlanPayload, State, StoredChange, StoredFinding, core_env, internal, invalid, load, not_found,
    parse_uuid, store_plan, unit_diff,
};
use connectrpc::{
    ConnectError, ErrorCode, RequestContext, Response, ServiceRequest, ServiceResult,
};
use maurice_agent::UserRef;
use maurice_core::{Env as CoreEnv, TlsMode, Visibility};
use maurice_db::{DomainRow, RouteRow};
use maurice_quadlet::{EDGE_ADMIN_SOCKET, EDGE_CONFIG_PATH, HOST_EDGE_ADMIN, HOST_EDGE_CONTAINER};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use time::OffsetDateTime;
use uuid::Uuid;

/// What an edge plan does when applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EdgeOp {
    Expose {
        route: RouteRow,
        domain: Option<DomainRow>,
    },
    Unexpose {
        route_id: Uuid,
    },
    AddDomain {
        domain: DomainRow,
    },
    RemoveDomain {
        domain_id: Uuid,
    },
}

pub const EDGE_UNIT: &str = "edge.service";
pub const EDGE_CONFIG_REL: &str = "edge/edge.json";

fn host_config_path(state: &State) -> String {
    format!("{}/host.json", state.host_edge_dir)
}

fn visibility_from_proto(
    v: buffa::EnumValue<crate::proto::maurice::v1::Visibility>,
) -> Option<Visibility> {
    use crate::proto::maurice::v1::Visibility as P;
    match v.as_known() {
        Some(P::VISIBILITY_PUBLIC) => Some(Visibility::Public),
        Some(P::VISIBILITY_INTERNAL) => Some(Visibility::Internal),
        _ => None,
    }
}

fn visibility_to_proto(s: &str) -> crate::proto::maurice::v1::Visibility {
    use crate::proto::maurice::v1::Visibility as P;
    match s {
        "public" => P::VISIBILITY_PUBLIC,
        "internal" => P::VISIBILITY_INTERNAL,
        _ => P::VISIBILITY_UNSPECIFIED,
    }
}

fn tls_from_proto(t: buffa::EnumValue<crate::proto::maurice::v1::TlsMode>) -> Option<TlsMode> {
    use crate::proto::maurice::v1::TlsMode as P;
    match t.as_known() {
        Some(P::TLS_MODE_AUTO) => Some(TlsMode::Auto),
        Some(P::TLS_MODE_INTERNAL) => Some(TlsMode::Internal),
        Some(P::TLS_MODE_OFF) => Some(TlsMode::Off),
        _ => None,
    }
}

fn tls_to_proto(s: &str) -> crate::proto::maurice::v1::TlsMode {
    use crate::proto::maurice::v1::TlsMode as P;
    match s {
        "auto" => P::TLS_MODE_AUTO,
        "internal" => P::TLS_MODE_INTERNAL,
        "off" => P::TLS_MODE_OFF,
        _ => P::TLS_MODE_UNSPECIFIED,
    }
}

fn domain_to_proto(d: &DomainRow) -> Domain {
    Domain {
        id: d.id.to_string(),
        route_id: d.route_id.to_string(),
        name: d.name.clone(),
        public_ip: d.public_ip.clone().unwrap_or_default(),
        tls: tls_to_proto(&d.tls_mode).into(),
        ..Default::default()
    }
}

/// The container name a service runs under right now, from its latest deployment.
async fn upstream_container(
    state: &State,
    service_id: Uuid,
    stem: &str,
) -> Result<Option<String>, ConnectError> {
    let Some(dep) = state
        .store
        .latest_deployment(service_id)
        .await
        .map_err(internal)?
    else {
        return Ok(None);
    };
    if dep.status != "healthy" {
        return Ok(None);
    }
    let version = dep
        .git_sha
        .as_deref()
        .map(|s| s.get(..12).unwrap_or(s).to_string())
        .unwrap_or_else(|| maurice_quadlet::image_version(&dep.image_digest));
    Ok(Some(format!("{stem}-{version}")))
}

/// Every host port a workspace uses: published ports and routes.
pub fn used_host_ports(services: &[maurice_db::ServiceFull]) -> HashSet<u16> {
    let mut used = HashSet::new();
    for s in services {
        for p in s.ports.iter().filter(|p| p.published) {
            if let Some(hp) = p.host_port {
                used.insert(hp as u16);
            }
        }
        for (r, _) in &s.routes {
            used.insert(r.block_port as u16);
        }
    }
    used
}

/// Desired state of a workspace edge: unit text and config text, computed from rows.
pub struct EdgeDesired {
    pub unit: maurice_quadlet::Unit,
    pub volumes: Vec<maurice_quadlet::Unit>,
    pub config: String,
    pub has_routes: bool,
}

pub async fn desired_workspace_edge(
    state: &State,
    ws: &maurice_db::WorkspaceRow,
    environments: &[maurice_db::EnvironmentRow],
    services: &[maurice_db::ServiceFull],
) -> Result<EdgeDesired, ConnectError> {
    let core = model_map::workspace_to_core(ws, environments, &[], &[], &|_| None);
    let mut direct_ports: Vec<u16> = services
        .iter()
        .flat_map(|s| s.ports.iter())
        .filter(|p| p.published)
        .filter_map(|p| p.host_port.map(|hp| hp as u16))
        .collect();
    direct_ports.sort_unstable();
    direct_ports.dedup();
    let unit = maurice_quadlet::render_edge(&maurice_quadlet::EdgeInput {
        workspace: &core,
        internal_addr: state.internal_addr,
        direct_ports,
    });
    let mut routes = Vec::new();
    let mut has_routes = false;
    for s in services {
        for (r, _) in &s.routes {
            has_routes = true;
            let env = core_env(&s.environment)?;
            let stem = maurice_quadlet::unit_stem(env, &s.service.name);
            if let Some(container) = upstream_container(state, s.service.id, &stem).await? {
                routes.push(maurice_quadlet::EdgeRoute {
                    block_port: r.block_port as u16,
                    upstream: format!("{container}:{}", r.container_port),
                });
            }
        }
    }
    Ok(EdgeDesired {
        unit,
        volumes: maurice_quadlet::render_edge_volumes(),
        config: maurice_quadlet::render_edge_config(&routes),
        has_routes,
    })
}

/// Desired host edge config from every public domain on the host.
pub async fn desired_host_config(state: &State) -> Result<String, ConnectError> {
    let domains = state.store.list_host_domains().await.map_err(internal)?;
    let routes: Vec<maurice_quadlet::HostRoute> = domains
        .iter()
        .map(|d| maurice_quadlet::HostRoute {
            domain: d.name.clone(),
            public_ip: d.public_ip.as_deref().and_then(|s| s.parse().ok()),
            tls: TlsMode::parse(&d.tls_mode).unwrap_or(TlsMode::Auto),
            upstream_port: d.block_port as u16,
        })
        .collect();
    Ok(maurice_quadlet::render_host_edge_config(
        &routes,
        state.acme_email.as_deref(),
    ))
}

/// Bring a workspace edge in line with the database: write the unit and config,
/// start or reload. Returns a short description of what happened.
pub async fn sync_workspace_edge(state: &State, workspace_id: Uuid) -> anyhow::Result<String> {
    let Some(ws) = state.store.get_workspace(workspace_id).await? else {
        return Ok("workspace gone".into());
    };
    let environments = state.store.list_environments(ws.id).await?;
    let services = state.store.list_services(ws.id).await?;
    let desired = desired_workspace_edge(state, &ws, &environments, &services)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let user = UserRef {
        name: ws.unix_user.clone(),
        uid: ws.uid as u32,
    };
    if !desired.has_routes {
        // Nothing to serve: leave an existing edge running with an empty config.
        if state
            .agent
            .read_unit(&user, &desired.unit.file_name)
            .await?
            .is_none()
        {
            return Ok("no routes, no edge".into());
        }
    }
    let mut unit_changed = false;
    for v in &desired.volumes {
        if state.agent.read_unit(&user, &v.file_name).await?.as_deref() != Some(v.content.as_str())
        {
            state
                .agent
                .write_unit(&user, &v.file_name, &v.content)
                .await?;
            unit_changed = true;
        }
    }
    if state
        .agent
        .read_unit(&user, &desired.unit.file_name)
        .await?
        .as_deref()
        != Some(desired.unit.content.as_str())
    {
        state
            .agent
            .write_unit(&user, &desired.unit.file_name, &desired.unit.content)
            .await?;
        unit_changed = true;
    }
    let config_changed = state
        .agent
        .read_user_file(&user, EDGE_CONFIG_REL)
        .await?
        .as_deref()
        != Some(desired.config.as_str());
    if config_changed {
        state
            .agent
            .write_user_file(&user, EDGE_CONFIG_REL, &desired.config)
            .await?;
    }
    if unit_changed {
        state.agent.daemon_reload(&user).await?;
    }
    let st = state.agent.unit_status(&user, EDGE_UNIT).await?;
    if st.active_state != "active" {
        state.agent.start_unit(&user, EDGE_UNIT).await?;
        return Ok("edge started".into());
    }
    if unit_changed {
        state.agent.restart_unit(&user, EDGE_UNIT).await?;
        return Ok("edge restarted (unit changed)".into());
    }
    if !config_changed {
        return Ok("edge unchanged".into());
    }
    state
        .agent
        .podman_exec(
            &user,
            "edge",
            &[
                "caddy",
                "reload",
                "--config",
                EDGE_CONFIG_PATH,
                "--address",
                EDGE_ADMIN_SOCKET,
            ],
        )
        .await?;
    Ok("edge reloaded".into())
}

/// Bring the host edge in line with the database. Never fatal for the caller's
/// plan: the host edge may be absent on a host that was not prepared for it.
pub async fn sync_host_edge(state: &State) -> anyhow::Result<String> {
    let config = desired_host_config(state)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let path = host_config_path(state);
    if state.agent.read_host_file(&path).await?.as_deref() == Some(config.as_str()) {
        return Ok("host edge unchanged".into());
    }
    state.agent.write_host_file(&path, &config).await?;
    match state
        .agent
        .podman_exec_root(
            HOST_EDGE_CONTAINER,
            &[
                "caddy",
                "reload",
                "--config",
                "/cfg/host.json",
                "--address",
                HOST_EDGE_ADMIN,
            ],
        )
        .await
    {
        Ok(_) => Ok("host edge reloaded".into()),
        Err(e) => Ok(format!("host edge config written; reload failed: {e}")),
    }
}

/// Apply one edge change: rows first, then both edges.
pub async fn execute_edge(
    state: &State,
    workspace_id: Uuid,
    op: EdgeOp,
    events: &mut Vec<ApplyEvent>,
) -> anyhow::Result<()> {
    use apply_event::Status::{STATUS_DONE as DONE, STATUS_STARTED as STARTED};
    let ev = |i: usize, st: apply_event::Status, d: String| ApplyEvent {
        change_index: i as u32,
        status: st.into(),
        detail: d,
        ..Default::default()
    };
    events.push(ev(0, STARTED, "saving route".into()));
    match op {
        EdgeOp::Expose { route, domain } => {
            state.store.insert_route(&route).await?;
            if let Some(d) = domain {
                state.store.insert_domain(&d).await?;
            }
        }
        EdgeOp::Unexpose { route_id } => {
            state.store.delete_route(route_id).await?;
        }
        EdgeOp::AddDomain { domain } => {
            state.store.insert_domain(&domain).await?;
        }
        EdgeOp::RemoveDomain { domain_id } => {
            state.store.delete_domain(domain_id).await?;
        }
    }
    events.push(ev(0, DONE, "saved".into()));
    events.push(ev(1, STARTED, "workspace edge".into()));
    let r = sync_workspace_edge(state, workspace_id).await?;
    events.push(ev(1, DONE, r));
    events.push(ev(2, STARTED, "host edge".into()));
    let r = sync_host_edge(state).await?;
    events.push(ev(2, DONE, r));
    Ok(())
}

// ---------------------------------------------------------------------------
// Edge service
// ---------------------------------------------------------------------------

pub struct EdgeSvc {
    pub state: Arc<State>,
}

impl EdgeSvc {
    async fn route_to_proto(&self, r: &maurice_db::RouteFull) -> Result<Route, ConnectError> {
        let env = core_env(&r.environment)?;
        let stem = maurice_quadlet::unit_stem(env, &r.service);
        let upstream = upstream_container(&self.state, r.route.service_id, &stem)
            .await?
            .map(|c| format!("{c}:{}", r.route.container_port))
            .unwrap_or_default();
        Ok(Route {
            id: r.route.id.to_string(),
            workspace_id: r.route.workspace_id.to_string(),
            environment: env_to_proto(&r.environment).into(),
            service_id: r.route.service_id.to_string(),
            service: r.service.clone(),
            container_port: r.route.container_port as u32,
            block_port: r.route.block_port as u32,
            visibility: visibility_to_proto(&r.route.visibility).into(),
            domains: r.domains.iter().map(domain_to_proto).collect(),
            upstream,
            ..Default::default()
        })
    }

    /// Plan: findings from the validator on the would-be model, plus the unit and
    /// config diffs both edges would see.
    async fn plan_edge(
        &self,
        p: &auth::Principal,
        loaded: &crate::services::Loaded,
        env: CoreEnv,
        core: maurice_core::Workspace,
        services_after: Vec<maurice_db::ServiceFull>,
        extra_domains: &[DomainRow],
        removed_domain: Option<&str>,
        op: EdgeOp,
        label: String,
    ) -> ServiceResult<Plan> {
        let mut findings: Vec<StoredFinding> = maurice_core::validate_workspace(&core)
            .iter()
            .filter(|f| {
                f.code.starts_with("port.")
                    || f.code.starts_with("route.")
                    || f.code.starts_with("domain.")
            })
            .map(StoredFinding::from)
            .collect();
        // Domains are unique on the whole host, not only in this workspace.
        for d in extra_domains {
            if let Some(existing) = self
                .state
                .store
                .get_domain_by_name(&d.name)
                .await
                .map_err(internal)?
                && existing.id != d.id
            {
                findings.push(StoredFinding {
                    severity: "error".into(),
                    code: "domain.taken".into(),
                    message: format!(
                        "'{}' is already attached to another route on this host",
                        d.name
                    ),
                    target: format!("domain:{}", d.name),
                });
            }
        }
        let desired = desired_workspace_edge(
            &self.state,
            &loaded.ws,
            &loaded.environments,
            &services_after,
        )
        .await?;
        let mut changes = vec![];
        let on_disk = self
            .state
            .agent
            .read_unit(&loaded.user, &desired.unit.file_name)
            .await
            .map_err(internal)?;
        let diff = unit_diff(on_disk.as_deref(), &desired.unit.content);
        if !diff.is_empty() {
            changes.push(StoredChange {
                kind: "write_unit".into(),
                target: format!("{}/{}", loaded.ws.unix_user, desired.unit.file_name),
                diff,
                destructive: false,
            });
        }
        let cfg_on_disk = self
            .state
            .agent
            .read_user_file(&loaded.user, EDGE_CONFIG_REL)
            .await
            .map_err(internal)?;
        let diff = unit_diff(cfg_on_disk.as_deref(), &desired.config);
        changes.push(StoredChange {
            kind: "write_unit".into(),
            target: format!("{}/{EDGE_CONFIG_REL} ({label})", loaded.ws.unix_user),
            diff,
            destructive: false,
        });
        changes.push(StoredChange {
            kind: if on_disk.is_some() {
                "restart_unit"
            } else {
                "start_unit"
            }
            .into(),
            target: format!("{EDGE_UNIT} (reload when the unit is unchanged)"),
            diff: String::new(),
            destructive: false,
        });
        // Host edge: the config with this plan's domains added or removed.
        let mut host_domains = self
            .state
            .store
            .list_host_domains()
            .await
            .map_err(internal)?;
        host_domains.retain(|d| Some(d.name.as_str()) != removed_domain);
        for d in extra_domains {
            if let Some(route) = services_after
                .iter()
                .flat_map(|s| s.routes.iter())
                .find(|(r, _)| r.id == d.route_id)
            {
                host_domains.push(maurice_db::HostDomain {
                    name: d.name.clone(),
                    public_ip: d.public_ip.clone(),
                    tls_mode: d.tls_mode.clone(),
                    block_port: route.0.block_port,
                });
            }
        }
        let host_routes: Vec<maurice_quadlet::HostRoute> = host_domains
            .iter()
            .map(|d| maurice_quadlet::HostRoute {
                domain: d.name.clone(),
                public_ip: d.public_ip.as_deref().and_then(|s| s.parse().ok()),
                tls: TlsMode::parse(&d.tls_mode).unwrap_or(TlsMode::Auto),
                upstream_port: d.block_port as u16,
            })
            .collect();
        let host_cfg = maurice_quadlet::render_host_edge_config(
            &host_routes,
            self.state.acme_email.as_deref(),
        );
        let host_on_disk = self
            .state
            .agent
            .read_host_file(&host_config_path(&self.state))
            .await
            .map_err(internal)?;
        let diff = unit_diff(host_on_disk.as_deref(), &host_cfg);
        if !diff.is_empty() {
            changes.push(StoredChange {
                kind: "write_unit".into(),
                target: format!("{} (host edge, reload)", host_config_path(&self.state)),
                diff,
                destructive: false,
            });
        }
        let destructive = matches!(op, EdgeOp::Unexpose { .. } | EdgeOp::RemoveDomain { .. });
        if destructive {
            for c in &mut changes {
                c.destructive = false;
            }
        }
        let payload = PlanPayload::EdgeChange {
            workspace_id: loaded.ws.id,
            environment: env.to_string(),
            op,
        };
        Response::ok(store_plan(&self.state, p, loaded.ws.id, payload, changes, findings).await?)
    }
}

/// Services with one of them replaced by an edited copy.
fn replace_service(
    services: &[maurice_db::ServiceFull],
    edited: maurice_db::ServiceFull,
) -> Vec<maurice_db::ServiceFull> {
    services
        .iter()
        .map(|s| {
            if s.service.id == edited.service.id {
                edited.clone()
            } else {
                s.clone()
            }
        })
        .collect()
}

#[allow(refining_impl_trait)]
impl Edge for EdgeSvc {
    async fn list_routes(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, ListRoutesRequest>,
    ) -> ServiceResult<ListRoutesResponse> {
        let wid = parse_uuid(req.workspace_id, "workspace")?;
        self.state
            .require(&ctx, Need::read().workspace(wid))
            .await?;
        let rows = self.state.store.list_routes(wid).await.map_err(internal)?;
        let mut routes = Vec::with_capacity(rows.len());
        for r in &rows {
            routes.push(self.route_to_proto(r).await?);
        }
        Response::ok(ListRoutesResponse {
            routes,
            ..Default::default()
        })
    }

    async fn get_edge_status(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, GetEdgeStatusRequest>,
    ) -> ServiceResult<EdgeStatus> {
        let wid = parse_uuid(req.workspace_id, "workspace")?;
        self.state
            .require(&ctx, Need::read().workspace(wid))
            .await?;
        let loaded = load(&self.state, wid).await?;
        let host_edge = match self
            .state
            .agent
            .podman_exec_root(HOST_EDGE_CONTAINER, &["caddy", "version"])
            .await
        {
            Ok(_) => "active".to_string(),
            Err(_) => "inactive".to_string(),
        };
        let workspace_edge = self
            .state
            .agent
            .unit_status(&loaded.user, EDGE_UNIT)
            .await
            .map(|s| s.active_state)
            .unwrap_or_else(|_| "inactive".into());
        Response::ok(EdgeStatus {
            host_edge,
            internal_addr: self
                .state
                .internal_addr
                .map(|a| a.to_string())
                .unwrap_or_default(),
            workspace_edge,
            ..Default::default()
        })
    }

    async fn plan_expose(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, PlanExposeRequest>,
    ) -> ServiceResult<Plan> {
        let sid = parse_uuid(req.service_id, "service")?;
        let Some(full) = self.state.store.get_service(sid).await.map_err(internal)? else {
            return Err(not_found("service", req.service_id));
        };
        let loaded = load(&self.state, full.project.workspace_id).await?;
        let env = core_env(&full.environment)?;
        let p = self
            .state
            .require(&ctx, Need::plan().env(env).workspace(loaded.ws.id))
            .await?;
        let visibility = visibility_from_proto(req.visibility).unwrap_or(Visibility::Public);
        if req.container_port == 0 || req.container_port > 65535 {
            return Err(invalid("container port is required"));
        }
        let block = model_map::port_block_of(&loaded.ws);
        let used = used_host_ports(&loaded.services);
        let block_port = if req.block_port == 0 {
            block
                .usable()
                .find(|p| !used.contains(p))
                .ok_or_else(|| invalid("the workspace's port block is full"))?
        } else {
            u16::try_from(req.block_port).map_err(|_| invalid("block port out of range"))?
        };
        let route = RouteRow {
            id: Uuid::now_v7(),
            workspace_id: loaded.ws.id,
            environment_id: full.service.environment_id,
            service_id: full.service.id,
            container_port: req.container_port as i32,
            block_port: block_port as i32,
            visibility: visibility.as_str().into(),
            created_at: OffsetDateTime::now_utc(),
        };
        let domain = if req.domain.is_empty() {
            None
        } else {
            let tls = tls_from_proto(req.tls).unwrap_or(TlsMode::Auto);
            if !req.public_ip.is_empty() {
                req.public_ip
                    .parse::<std::net::IpAddr>()
                    .map_err(|_| invalid("public ip is not an address"))?;
            }
            Some(DomainRow {
                id: Uuid::now_v7(),
                route_id: route.id,
                name: req.domain.trim().to_ascii_lowercase(),
                public_ip: (!req.public_ip.is_empty()).then(|| req.public_ip.to_string()),
                tls_mode: tls.as_str().into(),
                created_at: OffsetDateTime::now_utc(),
            })
        };
        let mut edited = full.clone();
        edited
            .routes
            .push((route.clone(), domain.iter().cloned().collect()));
        let services_after = replace_service(&loaded.services, edited);
        let core = model_map::workspace_to_core(
            &loaded.ws,
            &loaded.environments,
            &loaded.projects,
            &services_after,
            &loaded.secret_podman_name(),
        );
        let extra: Vec<DomainRow> = domain.iter().cloned().collect();
        let label = format!(
            "expose {}/{} :{} → :{block_port} {}",
            full.environment,
            full.service.name,
            req.container_port,
            visibility.as_str()
        );
        self.plan_edge(
            &p,
            &loaded,
            env,
            core,
            services_after,
            &extra,
            None,
            EdgeOp::Expose { route, domain },
            label,
        )
        .await
    }

    async fn plan_unexpose(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, PlanUnexposeRequest>,
    ) -> ServiceResult<Plan> {
        let rid = parse_uuid(req.route_id, "route")?;
        let Some(r) = self.state.store.get_route(rid).await.map_err(internal)? else {
            return Err(not_found("route", req.route_id));
        };
        let loaded = load(&self.state, r.route.workspace_id).await?;
        let env = core_env(&r.environment)?;
        let p = self
            .state
            .require(&ctx, Need::plan().env(env).workspace(loaded.ws.id))
            .await?;
        let Some(full) = loaded
            .services
            .iter()
            .find(|s| s.service.id == r.route.service_id)
        else {
            return Err(not_found("service", &r.route.service_id.to_string()));
        };
        let mut edited = full.clone();
        edited.routes.retain(|(x, _)| x.id != rid);
        let services_after = replace_service(&loaded.services, edited);
        let core = model_map::workspace_to_core(
            &loaded.ws,
            &loaded.environments,
            &loaded.projects,
            &services_after,
            &loaded.secret_podman_name(),
        );
        // All domains of the route leave the host edge with it.
        let removed = r.domains.first().map(|d| d.name.clone());
        let label = format!(
            "unexpose {}/{} :{}",
            r.environment, r.service, r.route.block_port
        );
        self.plan_edge(
            &p,
            &loaded,
            env,
            core,
            services_after,
            &[],
            removed.as_deref(),
            EdgeOp::Unexpose { route_id: rid },
            label,
        )
        .await
    }

    async fn plan_add_domain(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, PlanAddDomainRequest>,
    ) -> ServiceResult<Plan> {
        let rid = parse_uuid(req.route_id, "route")?;
        let Some(r) = self.state.store.get_route(rid).await.map_err(internal)? else {
            return Err(not_found("route", req.route_id));
        };
        let loaded = load(&self.state, r.route.workspace_id).await?;
        let env = core_env(&r.environment)?;
        let p = self
            .state
            .require(&ctx, Need::plan().env(env).workspace(loaded.ws.id))
            .await?;
        if req.domain.trim().is_empty() {
            return Err(invalid("domain is required"));
        }
        if !req.public_ip.is_empty() {
            req.public_ip
                .parse::<std::net::IpAddr>()
                .map_err(|_| invalid("public ip is not an address"))?;
        }
        let domain = DomainRow {
            id: Uuid::now_v7(),
            route_id: rid,
            name: req.domain.trim().to_ascii_lowercase(),
            public_ip: (!req.public_ip.is_empty()).then(|| req.public_ip.to_string()),
            tls_mode: tls_from_proto(req.tls)
                .unwrap_or(TlsMode::Auto)
                .as_str()
                .into(),
            created_at: OffsetDateTime::now_utc(),
        };
        let Some(full) = loaded
            .services
            .iter()
            .find(|s| s.service.id == r.route.service_id)
        else {
            return Err(not_found("service", &r.route.service_id.to_string()));
        };
        let mut edited = full.clone();
        for (x, ds) in &mut edited.routes {
            if x.id == rid {
                ds.push(domain.clone());
            }
        }
        let services_after = replace_service(&loaded.services, edited);
        let core = model_map::workspace_to_core(
            &loaded.ws,
            &loaded.environments,
            &loaded.projects,
            &services_after,
            &loaded.secret_podman_name(),
        );
        let label = format!(
            "domain {} → {}/{} :{}",
            domain.name, r.environment, r.service, r.route.block_port
        );
        let extra = vec![domain.clone()];
        self.plan_edge(
            &p,
            &loaded,
            env,
            core,
            services_after,
            &extra,
            None,
            EdgeOp::AddDomain { domain },
            label,
        )
        .await
    }

    async fn plan_remove_domain(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, PlanRemoveDomainRequest>,
    ) -> ServiceResult<Plan> {
        let did = parse_uuid(req.domain_id, "domain")?;
        let Some(d) = self.state.store.get_domain(did).await.map_err(internal)? else {
            return Err(not_found("domain", req.domain_id));
        };
        let Some(r) = self
            .state
            .store
            .get_route(d.route_id)
            .await
            .map_err(internal)?
        else {
            return Err(not_found("route", &d.route_id.to_string()));
        };
        let loaded = load(&self.state, r.route.workspace_id).await?;
        let env = core_env(&r.environment)?;
        let p = self
            .state
            .require(&ctx, Need::plan().env(env).workspace(loaded.ws.id))
            .await?;
        let Some(full) = loaded
            .services
            .iter()
            .find(|s| s.service.id == r.route.service_id)
        else {
            return Err(not_found("service", &r.route.service_id.to_string()));
        };
        let mut edited = full.clone();
        for (_, ds) in &mut edited.routes {
            ds.retain(|x| x.id != did);
        }
        let services_after = replace_service(&loaded.services, edited);
        let core = model_map::workspace_to_core(
            &loaded.ws,
            &loaded.environments,
            &loaded.projects,
            &services_after,
            &loaded.secret_podman_name(),
        );
        let label = format!("remove domain {}", d.name);
        self.plan_edge(
            &p,
            &loaded,
            env,
            core,
            services_after,
            &[],
            Some(&d.name),
            EdgeOp::RemoveDomain { domain_id: did },
            label,
        )
        .await
    }
}

#[allow(dead_code)]
fn _unused(_: ErrorCode) {}
