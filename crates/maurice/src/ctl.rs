//! Client-side commands: talk to a running panel over Connect. This is what
//! `marik` and `maurice` do for day-to-day work; `host` commands bypass the
//! panel and are for the host itself.

use crate::tier::Tier;
use clap::Subcommand;
use connectrpc::client::{ClientConfig, HttpClient};
use maurice_api::proto::maurice::v1::*;

#[derive(Subcommand)]
pub enum WorkspaceCommand {
    /// List workspaces.
    List,
    /// Show one workspace.
    Get { id: String },
    /// Plan creating a workspace; prints the plan. Apply it with `plan apply`.
    Create {
        name: String,
        #[arg(long, default_value = "dev")]
        env: String,
    },
    /// Plan deleting a workspace. Destructive.
    Delete {
        id: String,
        /// Also remove home, containers and volumes.
        #[arg(long)]
        purge: bool,
    },
}

#[derive(Subcommand)]
pub enum PlanCommand {
    /// Show a plan.
    Get { id: String },
    /// Apply a plan and stream progress.
    Apply {
        id: String,
        /// Confirm a destructive plan.
        #[arg(long)]
        confirm: bool,
    },
    /// Discard a plan.
    Discard { id: String },
}

type Client = HttpClient;

fn config(server: &str) -> anyhow::Result<ClientConfig> {
    Ok(ClientConfig::new(server.parse()?))
}

fn print_json<T: serde::Serialize>(v: &T) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

fn print_plan(p: &Plan) {
    println!(
        "plan {}{}",
        p.id,
        if p.destructive { "  [destructive]" } else { "" }
    );
    for c in &p.changes {
        println!(
            "  {:<14} {}",
            format!("{:?}", c.kind).to_lowercase().replace("kind_", ""),
            c.target
        );
        if !c.diff.is_empty() {
            for line in c.diff.lines() {
                println!("    {line}");
            }
        }
    }
    for f in &p.findings {
        println!(
            "  {:<8} {} {} — {}",
            format!("{:?}", f.severity)
                .to_lowercase()
                .replace("severity_", ""),
            f.code,
            f.target,
            f.message
        );
    }
    let has_errors = p
        .findings
        .iter()
        .any(|f| f.severity == Severity::SEVERITY_ERROR);
    if has_errors {
        println!("  → plan cannot be applied until the errors are fixed");
    } else if p.destructive {
        println!("  → apply with: maurice plan apply {} --confirm", p.id);
    } else {
        println!("  → apply with: marik plan apply {}", p.id);
    }
}

pub async fn workspace(cmd: WorkspaceCommand, server: &str, _tier: Tier) -> anyhow::Result<()> {
    let client = WorkspacesClient::new(Client::plaintext(), config(server)?);
    match cmd {
        WorkspaceCommand::List => {
            let resp = client
                .list(ListWorkspacesRequest::default())
                .await?
                .into_owned();
            if resp.workspaces.is_empty() {
                println!("no workspaces");
            }
            for w in &resp.workspaces {
                println!(
                    "{:<36} {:<16} {:<5} {:<12} uid {:<6} subuid {}+{}  {}",
                    w.id,
                    w.name,
                    format!("{:?}", w.env).to_lowercase().replace("env_", ""),
                    w.unix_user,
                    w.uid,
                    w.subuid_start,
                    w.subuid_len,
                    w.home
                );
            }
        }
        WorkspaceCommand::Get { id } => {
            let w = client
                .get(GetWorkspaceRequest {
                    id,
                    ..Default::default()
                })
                .await?
                .into_owned();
            print_json(&w)?;
        }
        WorkspaceCommand::Create { name, env } => {
            let env = match env.as_str() {
                "prod" => Env::ENV_PROD,
                "dev" => Env::ENV_DEV,
                other => anyhow::bail!("env must be prod or dev, got {other}"),
            };
            let p = client
                .plan_create(PlanCreateWorkspaceRequest {
                    name,
                    env: env.into(),
                    ..Default::default()
                })
                .await?
                .into_owned();
            print_plan(&p);
        }
        WorkspaceCommand::Delete { id, purge } => {
            let p = client
                .plan_delete(PlanDeleteWorkspaceRequest {
                    id,
                    purge,
                    ..Default::default()
                })
                .await?
                .into_owned();
            print_plan(&p);
        }
    }
    Ok(())
}

pub async fn plan(cmd: PlanCommand, server: &str, tier: Tier) -> anyhow::Result<()> {
    let client = PlansClient::new(Client::plaintext(), config(server)?);
    match cmd {
        PlanCommand::Get { id } => {
            let p = client
                .get(GetPlanRequest {
                    id,
                    ..Default::default()
                })
                .await?
                .into_owned();
            print_plan(&p);
        }
        PlanCommand::Apply { id, confirm } => {
            let p = client
                .get(GetPlanRequest {
                    id: id.clone(),
                    ..Default::default()
                })
                .await?
                .into_owned();
            if p.destructive {
                // ADR-007: marik refuses, maurice needs --confirm, the long name needs the extra guard.
                tier.require_at_least(Tier::Maurice)?;
                if tier == Tier::OdysseusStella {
                    tier.require_force_guard()?;
                } else if !confirm {
                    anyhow::bail!("plan {id} is destructive; re-run with --confirm");
                }
            }
            let mut stream = client
                .apply(ApplyPlanRequest {
                    plan_id: id,
                    confirmed: confirm || tier == Tier::OdysseusStella,
                    ..Default::default()
                })
                .await?;
            let mut failed = false;
            while let Some(msg) = stream.message().await? {
                let ev = msg.to_owned_message();
                let status = format!("{:?}", ev.status)
                    .to_lowercase()
                    .replace("status_", "");
                println!("[{}] {:<8} {}", ev.change_index, status, ev.detail);
                if ev.status == apply_event::Status::STATUS_FAILED {
                    failed = true;
                }
            }
            if failed {
                anyhow::bail!("apply failed");
            }
        }
        PlanCommand::Discard { id } => {
            client
                .discard(DiscardPlanRequest {
                    id: id.clone(),
                    ..Default::default()
                })
                .await?;
            println!("plan {id} discarded");
        }
    }
    Ok(())
}
