//! Bounded PostgreSQL access with redacted handling.
//!
//! Every limit is finite: connection count, acquisition wait, idle retention,
//! and per-statement execution. Failures are typed ([`PoolError`]) and carry
//! static reasons only — URLs, users, passwords, and database names never
//! appear in `Display` or `Debug`. Test-environment guards
//! ([`connect_disposable`]) refuse non-disposable databases before any
//! connection attempt.

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{fmt, str::FromStr, time::Duration};

/// Per-statement execution bound applied to every pooled connection.
pub const STATEMENT_TIMEOUT: Duration = Duration::from_secs(10);

/// Finite pool configuration. All fields bounded; see `Default`.
#[derive(Debug, Clone, Copy)]
pub struct PoolConfig {
    /// Maximum simultaneous connections. Small on purpose.
    pub max_connections: u32,
    /// Longest wait for a free connection before a typed failure.
    pub acquire_timeout: Duration,
    /// Idle retention before a connection is closed.
    pub idle_timeout: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 10,
            acquire_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(300),
        }
    }
}

/// Typed pool failure. Static reasons only — never a URL, user, or password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolError {
    /// The URL does not parse as a postgres connection string.
    InvalidUrl,
    /// The database is not an explicitly disposable test database, or it
    /// equals the application database.
    ForbiddenDatabase,
    /// No connection could be established within the acquire bound.
    ConnectionFailed,
    /// An acquired connection failed to execute (loss, timeout, rejection).
    QueryFailed,
}

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Self::InvalidUrl => "invalid database URL",
            Self::ForbiddenDatabase => {
                "forbidden database (not an explicitly disposable test database)"
            }
            Self::ConnectionFailed => "database connection failed",
            Self::QueryFailed => "database query failed",
        };
        f.write_str(reason)
    }
}

impl std::error::Error for PoolError {}

/// Database name segment of a postgres URL (`db` of `.../db?opts`).
fn database_name(url: &str) -> Result<&str, PoolError> {
    let after_scheme = url.split("://").nth(1).ok_or(PoolError::InvalidUrl)?;
    let path = after_scheme
        .split('/')
        .next_back()
        .ok_or(PoolError::InvalidUrl)?;
    let name = path.split('?').next().unwrap_or_default();
    if name.is_empty() {
        return Err(PoolError::InvalidUrl);
    }
    Ok(name)
}

/// `host:port/database` label for logs. Never the user, password, or full URL.
pub fn connection_label(url: &str) -> Result<String, PoolError> {
    let after_scheme = url.split("://").nth(1).ok_or(PoolError::InvalidUrl)?;
    let authority = after_scheme
        .split('/')
        .next()
        .ok_or(PoolError::InvalidUrl)?;
    let hostport = authority.rsplit('@').next().ok_or(PoolError::InvalidUrl)?;
    let (host, port) = match hostport.split_once(':') {
        Some((host, port)) => (host, port),
        None => (hostport, "5432"),
    };
    if host.is_empty() {
        return Err(PoolError::InvalidUrl);
    }
    Ok(format!("{host}:{port}/{}", database_name(url)?))
}

/// Connect with finite limits and verify with one acquisition.
///
/// The verification acquisition is what makes an unreachable database a
/// deterministic typed failure instead of a lazy surprise.
///
/// # Errors
///
/// Returns [`PoolError`] without ever exposing the URL.
pub async fn connect(url: &str, config: &PoolConfig) -> Result<sqlx::PgPool, PoolError> {
    let options = PgConnectOptions::from_str(url).map_err(|_| PoolError::InvalidUrl)?;
    let pool = PgPoolOptions::new()
        .max_connections(config.max_connections)
        .acquire_timeout(config.acquire_timeout)
        .idle_timeout(config.idle_timeout)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::Executor::execute(connection, "SET statement_timeout = '10s'")
                    .await
                    .map(|_| ())
            })
        })
        .connect_with(options)
        .await
        .map_err(|_| PoolError::ConnectionFailed)?;
    // Fail fast: prove the database answers instead of handing out a lazy pool.
    pool.acquire()
        .await
        .map_err(|_| PoolError::ConnectionFailed)?;
    Ok(pool)
}

/// Connect only to an explicitly disposable test database.
///
/// Refuses non-`procurali_test_*` names and any URL equal to the application
/// `DATABASE_URL` before any connection attempt.
///
/// # Errors
///
/// Returns [`PoolError::ForbiddenDatabase`] for refused names (no values),
/// otherwise the [`connect`] outcome.
pub async fn connect_disposable(url: &str, config: &PoolConfig) -> Result<sqlx::PgPool, PoolError> {
    let name = database_name(url)?;
    if !name.starts_with("procurali_test_") {
        return Err(PoolError::ForbiddenDatabase);
    }
    if let Ok(app_url) = std::env::var("DATABASE_URL") {
        if !app_url.is_empty() && url == app_url {
            return Err(PoolError::ForbiddenDatabase);
        }
    }
    connect(url, config).await
}

/// Liveness probe: exactly one `SELECT 1` through the pool.
///
/// # Errors
///
/// Returns [`PoolError::QueryFailed`] when no answer arrives in bounds.
pub async fn probe(pool: &sqlx::PgPool) -> Result<(), PoolError> {
    let one: i32 = sqlx::query_scalar("SELECT 1")
        .fetch_one(pool)
        .await
        .map_err(|_| PoolError::QueryFailed)?;
    if one == 1 {
        Ok(())
    } else {
        Err(PoolError::QueryFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_finite() {
        let config = PoolConfig::default();
        assert!(config.max_connections > 0 && config.max_connections <= 100);
        assert!(!config.acquire_timeout.is_zero());
        assert!(!config.idle_timeout.is_zero());
        assert!(!STATEMENT_TIMEOUT.is_zero());
    }

    #[test]
    fn labels_hide_credentials() {
        let label = connection_label(
            "postgres://procurali_test:canary-password@127.0.0.1:55436/procurali_test_x",
        )
        .expect("valid URL labels");
        assert_eq!(label, "127.0.0.1:55436/procurali_test_x");
        assert!(!label.contains("canary-password"));
    }

    #[test]
    fn malformed_urls_are_refused() {
        assert_eq!(connection_label("not-a-url"), Err(PoolError::InvalidUrl));
        assert_eq!(
            connection_label("postgres://user@127.0.0.1:5432/"),
            Err(PoolError::InvalidUrl)
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            PoolError::InvalidUrl,
            PoolError::ForbiddenDatabase,
            PoolError::ConnectionFailed,
            PoolError::QueryFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
            assert!(!rendered.contains("postgres://"));
        }
    }
}
