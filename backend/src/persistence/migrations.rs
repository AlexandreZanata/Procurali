//! Migration runner over the compile-time embedded journal.
//!
//! - Migrations are append-only: SQLx records checksums in `_sqlx_migrations`
//!   and refuses a dirty or edited history instead of partially applying it.
//!   Each migration file runs inside a transaction, so a failing statement
//!   rolls back without recording a version.
//! - Privilege separation: [`apply`] takes an already-connected pool so the
//!   caller chooses the credential. Local/test flows use the owner URL from the
//!   Compose service; production runs migrations with a dedicated owner role
//!   while the runtime uses a least-privilege DML role (production runbook,
//!   later phase). The runner itself never reads URLs, secrets, or environment.

use std::fmt;

/// Embedded migration journal (`backend/migrations`, locked at compile time).
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Apply every pending migration to a fresh or current database.
///
/// # Errors
///
/// Returns [`sqlx::migrate::MigrateError`] when the database is unreachable,
/// the history was edited after application, or a migration statement fails
/// (in which case that version is not recorded).
pub async fn apply(pool: &sqlx::PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}

/// Typed schema-agreement failure. Static reasons only — no versions, no
/// database names, no URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaError {
    /// The database could not be reached or the journal table is missing.
    Unreachable,
    /// The applied versions differ from the embedded journal.
    Mismatch,
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Self::Unreachable => "schema history unreachable",
            Self::Mismatch => "applied schema differs from the tracked migrations",
        };
        f.write_str(reason)
    }
}

impl std::error::Error for SchemaError {}

/// Check that the applied schema equals the embedded journal exactly.
///
/// The served application refuses to start on a mismatch: a partially
/// migrated database would otherwise serve requests against an unknown
/// schema. This check never applies anything — migration stays a separate,
/// explicitly authorized step.
///
/// # Errors
///
/// Returns [`SchemaError::Unreachable`] when the history table cannot be read
/// and [`SchemaError::Mismatch`] when the applied versions differ from the
/// tracked ones.
pub async fn verify_schema(pool: &sqlx::PgPool) -> Result<(), SchemaError> {
    let applied: Vec<i64> = sqlx::query_scalar(
        "SELECT version FROM _sqlx_migrations WHERE success = TRUE ORDER BY version",
    )
    .fetch_all(pool)
    .await
    .map_err(|_| SchemaError::Unreachable)?;
    let expected: Vec<i64> = MIGRATOR.iter().map(|migration| migration.version).collect();
    if applied == expected {
        Ok(())
    } else {
        Err(SchemaError::Mismatch)
    }
}
