use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// Workspace environment. Prod workspaces never accept `apply` from non-human tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Env {
    Prod,
    Dev,
}

/// Deploy strategy follows from the kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceKind {
    /// Rolled blue-green: a new container starts, traffic switches, the old one drains.
    Stateless,
    /// Restarted in place and backed up; never two instances at once.
    Stateful,
}

/// A workspace is a system user with its own rootless Podman, network and home.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub name: String,
    pub unix_user: String,
    pub uid: u32,
    pub subuid_start: u32,
    pub subuid_len: u32,
    pub home: String,
    pub env: Env,
    pub cpu_slice: Option<String>,
    pub projects: Vec<Project>,
}

impl Workspace {
    /// Name of the workspace-wide Podman network (and its Quadlet unit stem).
    pub fn network_name(&self) -> String {
        format!("{}-net", self.name)
    }

    pub fn services(&self) -> impl Iterator<Item = &Service> {
        self.projects.iter().flat_map(|p| p.services.iter())
    }
}

/// Groups the services of one repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub git_repo: Option<String>,
    pub services: Vec<Service>,
}

/// Where the image comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Pull by reference; digests are preferred once a deployment exists.
    Image { reference: String },
    /// Build from a Dockerfile in the project repository; tagged by git sha.
    Build { dockerfile: String, context: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Service {
    /// Stable network alias inside the workspace. Unique per workspace.
    pub name: String,
    pub kind: ServiceKind,
    pub source: Source,
    pub ports: Vec<Port>,
    pub env: Vec<EnvVar>,
    pub volumes: Vec<Volume>,
    pub resources: Resources,
    /// Run under a dedicated `ws-<project>-<service>` user instead of the workspace user.
    pub isolated: bool,
    /// Health check command run inside the container; enables `Notify=healthy`.
    pub health_cmd: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Port {
    pub container_port: u16,
    /// Nothing is reachable from outside the workspace network unless published.
    pub published: Option<Published>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Published {
    /// Host address to bind. Defaults to loopback so only the edge proxy sees it.
    pub host_ip: IpAddr,
    pub host_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    pub key: String,
    pub value: EnvValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvValue {
    Literal(String),
    /// `{{service.field}}`, resolved by the panel at generation time.
    Ref(String),
    /// Name of a podman secret; the value never appears in a unit file.
    Secret(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Volume {
    /// Named volume (`<service>-<name>.volume`) or a host path when it starts with `/`.
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Resources {
    /// Fractional cores; `None` = unlimited.
    pub cpu: Option<f32>,
    /// Bytes; `None` = unlimited.
    pub memory: Option<u64>,
}
