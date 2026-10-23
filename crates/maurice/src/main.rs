//! One binary, three names. The name it is invoked by selects the safety tier
//! (ADR-007): `marik` for reversible actions, `maurice` for everything with
//! confirmation on destructive actions, `maurice-odysseus-stella` for forced,
//! scripted recovery behind an extra guard.

mod ctl;
mod host;
mod tier;

use clap::{Parser, Subcommand};
use std::sync::Arc;
use tier::Tier;

#[derive(Parser)]
#[command(
    name = "maurice",
    version,
    about = "Maurice PaaS: deploy on rootless Podman"
)]
struct Cli {
    /// Panel address for client commands.
    #[arg(
        long,
        global = true,
        env = "MAURICE_SERVER",
        default_value = "http://127.0.0.1:7700"
    )]
    server: String,
    /// Accepted by the forced tier only; see ADR-007.
    #[arg(long = "yes-i-mean-it", global = true, hide = true)]
    _yes_i_mean_it: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the panel: API, UI and agent in one process.
    Serve {
        /// Address to listen on. Keep it on loopback until an edge proxy is in front.
        #[arg(long, env = "MAURICE_LISTEN", default_value = "127.0.0.1:7700")]
        listen: String,
        /// PostgreSQL connection string. Defaults to DATABASE_URL from the environment or /etc/maurice/maurice.env.
        #[arg(long, env = "DATABASE_URL")]
        database_url: Option<String>,
        /// Record host operations instead of performing them (no root needed).
        #[arg(long)]
        dry_run: bool,
    },
    /// Workspaces (through the panel).
    Workspace {
        #[command(subcommand)]
        command: ctl::WorkspaceCommand,
    },
    /// Plans (through the panel).
    Plan {
        #[command(subcommand)]
        command: ctl::PlanCommand,
    },
    /// Services (through the panel).
    Service {
        #[command(subcommand)]
        command: ctl::ServiceCommand,
    },
    /// Secrets (through the panel).
    Secret {
        #[command(subcommand)]
        command: ctl::SecretCommand,
    },
    /// Projects: repositories and deploy keys (through the panel).
    Project {
        #[command(subcommand)]
        command: ctl::ProjectCommand,
    },
    /// Render the Quadlet units a workspace definition would produce, without touching the host.
    Render {
        /// Path to a workspace definition (JSON).
        file: std::path::PathBuf,
    },
    /// Print the safety tier this invocation runs with.
    Tier,
    /// Operate on this host as root: users, units, systemd. The agent's surface as commands.
    Host {
        #[command(subcommand)]
        command: host::HostCommand,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?),
        )
        .init();

    let tier = Tier::from_argv0();
    let cli = Cli::parse();

    match cli.command {
        Command::Tier => {
            println!("{tier}");
        }
        Command::Render { file } => {
            let text = std::fs::read_to_string(&file)?;
            let ws: maurice_core::Workspace = serde_json::from_str(&text)?;
            let findings = maurice_core::validate_workspace(&ws);
            for f in &findings {
                eprintln!("{:?} {} {} — {}", f.severity, f.code, f.target, f.message);
            }
            if maurice_core::validate::has_errors(&findings) {
                anyhow::bail!("validation failed");
            }
            let net = maurice_quadlet::render_network(&ws);
            println!("# ==> {}\n{}", net.file_name, net.content);
            for svc in ws.services() {
                for v in maurice_quadlet::render_volumes(svc) {
                    println!("# ==> {}\n{}", v.file_name, v.content);
                }
                let image = match &svc.source {
                    maurice_core::Source::Image { reference } => reference.clone(),
                    maurice_core::Source::Build { .. } => {
                        format!("localhost/{}/{}:dev", ws.name, svc.name)
                    }
                };
                let unit = maurice_quadlet::render_container(&maurice_quadlet::RenderInput {
                    workspace: &ws,
                    service: svc,
                    image: &image,
                    version: "dev",
                    resolve_ref: &|r| Some(format!("<{r}>")),
                })?;
                println!("# ==> {}\n{}", unit.file_name, unit.content);
            }
        }
        Command::Host { command } => {
            host::run(command, tier).await?;
        }
        Command::Workspace { command } => ctl::workspace(command, &cli.server, tier).await?,
        Command::Plan { command } => ctl::plan(command, &cli.server, tier).await?,
        Command::Service { command } => ctl::service(command, &cli.server, tier).await?,
        Command::Secret { command } => ctl::secret(command, &cli.server, tier).await?,
        Command::Project { command } => ctl::project(command, &cli.server, tier).await?,
        Command::Serve {
            listen,
            database_url,
            dry_run,
        } => {
            tier.require_at_least(Tier::Maurice)?;
            let database_url = match database_url {
                Some(u) => u,
                None => read_env_file_var("/etc/maurice/maurice.env", "DATABASE_URL").ok_or_else(
                    || {
                        anyhow::anyhow!(
                            "no DATABASE_URL; pass --database-url or run `maurice host prepare`"
                        )
                    },
                )?,
            };
            let master_key_hex = std::env::var("MAURICE_MASTER_KEY")
                .ok()
                .or_else(|| read_env_file_var("/etc/maurice/maurice.env", "MAURICE_MASTER_KEY"))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "no MAURICE_MASTER_KEY; run `maurice host prepare` or set the variable"
                    )
                })?;
            let master_key = maurice_api::crypto::MasterKey::from_hex(&master_key_hex)?;
            let pool = maurice_db::connect(&database_url).await?;
            let agent: Arc<dyn maurice_agent::Agent> = if dry_run {
                tracing::warn!("dry-run agent: host operations are recorded, not performed");
                Arc::new(maurice_agent::DryRunAgent::new())
            } else {
                maurice_agent::HostAgent::check_privileges()?;
                Arc::new(maurice_agent::HostAgent::new())
            };
            let state = Arc::new(maurice_api::services::State::new(
                agent,
                maurice_db::Store::new(pool),
                master_key,
            ));
            let app = maurice_api::app(state);
            let listener = tokio::net::TcpListener::bind(&listen).await?;
            tracing::info!(%listen, %tier, "maurice serving");
            axum::serve(listener, app).await?;
        }
    }
    Ok(())
}

/// Read `KEY=value` from a simple env file; quotes around the value are stripped.
fn read_env_file_var(path: &str, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().find_map(|l| {
        let l = l.trim();
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"').to_string())
    })
}
