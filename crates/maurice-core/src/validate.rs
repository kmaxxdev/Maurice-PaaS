//! Deterministic checks that run before any plan is produced. Errors block
//! apply; warnings are shown. No I/O, no guessing: a finding is either
//! provable from the model or it is not emitted.

use crate::{Env, EnvValue, Environment, Workspace};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    /// Stable machine-readable code, e.g. `env.unresolved_ref`.
    pub code: &'static str,
    pub message: String,
    /// What the finding points at: `service:web`, `env:web/DATABASE_URL`, `port:127.0.0.1:8080`.
    pub target: String,
}

impl Finding {
    fn error(code: &'static str, target: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code,
            message: message.into(),
            target: target.into(),
        }
    }
    fn warning(code: &'static str, target: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code,
            message: message.into(),
            target: target.into(),
        }
    }
}

/// Validate one workspace in isolation. Cross-workspace checks (host port
/// conflicts between workspaces) are the planner's job because they need the
/// other workspaces as input.
///
/// Aliases and references live inside an environment (each has its own
/// network); host ports are shared by the whole workspace.
pub fn validate_workspace(ws: &Workspace) -> Vec<Finding> {
    let mut findings = Vec::new();

    let mut seen_envs: HashSet<Env> = HashSet::new();
    for e in &ws.environments {
        if !seen_envs.insert(e.env) {
            findings.push(Finding::error(
                "environment.duplicate",
                format!("environment:{}", e.env),
                format!("environment '{}' is declared twice", e.env),
            ));
        }
        validate_environment(ws, e, &mut findings);
    }

    // Published host ports and edge routes must not collide anywhere in the workspace,
    // must lie in the block, and must leave the edge its own port.
    let mut bound: HashMap<String, String> = HashMap::new();
    let mut domains: HashMap<String, String> = HashMap::new();
    for (env, s) in ws.services() {
        let who = format!("{env}/{}", s.name);
        for r in &s.routes {
            let key = format!("127.0.0.1:{}", r.block_port);
            if r.block_port == ws.port_block.edge_port() {
                findings.push(Finding::error(
                    "port.reserved",
                    format!("port:{key}"),
                    format!(
                        "'{who}' routes through {} which is the edge's own port",
                        r.block_port
                    ),
                ));
            } else if !ws.port_block.contains(r.block_port) {
                findings.push(Finding::error(
                    "port.outside_block",
                    format!("port:{key}"),
                    format!(
                        "'{who}' routes through {} but the workspace owns ports {}",
                        r.block_port, ws.port_block
                    ),
                ));
            }
            if !s.ports.iter().any(|p| p.container_port == r.container_port) {
                findings.push(Finding::error(
                    "route.unknown_port",
                    format!("route:{}/{}", s.name, r.block_port),
                    format!(
                        "'{who}' routes to container port {} which the service does not declare",
                        r.container_port
                    ),
                ));
            }
            if let Some(other) = bound.insert(key.clone(), who.clone()) {
                findings.push(Finding::error(
                    "port.conflict",
                    format!("port:{key}"),
                    format!("{key} is used by both '{other}' and '{who}'"),
                ));
            }
            for d in &r.domains {
                if !is_hostname(&d.name) {
                    findings.push(Finding::error(
                        "domain.invalid",
                        format!("domain:{}", d.name),
                        format!("'{}' is not a valid host name", d.name),
                    ));
                }
                if r.visibility != crate::Visibility::Public {
                    findings.push(Finding::error(
                        "domain.internal_route",
                        format!("domain:{}", d.name),
                        format!("'{}' is attached to an internal route of '{who}'", d.name),
                    ));
                }
                if let Some(other) = domains.insert(d.name.clone(), who.clone()) {
                    findings.push(Finding::error(
                        "domain.duplicate",
                        format!("domain:{}", d.name),
                        format!("'{}' is attached to both '{other}' and '{who}'", d.name),
                    ));
                }
            }
        }
        let mut container_ports = HashSet::new();
        for p in &s.ports {
            if !container_ports.insert(p.container_port) {
                findings.push(Finding::error(
                    "port.duplicate",
                    format!("port:{}/{}", s.name, p.container_port),
                    format!("container port {} is declared twice", p.container_port),
                ));
            }
            if let Some(pubd) = p.published {
                let key = format!("{}:{}", pubd.host_ip, pubd.host_port);
                if pubd.host_port == ws.port_block.edge_port() {
                    findings.push(Finding::error(
                        "port.reserved",
                        format!("port:{key}"),
                        format!(
                            "'{who}' publishes {} which is the edge's own port",
                            pubd.host_port
                        ),
                    ));
                } else if !ws.port_block.contains(pubd.host_port) {
                    findings.push(Finding::error(
                        "port.outside_block",
                        format!("port:{key}"),
                        format!(
                            "'{who}' publishes {} but the workspace owns ports {}",
                            pubd.host_port, ws.port_block
                        ),
                    ));
                }
                if let Some(other) = bound.insert(key.clone(), who.clone()) {
                    findings.push(Finding::error(
                        "port.conflict",
                        format!("port:{key}"),
                        format!("{key} is published by both '{other}' and '{who}'"),
                    ));
                }
                if pubd.host_ip.is_unspecified() {
                    findings.push(Finding::warning(
                        "port.public",
                        format!("port:{key}"),
                        format!(
                            "'{}' publishes {} on all interfaces",
                            s.name, pubd.host_port
                        ),
                    ));
                }
            }
        }
    }

    // Stateful services without a volume lose their data on every restart.
    for (_, s) in ws.services() {
        if s.kind == crate::ServiceKind::Stateful && s.volumes.is_empty() {
            findings.push(Finding::warning(
                "service.stateful_without_volume",
                format!("service:{}", s.name),
                "stateful service has no volume; its data will not survive a redeploy",
            ));
        }
    }

    findings.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.target.cmp(&b.target)));
    findings
}

fn validate_environment(ws: &Workspace, env: &Environment, findings: &mut Vec<Finding>) {
    let names: HashSet<&str> = env.services().map(|s| s.name.as_str()).collect();

    // Service names are network aliases: unique and DNS-safe.
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for s in env.services() {
        *seen.entry(s.name.as_str()).or_default() += 1;
        if !is_dns_label(&s.name) {
            findings.push(Finding::error(
                "service.invalid_name",
                format!("service:{}", s.name),
                "service name must be a DNS label: lowercase letters, digits and dashes, 1-63 chars",
            ));
        }
    }
    for (name, count) in seen {
        if count > 1 {
            findings.push(Finding::error(
                "service.duplicate_name",
                format!("service:{name}"),
                format!(
                    "{count} services share the alias '{name}' in {}/{}",
                    ws.name, env.env
                ),
            ));
        }
    }

    // Env references must point at an existing service in the same environment.
    for s in env.services() {
        let mut keys = HashSet::new();
        for e in &s.env {
            if !keys.insert(e.key.as_str()) {
                findings.push(Finding::error(
                    "env.duplicate_key",
                    format!("env:{}/{}", s.name, e.key),
                    format!("variable '{}' is set twice", e.key),
                ));
            }
            if !is_env_key(&e.key) {
                findings.push(Finding::error(
                    "env.invalid_key",
                    format!("env:{}/{}", s.name, e.key),
                    "variable names must match [A-Za-z_][A-Za-z0-9_]*",
                ));
            }
            if let EnvValue::Ref(r) = &e.value {
                match parse_ref(r) {
                    Some((target, _field)) if names.contains(target) => {}
                    Some((target, _)) => findings.push(Finding::error(
                        "env.unresolved_ref",
                        format!("env:{}/{}", s.name, e.key),
                        format!(
                            "reference '{r}' points at unknown service '{target}' in {}",
                            env.env
                        ),
                    )),
                    None => findings.push(Finding::error(
                        "env.malformed_ref",
                        format!("env:{}/{}", s.name, e.key),
                        format!("reference '{r}' must look like {{{{service.field}}}}"),
                    )),
                }
            }
        }
    }
}

pub fn has_errors(findings: &[Finding]) -> bool {
    findings.iter().any(|f| f.severity == Severity::Error)
}

/// A host name for a domain: dot-separated DNS labels, at most 253 characters.
pub fn is_hostname(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && !s.ends_with('.')
        && s.split('.').all(is_dns_label)
        && s.contains('.')
}

/// Workspace and service names are DNS labels: lowercase letters, digits, dashes, 1-63 chars.
pub fn is_dns_label(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

fn is_env_key(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Parse `{{service.field}}` into `(service, field)`.
pub fn parse_ref(r: &str) -> Option<(&str, &str)> {
    let inner = r.strip_prefix("{{")?.strip_suffix("}}")?.trim();
    let (svc, field) = inner.split_once('.')?;
    if svc.is_empty() || field.is_empty() {
        return None;
    }
    Some((svc, field))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;
    use std::net::Ipv4Addr;

    fn svc(name: &str) -> Service {
        Service {
            name: name.into(),
            kind: ServiceKind::Stateless,
            source: Source::Image {
                reference: "docker.io/library/nginx:1".into(),
            },
            ports: vec![],
            env: vec![],
            volumes: vec![],
            resources: Resources::default(),
            isolated: false,
            health_cmd: None,
            routes: vec![],
        }
    }

    fn ws(services: Vec<Service>) -> Workspace {
        ws_envs(vec![(Env::Dev, services)])
    }

    fn ws_envs(envs: Vec<(Env, Vec<Service>)>) -> Workspace {
        Workspace {
            name: "shop".into(),
            unix_user: "ws-shop".into(),
            uid: 60001,
            subuid_start: 1_000_000,
            subuid_len: 65536,
            home: "/srv/ws/shop".into(),
            cpu_slice: None,
            port_block: PortBlock {
                start: 8000,
                size: 1000,
            },
            environments: envs
                .into_iter()
                .map(|(env, services)| Environment {
                    env,
                    projects: vec![Project {
                        name: "storefront".into(),
                        git_repo: None,
                        services,
                    }],
                })
                .collect(),
        }
    }

    #[test]
    fn clean_workspace_has_no_findings() {
        assert!(validate_workspace(&ws(vec![svc("web")])).is_empty());
    }

    #[test]
    fn unresolved_ref_is_an_error() {
        let mut web = svc("web");
        web.env.push(EnvVar {
            key: "DATABASE_URL".into(),
            value: EnvValue::Ref("{{postgres.url}}".into()),
        });
        let f = validate_workspace(&ws(vec![web]));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].code, "env.unresolved_ref");
        assert_eq!(f[0].severity, Severity::Error);
    }

    #[test]
    fn resolved_ref_passes() {
        let mut web = svc("web");
        web.env.push(EnvVar {
            key: "DATABASE_URL".into(),
            value: EnvValue::Ref("{{postgres.url}}".into()),
        });
        let mut pg = svc("postgres");
        pg.kind = ServiceKind::Stateful;
        pg.volumes.push(Volume {
            source: "data".into(),
            target: "/var/lib/postgresql".into(),
            read_only: false,
        });
        assert!(validate_workspace(&ws(vec![web, pg])).is_empty());
    }

    #[test]
    fn host_port_conflict_and_public_bind() {
        let lo = Published {
            host_ip: Ipv4Addr::LOCALHOST.into(),
            host_port: 8080,
        };
        let mut a = svc("a");
        a.ports.push(Port {
            container_port: 80,
            published: Some(lo),
        });
        let mut b = svc("b");
        b.ports.push(Port {
            container_port: 80,
            published: Some(lo),
        });
        let mut c = svc("c");
        c.ports.push(Port {
            container_port: 80,
            published: Some(Published {
                host_ip: Ipv4Addr::UNSPECIFIED.into(),
                host_port: 9000,
            }),
        });
        let f = validate_workspace(&ws(vec![a, b, c]));
        assert!(
            f.iter()
                .any(|x| x.code == "port.conflict" && x.severity == Severity::Error)
        );
        assert!(
            f.iter()
                .any(|x| x.code == "port.public" && x.severity == Severity::Warning)
        );
    }

    #[test]
    fn duplicate_and_invalid_names() {
        let f = validate_workspace(&ws(vec![svc("web"), svc("web"), svc("Bad_Name")]));
        assert!(f.iter().any(|x| x.code == "service.duplicate_name"));
        assert!(f.iter().any(|x| x.code == "service.invalid_name"));
    }

    #[test]
    fn same_alias_in_two_environments_is_fine_but_refs_stay_inside() {
        let mut web = svc("web");
        web.env.push(EnvVar {
            key: "DATABASE_URL".into(),
            value: EnvValue::Ref("{{postgres.url}}".into()),
        });
        let mut pg = svc("postgres");
        pg.kind = ServiceKind::Stateful;
        pg.volumes.push(Volume {
            source: "data".into(),
            target: "/var/lib/postgresql".into(),
            read_only: false,
        });
        // prod has both; dev has only web, so its reference dangles.
        let f = validate_workspace(&ws_envs(vec![
            (Env::Prod, vec![web.clone(), pg]),
            (Env::Dev, vec![web]),
        ]));
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].code, "env.unresolved_ref");
        assert!(f[0].message.contains("in dev"));
    }

    #[test]
    fn host_ports_are_shared_across_environments() {
        let lo = Published {
            host_ip: Ipv4Addr::LOCALHOST.into(),
            host_port: 8080,
        };
        let mut a = svc("web");
        a.ports.push(Port {
            container_port: 80,
            published: Some(lo),
        });
        let f = validate_workspace(&ws_envs(vec![
            (Env::Dev, vec![a.clone()]),
            (Env::Prod, vec![a]),
        ]));
        assert!(f.iter().any(|x| x.code == "port.conflict"), "{f:?}");
    }

    #[test]
    fn host_port_outside_the_block_is_an_error() {
        let mut a = svc("web");
        a.ports.push(Port {
            container_port: 80,
            published: Some(Published {
                host_ip: Ipv4Addr::LOCALHOST.into(),
                host_port: 18080,
            }),
        });
        let f = validate_workspace(&ws(vec![a]));
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].code, "port.outside_block");
        assert!(f[0].message.contains("8000-8999"));
    }

    #[test]
    fn routes_follow_the_block_rules() {
        let mut web = svc("web");
        web.ports.push(Port {
            container_port: 80,
            published: None,
        });
        web.routes.push(Route {
            container_port: 80,
            block_port: 8080,
            visibility: Visibility::Public,
            domains: vec![Domain {
                name: "shop.example.com".into(),
                public_ip: None,
                tls: TlsMode::Auto,
            }],
        });
        assert!(validate_workspace(&ws(vec![web.clone()])).is_empty());
        // Reserved edge port, unknown container port, bad and duplicate domains.
        web.routes.push(Route {
            container_port: 81,
            block_port: 8999,
            visibility: Visibility::Internal,
            domains: vec![Domain {
                name: "shop.example.com".into(),
                public_ip: None,
                tls: TlsMode::Off,
            }],
        });
        let f = validate_workspace(&ws(vec![web]));
        let codes: Vec<&str> = f.iter().map(|x| x.code).collect();
        for c in [
            "port.reserved",
            "route.unknown_port",
            "domain.duplicate",
            "domain.internal_route",
        ] {
            assert!(codes.contains(&c), "{c} missing in {codes:?}");
        }
    }

    #[test]
    fn hostnames() {
        assert!(is_hostname("shop.example.com"));
        assert!(is_hostname("a.b"));
        assert!(!is_hostname("localhost"));
        assert!(!is_hostname("Shop.Example.com"));
        assert!(!is_hostname("shop..com"));
    }

    #[test]
    fn port_block_bounds() {
        let b = PortBlock {
            start: 21000,
            size: 500,
        };
        assert_eq!(b.end(), 21499);
        assert!(b.contains(21000) && b.contains(21499));
        assert!(!b.contains(20999) && !b.contains(21500));
        assert_eq!(b.to_string(), "21000-21499");
    }

    #[test]
    fn ref_parser() {
        assert_eq!(parse_ref("{{postgres.url}}"), Some(("postgres", "url")));
        assert_eq!(parse_ref("{{ postgres.url }}"), Some(("postgres", "url")));
        assert_eq!(parse_ref("{{postgres}}"), None);
        assert_eq!(parse_ref("postgres.url"), None);
    }
}
