//! PostgreSQL access for the panel. Migrations are embedded and run at start.

pub use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

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
