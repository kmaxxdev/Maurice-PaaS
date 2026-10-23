//! Handlers. State is PostgreSQL; side effects on the host go through the
//! agent. A plan is stored with a typed payload (`PlanPayload`) that `apply`
//! executes step by step while streaming events.

use crate::proto::maurice::v1::*;
use connectrpc::{
    ConnectError, ErrorCode, RequestContext, Response, ServiceRequest, ServiceResult, ServiceStream,
};
use maurice_agent::{Agent, NewUser, UserRef};
use maurice_db::{PlanRow, Store, WorkspaceRow};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

pub struct State {
    pub agent: Arc<dyn Agent>,
    pub store: Store,
    /// Where workspace homes live.
    pub home_root: String,
}

impl State {
    pub fn new(agent: Arc<dyn Agent>, store: Store) -> Self {
        Self {
            agent,
            store,
            home_root: "/srv/ws".into(),
        }
    }
}

/// What a plan does when applied. Stored as JSON in `plans.changes` next to the
/// human-readable `PlannedChange` list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanPayload {
    CreateWorkspace {
        name: String,
        env: String,
        unix_user: String,
        home: String,
        subuid_start: i64,
        subuid_len: i32,
    },
    DeleteWorkspace {
        workspace_id: Uuid,
        unix_user: String,
        uid: i32,
        purge: bool,
    },
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

fn internal(e: impl std::fmt::Display) -> ConnectError {
    tracing::error!(error = %e, "internal");
    ConnectError::new(ErrorCode::Internal, e.to_string())
}

fn not_found(what: &str, id: &str) -> ConnectError {
    ConnectError::new(ErrorCode::NotFound, format!("{what} {id} not found"))
}

fn parse_uuid(s: &str, what: &str) -> Result<Uuid, ConnectError> {
    Uuid::parse_str(s).map_err(|_| {
        ConnectError::new(
            ErrorCode::InvalidArgument,
            format!("{what} id must be a uuid"),
        )
    })
}

fn env_to_str(e: buffa::EnumValue<Env>) -> Result<&'static str, ConnectError> {
    match e.as_known() {
        Some(Env::ENV_PROD) => Ok("prod"),
        Some(Env::ENV_DEV) => Ok("dev"),
        _ => Err(ConnectError::new(
            ErrorCode::InvalidArgument,
            "env must be ENV_PROD or ENV_DEV",
        )),
    }
}

fn env_from_str(s: &str) -> Env {
    match s {
        "prod" => Env::ENV_PROD,
        "dev" => Env::ENV_DEV,
        _ => Env::ENV_UNSPECIFIED,
    }
}

fn workspace_to_proto(w: &WorkspaceRow) -> Workspace {
    Workspace {
        id: w.id.to_string(),
        name: w.name.clone(),
        unix_user: w.unix_user.clone(),
        uid: w.uid as u32,
        subuid_start: w.subuid_start as u32,
        subuid_len: w.subuid_len as u32,
        home: w.home.clone(),
        env: env_from_str(&w.env).into(),
        cpu_slice: w.cpu_slice.clone().unwrap_or_default(),
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
        Response::ok(ListWorkspacesResponse {
            workspaces: rows.iter().map(workspace_to_proto).collect(),
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
            Some(w) => Response::ok(workspace_to_proto(&w)),
            None => Err(not_found("workspace", req.id)),
        }
    }

    async fn plan_create(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, PlanCreateWorkspaceRequest>,
    ) -> ServiceResult<Plan> {
        let name = req.name.to_string();
        let env = env_to_str(req.env)?;
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
        let changes = vec![StoredChange {
            kind: "create_user".into(),
            target: format!(
                "{unix_user} (uid auto, subuid {subuid_start}+{subuid_len}, home {home})"
            ),
            diff: String::new(),
            destructive: false,
        }];
        let payload = PlanPayload::CreateWorkspace {
            name,
            env: env.into(),
            unix_user,
            home,
            subuid_start,
            subuid_len,
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
        let state = self.state.clone();
        let outcome: anyhow::Result<()> = async {
            match stored.payload {
                PlanPayload::CreateWorkspace {
                    name,
                    env,
                    unix_user,
                    home,
                    subuid_start,
                    subuid_len,
                } => {
                    events.push(event(
                        0,
                        apply_event::Status::STATUS_STARTED,
                        format!("creating {unix_user}"),
                    ));
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
                        env,
                        cpu_slice: None,
                        created_at: OffsetDateTime::now_utc(),
                    };
                    state.store.insert_workspace(&wrow).await?;
                    events.push(event(
                        0,
                        apply_event::Status::STATUS_DONE,
                        format!("workspace {} uid {}", wrow.id, user.uid),
                    ));
                }
                PlanPayload::DeleteWorkspace {
                    workspace_id,
                    unix_user,
                    uid,
                    purge,
                } => {
                    events.push(event(
                        0,
                        apply_event::Status::STATUS_STARTED,
                        format!("deleting {unix_user}"),
                    ));
                    state
                        .agent
                        .delete_user(
                            &UserRef {
                                name: unix_user.clone(),
                                uid: uid as u32,
                            },
                            purge,
                        )
                        .await?;
                    state.store.delete_workspace(workspace_id).await?;
                    events.push(event(
                        0,
                        apply_event::Status::STATUS_DONE,
                        format!("{unix_user} removed"),
                    ));
                }
            }
            Ok(())
        }
        .await;
        match outcome {
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
