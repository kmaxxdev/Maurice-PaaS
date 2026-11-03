//! Client-side commands: talk to a running panel over Connect. This is what
//! `marik` and `maurice` do for day-to-day work; `host` commands bypass the
//! panel and are for the host itself.

use crate::tier::Tier;
use clap::Subcommand;
use connectrpc::client::{ClientConfig, HttpClient};
use maurice_api::proto::maurice::v1::*;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum WorkspaceCommand {
    /// List workspaces.
    List,
    /// Show one workspace.
    Get { id: String },
    /// Plan creating a workspace; prints the plan. Apply it with `plan apply`.
    Create {
        name: String,
        /// Environments to create: dev, staging, prod. Repeat the flag; default dev and prod.
        #[arg(long = "env", value_name = "ENV")]
        envs: Vec<String>,
        /// Host ports reserved for the workspace: 100, 500 or 1000.
        #[arg(long, default_value_t = 500)]
        block_size: u32,
    },
    /// Show the port block of a workspace, what is bound in it and the next free port.
    Ports { id: String },
    /// Plan adding an environment (and its network) to a workspace.
    AddEnv {
        id: String,
        /// dev, staging or prod.
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
    List {
        workspace: String,
        /// Only this environment.
        #[arg(long)]
        env: Option<String>,
    },
    /// Show one service as JSON.
    Get { id: String },
    /// Show the unit file that is on disk for a service.
    Unit { id: String },
    /// Print recent journal lines of a service.
    Logs {
        id: String,
        #[arg(long, default_value_t = 100)]
        tail: u32,
        /// Keep printing new lines until interrupted.
        #[arg(long, short = 'f')]
        follow: bool,
    },
    /// Stream resource usage of a service's container.
    Stats {
        id: String,
        /// Stop after this many samples (0 = until interrupted).
        #[arg(long, default_value_t = 5)]
        count: u32,
    },
    /// Plan creating or replacing a service from a JSON definition (proto JSON, see examples/).
    Upsert {
        workspace: String,
        #[arg(long, short = 'f')]
        file: std::path::PathBuf,
        #[arg(long, default_value = "default")]
        project: String,
        /// Environment to place the service in (required for a new service).
        #[arg(long)]
        env: Option<String>,
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
pub enum ProjectCommand {
    /// List projects of a workspace with their repositories.
    List { workspace: String },
    /// Create a project or set its repository, branch and deploy key secret.
    Set {
        workspace: String,
        name: String,
        /// Clone URL. Use ssh:// or git@ for private repositories and pass --key-secret.
        #[arg(long)]
        repo: Option<String>,
        /// Branch or tag to build from.
        #[arg(long = "ref", default_value = "main")]
        git_ref: String,
        /// Workspace secret holding the private deploy key (PEM).
        #[arg(long)]
        key_secret: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum SecretCommand {
    /// List secret names and versions of a workspace.
    List { workspace: String },
    /// Set a secret value; reads the value from stdin.
    Set {
        workspace: String,
        name: String,
        /// Scope the secret to one environment; without it the secret is shared by the workspace.
        #[arg(long)]
        env: Option<String>,
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

#[derive(Subcommand)]
pub enum RouteCommand {
    /// List routes of a workspace: block port → service port, visibility, domains.
    List { workspace: String },
    /// Show both edges' state for a workspace.
    Status { workspace: String },
    /// Plan exposing a container port of a service on the workspace edge.
    Expose {
        service: String,
        /// Container port to proxy to.
        #[arg(long)]
        port: u32,
        /// Reachable only from other workspaces on the internal address.
        #[arg(long)]
        internal: bool,
        /// Block port to listen on; default: next free.
        #[arg(long, default_value_t = 0)]
        block_port: u32,
        /// Attach a domain on the host edge (public routes).
        #[arg(long)]
        domain: Option<String>,
        /// auto (public CA), internal (Caddy's CA) or off (plain HTTP).
        #[arg(long, default_value = "auto")]
        tls: String,
        /// Public address the domain listens on; default: every address.
        #[arg(long)]
        public_ip: Option<String>,
    },
    /// Plan removing a route and its domains.
    Unexpose { route: String },
    /// Plan attaching a domain to a public route.
    AddDomain {
        route: String,
        domain: String,
        #[arg(long, default_value = "auto")]
        tls: String,
        #[arg(long)]
        public_ip: Option<String>,
    },
    /// Plan removing a domain.
    RemoveDomain { domain: String },
}

#[derive(Subcommand)]
pub enum AuthCommand {
    /// Sign in with a login and password; stores an API token for later calls.
    Login {
        /// Panel login; asked for when omitted.
        #[arg(long, short = 'u')]
        user: Option<String>,
    },
    /// Forget the stored token.
    Logout,
    /// Show who the stored token belongs to.
    Whoami,
}

type Client = HttpClient;

/// Where the CLI keeps its token: `$XDG_CONFIG_HOME/maurice/token` or `~/.config/maurice/token`.
fn token_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("maurice").join("token"))
}

fn stored_token() -> Option<String> {
    if let Ok(t) = std::env::var("MAURICE_TOKEN")
        && !t.trim().is_empty()
    {
        return Some(t.trim().to_string());
    }
    let text = std::fs::read_to_string(token_path()?).ok()?;
    let t = text.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn save_token(token: &str) -> anyhow::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = token_path().ok_or_else(|| anyhow::anyhow!("no HOME to store the token in"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, format!("{token}\n"))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(path)
}

/// Client configuration: the stored token as a bearer and the tier for the audit log.
fn config(server: &str, tier: Tier) -> anyhow::Result<ClientConfig> {
    let mut headers = http::HeaderMap::new();
    if let Some(t) = stored_token() {
        headers.insert(http::header::AUTHORIZATION, format!("Bearer {t}").parse()?);
    }
    headers.insert(
        http::HeaderName::from_static(maurice_api::auth::TIER_HEADER),
        tier.to_string().parse()?,
    );
    Ok(ClientConfig::new(server.parse()?).with_default_headers(headers))
}

fn parse_tls(s: &str) -> anyhow::Result<TlsMode> {
    match s {
        "auto" => Ok(TlsMode::TLS_MODE_AUTO),
        "internal" => Ok(TlsMode::TLS_MODE_INTERNAL),
        "off" => Ok(TlsMode::TLS_MODE_OFF),
        other => anyhow::bail!("tls must be auto, internal or off, got {other}"),
    }
}

pub async fn route(cmd: RouteCommand, server: &str, tier: Tier) -> anyhow::Result<()> {
    let client = EdgeClient::new(Client::plaintext(), config(server, tier)?);
    match cmd {
        RouteCommand::List { workspace } => {
            let resp = client
                .list_routes(ListRoutesRequest {
                    workspace_id: workspace,
                    ..Default::default()
                })
                .await?
                .into_owned();
            if resp.routes.is_empty() {
                println!("no routes");
            }
            for r in &resp.routes {
                let domains: Vec<String> = r
                    .domains
                    .iter()
                    .map(|d| {
                        format!(
                            "{} ({})",
                            d.name,
                            format!("{:?}", d.tls)
                                .to_lowercase()
                                .replace("tls_mode_", "")
                        )
                    })
                    .collect();
                println!(
                    "{:<36} :{:<6} → {:<8} {:<10}:{:<6} {:<9} {:<28} {}",
                    r.id,
                    r.block_port,
                    env_label(r.environment.as_known()),
                    r.service,
                    r.container_port,
                    format!("{:?}", r.visibility)
                        .to_lowercase()
                        .replace("visibility_", ""),
                    if r.upstream.is_empty() {
                        "(not deployed)".to_string()
                    } else {
                        r.upstream.clone()
                    },
                    domains.join(", ")
                );
            }
        }
        RouteCommand::Status { workspace } => {
            let s = client
                .get_edge_status(GetEdgeStatusRequest {
                    workspace_id: workspace,
                    ..Default::default()
                })
                .await?
                .into_owned();
            println!(
                "host edge: {}  workspace edge: {}  internal address: {}",
                s.host_edge,
                s.workspace_edge,
                if s.internal_addr.is_empty() {
                    "-"
                } else {
                    &s.internal_addr
                }
            );
        }
        RouteCommand::Expose {
            service,
            port,
            internal,
            block_port,
            domain,
            tls,
            public_ip,
        } => {
            let p = client
                .plan_expose(PlanExposeRequest {
                    service_id: service,
                    container_port: port,
                    visibility: if internal {
                        Visibility::VISIBILITY_INTERNAL
                    } else {
                        Visibility::VISIBILITY_PUBLIC
                    }
                    .into(),
                    block_port,
                    domain: domain.unwrap_or_default(),
                    public_ip: public_ip.unwrap_or_default(),
                    tls: parse_tls(&tls)?.into(),
                    ..Default::default()
                })
                .await?
                .into_owned();
            print_plan(&p);
        }
        RouteCommand::Unexpose { route } => {
            let p = client
                .plan_unexpose(PlanUnexposeRequest {
                    route_id: route,
                    ..Default::default()
                })
                .await?
                .into_owned();
            print_plan(&p);
        }
        RouteCommand::AddDomain {
            route,
            domain,
            tls,
            public_ip,
        } => {
            let p = client
                .plan_add_domain(PlanAddDomainRequest {
                    route_id: route,
                    domain,
                    public_ip: public_ip.unwrap_or_default(),
                    tls: parse_tls(&tls)?.into(),
                    ..Default::default()
                })
                .await?
                .into_owned();
            print_plan(&p);
        }
        RouteCommand::RemoveDomain { domain } => {
            let p = client
                .plan_remove_domain(PlanRemoveDomainRequest {
                    domain_id: domain,
                    ..Default::default()
                })
                .await?
                .into_owned();
            print_plan(&p);
        }
    }
    Ok(())
}

pub async fn auth(cmd: AuthCommand, server: &str, tier: Tier) -> anyhow::Result<()> {
    let client = AuthClient::new(Client::plaintext(), config(server, tier)?);
    match cmd {
        AuthCommand::Login { user } => {
            let login = match user {
                Some(u) => u,
                None => {
                    eprint!("login: ");
                    let mut l = String::new();
                    std::io::stdin().read_line(&mut l)?;
                    l.trim().to_string()
                }
            };
            // Scripts pass the password in the environment; humans are prompted.
            let password = match std::env::var("MAURICE_PASSWORD") {
                Ok(p) if !p.is_empty() => p,
                _ => rpassword::prompt_password("password: ")?,
            };
            let host = std::fs::read_to_string("/etc/hostname")
                .map(|h| h.trim().to_string())
                .unwrap_or_else(|_| "host".into());
            let resp = client
                .login(LoginRequest {
                    login: login.clone(),
                    password,
                    create_token: true,
                    token_name: format!("cli@{host}"),
                    ..Default::default()
                })
                .await?
                .into_owned();
            let path = save_token(&resp.token)?;
            let role = resp
                .user
                .as_option()
                .map(|u| format!("{:?}", u.role).to_lowercase().replace("role_", ""))
                .unwrap_or_default();
            println!(
                "signed in as {login} ({role}); token stored in {}",
                path.display()
            );
        }
        AuthCommand::Logout => {
            if let Some(p) = token_path()
                && p.exists()
            {
                std::fs::remove_file(&p)?;
                println!("token removed from {}", p.display());
            } else {
                println!("no stored token");
            }
        }
        AuthCommand::Whoami => {
            let u = client.me(MeRequest::default()).await?.into_owned();
            println!(
                "{} ({}){}",
                u.login,
                format!("{:?}", u.role).to_lowercase().replace("role_", ""),
                if u.disabled { "  [disabled]" } else { "" }
            );
        }
    }
    Ok(())
}

fn parse_env(s: &str) -> anyhow::Result<Env> {
    match s {
        "dev" => Ok(Env::ENV_DEV),
        "staging" => Ok(Env::ENV_STAGING),
        "prod" => Ok(Env::ENV_PROD),
        other => anyhow::bail!("env must be dev, staging or prod, got {other}"),
    }
}

fn env_label(e: Option<Env>) -> &'static str {
    match e {
        Some(Env::ENV_DEV) => "dev",
        Some(Env::ENV_STAGING) => "staging",
        Some(Env::ENV_PROD) => "prod",
        _ => "-",
    }
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

pub async fn workspace(cmd: WorkspaceCommand, server: &str, tier: Tier) -> anyhow::Result<()> {
    let client = WorkspacesClient::new(Client::plaintext(), config(server, tier)?);
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
                let envs: Vec<&str> = w
                    .environments
                    .iter()
                    .map(|e| env_label(e.as_known()))
                    .collect();
                println!(
                    "{:<36} {:<16} {:<18} {:<12} uid {:<6} ports {}-{}  subuid {}+{}  {}",
                    w.id,
                    w.name,
                    envs.join(","),
                    w.unix_user,
                    w.uid,
                    w.port_block_start,
                    w.port_block_start + w.port_block_size - 1,
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
        WorkspaceCommand::Create {
            name,
            envs,
            block_size,
        } => {
            let environments = envs
                .iter()
                .map(|e| parse_env(e).map(Into::into))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let p = client
                .plan_create(PlanCreateWorkspaceRequest {
                    name,
                    environments,
                    port_block_size: block_size,
                    ..Default::default()
                })
                .await?
                .into_owned();
            print_plan(&p);
        }
        WorkspaceCommand::Ports { id } => {
            let p = client
                .get_ports(GetPortsRequest {
                    workspace_id: id,
                    ..Default::default()
                })
                .await?
                .into_owned();
            println!(
                "block {}-{} ({} ports), next free {}",
                p.block_start,
                p.block_start + p.block_size - 1,
                p.block_size,
                if p.next_free == 0 {
                    "none".to_string()
                } else {
                    p.next_free.to_string()
                }
            );
            for u in &p.used {
                println!(
                    "  {}:{:<6} {:<8} {}",
                    u.host_ip,
                    u.host_port,
                    env_label(u.environment.as_known()),
                    u.service
                );
            }
        }
        WorkspaceCommand::AddEnv { id, env } => {
            let p = client
                .plan_add_environment(PlanAddEnvironmentRequest {
                    workspace_id: id,
                    environment: parse_env(&env)?.into(),
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
    let client = PlansClient::new(Client::plaintext(), config(server, tier)?);
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

pub async fn service(cmd: ServiceCommand, server: &str, tier: Tier) -> anyhow::Result<()> {
    let client = ServicesClient::new(Client::plaintext(), config(server, tier)?);
    match cmd {
        ServiceCommand::List { workspace, env } => {
            let environment = match env {
                Some(e) => parse_env(&e)?.into(),
                None => Env::ENV_UNSPECIFIED.into(),
            };
            let resp = client
                .list(ListServicesRequest {
                    workspace_id: workspace,
                    environment,
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
                    "{:<36} {:<8} {:<14} {:<10} {:<40} ports {}",
                    s.id,
                    env_label(s.environment.as_known()),
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
        ServiceCommand::Stats { id, count } => {
            let mut stream = client
                .stats(StatsRequest {
                    service_id: id,
                    ..Default::default()
                })
                .await?;
            let mut n = 0;
            println!(
                "{:<10} {:>8} {:>10} {:>12} {:>12}",
                "time", "cpu", "memory", "net rx", "net tx"
            );
            while let Some(msg) = stream.message().await? {
                let s = msg.to_owned_message();
                let t = s.time.as_option().map(|t| t.seconds).unwrap_or(0);
                if s.memory_bytes == 0 && s.cpu_cores == 0.0 {
                    println!("{t:<10} (not running)");
                } else {
                    println!(
                        "{t:<10} {:>7.2}c {:>6} MiB {:>12} {:>12}",
                        s.cpu_cores,
                        s.memory_bytes / 1_048_576,
                        s.net_rx_bytes,
                        s.net_tx_bytes
                    );
                }
                n += 1;
                if count > 0 && n >= count {
                    break;
                }
            }
        }
        ServiceCommand::Logs { id, tail, follow } => {
            let mut stream = client
                .logs(LogsRequest {
                    service_id: id,
                    tail,
                    follow,
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
            env,
        } => {
            let text = std::fs::read_to_string(&file)?;
            let svc: Service = serde_json::from_str(&text)?;
            let environment = match env {
                Some(e) => parse_env(&e)?.into(),
                None => Env::ENV_UNSPECIFIED.into(),
            };
            let p = client
                .plan_upsert(PlanUpsertServiceRequest {
                    workspace_id: workspace,
                    project,
                    service: svc.into(),
                    environment,
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

pub async fn secret(cmd: SecretCommand, server: &str, tier: Tier) -> anyhow::Result<()> {
    let client = SecretsClient::new(Client::plaintext(), config(server, tier)?);
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
                let scope = match s.environment.as_known() {
                    Some(Env::ENV_UNSPECIFIED) | None => "shared",
                    _ => env_label(s.environment.as_known()),
                };
                println!("{:<36} {:<8} {:<24} v{}", s.id, scope, s.name, s.version);
            }
        }
        SecretCommand::Set {
            workspace,
            name,
            env,
        } => {
            let environment = match env {
                Some(e) => parse_env(&e)?.into(),
                None => Env::ENV_UNSPECIFIED.into(),
            };
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
                    environment,
                    ..Default::default()
                })
                .await?
                .into_owned();
            println!("secret {name} stored as version {}", info.version);
        }
    }
    Ok(())
}

pub async fn project(cmd: ProjectCommand, server: &str, tier: Tier) -> anyhow::Result<()> {
    let client = ProjectsClient::new(Client::plaintext(), config(server, tier)?);
    match cmd {
        ProjectCommand::List { workspace } => {
            let resp = client
                .list(ListProjectsRequest {
                    workspace_id: workspace,
                    ..Default::default()
                })
                .await?
                .into_owned();
            if resp.projects.is_empty() {
                println!("no projects");
            }
            for p in &resp.projects {
                println!(
                    "{:<36} {:<14} {} @ {}{}",
                    p.id,
                    p.name,
                    if p.git_repo.is_empty() {
                        "(no repository)"
                    } else {
                        &p.git_repo
                    },
                    p.git_ref,
                    if p.git_key_secret.is_empty() {
                        String::new()
                    } else {
                        format!("  key: {}", p.git_key_secret)
                    }
                );
            }
        }
        ProjectCommand::Set {
            workspace,
            name,
            repo,
            git_ref,
            key_secret,
        } => {
            let p = client
                .upsert(UpsertProjectRequest {
                    workspace_id: workspace,
                    name,
                    git_repo: repo.unwrap_or_default(),
                    git_ref,
                    git_key_secret: key_secret.unwrap_or_default(),
                    ..Default::default()
                })
                .await?
                .into_owned();
            println!(
                "project {} → {} @ {}",
                p.name,
                if p.git_repo.is_empty() {
                    "(no repository)"
                } else {
                    &p.git_repo
                },
                p.git_ref
            );
        }
    }
    Ok(())
}
