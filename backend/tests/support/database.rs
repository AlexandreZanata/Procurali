//! Isolated disposable test databases for integration suites.
//!
//! Each suite owns a freshly created `procurali_test_<tag>_<pid>` database:
//! the tag gives a known suite identity, the process id keeps parallel `cargo
//! test` processes disjoint. Real migrations run on creation; [`TestDatabase`]
//! drops exactly its own database on [`TestDatabase::cleanup`].
//!
//! Failure policy: a missing or non-disposable `TEST_DATABASE_URL` fails loudly
//! (never skips, never falls back to SQLite, memory, or the application
//! database). Errors carry static phases only — no URLs or credentials.

use sqlx::PgPool;

/// Static failure phases for database setup/teardown. No values attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbError {
    /// `TEST_DATABASE_URL` is missing.
    MissingUrl,
    /// The URL is not explicitly disposable, or equals the application URL.
    ForbiddenDatabase,
    /// A setup phase failed (`create database`, `migrate`, `connect`, `tag`).
    SetupFailed(&'static str),
    /// Teardown failed (`admin connect`, `drop database`).
    CleanupFailed(&'static str),
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::MissingUrl => "missing TEST_DATABASE_URL",
            Self::ForbiddenDatabase => "forbidden database (not explicitly disposable)",
            Self::SetupFailed(phase) => phase,
            Self::CleanupFailed(phase) => phase,
        };
        f.write_str(reason)
    }
}

impl std::error::Error for DbError {}

/// An owned disposable database: migrated on creation, dropped on cleanup.
pub struct TestDatabase {
    name: String,
    pool: PgPool,
    admin_url: String,
}

impl TestDatabase {
    /// Read and validate the caller's `TEST_DATABASE_URL`.
    fn test_url() -> Result<String, DbError> {
        let url = std::env::var("TEST_DATABASE_URL").map_err(|_| DbError::MissingUrl)?;
        if !url.contains("/procurali_test_") {
            return Err(DbError::ForbiddenDatabase);
        }
        if let Ok(app_url) = std::env::var("DATABASE_URL") {
            if !app_url.is_empty() && url == app_url {
                return Err(DbError::ForbiddenDatabase);
            }
        }
        Ok(url)
    }

    /// Replace the database path segment, keeping authority intact.
    fn url_with_database(url: &str, database: &str) -> String {
        let (head, _) = url.rsplit_once('/').unwrap_or((url, ""));
        format!("{head}/{database}")
    }

    /// Create `procurali_test_<tag>_<pid>`, migrate it, and connect.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] without exposing URLs or credentials.
    pub async fn create(suite_tag: &str) -> Result<Self, DbError> {
        if suite_tag.is_empty()
            || suite_tag.len() > 32
            || !suite_tag
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(DbError::SetupFailed("tag"));
        }
        let url = Self::test_url()?;
        let name = format!("procurali_test_{}_{}", suite_tag, std::process::id());
        let admin_url = Self::url_with_database(&url, "postgres");
        let admin = PgPool::connect(&admin_url)
            .await
            .map_err(|_| DbError::SetupFailed("admin connect"))?;
        sqlx::query(&format!("DROP DATABASE IF EXISTS \"{name}\""))
            .execute(&admin)
            .await
            .map_err(|_| DbError::SetupFailed("create database"))?;
        sqlx::query(&format!("CREATE DATABASE \"{name}\""))
            .execute(&admin)
            .await
            .map_err(|_| DbError::SetupFailed("create database"))?;
        admin.close().await;
        // Reuses the guarded pool path: prefix and app-URL checks run again here.
        let pool = procurali_backend::persistence::pool::connect_disposable(
            &Self::url_with_database(&url, &name),
            &procurali_backend::persistence::pool::PoolConfig::default(),
        )
        .await
        .map_err(|_| DbError::SetupFailed("connect"))?;
        procurali_backend::persistence::migrations::apply(&pool)
            .await
            .map_err(|_| DbError::SetupFailed("migrate"))?;
        Ok(Self {
            name,
            pool,
            admin_url,
        })
    }

    /// The owned database name (known suite identity).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The migrated pool for fixtures and assertions.
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Close and drop exactly the owned database. Strict: a missing database
    /// fails loudly instead of silently passing.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::CleanupFailed`] without exposing values.
    pub async fn cleanup(self) -> Result<(), DbError> {
        self.pool.close().await;
        let admin = PgPool::connect(&self.admin_url)
            .await
            .map_err(|_| DbError::CleanupFailed("admin connect"))?;
        sqlx::query(&format!("DROP DATABASE \"{}\"", self.name))
            .execute(&admin)
            .await
            .map_err(|_| DbError::CleanupFailed("drop database"))?;
        admin.close().await;
        Ok(())
    }
}
