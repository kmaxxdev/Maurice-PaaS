//! The real agent. Runs as root on the host and performs exactly the
//! operations of the [`Agent`] trait through fixed system tools: `useradd`,
//! `usermod`, `userdel`, `loginctl`, `systemctl --user -M`, `getent`. Every
//! argument is validated before it reaches a command line, and no command is
//! ever composed from free text.

use crate::{
    Agent, AgentError, BuildResult, BuildSpec, JournalLine, NewUser, Result, UnitStatus, UserRef,
};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Where Quadlet looks for user units, relative to the user's home.
const QUADLET_DIR: &str = ".config/containers/systemd";

pub struct HostAgent {
    /// Shell for workspace users; they never log in interactively.
    pub nologin: PathBuf,
}

impl Default for HostAgent {
    fn default() -> Self {
        Self {
            nologin: PathBuf::from("/usr/sbin/nologin"),
        }
    }
}

impl HostAgent {
    pub fn new() -> Self {
        Self::default()
    }

    /// Refuse to run unless we are root; nothing below works otherwise.
    pub fn check_privileges() -> Result<()> {
        if unsafe_geteuid() != 0 {
            return Err(AgentError::Refused(
                "the host agent must run as root".into(),
            ));
        }
        Ok(())
    }

    async fn run(&self, program: &str, args: &[&str]) -> Result<String> {
        tracing::debug!(program, ?args, "exec");
        let out = Command::new(program)
            .args(args)
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .env("LC_ALL", "C")
            .output()
            .await
            .map_err(|e| AgentError::Host(format!("{program}: {e}")))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(AgentError::Host(format!(
                "{program} {}: {stderr}",
                args.join(" ")
            )))
        }
    }

    /// Run a command inside the user's manager (`systemd-run -M user@ --user`), feeding stdin.
    async fn run_as_user_with_stdin(
        &self,
        user: &UserRef,
        args: &[&str],
        stdin: &[u8],
    ) -> Result<String> {
        self.run_as_user_env(user, &[], args, stdin).await
    }

    /// Same, with extra `KEY=value` environment entries for the command.
    async fn run_as_user_env(
        &self,
        user: &UserRef,
        env: &[&str],
        args: &[&str],
        stdin: &[u8],
    ) -> Result<String> {
        use tokio::io::AsyncWriteExt;
        let machine = format!("{}@", user.name);
        let mut full = vec![
            "--quiet",
            "--wait",
            "--pipe",
            "--collect",
            "-M",
            machine.as_str(),
            "--user",
        ];
        for e in env {
            full.push("-E");
            full.push(e);
        }
        full.extend_from_slice(args);
        tracing::debug!(?full, "systemd-run");
        let mut child = Command::new("systemd-run")
            .args(&full)
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .env("LC_ALL", "C")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| AgentError::Host(format!("systemd-run: {e}")))?;
        if let Some(mut si) = child.stdin.take() {
            si.write_all(stdin)
                .await
                .map_err(|e| AgentError::Host(e.to_string()))?;
            drop(si);
        }
        let out = child
            .wait_with_output()
            .await
            .map_err(|e| AgentError::Host(e.to_string()))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(AgentError::Host(format!(
                "systemd-run {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )))
        }
    }

    /// `systemctl --user` of another user, as root, through systemd's own
    /// machine switch. This is the documented way to drive a user manager
    /// without a login session.
    async fn user_systemctl(&self, user: &UserRef, args: &[&str]) -> Result<String> {
        let machine = format!("{}@", user.name);
        let mut full = vec!["--user", "-M", machine.as_str()];
        full.extend_from_slice(args);
        self.run("systemctl", &full).await
    }

    /// `user@<uid>.service` is what `systemd --user` runs in; starting it blocks
    /// until the manager is up, then we confirm it accepts connections.
    async fn wait_for_user_manager(&self, uid: u32, name: &str) -> Result<()> {
        let unit = format!("user@{uid}.service");
        self.run("systemctl", &["start", &unit]).await?;
        let machine = format!("{name}@");
        for _ in 0..50 {
            let ok = Command::new("systemctl")
                .args(["--user", "-M", &machine, "is-system-running", "--quiet"])
                .env_clear()
                .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
                .status()
                .await
                .map(|st| st.success())
                .unwrap_or(false);
            if ok {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        Err(AgentError::Host(format!(
            "user manager for {name} did not come up"
        )))
    }

    /// `(uid, gid, home)` from the passwd database, or `None` if the user does not exist.
    async fn passwd_entry(&self, name: &str) -> Result<Option<(u32, u32, PathBuf)>> {
        let out = Command::new("getent")
            .args(["passwd", name])
            .output()
            .await
            .map_err(|e| AgentError::Host(format!("getent: {e}")))?;
        if !out.status.success() {
            return Ok(None);
        }
        let line = String::from_utf8_lossy(&out.stdout);
        let fields: Vec<&str> = line.trim().split(':').collect();
        if fields.len() < 6 {
            return Err(AgentError::Host(format!(
                "unexpected passwd entry for {name}"
            )));
        }
        let uid = fields[2]
            .parse()
            .map_err(|_| AgentError::Host("bad uid in passwd".into()))?;
        let gid = fields[3]
            .parse()
            .map_err(|_| AgentError::Host("bad gid in passwd".into()))?;
        Ok(Some((uid, gid, PathBuf::from(fields[5]))))
    }

    /// `(gid, home)` of a verified workspace user.
    async fn home_of(&self, user: &UserRef) -> Result<(u32, PathBuf)> {
        match self.passwd_entry(&user.name).await? {
            Some((uid, gid, home)) if uid == user.uid => Ok((gid, home)),
            Some((uid, _, _)) => Err(AgentError::Refused(format!(
                "user {} has uid {uid}, expected {}",
                user.name, user.uid
            ))),
            None => Err(AgentError::NoSuchUser(user.name.clone())),
        }
    }

    /// `(gid, quadlet dir)` of a verified workspace user.
    async fn quadlet_dir(&self, user: &UserRef) -> Result<(u32, PathBuf)> {
        let (gid, home) = self.home_of(user).await?;
        Ok((gid, home.join(QUADLET_DIR)))
    }
}

/// Workspace users are `ws-<name>`; anything else is refused before it reaches a command.
pub fn validate_user_name(name: &str) -> Result<()> {
    let ok = name.len() <= 31
        && name.starts_with("ws-")
        && name.len() > 3
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.ends_with('-');
    if ok {
        Ok(())
    } else {
        Err(AgentError::Refused(format!(
            "invalid workspace user name '{name}'"
        )))
    }
}

/// Unit file names are a single path component with a Quadlet extension.
pub fn validate_unit_file_name(file_name: &str) -> Result<()> {
    let ok = !file_name.is_empty()
        && !file_name.contains('/')
        && !file_name.starts_with('.')
        && [
            ".container",
            ".network",
            ".volume",
            ".pod",
            ".kube",
            ".build",
            ".image",
        ]
        .iter()
        .any(|ext| file_name.ends_with(ext));
    if ok {
        Ok(())
    } else {
        Err(AgentError::Refused(format!(
            "invalid unit file name '{file_name}'"
        )))
    }
}

/// Unit names passed to systemctl: `name.service` or a Quadlet-generated service name.
pub fn validate_unit_name(unit: &str) -> Result<()> {
    let ok = unit.ends_with(".service")
        && unit.len() > 8
        && unit
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'@');
    if ok {
        Ok(())
    } else {
        Err(AgentError::Refused(format!("invalid unit name '{unit}'")))
    }
}

pub fn validate_secret_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.');
    if ok {
        Ok(())
    } else {
        Err(AgentError::Refused(format!("invalid secret name '{name}'")))
    }
}

fn validate_checkout_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(AgentError::Refused(format!(
            "invalid checkout name '{name}'"
        )))
    }
}

/// Paths inside a repository: relative, no parent traversal, no leading dash.
fn validate_rel_path(p: &str) -> Result<()> {
    let ok = !p.is_empty()
        && !p.starts_with('/')
        && !p.starts_with('-')
        && !std::path::Path::new(p)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir));
    if ok {
        Ok(())
    } else {
        Err(AgentError::Refused(format!(
            "invalid repository path '{p}'"
        )))
    }
}

fn validate_image_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 200
        && name.bytes().all(|b| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || b == b'/'
                || b == b'-'
                || b == b'.'
                || b == b'_'
        })
        && !name.contains(':');
    if ok {
        Ok(())
    } else {
        Err(AgentError::Refused(format!("invalid image name '{name}'")))
    }
}

fn validate_home(home: &str) -> Result<()> {
    let p = Path::new(home);
    if p.is_absolute() && p.components().count() >= 3 && !home.contains("..") {
        Ok(())
    } else {
        Err(AgentError::Refused(format!(
            "home must be an absolute path at least two levels deep: '{home}'"
        )))
    }
}

#[async_trait]
impl Agent for HostAgent {
    async fn create_user(&self, user: &NewUser) -> Result<UserRef> {
        validate_user_name(&user.name)?;
        validate_home(&user.home)?;
        if self.passwd_entry(&user.name).await?.is_some() {
            return Err(AgentError::UserExists(user.name.clone()));
        }
        let nologin = self.nologin.to_string_lossy().into_owned();
        self.run(
            "useradd",
            &[
                "--system",
                "--user-group",
                "--create-home",
                "--home-dir",
                &user.home,
                "--shell",
                &nologin,
                "--comment",
                "Maurice workspace",
                &user.name,
            ],
        )
        .await?;
        let end = user.subuid_start + user.subuid_len - 1;
        let range = format!("{}-{}", user.subuid_start, end);
        self.run(
            "usermod",
            &["--add-subuids", &range, "--add-subgids", &range, &user.name],
        )
        .await?;
        self.run("loginctl", &["enable-linger", &user.name]).await?;

        let (uid, gid, home) = self
            .passwd_entry(&user.name)
            .await?
            .ok_or_else(|| AgentError::Host("user vanished after useradd".into()))?;
        // Linger starts the user manager asynchronously; wait until it answers,
        // otherwise the first `systemctl --user -M` call races it.
        self.wait_for_user_manager(uid, &user.name).await?;
        let quadlet = home.join(QUADLET_DIR);
        tokio::fs::create_dir_all(&quadlet)
            .await
            .map_err(|e| AgentError::Host(e.to_string()))?;
        chown_recursive(&home.join(".config"), uid, gid).await?;
        // Home must not be world-readable: it holds unit files and volume data.
        self.run("chmod", &["0750", &home.to_string_lossy()])
            .await?;

        tracing::info!(user = %user.name, uid, "workspace user created");
        Ok(UserRef {
            name: user.name.clone(),
            uid,
        })
    }

    async fn delete_user(&self, user: &UserRef, purge_home: bool) -> Result<()> {
        validate_user_name(&user.name)?;
        let _ = self.home_of(user).await?;
        // Stop everything the user manager runs, then the manager itself.
        let _ = self.user_systemctl(user, &["stop", "default.target"]).await;
        let _ = self.run("loginctl", &["disable-linger", &user.name]).await;
        let _ = self.run("loginctl", &["terminate-user", &user.name]).await;
        let uid = user.uid.to_string();
        let _ = self
            .run("systemctl", &["stop", &format!("user@{uid}.service")])
            .await;
        // Containers, images and volumes live under the user's home; reset as the user.
        if purge_home {
            let _ = self
                .run(
                    "systemd-run",
                    &[
                        "--wait",
                        "--pipe",
                        "-M",
                        &format!("{}@", user.name),
                        "--user",
                        "podman",
                        "system",
                        "reset",
                        "--force",
                    ],
                )
                .await;
        }
        let mut args = vec!["--force"];
        if purge_home {
            args.push("--remove");
        }
        args.push(&user.name);
        self.run("userdel", &args).await?;
        tracing::info!(user = %user.name, purge_home, "workspace user deleted");
        Ok(())
    }

    async fn write_unit(&self, user: &UserRef, file_name: &str, content: &str) -> Result<()> {
        validate_unit_file_name(file_name)?;
        let (gid, dir) = self.quadlet_dir(user).await?;
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| AgentError::Host(e.to_string()))?;
        let path = dir.join(file_name);
        let tmp = dir.join(format!(".{file_name}.tmp"));
        tokio::fs::write(&tmp, content)
            .await
            .map_err(|e| AgentError::Host(e.to_string()))?;
        std::os::unix::fs::chown(&tmp, Some(user.uid), Some(gid))
            .map_err(|e| AgentError::Host(e.to_string()))?;
        tokio::fs::rename(&tmp, &path)
            .await
            .map_err(|e| AgentError::Host(e.to_string()))?;
        tracing::info!(user = %user.name, file = %path.display(), "unit written");
        Ok(())
    }

    async fn remove_unit(&self, user: &UserRef, file_name: &str) -> Result<()> {
        validate_unit_file_name(file_name)?;
        let path = self.quadlet_dir(user).await?.1.join(file_name);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(AgentError::NoSuchUnit(file_name.into()))
            }
            Err(e) => Err(AgentError::Host(e.to_string())),
        }
    }

    async fn read_unit(&self, user: &UserRef, file_name: &str) -> Result<Option<String>> {
        validate_unit_file_name(file_name)?;
        let path = self.quadlet_dir(user).await?.1.join(file_name);
        match tokio::fs::read_to_string(&path).await {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(AgentError::Host(e.to_string())),
        }
    }

    async fn daemon_reload(&self, user: &UserRef) -> Result<()> {
        self.user_systemctl(user, &["daemon-reload"])
            .await
            .map(drop)
    }

    async fn start_unit(&self, user: &UserRef, unit: &str) -> Result<()> {
        validate_unit_name(unit)?;
        self.user_systemctl(user, &["start", unit]).await.map(drop)
    }

    async fn stop_unit(&self, user: &UserRef, unit: &str) -> Result<()> {
        validate_unit_name(unit)?;
        self.user_systemctl(user, &["stop", unit]).await.map(drop)
    }

    async fn restart_unit(&self, user: &UserRef, unit: &str) -> Result<()> {
        validate_unit_name(unit)?;
        self.user_systemctl(user, &["restart", unit])
            .await
            .map(drop)
    }

    async fn set_secret(&self, user: &UserRef, name: &str, value: &[u8]) -> Result<()> {
        validate_secret_name(name)?;
        // Replace if present: podman has no upsert, so remove first and ignore "not found".
        let _ = self
            .run_as_user_with_stdin(user, &["podman", "secret", "rm", name], b"")
            .await;
        self.run_as_user_with_stdin(user, &["podman", "secret", "create", name, "-"], value)
            .await
            .map(drop)
    }

    async fn build_image(&self, user: &UserRef, spec: &BuildSpec) -> Result<BuildResult> {
        validate_checkout_name(&spec.checkout)?;
        validate_rel_path(&spec.dockerfile)?;
        validate_rel_path(&spec.context)?;
        validate_image_name(&spec.image)?;
        if spec.git_ref.is_empty() || spec.git_ref.starts_with('-') || spec.git_ref.contains("..") {
            return Err(AgentError::Refused(format!(
                "invalid git ref '{}'",
                spec.git_ref
            )));
        }
        if spec.repo.starts_with('-') || spec.repo.chars().any(char::is_whitespace) {
            return Err(AgentError::Refused("invalid repository url".into()));
        }
        let (gid, home) = self.home_of(user).await?;
        let builds = home.join(".cache/maurice/builds");
        let dir = builds.join(&spec.checkout);
        tokio::fs::create_dir_all(&builds)
            .await
            .map_err(|e| AgentError::Host(e.to_string()))?;
        chown_recursive(&home.join(".cache"), user.uid, gid).await?;

        // Deploy key, if any, lives in the user's home with mode 0600.
        let mut git_env: Vec<String> = vec!["GIT_TERMINAL_PROMPT=0".into()];
        if let Some(key) = &spec.ssh_key {
            use std::os::unix::fs::PermissionsExt;
            let keys = home.join(".config/maurice/keys");
            tokio::fs::create_dir_all(&keys)
                .await
                .map_err(|e| AgentError::Host(e.to_string()))?;
            let key_path = keys.join(&spec.checkout);
            tokio::fs::write(&key_path, key)
                .await
                .map_err(|e| AgentError::Host(e.to_string()))?;
            tokio::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
                .await
                .map_err(|e| AgentError::Host(e.to_string()))?;
            chown_recursive(&home.join(".config/maurice"), user.uid, gid).await?;
            git_env.push(format!(
                "GIT_SSH_COMMAND=ssh -i {} -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new",
                key_path.display()
            ));
        }

        let dir_s = dir.to_string_lossy().into_owned();
        // Fresh shallow clone every time: builds come from the ref, not from leftovers.
        self.run_as_user_env(user, &[], &["rm", "-rf", &dir_s], b"")
            .await?;
        let env_refs: Vec<&str> = git_env.iter().map(String::as_str).collect();
        self.run_as_user_env(
            user,
            &env_refs,
            &[
                "git",
                "clone",
                "--quiet",
                "--depth",
                "1",
                "--branch",
                &spec.git_ref,
                "--",
                &spec.repo,
                &dir_s,
            ],
            b"",
        )
        .await?;
        let sha = self
            .run_as_user_env(user, &[], &["git", "-C", &dir_s, "rev-parse", "HEAD"], b"")
            .await?
            .trim()
            .to_string();
        let short = sha.get(..12).unwrap_or(&sha).to_string();
        let image_ref = format!("{}:{short}", spec.image);
        let dockerfile = dir.join(&spec.dockerfile).to_string_lossy().into_owned();
        let context = dir.join(&spec.context).to_string_lossy().into_owned();
        tracing::info!(user = %user.name, %image_ref, "building image");
        let out = self
            .run_as_user_env(
                user,
                &[],
                &[
                    "podman",
                    "build",
                    "--pull=newer",
                    "-t",
                    &image_ref,
                    "-f",
                    &dockerfile,
                    &context,
                ],
                b"",
            )
            .await?;
        let lines: Vec<&str> = out.lines().collect();
        let log_tail = lines[lines.len().saturating_sub(20)..].join("\n");
        Ok(BuildResult {
            git_sha: sha,
            image_ref,
            log_tail,
        })
    }

    async fn read_journal(
        &self,
        user: &UserRef,
        unit: &str,
        tail: u32,
        since_unix: Option<u64>,
    ) -> Result<Vec<JournalLine>> {
        validate_unit_name(unit)?;
        let uid = user.uid.to_string();
        let n = tail.clamp(1, 5000).to_string();
        let since = since_unix.map(|t| format!("@{t}"));
        let uid_f = format!("_UID={uid}");
        let unit_f = format!("_SYSTEMD_USER_UNIT={unit}");
        let mut args = vec!["--no-pager", "-o", "json", "-n", &n, &uid_f, &unit_f];
        if let Some(s) = &since {
            args.push("--since");
            args.push(s);
        }
        let out = self.run("journalctl", &&args).await?;
        let mut lines = Vec::new();
        for raw in out.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
                continue;
            };
            let msg = match &v["MESSAGE"] {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Array(bytes) => String::from_utf8_lossy(
                    &bytes
                        .iter()
                        .filter_map(|b| b.as_u64().map(|b| b as u8))
                        .collect::<Vec<_>>(),
                )
                .into_owned(),
                _ => String::new(),
            };
            lines.push(JournalLine {
                realtime_usec: v["__REALTIME_TIMESTAMP"]
                    .as_str()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
                priority: v["PRIORITY"]
                    .as_str()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(6),
                identifier: v["SYSLOG_IDENTIFIER"].as_str().unwrap_or("").to_string(),
                message: msg,
            });
        }
        Ok(lines)
    }

    async fn unit_status(&self, user: &UserRef, unit: &str) -> Result<UnitStatus> {
        validate_unit_name(unit)?;
        let out = self
            .user_systemctl(
                user,
                &[
                    "show",
                    "-p",
                    "ActiveState,SubState,ActiveEnterTimestamp",
                    unit,
                ],
            )
            .await?;
        let mut status = UnitStatus {
            unit: unit.into(),
            active_state: String::new(),
            sub_state: String::new(),
            since_unix: None,
        };
        for line in out.lines() {
            if let Some((k, v)) = line.split_once('=') {
                match k {
                    "ActiveState" => status.active_state = v.into(),
                    "SubState" => status.sub_state = v.into(),
                    "ActiveEnterTimestamp" => status.since_unix = parse_systemd_timestamp(v),
                    _ => {}
                }
            }
        }
        Ok(status)
    }
}

async fn chown_recursive(root: &Path, uid: u32, gid: u32) -> Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(p) = stack.pop() {
        std::os::unix::fs::chown(&p, Some(uid), Some(gid))
            .map_err(|e| AgentError::Host(e.to_string()))?;
        if p.is_dir() {
            let mut rd = tokio::fs::read_dir(&p)
                .await
                .map_err(|e| AgentError::Host(e.to_string()))?;
            while let Some(entry) = rd
                .next_entry()
                .await
                .map_err(|e| AgentError::Host(e.to_string()))?
            {
                stack.push(entry.path());
            }
        }
    }
    Ok(())
}

/// systemd prints `Thu 2026-10-23 08:55:07 UTC`; we only need a rough epoch, so use `date` semantics lazily.
fn parse_systemd_timestamp(v: &str) -> Option<u64> {
    if v.is_empty() {
        return None;
    }
    // Keep it dependency-free: "DOW YYYY-MM-DD HH:MM:SS TZ"
    let mut it = v.split_whitespace();
    let _dow = it.next()?;
    let date = it.next()?;
    let time = it.next()?;
    let (y, m, d) = {
        let mut p = date.split('-');
        (
            p.next()?.parse::<i64>().ok()?,
            p.next()?.parse::<i64>().ok()?,
            p.next()?.parse::<i64>().ok()?,
        )
    };
    let (hh, mm, ss) = {
        let mut p = time.split(':');
        (
            p.next()?.parse::<i64>().ok()?,
            p.next()?.parse::<i64>().ok()?,
            p.next()?.parse::<i64>().ok()?,
        )
    };
    // days from civil (Howard Hinnant's algorithm)
    let (y2, m2) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some((days * 86400 + hh * 3600 + mm * 60 + ss).max(0) as u64)
}

fn unsafe_geteuid() -> u32 {
    // Reading /proc is the dependency-free way to learn our uid without libc bindings.
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|u| u.parse().ok())
        })
        .unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_names() {
        assert!(validate_user_name("ws-shop").is_ok());
        assert!(validate_user_name("ws-shop-2").is_ok());
        assert!(validate_user_name("shop").is_err());
        assert!(validate_user_name("ws-").is_err());
        assert!(validate_user_name("ws-Shop").is_err());
        assert!(validate_user_name("ws-a b").is_err());
        assert!(validate_user_name("ws-../x").is_err());
    }

    #[test]
    fn unit_file_names() {
        assert!(validate_unit_file_name("web.container").is_ok());
        assert!(validate_unit_file_name("shop-net.network").is_ok());
        assert!(validate_unit_file_name("../web.container").is_err());
        assert!(validate_unit_file_name("web.service").is_err());
        assert!(validate_unit_file_name(".hidden.container").is_err());
    }

    #[test]
    fn unit_names() {
        assert!(validate_unit_name("postgres.service").is_ok());
        assert!(validate_unit_name("web@a1b2.service").is_ok());
        assert!(validate_unit_name("postgres").is_err());
        assert!(validate_unit_name("x; rm -rf /.service").is_err());
    }

    #[test]
    fn systemd_timestamp() {
        assert_eq!(
            parse_systemd_timestamp("Thu 2026-10-23 08:55:07 UTC"),
            Some(1_792_745_707)
        );
        assert_eq!(parse_systemd_timestamp(""), None);
    }
}
