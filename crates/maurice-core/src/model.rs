use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// An environment inside a workspace. Each has its own network, services and
/// secrets. Prod and staging never accept `apply` from non-human tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Env {
    Dev,
    Staging,
    Prod,
}

impl Env {
    pub const ALL: [Env; 3] = [Env::Dev, Env::Staging, Env::Prod];

    pub fn as_str(self) -> &'static str {
        match self {
            Env::Dev => "dev",
            Env::Staging => "staging",
            Env::Prod => "prod",
        }
    }

    pub fn parse(s: &str) -> Option<Env> {
        match s {
            "dev" => Some(Env::Dev),
            "staging" => Some(Env::Staging),
            "prod" => Some(Env::Prod),
            _ => None,
        }
    }

    /// Environments where changes must come from a human (ADR-011, ADR-012).
    pub fn human_only(self) -> bool {
        matches!(self, Env::Staging | Env::Prod)
    }
}

impl std::fmt::Display for Env {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
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

/// Contiguous range of host ports a workspace may bind (ADR-012). Every host
/// binding of the workspace lives inside it, so workspaces cannot collide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortBlock {
    pub start: u16,
    pub size: u16,
}

impl PortBlock {
    /// Block sizes a workspace may be created with.
    pub const SIZES: [u16; 3] = [100, 500, 1000];
    pub const DEFAULT_SIZE: u16 = 500;

    /// Last port of the block, inclusive.
    pub fn end(&self) -> u16 {
        self.start + self.size - 1
    }

    pub fn contains(&self, port: u16) -> bool {
        port >= self.start && port <= self.end()
    }

    /// The workspace edge's own listener: the last port of the block, never handed
    /// to a service.
    pub fn edge_port(&self) -> u16 {
        self.end()
    }

    /// Ports services may bind: the block without the edge's reserved port.
    pub fn usable(&self) -> std::ops::RangeInclusive<u16> {
        self.start..=self.end() - 1
    }
}

impl std::fmt::Display for PortBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.start, self.end())
    }
}

/// A workspace is a system user with its own rootless Podman and home. It owns
/// one or more environments, each with a network of its own, and a block of
/// host ports (ADR-012).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub name: String,
    pub unix_user: String,
    pub uid: u32,
    pub subuid_start: u32,
    pub subuid_len: u32,
    pub home: String,
    pub cpu_slice: Option<String>,
    pub port_block: PortBlock,
    pub environments: Vec<Environment>,
}

impl Workspace {
    pub fn environment(&self, env: Env) -> Option<&Environment> {
        self.environments.iter().find(|e| e.env == env)
    }

    /// Every service in every environment, with the environment it belongs to.
    pub fn services(&self) -> impl Iterator<Item = (Env, &Service)> {
        self.environments
            .iter()
            .flat_map(|e| e.services().map(move |s| (e.env, s)))
    }

    /// Name of an environment's Podman network (and its Quadlet unit stem).
    pub fn network_name(&self, env: Env) -> String {
        format!("{}-{env}", self.name)
    }
}

/// One environment of a workspace: a network and the services in it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    pub env: Env,
    pub projects: Vec<Project>,
}

impl Environment {
    pub fn services(&self) -> impl Iterator<Item = &Service> {
        self.projects.iter().flat_map(|p| p.services.iter())
    }
}

/// Groups the services of one repository. A project is shared by the
/// environments of a workspace; here it carries the services of one environment.
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
    /// Stable network alias inside the environment. Unique per environment.
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
    /// HTTP routes on the workspace edge ("expose" in the panel, ADR-009).
    #[serde(default)]
    pub routes: Vec<Route>,
}

/// Who may reach a route: the internet through the host edge, or other
/// workspaces on the internal host address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    Internal,
}

impl Visibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Internal => "internal",
        }
    }
    pub fn parse(s: &str) -> Option<Visibility> {
        match s {
            "public" => Some(Visibility::Public),
            "internal" => Some(Visibility::Internal),
            _ => None,
        }
    }
}

/// A route on the workspace edge: a block port that proxies to a container port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    pub container_port: u16,
    /// Host port inside the workspace block the edge listens on.
    pub block_port: u16,
    pub visibility: Visibility,
    /// Domains the host edge attaches to this route (public routes only).
    #[serde(default)]
    pub domains: Vec<Domain>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Domain {
    pub name: String,
    /// Public address to listen on; `None` = every address.
    pub public_ip: Option<IpAddr>,
    pub tls: TlsMode,
}

/// How the host edge serves a domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsMode {
    /// Certificates from a public CA (HTTP-01); plain HTTP redirects.
    Auto,
    /// Caddy's internal CA; for test hosts without public DNS.
    Internal,
    /// Plain HTTP only.
    Off,
}

impl TlsMode {
    pub fn as_str(self) -> &'static str {
        match self {
            TlsMode::Auto => "auto",
            TlsMode::Internal => "internal",
            TlsMode::Off => "off",
        }
    }
    pub fn parse(s: &str) -> Option<TlsMode> {
        match s {
            "auto" => Some(TlsMode::Auto),
            "internal" => Some(TlsMode::Internal),
            "off" => Some(TlsMode::Off),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Port {
    pub container_port: u16,
    /// Nothing is reachable from outside the environment network unless published.
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
    /// Named volume (`<env>-<service>-<name>.volume`) or a host path when it starts with `/`.
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
