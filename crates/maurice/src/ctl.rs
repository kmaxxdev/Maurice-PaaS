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
pub enum ServiceCommand {
    /// List services of a workspace.
    List { workspace: String },
    /// Show one service as JSON.
    Get { id: String },
    /// Show the unit file that is on disk for a service.
    Unit { id: String },
    /// Print recent journal lines of a service.
    Logs {
        id: String,
        #[arg(long, default_value_t = 100)]
        tail: u32,
    },
    /// Plan creating or replacing a service from a JSON definition (proto JSON, see examples/).
    Upsert {
        workspace: String,
        #[arg(long, short = 'f')]
        file: std::path::PathBuf,
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Plan starting (or restarting) a service from its units.
    Deploy { id: String },
    /// Plan deleting a service. Destructive.
    Delete {
        id: String,
        #[arg(long)]
        purge_volumes: bool,
    },
    /// Restart a service now (reversible, no plan).
    Restart { id: String },
    /// Stop a service now (reversible, no plan).
    Stop { id: String },
}

#[derive(Subcommand)]
pub enum SecretCommand {
    /// List secret names and versions of a workspace.
    List { workspace: String },
    /// Set a secret value; reads the value from stdin.
    Set { workspace: String, name: String },
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

pub async fn service(cmd: ServiceCommand, server: &str, _tier: Tier) -> anyhow::Result<()> {
    let client = ServicesClient::new(Client::plaintext(), config(server)?);
    match cmd {
        ServiceCommand::List { workspace } => {
            let resp = client
                .list(ListServicesRequest {
                    workspace_id: workspace,
                    ..Default::default()
                })
                .await?
                .into_owned();
            if resp.services.is_empty() {
                println!("no services");
            }
            for s in &resp.services {
                let source = match &s.source {
                    Some(maurice_api::proto::maurice::v1::__buffa::oneof::service::Source::ImageRef(r)) => r.clone(),
                    Some(maurice_api::proto::maurice::v1::__buffa::oneof::service::Source::DockerfilePath(p)) => format!("build:{p}"),
                    None => "-".into(),
                };
                let ports: Vec<String> = s
                    .ports
                    .iter()
                    .map(|p| {
                        if p.published {
                            format!("{}→{}:{}", p.container_port, p.host_ip, p.host_port)
                        } else {
                            p.container_port.to_string()
                        }
                    })
                    .collect();
                println!(
                    "{:<36} {:<14} {:<10} {:<40} ports {}",
                    s.id,
                    s.name,
                    format!("{:?}", s.kind)
                        .to_lowercase()
                        .replace("service_kind_", ""),
                    source,
                    if ports.is_empty() {
                        "-".to_string()
                    } else {
                        ports.join(",")
                    }
                );
            }
        }
        ServiceCommand::Get { id } => {
            let s = client
                .get(GetServiceRequest {
                    id,
                    ..Default::default()
                })
                .await?
                .into_owned();
            print_json(&s)?;
        }
        ServiceCommand::Unit { id } => {
            let u = client
                .get_unit(GetUnitRequest {
                    service_id: id,
                    ..Default::default()
                })
                .await?
                .into_owned();
            println!(
                "# {}{}",
                u.path,
                if u.deployment_id.is_empty() {
                    String::new()
                } else {
                    format!("  (deployment {})", u.deployment_id)
                }
            );
            if u.content.is_empty() {
                println!("# (no unit on disk yet; apply an upsert plan)");
            } else {
                print!("{}", u.content);
            }
        }
        ServiceCommand::Logs { id, tail } => {
            let mut stream = client
                .logs(LogsRequest {
                    service_id: id,
                    tail,
                    ..Default::default()
                })
                .await?;
            while let Some(msg) = stream.message().await? {
                let l = msg.to_owned_message();
                let secs = l.time.as_option().map(|t| t.seconds).unwrap_or(0);
                println!("{secs} [{}] {}", l.priority, l.message);
            }
        }
        ServiceCommand::Upsert {
            workspace,
            file,
            project,
        } => {
            let text = std::fs::read_to_string(&file)?;
            let svc: Service = serde_json::from_str(&text)?;
            let p = client
                .plan_upsert(PlanUpsertServiceRequest {
                    workspace_id: workspace,
                    project,
                    service: svc.into(),
                    ..Default::default()
                })
                .await?
                .into_owned();
            print_plan(&p);
        }
        ServiceCommand::Deploy { id } => {
            let p = client
                .plan_deploy(PlanDeployRequest {
                    service_id: id,
                    ..Default::default()
                })
                .await?
                .into_owned();
            print_plan(&p);
        }
        ServiceCommand::Delete { id, purge_volumes } => {
            let p = client
                .plan_delete(PlanDeleteServiceRequest {
                    id,
                    purge_volumes,
                    ..Default::default()
                })
                .await?
                .into_owned();
            print_plan(&p);
        }
        ServiceCommand::Restart { id } => {
            client
                .restart(RestartRequest {
                    service_id: id.clone(),
                    ..Default::default()
                })
                .await?;
            println!("restarted {id}");
        }
        ServiceCommand::Stop { id } => {
            client
                .stop(StopRequest {
                    service_id: id.clone(),
                    ..Default::default()
                })
                .await?;
            println!("stopped {id}");
        }
    }
    Ok(())
}

pub async fn secret(cmd: SecretCommand, server: &str, _tier: Tier) -> anyhow::Result<()> {
    let client = SecretsClient::new(Client::plaintext(), config(server)?);
    match cmd {
        SecretCommand::List { workspace } => {
            let resp = client
                .list(ListSecretsRequest {
                    workspace_id: workspace,
                    ..Default::default()
                })
                .await?
                .into_owned();
            if resp.secrets.is_empty() {
                println!("no secrets");
            }
            for s in &resp.secrets {
                println!("{:<36} {:<24} v{}", s.id, s.name, s.version);
            }
        }
        SecretCommand::Set { workspace, name } => {
            use std::io::Read;
            let mut value = Vec::new();
            std::io::stdin().read_to_end(&mut value)?;
            while value.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
                value.pop();
            }
            let info = client
                .set(SetSecretRequest {
                    workspace_id: workspace,
                    name: name.clone(),
                    value: value.into(),
                    ..Default::default()
                })
                .await?
                .into_owned();
            println!("secret {name} stored as version {}", info.version);
        }
    }
    Ok(())
}
