//! The agent is the only part of Maurice that touches the host. Its surface is
//! this trait and nothing else: no "run a command", no shell. Every method is
//! one of the operations listed in ADR-002, and every call is logged.
//!
//! [`DryRunAgent`] records what it would do; [`HostAgent`] does it, as root,
//! through fixed system tools.

pub mod host;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

pub use host::HostAgent;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("user {0} already exists")]
    UserExists(String),
    #[error("user {0} does not exist")]
    NoSuchUser(String),
    #[error("unit {0} not found")]
    NoSuchUnit(String),
    #[error("operation refused: {0}")]
    Refused(String),
    #[error("host error: {0}")]
    Host(String),
}

pub type Result<T> = std::result::Result<T, AgentError>;

/// Identifies the workspace user an operation runs as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserRef {
    pub name: String,
    pub uid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewUser {
    pub name: String,
    pub home: String,
    pub subuid_start: u32,
    pub subuid_len: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitStatus {
    pub unit: String,
    /// systemd ActiveState: active, inactive, failed, activating, deactivating
    pub active_state: String,
    /// systemd SubState: running, dead, exited, ...
    pub sub_state: String,
    pub since_unix: Option<u64>,
}

/// The whole privileged surface of Maurice.
#[async_trait]
pub trait Agent: Send + Sync {
    // --- user lifecycle (root) ---
    async fn create_user(&self, user: &NewUser) -> Result<UserRef>;
    async fn delete_user(&self, user: &UserRef, purge_home: bool) -> Result<()>;

    // --- unit files, inside the user's quadlet directory ---
    async fn write_unit(&self, user: &UserRef, file_name: &str, content: &str) -> Result<()>;
    async fn remove_unit(&self, user: &UserRef, file_name: &str) -> Result<()>;
    async fn read_unit(&self, user: &UserRef, file_name: &str) -> Result<Option<String>>;

    // --- systemd --user of that user ---
    async fn daemon_reload(&self, user: &UserRef) -> Result<()>;
    async fn start_unit(&self, user: &UserRef, unit: &str) -> Result<()>;
    async fn stop_unit(&self, user: &UserRef, unit: &str) -> Result<()>;
    async fn restart_unit(&self, user: &UserRef, unit: &str) -> Result<()>;
    async fn unit_status(&self, user: &UserRef, unit: &str) -> Result<UnitStatus>;

    // --- podman secrets of that user; the value is passed on stdin, never on argv ---
    async fn set_secret(&self, user: &UserRef, name: &str, value: &[u8]) -> Result<()>;

    // --- journal of one unit of that user, newest last ---
    async fn read_journal(&self, user: &UserRef, unit: &str, tail: u32)
    -> Result<Vec<JournalLine>>;

    // --- clone a repository and build an image, both as the workspace user ---
    async fn build_image(&self, user: &UserRef, spec: &BuildSpec) -> Result<BuildResult>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildSpec {
    /// Clone URL (https or ssh).
    pub repo: String,
    /// Branch or tag.
    pub git_ref: String,
    /// Private key in PEM for ssh clones; written to the user's home with mode 0600.
    #[serde(skip_serializing)]
    pub ssh_key: Option<Vec<u8>>,
    /// Dockerfile path and context directory, both relative to the repository root.
    pub dockerfile: String,
    pub context: String,
    /// Image name without tag, e.g. `localhost/shop/web`; the tag becomes the commit sha.
    pub image: String,
    /// Scratch name for the checkout directory, e.g. the service name.
    pub checkout: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildResult {
    pub git_sha: String,
    /// Full image reference that was built, `<image>:<short sha>`.
    pub image_ref: String,
    /// Tail of the build output for the apply log.
    pub log_tail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalLine {
    /// Microseconds since the Unix epoch.
    pub realtime_usec: u64,
    pub priority: u8,
    pub identifier: String,
    pub message: String,
}

/// One recorded call, for tests, dry runs and the audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    CreateUser(NewUser),
    DeleteUser {
        user: String,
        purge_home: bool,
    },
    WriteUnit {
        user: String,
        file_name: String,
        content: String,
    },
    RemoveUnit {
        user: String,
        file_name: String,
    },
    DaemonReload {
        user: String,
    },
    StartUnit {
        user: String,
        unit: String,
    },
    StopUnit {
        user: String,
        unit: String,
    },
    RestartUnit {
        user: String,
        unit: String,
    },
    /// The value is deliberately not recorded.
    SetSecret {
        user: String,
        name: String,
    },
    BuildImage {
        user: String,
        repo: String,
        git_ref: String,
        image: String,
    },
}

/// Records operations instead of performing them.
#[derive(Default)]
pub struct DryRunAgent {
    ops: Mutex<Vec<Op>>,
    next_uid: Mutex<u32>,
}

impl DryRunAgent {
    pub fn new() -> Self {
        Self {
            ops: Mutex::new(Vec::new()),
            next_uid: Mutex::new(60001),
        }
    }
    pub fn ops(&self) -> Vec<Op> {
        self.ops.lock().expect("dry-run log").clone()
    }
    fn record(&self, op: Op) {
        tracing::info!(?op, "dry-run");
        self.ops.lock().expect("dry-run log").push(op);
    }
}

#[async_trait]
impl Agent for DryRunAgent {
    async fn create_user(&self, user: &NewUser) -> Result<UserRef> {
        self.record(Op::CreateUser(user.clone()));
        let mut uid = self.next_uid.lock().expect("uid counter");
        let assigned = *uid;
        *uid += 1;
        Ok(UserRef {
            name: user.name.clone(),
            uid: assigned,
        })
    }
    async fn delete_user(&self, user: &UserRef, purge_home: bool) -> Result<()> {
        self.record(Op::DeleteUser {
            user: user.name.clone(),
            purge_home,
        });
        Ok(())
    }
    async fn write_unit(&self, user: &UserRef, file_name: &str, content: &str) -> Result<()> {
        self.record(Op::WriteUnit {
            user: user.name.clone(),
            file_name: file_name.into(),
            content: content.into(),
        });
        Ok(())
    }
    async fn remove_unit(&self, user: &UserRef, file_name: &str) -> Result<()> {
        self.record(Op::RemoveUnit {
            user: user.name.clone(),
            file_name: file_name.into(),
        });
        Ok(())
    }
    async fn read_unit(&self, _user: &UserRef, _file_name: &str) -> Result<Option<String>> {
        Ok(None)
    }
    async fn daemon_reload(&self, user: &UserRef) -> Result<()> {
        self.record(Op::DaemonReload {
            user: user.name.clone(),
        });
        Ok(())
    }
    async fn start_unit(&self, user: &UserRef, unit: &str) -> Result<()> {
        self.record(Op::StartUnit {
            user: user.name.clone(),
            unit: unit.into(),
        });
        Ok(())
    }
    async fn stop_unit(&self, user: &UserRef, unit: &str) -> Result<()> {
        self.record(Op::StopUnit {
            user: user.name.clone(),
            unit: unit.into(),
        });
        Ok(())
    }
    async fn restart_unit(&self, user: &UserRef, unit: &str) -> Result<()> {
        self.record(Op::RestartUnit {
            user: user.name.clone(),
            unit: unit.into(),
        });
        Ok(())
    }
    async fn unit_status(&self, _user: &UserRef, unit: &str) -> Result<UnitStatus> {
        Ok(UnitStatus {
            unit: unit.into(),
            active_state: "inactive".into(),
            sub_state: "dead".into(),
            since_unix: None,
        })
    }
    async fn set_secret(&self, user: &UserRef, name: &str, _value: &[u8]) -> Result<()> {
        self.record(Op::SetSecret {
            user: user.name.clone(),
            name: name.into(),
        });
        Ok(())
    }
    async fn read_journal(
        &self,
        _user: &UserRef,
        _unit: &str,
        _tail: u32,
    ) -> Result<Vec<JournalLine>> {
        Ok(vec![])
    }
    async fn build_image(&self, user: &UserRef, spec: &BuildSpec) -> Result<BuildResult> {
        self.record(Op::BuildImage {
            user: user.name.clone(),
            repo: spec.repo.clone(),
            git_ref: spec.git_ref.clone(),
            image: spec.image.clone(),
        });
        Ok(BuildResult {
            git_sha: "0000000".into(),
            image_ref: format!("{}:0000000", spec.image),
            log_tail: "dry-run".into(),
        })
    }
}
