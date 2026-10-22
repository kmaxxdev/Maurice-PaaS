//! Handlers. Alpha starts with in-memory state so the API shape can be
//! exercised end to end; PostgreSQL replaces the store behind the same
//! handlers.

use crate::proto::maurice::v1::*;
use connectrpc::{
    ConnectError, ErrorCode, RequestContext, Response, ServiceRequest, ServiceResult, ServiceStream,
};
use maurice_agent::Agent;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub struct State {
    pub agent: Arc<dyn Agent>,
    pub workspaces: RwLock<HashMap<String, Workspace>>,
    pub plans: RwLock<HashMap<String, Plan>>,
}

impl State {
    pub fn new(agent: Arc<dyn Agent>) -> Self {
        Self {
            agent,
            workspaces: RwLock::new(HashMap::new()),
            plans: RwLock::new(HashMap::new()),
        }
    }
}

fn not_found(what: &str, id: &str) -> ConnectError {
    ConnectError::new(ErrorCode::NotFound, format!("{what} {id} not found"))
}

fn new_id() -> String {
    // Time-ordered ids keep plan and deployment lists naturally sorted.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
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
        let ws = self.state.workspaces.read().expect("workspaces");
        let mut workspaces: Vec<Workspace> = ws.values().cloned().collect();
        workspaces.sort_by(|a, b| a.name.cmp(&b.name));
        Response::ok(ListWorkspacesResponse {
            workspaces,
            ..Default::default()
        })
    }

    async fn get(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, GetWorkspaceRequest>,
    ) -> ServiceResult<Workspace> {
        let ws = self.state.workspaces.read().expect("workspaces");
        match ws.get(req.id) {
            Some(w) => Response::ok(w.clone()),
            None => Err(not_found("workspace", req.id)),
        }
    }

    async fn plan_create(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, PlanCreateWorkspaceRequest>,
    ) -> ServiceResult<Plan> {
        let name = req.name.to_string();
        let mut findings = Vec::new();
        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            || name.is_empty()
        {
            findings.push(Finding {
                severity: Severity::SEVERITY_ERROR.into(),
                code: "workspace.invalid_name".into(),
                message: "workspace name must be lowercase letters, digits and dashes".into(),
                target: format!("workspace:{name}"),
                ..Default::default()
            });
        }
        let plan = Plan {
            id: new_id(),
            workspace_id: String::new(),
            changes: vec![PlannedChange {
                kind: planned_change::Kind::KIND_CREATE_USER.into(),
                target: format!("ws-{name}"),
                diff: String::new(),
                destructive: false,
                ..Default::default()
            }],
            findings,
            destructive: false,
            created_by: "anonymous".into(),
            ..Default::default()
        };
        self.state
            .plans
            .write()
            .expect("plans")
            .insert(plan.id.clone(), plan.clone());
        Response::ok(plan)
    }

    async fn plan_delete(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, PlanDeleteWorkspaceRequest>,
    ) -> ServiceResult<Plan> {
        let ws = self.state.workspaces.read().expect("workspaces");
        let Some(w) = ws.get(req.id) else {
            return Err(not_found("workspace", req.id));
        };
        let plan = Plan {
            id: new_id(),
            workspace_id: w.id.clone(),
            changes: vec![PlannedChange {
                kind: planned_change::Kind::KIND_DELETE_USER.into(),
                target: w.unix_user.clone(),
                diff: String::new(),
                destructive: true,
                ..Default::default()
            }],
            findings: vec![],
            destructive: true,
            created_by: "anonymous".into(),
            ..Default::default()
        };
        drop(ws);
        self.state
            .plans
            .write()
            .expect("plans")
            .insert(plan.id.clone(), plan.clone());
        Response::ok(plan)
    }
}

pub struct PlansSvc {
    pub state: Arc<State>,
}

#[allow(refining_impl_trait)]
impl Plans for PlansSvc {
    async fn get(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, GetPlanRequest>,
    ) -> ServiceResult<Plan> {
        let plans = self.state.plans.read().expect("plans");
        match plans.get(req.id) {
            Some(p) => Response::ok(p.clone()),
            None => Err(not_found("plan", req.id)),
        }
    }

    async fn apply(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, ApplyPlanRequest>,
    ) -> ServiceResult<ServiceStream<ApplyEvent>> {
        let plan = {
            let plans = self.state.plans.read().expect("plans");
            plans
                .get(req.plan_id)
                .cloned()
                .ok_or_else(|| not_found("plan", req.plan_id))?
        };
        if plan
            .findings
            .iter()
            .any(|f| f.severity == Severity::SEVERITY_ERROR)
        {
            return Err(ConnectError::new(
                ErrorCode::FailedPrecondition,
                "plan has validation errors",
            ));
        }
        if plan.destructive && !req.confirmed {
            return Err(ConnectError::new(
                ErrorCode::FailedPrecondition,
                "plan is destructive; set confirmed=true to apply",
            ));
        }
        let events: Vec<ApplyEvent> = plan
            .changes
            .iter()
            .enumerate()
            .flat_map(|(i, c)| {
                [
                    ApplyEvent {
                        change_index: i as u32,
                        status: apply_event::Status::STATUS_STARTED.into(),
                        detail: c.target.clone(),
                        ..Default::default()
                    },
                    ApplyEvent {
                        change_index: i as u32,
                        status: apply_event::Status::STATUS_DONE.into(),
                        detail: "dry-run".into(),
                        ..Default::default()
                    },
                ]
            })
            .collect();
        Response::stream_ok(futures::stream::iter(events.into_iter().map(Ok)))
    }

    async fn discard(
        &self,
        _ctx: RequestContext,
        req: ServiceRequest<'_, DiscardPlanRequest>,
    ) -> ServiceResult<Empty> {
        let removed = self
            .state
            .plans
            .write()
            .expect("plans")
            .remove(req.id)
            .is_some();
        if removed {
            Response::ok(Empty::default())
        } else {
            Err(not_found("plan", req.id))
        }
    }
}
