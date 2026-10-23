//! Conversions between the three shapes of a service: proto message (API),
//! database rows (store) and the domain model (validator and renderer).

use crate::proto::maurice::v1::__buffa::oneof;
use crate::proto::maurice::v1::*;
use maurice_db::{EnvVarRow, PortRow, ProjectRow, ServiceFull, ServiceRow, WorkspaceRow};
use std::net::IpAddr;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum MapError {
    #[error("{0}")]
    Invalid(String),
}

fn invalid(msg: impl Into<String>) -> MapError {
    MapError::Invalid(msg.into())
}

/// Proto definition → rows. `project_id` is set by the caller; env secret
/// references are resolved to secret ids by the caller via `secret_ids`.
pub fn service_from_proto(
    svc: &Service,
    id: Uuid,
    project_id: Uuid,
    secret_ids: &dyn Fn(&str) -> Option<Uuid>,
) -> Result<(ServiceRow, Vec<PortRow>, Vec<EnvVarRow>), MapError> {
    let kind = match svc.kind.as_known() {
        Some(ServiceKind::SERVICE_KIND_STATELESS) => "stateless",
        Some(ServiceKind::SERVICE_KIND_STATEFUL) => "stateful",
        _ => {
            return Err(invalid(
                "kind must be SERVICE_KIND_STATELESS or SERVICE_KIND_STATEFUL",
            ));
        }
    };
    let (source_kind, image_ref, dockerfile, build_context) = match &svc.source {
        Some(oneof::service::Source::ImageRef(r)) if !r.is_empty() => {
            ("image", Some(r.clone()), None, None)
        }
        Some(oneof::service::Source::DockerfilePath(p)) if !p.is_empty() => {
            ("build", None, Some(p.clone()), Some(".".to_string()))
        }
        _ => return Err(invalid("source must be image_ref or dockerfile_path")),
    };
    let row = ServiceRow {
        id,
        project_id,
        name: svc.name.clone(),
        kind: kind.into(),
        source_kind: source_kind.into(),
        image_ref,
        dockerfile,
        build_context,
        cpu: (svc.cpu > 0.0).then_some(svc.cpu as f32),
        memory_bytes: (svc.memory > 0).then_some(svc.memory as i64),
        isolated: svc.isolated,
        health_cmd: None,
        created_at: OffsetDateTime::now_utc(),
    };
    let mut ports = Vec::new();
    for p in &svc.ports {
        if p.container_port == 0 || p.container_port > 65535 {
            return Err(invalid(format!(
                "container port {} out of range",
                p.container_port
            )));
        }
        let (host_ip, host_port) = if p.published {
            let ip = if p.host_ip.is_empty() {
                "127.0.0.1".to_string()
            } else {
                p.host_ip.clone()
            };
            ip.parse::<IpAddr>()
                .map_err(|_| invalid(format!("host ip '{ip}' is not an address")))?;
            if p.host_port == 0 || p.host_port > 65535 {
                return Err(invalid("published port needs a host port"));
            }
            (Some(ip), Some(p.host_port as i32))
        } else {
            (None, None)
        };
        ports.push(PortRow {
            id: Uuid::now_v7(),
            service_id: id,
            container_port: p.container_port as i32,
            published: p.published,
            host_ip,
            host_port,
        });
    }
    let mut env = Vec::new();
    for e in &svc.env {
        let (value_kind, value, secret_id) = match &e.value {
            Some(oneof::env_var::Value::Literal(v)) => ("literal", Some(v.clone()), None),
            Some(oneof::env_var::Value::Ref(v)) => ("ref", Some(v.clone()), None),
            Some(oneof::env_var::Value::SecretName(n)) => {
                let sid = secret_ids(n).ok_or_else(|| {
                    invalid(format!(
                        "env {}: secret '{n}' does not exist in this workspace",
                        e.key
                    ))
                })?;
                ("secret", None, Some(sid))
            }
            None => return Err(invalid(format!("env {} has no value", e.key))),
        };
        env.push(EnvVarRow {
            id: Uuid::now_v7(),
            service_id: id,
            key: e.key.clone(),
            value_kind: value_kind.into(),
            value,
            secret_id,
        });
    }
    Ok((row, ports, env))
}

/// Rows → proto message. Secret env values are shown by secret name.
pub fn service_to_proto(
    full: &ServiceFull,
    secret_name: &dyn Fn(Uuid) -> Option<String>,
) -> Service {
    let s = &full.service;
    Service {
        id: s.id.to_string(),
        project_id: s.project_id.to_string(),
        name: s.name.clone(),
        kind: match s.kind.as_str() {
            "stateless" => ServiceKind::SERVICE_KIND_STATELESS,
            "stateful" => ServiceKind::SERVICE_KIND_STATEFUL,
            _ => ServiceKind::SERVICE_KIND_UNSPECIFIED,
        }
        .into(),
        source: match s.source_kind.as_str() {
            "image" => s.image_ref.clone().map(oneof::service::Source::ImageRef),
            "build" => s
                .dockerfile
                .clone()
                .map(oneof::service::Source::DockerfilePath),
            _ => None,
        },
        ports: full
            .ports
            .iter()
            .map(|p| Port {
                container_port: p.container_port as u32,
                published: p.published,
                host_ip: p.host_ip.clone().unwrap_or_default(),
                host_port: p.host_port.unwrap_or(0) as u32,
                ..Default::default()
            })
            .collect(),
        env: full
            .env
            .iter()
            .map(|e| EnvVar {
                key: e.key.clone(),
                value: match e.value_kind.as_str() {
                    "literal" => e.value.clone().map(oneof::env_var::Value::Literal),
                    "ref" => e.value.clone().map(oneof::env_var::Value::Ref),
                    "secret" => e
                        .secret_id
                        .and_then(secret_name)
                        .map(oneof::env_var::Value::SecretName),
                    _ => None,
                },
                ..Default::default()
            })
            .collect(),
        cpu: s.cpu.unwrap_or(0.0) as f64,
        memory: s.memory_bytes.unwrap_or(0) as u64,
        isolated: s.isolated,
        ..Default::default()
    }
}

/// Rows → domain model, for the validator and the renderer.
pub fn service_to_core(
    full: &ServiceFull,
    secret_name: &dyn Fn(Uuid) -> Option<String>,
) -> maurice_core::Service {
    let s = &full.service;
    maurice_core::Service {
        name: s.name.clone(),
        kind: if s.kind == "stateful" {
            maurice_core::ServiceKind::Stateful
        } else {
            maurice_core::ServiceKind::Stateless
        },
        source: if s.source_kind == "build" {
            maurice_core::Source::Build {
                dockerfile: s.dockerfile.clone().unwrap_or_else(|| "Dockerfile".into()),
                context: s.build_context.clone().unwrap_or_else(|| ".".into()),
            }
        } else {
            maurice_core::Source::Image {
                reference: s.image_ref.clone().unwrap_or_default(),
            }
        },
        ports: full
            .ports
            .iter()
            .map(|p| maurice_core::Port {
                container_port: p.container_port as u16,
                published: if p.published {
                    Some(maurice_core::Published {
                        host_ip: p
                            .host_ip
                            .as_deref()
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(IpAddr::from([127, 0, 0, 1])),
                        host_port: p.host_port.unwrap_or(0) as u16,
                    })
                } else {
                    None
                },
            })
            .collect(),
        env: full
            .env
            .iter()
            .filter_map(|e| {
                let value = match e.value_kind.as_str() {
                    "literal" => maurice_core::EnvValue::Literal(e.value.clone()?),
                    "ref" => maurice_core::EnvValue::Ref(e.value.clone()?),
                    "secret" => maurice_core::EnvValue::Secret(secret_name(e.secret_id?)?),
                    _ => return None,
                };
                Some(maurice_core::EnvVar {
                    key: e.key.clone(),
                    value,
                })
            })
            .collect(),
        // Stateful services get a data volume by convention until volumes are part of the API.
        volumes: if s.kind == "stateful" {
            vec![maurice_core::Volume {
                source: "data".into(),
                target: default_data_dir(s.image_ref.as_deref()),
                read_only: false,
            }]
        } else {
            vec![]
        },
        resources: maurice_core::Resources {
            cpu: s.cpu,
            memory: s.memory_bytes.map(|m| m as u64),
        },
        isolated: s.isolated,
        health_cmd: s.health_cmd.clone(),
    }
}

/// Where well-known images keep their data; `/data` otherwise.
fn default_data_dir(image: Option<&str>) -> String {
    match image.unwrap_or("") {
        i if i.contains("postgres") => "/var/lib/postgresql/data".into(),
        i if i.contains("clickhouse") => "/var/lib/clickhouse".into(),
        i if i.contains("dragonfly") || i.contains("valkey") || i.contains("redis") => {
            "/data".into()
        }
        i if i.contains("rabbitmq") => "/var/lib/rabbitmq".into(),
        i if i.contains("minio") => "/data".into(),
        _ => "/data".into(),
    }
}

pub fn workspace_to_core(
    w: &WorkspaceRow,
    projects: &[ProjectRow],
    services: &[ServiceFull],
    secret_name: &dyn Fn(Uuid) -> Option<String>,
) -> maurice_core::Workspace {
    maurice_core::Workspace {
        name: w.name.clone(),
        unix_user: w.unix_user.clone(),
        uid: w.uid as u32,
        subuid_start: w.subuid_start as u32,
        subuid_len: w.subuid_len as u32,
        home: w.home.clone(),
        env: if w.env == "prod" {
            maurice_core::Env::Prod
        } else {
            maurice_core::Env::Dev
        },
        cpu_slice: w.cpu_slice.clone(),
        projects: projects
            .iter()
            .map(|p| maurice_core::Project {
                name: p.name.clone(),
                git_repo: p.git_repo.clone(),
                services: services
                    .iter()
                    .filter(|s| s.project.id == p.id)
                    .map(|s| service_to_core(s, secret_name))
                    .collect(),
            })
            .collect(),
    }
}
