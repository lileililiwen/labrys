use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::config::Config;
use crate::error::Result;

/// Shared PostgreSQL connection pool.
pub type Pool = PgPool;

/// Opens a bounded connection pool from process configuration.
pub async fn connect(config: &Config) -> Result<Pool> {
    let pool = PgPoolOptions::new()
        .max_connections(config.max_concurrency as u32 + 4)
        .acquire_timeout(std::time::Duration::from_millis(config.drain_timeout_ms))
        .connect(&config.database_url)
        .await?;
    Ok(pool)
}

/// Runs the crate's versioned migrations to completion.
///
/// Migrations are loaded from the crate's `migrations/` directory rather than
/// the compile-time `sqlx::migrate!` macro: that macro lives behind sqlx's
/// `macros` feature, which pulls every database driver (and, transitively, the
/// `rsa` crate carrying RUSTSEC-2023-0071) into the tree even though only
/// PostgreSQL is used here.
pub async fn run_migrations(pool: &Pool) -> Result<()> {
    let migrations = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let migrator = sqlx::migrate::Migrator::new(migrations).await?;
    migrator.run(pool).await?;
    Ok(())
}
