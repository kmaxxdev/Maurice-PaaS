//! One binary, three names. The name it is invoked by selects the safety tier
//! (ADR-007): `marik` for reversible actions, `maurice` for everything with
//! confirmation on destructive actions, `maurice-odysseus-stella` for forced,
//! scripted recovery behind an extra guard.

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
        Command::Serve { listen } => {
            tier.require_at_least(Tier::Maurice)?;
            let agent: Arc<dyn maurice_agent::Agent> = Arc::new(maurice_agent::DryRunAgent::new());
            let state = Arc::new(maurice_api::services::State::new(agent));
            let app = maurice_api::app(state);
            let listener = tokio::net::TcpListener::bind(&listen).await?;
            tracing::info!(%listen, %tier, "maurice serving");
            axum::serve(listener, app).await?;
        }
    }
    Ok(())
}
