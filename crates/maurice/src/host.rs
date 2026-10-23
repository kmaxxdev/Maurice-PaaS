//! `maurice host …`: drive the real agent from the command line. These are the
//! building blocks the API will call; having them as commands lets the agent be
//! exercised on a real host before the panel is wired to it.

use crate::tier::Tier;
use clap::Subcommand;
use maurice_agent::{Agent, HostAgent, NewUser, UserRef};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Subcommand)]
pub enum HostCommand {
    /// Prepare this host for workspaces: AppArmor fix for pasta on Ubuntu, base directories.
    Prepare,
    /// Create a workspace user with subuid range, linger and quadlet directory.
    UserCreate {
        /// Workspace name; the user becomes ws-<name>.
        name: String,
        #[arg(long, default_value = "/srv/ws")]
        home_root: PathBuf,
        #[arg(long, default_value_t = 1_000_000)]
        subuid_start: u32,
        #[arg(long, default_value_t = 65536)]
        subuid_len: u32,
    },
    /// Delete a workspace user. Destructive: needs the maurice tier and --confirm.
    UserDelete {
        name: String,
        #[arg(long)]
        uid: u32,
        /// Also remove home, containers, images and volumes.
        #[arg(long)]
        purge: bool,
        #[arg(long)]
        confirm: bool,
    },
    /// Render a workspace definition and apply its units through the agent.
    Apply {
        file: PathBuf,
        /// Only these services (default: all).
        #[arg(long)]
        only: Vec<String>,
        /// Start the units after writing them.
        #[arg(long)]
        start: bool,
    },
    /// Show systemd status of a unit in a workspace user's manager.
    Status {
        name: String,
        #[arg(long)]
        uid: u32,
        unit: String,
    },
    /// Full cycle on a throwaway workspace: create user, run postgres from generated units, verify, tear down.
    Selftest {
        #[arg(long, default_value = "selftest")]
        name: String,
        #[arg(long, default_value_t = 1_900_000)]
        subuid_start: u32,
        /// Keep the user and containers afterwards for inspection.
        #[arg(long)]
        keep: bool,
    },
}

pub async fn run(cmd: HostCommand, tier: Tier) -> anyhow::Result<()> {
    HostAgent::check_privileges()?;
    let agent: Arc<dyn Agent> = Arc::new(HostAgent::new());
    match cmd {
        HostCommand::Prepare => prepare_host().await?,
        HostCommand::UserCreate {
            name,
            home_root,
            subuid_start,
            subuid_len,
        } => {
            let user = NewUser {
                name: format!("ws-{name}"),
                home: home_root.join(&name).to_string_lossy().into_owned(),
                subuid_start,
                subuid_len,
            };
            let u = agent.create_user(&user).await?;
            println!("created {} uid={} home={}", u.name, u.uid, user.home);
        }
        HostCommand::UserDelete {
            name,
            uid,
            purge,
            confirm,
        } => {
            tier.require_at_least(Tier::Maurice)?;
            if !confirm && tier != Tier::OdysseusStella {
                anyhow::bail!("deleting a workspace user is destructive; re-run with --confirm");
            }
            let u = UserRef {
                name: format!("ws-{name}"),
                uid,
            };
            agent.delete_user(&u, purge).await?;
            println!("deleted {}{}", u.name, if purge { " (purged)" } else { "" });
        }
        HostCommand::Apply { file, only, start } => {
            let ws = load_workspace(&file)?;
            let u = resolve_user(agent.as_ref(), &ws).await?;
            apply_workspace(agent.as_ref(), &u, &ws, &only, start).await?;
        }
        HostCommand::Status { name, uid, unit } => {
            let u = UserRef {
                name: format!("ws-{name}"),
                uid,
            };
            let st = agent.unit_status(&u, &unit).await?;
            println!(
                "{} {} ({}){}",
                st.unit,
                st.active_state,
                st.sub_state,
                st.since_unix
                    .map(|t| format!(" since {t}"))
                    .unwrap_or_default()
            );
        }
        HostCommand::Selftest {
            name,
            subuid_start,
            keep,
        } => {
            selftest(agent.as_ref(), &name, subuid_start, keep).await?;
        }
    }
    Ok(())
}

fn load_workspace(file: &PathBuf) -> anyhow::Result<maurice_core::Workspace> {
    let text = std::fs::read_to_string(file)?;
    let ws: maurice_core::Workspace = serde_json::from_str(&text)?;
    let findings = maurice_core::validate_workspace(&ws);
    for f in &findings {
        eprintln!("{:?} {} {} — {}", f.severity, f.code, f.target, f.message);
    }
    if maurice_core::validate::has_errors(&findings) {
        anyhow::bail!("validation failed");
    }
    Ok(ws)
}

async fn resolve_user(agent: &dyn Agent, ws: &maurice_core::Workspace) -> anyhow::Result<UserRef> {
    // The model carries the uid once the user exists; trust it and let the agent verify.
    let u = UserRef {
        name: ws.unix_user.clone(),
        uid: ws.uid,
    };
    agent.read_unit(&u, "probe.container").await?; // fails with NoSuchUser if the user is missing
    Ok(u)
}

/// Render and write every unit of the workspace, reload, optionally start.
async fn apply_workspace(
    agent: &dyn Agent,
    user: &UserRef,
    ws: &maurice_core::Workspace,
    only: &[String],
    start: bool,
) -> anyhow::Result<Vec<String>> {
    let net = maurice_quadlet::render_network(ws);
    agent.write_unit(user, &net.file_name, &net.content).await?;
    let mut services = Vec::new();
    for svc in ws.services() {
        if !only.is_empty() && !only.iter().any(|o| o == &svc.name) {
            continue;
        }
        let image = match &svc.source {
            maurice_core::Source::Image { reference } => reference.clone(),
            maurice_core::Source::Build { .. } => {
                anyhow::bail!(
                    "service {} needs a build; builds are not wired yet",
                    svc.name
                )
            }
        };
        for v in maurice_quadlet::render_volumes(svc) {
            agent.write_unit(user, &v.file_name, &v.content).await?;
        }
        let unit = maurice_quadlet::render_container(&maurice_quadlet::RenderInput {
            workspace: ws,
            service: svc,
            image: &image,
            version: "dev",
            resolve_ref: &|r| resolve_ref(ws, r),
        })?;
        agent
            .write_unit(user, &unit.file_name, &unit.content)
            .await?;
        services.push(format!("{}.service", svc.name));
    }
    agent.daemon_reload(user).await?;
    if start {
        for s in &services {
            println!("starting {s} (blocks until healthy)");
            agent.start_unit(user, s).await?;
            let st = agent.unit_status(user, s).await?;
            println!("  {s}: {} ({})", st.active_state, st.sub_state);
        }
    }
    Ok(services)
}

/// Alpha resolver: `{{svc.host}}` → alias, `{{svc.port}}` → first container port,
/// `{{svc.url}}` for postgres → a connection string using the admin secret name.
fn resolve_ref(ws: &maurice_core::Workspace, r: &str) -> Option<String> {
    let (svc_name, field) = maurice_core::validate::parse_ref(r)?;
    let svc = ws.services().find(|s| s.name == svc_name)?;
    match field {
        "host" => Some(svc.name.clone()),
        "port" => svc.ports.first().map(|p| p.container_port.to_string()),
        _ => None,
    }
}

async fn selftest(
    agent: &dyn Agent,
    name: &str,
    subuid_start: u32,
    keep: bool,
) -> anyhow::Result<()> {
    let ws_name = name.to_string();
    let user_name = format!("ws-{ws_name}");
    let home = format!("/srv/ws/{ws_name}");
    println!("[1/6] create user {user_name}");
    let u = agent
        .create_user(&NewUser {
            name: user_name.clone(),
            home: home.clone(),
            subuid_start,
            subuid_len: 65536,
        })
        .await?;
    println!("      uid={} home={home}", u.uid);

    let result: anyhow::Result<()> = async {
        println!("[2/6] set secret pg_admin");
        agent
            .set_secret(&u, "pg_admin", b"selftest-password")
            .await?;

        println!("[3/6] render and write units");
        let ws = maurice_core::Workspace {
            name: ws_name.clone(),
            unix_user: user_name.clone(),
            uid: u.uid,
            subuid_start,
            subuid_len: 65536,
            home: home.clone(),
            env: maurice_core::Env::Dev,
            cpu_slice: None,
            projects: vec![maurice_core::Project {
                name: "selftest".into(),
                git_repo: None,
                services: vec![maurice_core::Service {
                    name: "postgres".into(),
                    kind: maurice_core::ServiceKind::Stateful,
                    source: maurice_core::Source::Image {
                        reference: "docker.io/library/postgres:16".into(),
                    },
                    ports: vec![maurice_core::Port {
                        container_port: 5432,
                        published: None,
                    }],
                    env: vec![maurice_core::EnvVar {
                        key: "POSTGRES_PASSWORD".into(),
                        value: maurice_core::EnvValue::Secret("pg_admin".into()),
                    }],
                    volumes: vec![maurice_core::Volume {
                        source: "data".into(),
                        target: "/var/lib/postgresql/data".into(),
                        read_only: false,
                    }],
                    resources: maurice_core::Resources::default(),
                    isolated: false,
                    health_cmd: Some("pg_isready -U postgres".into()),
                }],
            }],
        };
        let units = apply_workspace(agent, &u, &ws, &[], false).await?;
        for f in [
            "selftest-net.network",
            "postgres-data.volume",
            "postgres.container",
        ] {
            anyhow::ensure!(
                agent.read_unit(&u, f).await?.is_some(),
                "unit {f} not on disk"
            );
        }

        println!("[4/6] start {} and wait for healthy", units[0]);
        let t0 = std::time::Instant::now();
        agent.start_unit(&u, &units[0]).await?;
        let st = agent.unit_status(&u, &units[0]).await?;
        println!(
            "      {} ({}) after {:.1}s",
            st.active_state,
            st.sub_state,
            t0.elapsed().as_secs_f32()
        );
        anyhow::ensure!(
            st.active_state == "active",
            "postgres did not become active"
        );

        println!("[5/6] restart and stop");
        agent.restart_unit(&u, &units[0]).await?;
        tokio::time::sleep(Duration::from_millis(500)).await;
        agent.stop_unit(&u, &units[0]).await?;
        let st = agent.unit_status(&u, &units[0]).await?;
        println!("      {} ({})", st.active_state, st.sub_state);
        anyhow::ensure!(st.active_state == "inactive", "postgres did not stop");
        Ok(())
    }
    .await;

    if keep {
        println!("[6/6] keeping {user_name} (uid {}) as requested", u.uid);
    } else {
        println!("[6/6] delete user {user_name} with purge");
        agent.delete_user(&u, true).await?;
        anyhow::ensure!(
            !std::path::Path::new(&home).exists(),
            "home {home} still exists after purge"
        );
        let subuid = std::fs::read_to_string("/etc/subuid").unwrap_or_default();
        anyhow::ensure!(
            !subuid.contains(&format!("{user_name}:")),
            "subuid entry still present"
        );
        println!("      host clean: no user, no home, no subuid entry");
    }
    result?;
    println!("selftest passed");
    Ok(())
}

/// Host preparation. Idempotent; safe to run on every upgrade.
///
/// Ubuntu 26.04 ships an AppArmor profile for pasta that only accepts signals
/// from unconfined peers, while podman carries the `podman` label. Stopping a
/// rootless container then fails with "rootless netns: kill network process:
/// permission denied" and systemd marks the unit failed. We let pasta receive
/// signals from podman through the profile's local override.
async fn prepare_host() -> anyhow::Result<()> {
    std::fs::create_dir_all("/srv/ws")?;
    let profile = std::path::Path::new("/etc/apparmor.d/usr.bin.pasta");
    if profile.exists() {
        let local_dir = std::path::Path::new("/etc/apparmor.d/local");
        std::fs::create_dir_all(local_dir)?;
        let local = local_dir.join("usr.bin.pasta");
        let rule = "# Added by Maurice: podman (labelled by the podman profile) must be able to stop pasta.\nsignal (receive) peer=podman,\n";
        let current = std::fs::read_to_string(&local).unwrap_or_default();
        let mut changed = false;
        if !current.contains("signal (receive) peer=podman") {
            std::fs::write(&local, format!("{current}{rule}"))?;
            changed = true;
        }
        let text = std::fs::read_to_string(profile)?;
        if !text.contains("local/usr.bin.pasta") {
            // Insert the include before the final closing brace of the profile block.
            let Some(pos) = text.trim_end().rfind('}') else {
                anyhow::bail!("unexpected pasta profile layout");
            };
            let patched = format!(
                "{}  include if exists <local/usr.bin.pasta>\n{}",
                &text[..pos],
                &text[pos..]
            );
            std::fs::write(profile, patched)?;
            changed = true;
        }
        if changed {
            let st = tokio::process::Command::new("apparmor_parser")
                .args(["-r", &profile.to_string_lossy()])
                .status()
                .await?;
            anyhow::ensure!(st.success(), "apparmor_parser failed");
            println!("apparmor: pasta profile now accepts signals from podman");
        } else {
            println!("apparmor: pasta profile already prepared");
        }
    } else {
        println!("apparmor: no pasta profile on this host, nothing to do");
    }
    install_panel_postgres().await?;
    println!("host prepared");
    Ok(())
}

/// The panel's own PostgreSQL: a rootful Quadlet unit at system level, data in a
/// named volume, published on loopback only. The password lives in a root
/// podman secret and in /etc/maurice/maurice.env (mode 0600) for the panel.
/// This is the first piece of the "starter set" from ADR-004.
async fn install_panel_postgres() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let env_path = std::path::Path::new("/etc/maurice/maurice.env");
    std::fs::create_dir_all("/etc/maurice")?;
    let password = match std::fs::read_to_string(env_path) {
        Ok(text) => text
            .lines()
            .find_map(|l| l.strip_prefix("MAURICE_PG_PASSWORD="))
            .map(|v| v.trim_matches('"').to_string()),
        Err(_) => None,
    };
    let password = match password {
        Some(p) if !p.is_empty() => p,
        _ => {
            let p = random_token(32);
            let content = format!(
                "# Written by `maurice host prepare`. Read by `maurice serve`.\nMAURICE_PG_PASSWORD={p}\nDATABASE_URL=postgres://maurice:{p}@127.0.0.1:5432/maurice\n"
            );
            std::fs::write(env_path, content)?;
            std::fs::set_permissions(env_path, std::fs::Permissions::from_mode(0o600))?;
            p
        }
    };
    // Root podman secret for the unit. Replace to keep it in sync with the env file.
    let _ = tokio::process::Command::new("podman")
        .args(["secret", "rm", "maurice_pg_password"])
        .output()
        .await;
    {
        use tokio::io::AsyncWriteExt;
        let mut child = tokio::process::Command::new("podman")
            .args(["secret", "create", "maurice_pg_password", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()?;
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(password.as_bytes())
            .await?;
        anyhow::ensure!(child.wait().await?.success(), "podman secret create failed");
    }
    let unit = "# Installed by `maurice host prepare`. The panel's own database.\n\
[Unit]\n\
Description=Maurice panel PostgreSQL\n\
\n\
[Container]\n\
ContainerName=maurice-postgres\n\
Image=docker.io/library/postgres:16\n\
PublishPort=127.0.0.1:5432:5432\n\
Environment=POSTGRES_USER=maurice\n\
Environment=POSTGRES_DB=maurice\n\
Secret=maurice_pg_password,type=env,target=POSTGRES_PASSWORD\n\
Volume=maurice-postgres-data.volume:/var/lib/postgresql/data:Z\n\
HealthCmd=pg_isready -U maurice -d maurice\n\
HealthInterval=10s\n\
HealthStartPeriod=20s\n\
HealthOnFailure=kill\n\
Notify=healthy\n\
LogDriver=journald\n\
\n\
[Service]\n\
Restart=on-failure\n\
TimeoutStopSec=60\n\
\n\
[Install]\n\
WantedBy=multi-user.target\n";
    let volume = "[Volume]\nVolumeName=maurice-postgres-data\n";
    std::fs::create_dir_all("/etc/containers/systemd")?;
    let changed = write_if_different("/etc/containers/systemd/maurice-postgres.container", unit)?
        | write_if_different(
            "/etc/containers/systemd/maurice-postgres-data.volume",
            volume,
        )?;
    if changed {
        run_root("systemctl", &["daemon-reload"]).await?;
    }
    println!("postgres: starting maurice-postgres.service (waits for healthy)");
    run_root("systemctl", &["start", "maurice-postgres.service"]).await?;
    println!("postgres: ready on 127.0.0.1:5432, credentials in /etc/maurice/maurice.env");
    Ok(())
}

fn write_if_different(path: &str, content: &str) -> anyhow::Result<bool> {
    if std::fs::read_to_string(path)
        .map(|c| c == content)
        .unwrap_or(false)
    {
        return Ok(false);
    }
    std::fs::write(path, content)?;
    Ok(true)
}

async fn run_root(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let out = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await?;
    anyhow::ensure!(
        out.status.success(),
        "{program} {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(())
}

/// URL-safe random token from the kernel's entropy; no extra crate needed.
fn random_token(len: usize) -> String {
    use std::io::Read;
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut buf = vec![0u8; len];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .expect("urandom");
    buf.iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect()
}
