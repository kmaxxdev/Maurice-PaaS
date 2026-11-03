//! Routes on workspace edges and the domains the host edge serves (ADR-009).

use crate::Store;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct RouteRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub environment_id: Uuid,
    pub service_id: Uuid,
    pub container_port: i32,
    pub block_port: i32,
    pub visibility: String,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct DomainRow {
    pub id: Uuid,
    pub route_id: Uuid,
    pub name: String,
    pub public_ip: Option<String>,
    pub tls_mode: String,
    pub created_at: OffsetDateTime,
}

/// A route with its domains and the names of what it belongs to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteFull {
    pub route: RouteRow,
    pub domains: Vec<DomainRow>,
    pub service: String,
    pub environment: String,
}

/// What the host edge needs per domain: where to proxy.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct HostDomain {
    pub name: String,
    pub public_ip: Option<String>,
    pub tls_mode: String,
    pub block_port: i32,
}

impl Store {
    pub async fn list_routes(&self, workspace_id: Uuid) -> anyhow::Result<Vec<RouteFull>> {
        let rows = sqlx::query_as::<_, RouteRow>(
            "select * from routes where workspace_id = $1 order by block_port",
        )
        .bind(workspace_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(self.hydrate_route(r).await?);
        }
        Ok(out)
    }

    pub async fn get_route(&self, id: Uuid) -> anyhow::Result<Option<RouteFull>> {
        let Some(r) = sqlx::query_as::<_, RouteRow>("select * from routes where id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(self.hydrate_route(r).await?))
    }

    async fn hydrate_route(&self, r: RouteRow) -> anyhow::Result<RouteFull> {
        let domains = sqlx::query_as::<_, DomainRow>(
            "select * from domains where route_id = $1 order by name",
        )
        .bind(r.id)
        .fetch_all(&self.pool)
        .await?;
        let (service, environment): (String, String) = sqlx::query_as(
            "select s.name, e.name from services s join environments e on e.id = s.environment_id where s.id = $1",
        )
        .bind(r.service_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(RouteFull {
            route: r,
            domains,
            service,
            environment,
        })
    }

    pub async fn insert_route(&self, r: &RouteRow) -> anyhow::Result<()> {
        sqlx::query(
            "insert into routes (id, workspace_id, environment_id, service_id, container_port, block_port, visibility, created_at)
             values ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(r.id)
        .bind(r.workspace_id)
        .bind(r.environment_id)
        .bind(r.service_id)
        .bind(r.container_port)
        .bind(r.block_port)
        .bind(&r.visibility)
        .bind(r.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_route(&self, id: Uuid) -> anyhow::Result<bool> {
        let r = sqlx::query("delete from routes where id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    pub async fn get_domain(&self, id: Uuid) -> anyhow::Result<Option<DomainRow>> {
        Ok(
            sqlx::query_as::<_, DomainRow>("select * from domains where id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn get_domain_by_name(&self, name: &str) -> anyhow::Result<Option<DomainRow>> {
        Ok(
            sqlx::query_as::<_, DomainRow>("select * from domains where name = $1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn insert_domain(&self, d: &DomainRow) -> anyhow::Result<()> {
        sqlx::query(
            "insert into domains (id, route_id, name, public_ip, tls_mode, created_at) values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(d.id)
        .bind(d.route_id)
        .bind(&d.name)
        .bind(&d.public_ip)
        .bind(&d.tls_mode)
        .bind(d.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_domain(&self, id: Uuid) -> anyhow::Result<bool> {
        let r = sqlx::query("delete from domains where id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    /// Every domain on the host with the workspace edge port behind it.
    pub async fn list_host_domains(&self) -> anyhow::Result<Vec<HostDomain>> {
        Ok(sqlx::query_as::<_, HostDomain>(
            "select d.name, d.public_ip, d.tls_mode, r.block_port
             from domains d join routes r on r.id = d.route_id where r.visibility = 'public' order by d.name",
        )
        .fetch_all(&self.pool)
        .await?)
    }
}
