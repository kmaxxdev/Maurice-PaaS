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
