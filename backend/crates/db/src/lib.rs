//! Database layer.
//!
//! Holds the SQLx connection pool, runs migrations at startup, and exposes
//! repository modules per aggregate. Higher-level crates depend on this rather
//! than on `sqlx` directly.

use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

pub mod users;

#[derive(Debug, Clone)]
pub struct DbConfig {
    pub url: String,
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout_secs: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migrate: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

pub type DbResult<T> = Result<T, DbError>;

/// Embedded migrations from the workspace-root `migrations/` folder.
/// The path is relative to the *crate* manifest, so we walk up two levels.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// Build a connection pool. The caller decides whether to run migrations.
pub async fn connect(cfg: &DbConfig) -> DbResult<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(cfg.max_connections)
        .min_connections(cfg.min_connections)
        .acquire_timeout(Duration::from_secs(cfg.acquire_timeout_secs))
        .connect(&cfg.url)
        .await?;
    Ok(pool)
}

/// Run all pending migrations.
pub async fn run_migrations(pool: &PgPool) -> DbResult<()> {
    MIGRATOR.run(pool).await?;
    Ok(())
}

/// Convert DB errors into the workspace-wide `AppError`.
///
/// Postgres SQLSTATE `23505` (unique_violation) is surfaced as
/// `AppError::Conflict` so callers (e.g. `POST /auth/register`) return
/// HTTP 409 instead of leaking a 500 on duplicate inputs.
impl From<DbError> for generic_auth_core::AppError {
    fn from(e: DbError) -> Self {
        match &e {
            DbError::Sqlx(sqlx::Error::RowNotFound) => generic_auth_core::AppError::NotFound,
            DbError::Sqlx(sqlx::Error::Database(db)) if db.code().as_deref() == Some("23505") => {
                generic_auth_core::AppError::Conflict(unique_violation_message(db.constraint()))
            }
            _ => generic_auth_core::AppError::Database(e.to_string()),
        }
    }
}

fn unique_violation_message(constraint: Option<&str>) -> String {
    match constraint {
        Some("users_email_key") => "email already exists".into(),
        Some(c) => format!("already exists ({c})"),
        None => "already exists".into(),
    }
}
