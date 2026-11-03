//! Users, sessions, tokens and the audit log (ADR-011).

use crate::Store;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct UserRow {
    pub id: Uuid,
    pub login: String,
    pub password_hash: String,
    pub role: String,
    pub disabled: bool,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct SessionRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub hash: Vec<u8>,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub last_seen_at: OffsetDateTime,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct TokenRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub hash: Vec<u8>,
    pub scopes: Vec<String>,
    pub workspace_ids: Vec<Uuid>,
    pub human: bool,
    pub created_at: OffsetDateTime,
    pub expires_at: Option<OffsetDateTime>,
    pub last_used_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct AuditRow {
    pub id: Uuid,
    pub at: OffsetDateTime,
    pub user_id: Option<Uuid>,
    pub principal: String,
    pub action: String,
    pub target: String,
    pub tier: String,
    pub outcome: String,
    pub detail: String,
}

impl Store {
    // --- users ---

    pub async fn count_users(&self) -> anyhow::Result<i64> {
        Ok(sqlx::query_scalar("select count(*) from users")
            .fetch_one(&self.pool)
            .await?)
    }

    pub async fn list_users(&self) -> anyhow::Result<Vec<UserRow>> {
        Ok(
            sqlx::query_as::<_, UserRow>("select * from users order by login")
                .fetch_all(&self.pool)
                .await?,
        )
    }

    pub async fn get_user(&self, id: Uuid) -> anyhow::Result<Option<UserRow>> {
        Ok(
            sqlx::query_as::<_, UserRow>("select * from users where id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn get_user_by_login(&self, login: &str) -> anyhow::Result<Option<UserRow>> {
        Ok(
            sqlx::query_as::<_, UserRow>("select * from users where login = $1")
                .bind(login)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn insert_user(&self, u: &UserRow) -> anyhow::Result<()> {
        sqlx::query(
            "insert into users (id, login, password_hash, role, disabled, created_at) values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(u.id)
        .bind(&u.login)
        .bind(&u.password_hash)
        .bind(&u.role)
        .bind(u.disabled)
        .bind(u.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_user(
        &self,
        id: Uuid,
        role: &str,
        disabled: bool,
        password_hash: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "update users set role = $2, disabled = $3, password_hash = coalesce($4, password_hash) where id = $1",
        )
        .bind(id)
        .bind(role)
        .bind(disabled)
        .bind(password_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_password(&self, id: Uuid, password_hash: &str) -> anyhow::Result<()> {
        sqlx::query("update users set password_hash = $2 where id = $1")
            .bind(id)
            .bind(password_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn count_enabled_admins(&self) -> anyhow::Result<i64> {
        Ok(
            sqlx::query_scalar("select count(*) from users where role = 'admin' and not disabled")
                .fetch_one(&self.pool)
                .await?,
        )
    }

    // --- sessions ---

    pub async fn insert_session(&self, s: &SessionRow) -> anyhow::Result<()> {
        sqlx::query(
            "insert into sessions (id, user_id, hash, created_at, expires_at, last_seen_at) values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(s.id)
        .bind(s.user_id)
        .bind(&s.hash)
        .bind(s.created_at)
        .bind(s.expires_at)
        .bind(s.last_seen_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Live session for a cookie hash, with its idle expiry pushed forward.
    pub async fn touch_session(
        &self,
        hash: &[u8],
        idle: time::Duration,
    ) -> anyhow::Result<Option<SessionRow>> {
        Ok(sqlx::query_as::<_, SessionRow>(
            "update sessions set last_seen_at = now(), expires_at = now() + $2
             where hash = $1 and expires_at > now() returning *",
        )
        .bind(hash)
        .bind(idle)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn delete_session(&self, hash: &[u8]) -> anyhow::Result<()> {
        sqlx::query("delete from sessions where hash = $1")
            .bind(hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete_sessions_of(&self, user_id: Uuid) -> anyhow::Result<()> {
        sqlx::query("delete from sessions where user_id = $1")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // --- tokens ---

    pub async fn insert_token(&self, t: &TokenRow) -> anyhow::Result<()> {
        sqlx::query(
            "insert into tokens (id, user_id, name, hash, scopes, workspace_ids, human, created_at, expires_at)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(t.id)
        .bind(t.user_id)
        .bind(&t.name)
        .bind(&t.hash)
        .bind(&t.scopes)
        .bind(&t.workspace_ids)
        .bind(t.human)
        .bind(t.created_at)
        .bind(t.expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Live token for a secret hash, marking it used.
    pub async fn touch_token(&self, hash: &[u8]) -> anyhow::Result<Option<TokenRow>> {
        Ok(sqlx::query_as::<_, TokenRow>(
            "update tokens set last_used_at = now()
             where hash = $1 and revoked_at is null and (expires_at is null or expires_at > now()) returning *",
        )
        .bind(hash)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Tokens of one user, or of everyone when `user_id` is `None` (admins).
    pub async fn list_tokens(&self, user_id: Option<Uuid>) -> anyhow::Result<Vec<TokenRow>> {
        Ok(sqlx::query_as::<_, TokenRow>(
            "select * from tokens where revoked_at is null and ($1::uuid is null or user_id = $1) order by created_at desc",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_token(&self, id: Uuid) -> anyhow::Result<Option<TokenRow>> {
        Ok(
            sqlx::query_as::<_, TokenRow>("select * from tokens where id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn revoke_token(&self, id: Uuid) -> anyhow::Result<bool> {
        let r = sqlx::query(
            "update tokens set revoked_at = now() where id = $1 and revoked_at is null",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected() > 0)
    }

    // --- audit ---

    pub async fn insert_audit(&self, a: &AuditRow) -> anyhow::Result<()> {
        sqlx::query(
            "insert into audit (id, at, user_id, principal, action, target, tier, outcome, detail)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(a.id)
        .bind(a.at)
        .bind(a.user_id)
        .bind(&a.principal)
        .bind(&a.action)
        .bind(&a.target)
        .bind(&a.tier)
        .bind(&a.outcome)
        .bind(&a.detail)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_audit(&self, limit: i64) -> anyhow::Result<Vec<AuditRow>> {
        Ok(
            sqlx::query_as::<_, AuditRow>("select * from audit order by at desc limit $1")
                .bind(limit)
                .fetch_all(&self.pool)
                .await?,
        )
    }
}
