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
