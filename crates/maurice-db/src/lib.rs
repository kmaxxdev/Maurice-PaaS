//! PostgreSQL state store for the panel. Migrations are embedded and run at
//! start. Queries are runtime-checked for now; the compile-time checked
//! variants come once a CI database exists.

use serde::{Deserialize, Serialize};
pub use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;
use uuid::Uuid;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Connect and bring the schema up to date.
pub async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(database_url)
        .await?;
    MIGRATOR.run(&pool).await?;
    tracing::info!("database ready");
    Ok(pool)
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct WorkspaceRow {
    pub id: Uuid,
    pub name: String,
    pub unix_user: String,
    pub uid: i32,
    pub subuid_start: i64,
    pub subuid_len: i32,
    pub home: String,
    pub env: String,
    pub cpu_slice: Option<String>,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct PlanRow {
    pub id: Uuid,
    /// Nil for plans that create a workspace: the workspace does not exist yet.
    pub workspace_id: Uuid,
    pub changes: serde_json::Value,
    pub findings: serde_json::Value,
    pub destructive: bool,
    pub created_by: String,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub applied_at: Option<OffsetDateTime>,
    pub discarded_at: Option<OffsetDateTime>,
}

#[derive(Clone)]
pub struct Store {
    pool: PgPool,
}

/// First subuid range handed out; 65536 ids per workspace, so a thousand
/// workspaces fit well below the 32-bit uid ceiling.
pub const SUBUID_BASE: i64 = 1_000_000;

impl Store {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn list_workspaces(&self) -> anyhow::Result<Vec<WorkspaceRow>> {
        Ok(
            sqlx::query_as::<_, WorkspaceRow>("select * from workspaces order by name")
                .fetch_all(&self.pool)
                .await?,
        )
    }

    pub async fn get_workspace(&self, id: Uuid) -> anyhow::Result<Option<WorkspaceRow>> {
        Ok(
            sqlx::query_as::<_, WorkspaceRow>("select * from workspaces where id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn get_workspace_by_name(&self, name: &str) -> anyhow::Result<Option<WorkspaceRow>> {
        Ok(
            sqlx::query_as::<_, WorkspaceRow>("select * from workspaces where name = $1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn insert_workspace(&self, w: &WorkspaceRow) -> anyhow::Result<()> {
        sqlx::query(
            "insert into workspaces (id, name, unix_user, uid, subuid_start, subuid_len, home, env, cpu_slice, created_at)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(w.id)
        .bind(&w.name)
        .bind(&w.unix_user)
        .bind(w.uid)
        .bind(w.subuid_start)
        .bind(w.subuid_len)
        .bind(&w.home)
        .bind(&w.env)
        .bind(&w.cpu_slice)
        .bind(w.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_workspace(&self, id: Uuid) -> anyhow::Result<bool> {
        let r = sqlx::query("delete from workspaces where id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    /// Next free subuid range start: ranges are allocated by the panel and never reused
    /// while a workspace holds them.
    pub async fn next_subuid_start(&self, len: i64) -> anyhow::Result<i64> {
        let max: Option<i64> =
            sqlx::query_scalar("select max(subuid_start + subuid_len) from workspaces")
                .fetch_one(&self.pool)
                .await?;
        let _ = len; // every workspace gets the same length today; kept for variable ranges later
        Ok(max.map(|m| m.max(SUBUID_BASE)).unwrap_or(SUBUID_BASE))
    }

    pub async fn insert_plan(&self, p: &PlanRow) -> anyhow::Result<()> {
        sqlx::query(
            "insert into plans (id, workspace_id, changes, findings, destructive, created_by, created_at, expires_at)
             values ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(p.id)
        .bind(p.workspace_id)
        .bind(&p.changes)
        .bind(&p.findings)
        .bind(p.destructive)
        .bind(&p.created_by)
        .bind(p.created_at)
        .bind(p.expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_plan(&self, id: Uuid) -> anyhow::Result<Option<PlanRow>> {
        Ok(
            sqlx::query_as::<_, PlanRow>("select * from plans where id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn mark_plan_applied(&self, id: Uuid) -> anyhow::Result<()> {
        sqlx::query("update plans set applied_at = now() where id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn mark_plan_discarded(&self, id: Uuid) -> anyhow::Result<bool> {
        let r = sqlx::query("update plans set discarded_at = now() where id = $1 and applied_at is null and discarded_at is null")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }
}

// ---------------------------------------------------------------------------
// Projects, services and their parts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct ProjectRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub git_repo: Option<String>,
    pub git_ref: String,
    pub git_key_secret_id: Option<Uuid>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct ServiceRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub kind: String,
    pub source_kind: String,
    pub image_ref: Option<String>,
    pub dockerfile: Option<String>,
    pub build_context: Option<String>,
    pub cpu: Option<f32>,
    pub memory_bytes: Option<i64>,
    pub isolated: bool,
    pub health_cmd: Option<String>,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct PortRow {
    pub id: Uuid,
    pub service_id: Uuid,
    pub container_port: i32,
    pub published: bool,
    pub host_ip: Option<String>,
    pub host_port: Option<i32>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct EnvVarRow {
    pub id: Uuid,
    pub service_id: Uuid,
    pub key: String,
    pub value_kind: String,
    pub value: Option<String>,
    pub secret_id: Option<Uuid>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct SecretRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub service_id: Option<Uuid>,
    pub name: String,
    pub encrypted_value: Vec<u8>,
    pub version: i32,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct DeploymentRow {
    pub id: Uuid,
    pub service_id: Uuid,
    pub plan_id: Uuid,
    pub image_digest: String,
    pub git_sha: Option<String>,
    pub status: String,
    pub started_at: OffsetDateTime,
    pub finished_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct PublishedPort {
    pub host_ip: Option<String>,
    pub host_port: Option<i32>,
    pub workspace: String,
    pub service: String,
}

/// A service with its ports and env, as the API and the renderer want it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceFull {
    pub service: ServiceRow,
    pub project: ProjectRow,
    pub ports: Vec<PortRow>,
    pub env: Vec<EnvVarRow>,
}

impl Store {
    pub async fn list_projects(&self, workspace_id: Uuid) -> anyhow::Result<Vec<ProjectRow>> {
        Ok(sqlx::query_as::<_, ProjectRow>(
            "select * from projects where workspace_id = $1 order by name",
        )
        .bind(workspace_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn ensure_project(
        &self,
        workspace_id: Uuid,
        name: &str,
    ) -> anyhow::Result<ProjectRow> {
        if let Some(p) = sqlx::query_as::<_, ProjectRow>(
            "select * from projects where workspace_id = $1 and name = $2",
        )
        .bind(workspace_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?
        {
            return Ok(p);
        }
        let row = ProjectRow {
            id: Uuid::now_v7(),
            workspace_id,
            name: name.into(),
            git_repo: None,
            git_ref: "main".into(),
            git_key_secret_id: None,
        };
        sqlx::query("insert into projects (id, workspace_id, name) values ($1, $2, $3)")
            .bind(row.id)
            .bind(row.workspace_id)
            .bind(&row.name)
            .execute(&self.pool)
            .await?;
        Ok(row)
    }

    /// Create or update a project's repository settings.
    pub async fn upsert_project(
        &self,
        workspace_id: Uuid,
        name: &str,
        git_repo: Option<&str>,
        git_ref: &str,
        git_key_secret_id: Option<Uuid>,
    ) -> anyhow::Result<ProjectRow> {
        let existing = self.ensure_project(workspace_id, name).await?;
        sqlx::query(
            "update projects set git_repo = $2, git_ref = $3, git_key_secret_id = $4 where id = $1",
        )
        .bind(existing.id)
        .bind(git_repo)
        .bind(git_ref)
        .bind(git_key_secret_id)
        .execute(&self.pool)
        .await?;
        Ok(ProjectRow {
            git_repo: git_repo.map(Into::into),
            git_ref: git_ref.into(),
            git_key_secret_id,
            ..existing
        })
    }

    pub async fn list_services(&self, workspace_id: Uuid) -> anyhow::Result<Vec<ServiceFull>> {
        let services = sqlx::query_as::<_, ServiceRow>(
            "select s.* from services s join projects p on p.id = s.project_id where p.workspace_id = $1 order by s.name",
        )
        .bind(workspace_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(services.len());
        for s in services {
            out.push(self.hydrate(s).await?);
        }
        Ok(out)
    }

    pub async fn get_service(&self, id: Uuid) -> anyhow::Result<Option<ServiceFull>> {
        let Some(s) = sqlx::query_as::<_, ServiceRow>("select * from services where id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(self.hydrate(s).await?))
    }

    pub async fn get_service_by_name(
        &self,
        workspace_id: Uuid,
        name: &str,
    ) -> anyhow::Result<Option<ServiceFull>> {
        let row = sqlx::query_as::<_, ServiceRow>(
            "select s.* from services s join projects p on p.id = s.project_id where p.workspace_id = $1 and s.name = $2",
        )
        .bind(workspace_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(s) => Ok(Some(self.hydrate(s).await?)),
            None => Ok(None),
        }
    }

    async fn hydrate(&self, s: ServiceRow) -> anyhow::Result<ServiceFull> {
        let project = sqlx::query_as::<_, ProjectRow>("select * from projects where id = $1")
            .bind(s.project_id)
            .fetch_one(&self.pool)
            .await?;
        let ports = sqlx::query_as::<_, PortRow>(
            "select * from ports where service_id = $1 order by container_port",
        )
        .bind(s.id)
        .fetch_all(&self.pool)
        .await?;
        let env = sqlx::query_as::<_, EnvVarRow>(
            "select * from env_vars where service_id = $1 order by key",
        )
        .bind(s.id)
        .fetch_all(&self.pool)
        .await?;
        Ok(ServiceFull {
            service: s,
            project,
            ports,
            env,
        })
    }

    /// Replace a service definition (rows for service, ports, env) in one transaction.
    pub async fn upsert_service(
        &self,
        s: &ServiceRow,
        ports: &[PortRow],
        env: &[EnvVarRow],
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "insert into services (id, project_id, name, kind, source_kind, image_ref, dockerfile, build_context, cpu, memory_bytes, isolated, health_cmd, created_at)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
             on conflict (id) do update set kind = excluded.kind, source_kind = excluded.source_kind, image_ref = excluded.image_ref,
               dockerfile = excluded.dockerfile, build_context = excluded.build_context, cpu = excluded.cpu, memory_bytes = excluded.memory_bytes,
               isolated = excluded.isolated, health_cmd = excluded.health_cmd",
        )
        .bind(s.id)
        .bind(s.project_id)
        .bind(&s.name)
        .bind(&s.kind)
        .bind(&s.source_kind)
        .bind(&s.image_ref)
        .bind(&s.dockerfile)
        .bind(&s.build_context)
        .bind(s.cpu)
        .bind(s.memory_bytes)
        .bind(s.isolated)
        .bind(&s.health_cmd)
        .bind(s.created_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query("delete from ports where service_id = $1")
            .bind(s.id)
            .execute(&mut *tx)
            .await?;
        for p in ports {
            sqlx::query("insert into ports (id, service_id, container_port, published, host_ip, host_port) values ($1, $2, $3, $4, $5, $6)")
                .bind(p.id)
                .bind(s.id)
                .bind(p.container_port)
                .bind(p.published)
                .bind(&p.host_ip)
                .bind(p.host_port)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("delete from env_vars where service_id = $1")
            .bind(s.id)
            .execute(&mut *tx)
            .await?;
        for e in env {
            sqlx::query("insert into env_vars (id, service_id, key, value_kind, value, secret_id) values ($1, $2, $3, $4, $5, $6)")
                .bind(e.id)
                .bind(s.id)
                .bind(&e.key)
                .bind(&e.value_kind)
                .bind(&e.value)
                .bind(e.secret_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Every published host binding on the host with the workspace and service that owns it.
    pub async fn list_published_ports(&self) -> anyhow::Result<Vec<PublishedPort>> {
        Ok(sqlx::query_as::<_, PublishedPort>(
            "select p.host_ip, p.host_port, w.name as workspace, s.name as service
             from ports p join services s on s.id = p.service_id join projects pr on pr.id = s.project_id
             join workspaces w on w.id = pr.workspace_id where p.published",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn delete_service(&self, id: Uuid) -> anyhow::Result<bool> {
        let r = sqlx::query("delete from services where id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    // --- secrets ---

    pub async fn list_secrets(&self, workspace_id: Uuid) -> anyhow::Result<Vec<SecretRow>> {
        Ok(sqlx::query_as::<_, SecretRow>(
            "select distinct on (name) * from secrets where workspace_id = $1 order by name, version desc",
        )
        .bind(workspace_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_secret(&self, id: Uuid) -> anyhow::Result<Option<SecretRow>> {
        Ok(
            sqlx::query_as::<_, SecretRow>("select * from secrets where id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn latest_secret(
        &self,
        workspace_id: Uuid,
        name: &str,
    ) -> anyhow::Result<Option<SecretRow>> {
        Ok(sqlx::query_as::<_, SecretRow>(
            "select * from secrets where workspace_id = $1 and name = $2 order by version desc limit 1",
        )
        .bind(workspace_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Store a new version of a secret; returns the row.
    pub async fn insert_secret_version(
        &self,
        workspace_id: Uuid,
        name: &str,
        encrypted_value: &[u8],
    ) -> anyhow::Result<SecretRow> {
        let version = self
            .latest_secret(workspace_id, name)
            .await?
            .map(|s| s.version + 1)
            .unwrap_or(1);
        let row = SecretRow {
            id: Uuid::now_v7(),
            workspace_id,
            service_id: None,
            name: name.into(),
            encrypted_value: encrypted_value.to_vec(),
            version,
            created_at: OffsetDateTime::now_utc(),
        };
        sqlx::query("insert into secrets (id, workspace_id, service_id, name, encrypted_value, version, created_at) values ($1, $2, $3, $4, $5, $6, $7)")
            .bind(row.id)
            .bind(row.workspace_id)
            .bind(row.service_id)
            .bind(&row.name)
            .bind(&row.encrypted_value)
            .bind(row.version)
            .bind(row.created_at)
            .execute(&self.pool)
            .await?;
        Ok(row)
    }

    // --- deployments ---

    pub async fn insert_deployment(&self, d: &DeploymentRow) -> anyhow::Result<()> {
        sqlx::query("insert into deployments (id, service_id, plan_id, image_digest, git_sha, status, started_at, finished_at) values ($1, $2, $3, $4, $5, $6, $7, $8)")
            .bind(d.id)
            .bind(d.service_id)
            .bind(d.plan_id)
            .bind(&d.image_digest)
            .bind(&d.git_sha)
            .bind(&d.status)
            .bind(d.started_at)
            .bind(d.finished_at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn latest_deployment(
        &self,
        service_id: Uuid,
    ) -> anyhow::Result<Option<DeploymentRow>> {
        Ok(sqlx::query_as::<_, DeploymentRow>(
            "select * from deployments where service_id = $1 order by started_at desc limit 1",
        )
        .bind(service_id)
        .fetch_optional(&self.pool)
        .await?)
    }
}
