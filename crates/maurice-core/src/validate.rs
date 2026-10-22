//! Deterministic checks that run before any plan is produced. Errors block
//! apply; warnings are shown. No I/O, no guessing: a finding is either
//! provable from the model or it is not emitted.

use crate::{EnvValue, Workspace};
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
pub fn validate_workspace(ws: &Workspace) -> Vec<Finding> {
    let mut findings = Vec::new();
    let names: HashSet<&str> = ws.services().map(|s| s.name.as_str()).collect();

    // Service names are network aliases: unique and DNS-safe.
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for s in ws.services() {
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
                    "{count} services share the alias '{name}' in workspace '{}'",
                    ws.name
                ),
            ));
        }
    }

    // Env references must point at an existing service in this workspace.
    for s in ws.services() {
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
                        format!("reference '{r}' points at unknown service '{target}'"),
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

    // Published host ports must not collide inside the workspace.
    let mut bound: HashMap<String, &str> = HashMap::new();
    for s in ws.services() {
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
                if let Some(other) = bound.insert(key.clone(), &s.name) {
                    findings.push(Finding::error(
                        "port.conflict",
                        format!("port:{key}"),
                        format!("{key} is published by both '{other}' and '{}'", s.name),
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
    for s in ws.services() {
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

pub fn has_errors(findings: &[Finding]) -> bool {
    findings.iter().any(|f| f.severity == Severity::Error)
}

fn is_dns_label(s: &str) -> bool {
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
        }
    }

    fn ws(services: Vec<Service>) -> Workspace {
        Workspace {
            name: "shop".into(),
            unix_user: "ws-shop".into(),
            uid: 60001,
            subuid_start: 1_000_000,
            subuid_len: 65536,
            home: "/srv/ws/shop".into(),
            env: Env::Dev,
            cpu_slice: None,
            projects: vec![Project {
                name: "storefront".into(),
                git_repo: None,
                services,
            }],
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
    fn ref_parser() {
        assert_eq!(parse_ref("{{postgres.url}}"), Some(("postgres", "url")));
        assert_eq!(parse_ref("{{ postgres.url }}"), Some(("postgres", "url")));
        assert_eq!(parse_ref("{{postgres}}"), None);
        assert_eq!(parse_ref("postgres.url"), None);
    }
}
