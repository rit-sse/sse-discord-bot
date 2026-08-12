use anyhow::{Context, Result};
use secrecy::ExposeSecret;
use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::config::DBConfig;

pub async fn connect(config: &DBConfig) -> Result<PgPool> {
    tracing::info!(
        max_connections = config.max_connections,
        "connecting to Postgres..."
    );

    let pool = PgPoolOptions::new()
        .max_connections(config.max_connections)
        .connect(config.url.expose_secret())
        .await
        .context("failed to connect to Postgres")?;

    tracing::info!(
        max_connections = config.max_connections,
        "󰄬 connected to Postgres"
    );

    Ok(pool)
}
