//! PostgreSQL foundation acceptance: real 18.6 features, never substitutes.
//!
//! Requires an explicit `TEST_DATABASE_URL` pointing at a disposable
//! `procurali_test_*` database (see `scripts/infra.sh up-test`). A missing or
//! non-disposable URL fails loudly — database absence is an error, never a skip.
//! Each test owns a freshly created database, so parallel execution is safe.

use procurali_backend::http::health::probe_pool;
use procurali_backend::persistence::migrations;
use procurali_backend::persistence::pool::{connect_disposable, probe, PoolConfig, PoolError};
use sqlx::{PgPool, Row};
use std::time::{Duration, Instant};

/// This file's queryset: every name starts with `procurali_test_`.
const DISPOSABLE_PREFIX: &str = "procurali_test_";

fn test_base_url() -> String {
    let url = std::env::var("TEST_DATABASE_URL").expect(
        "TEST_DATABASE_URL must point at a disposable procurali_test_* database (see scripts/infra.sh up-test)",
    );
    assert!(
        url.contains("/procurali_test_"),
        "refusing non-disposable TEST_DATABASE_URL"
    );
    url
}

/// Replace the database path segment of a postgres URL, keeping authority intact.
fn url_with_database(url: &str, database: &str) -> String {
    let (head, _) = url.rsplit_once('/').expect("URL has a database path");
    format!("{head}/{database}")
}

/// Drop (if present) and create a pristine database, then connect and migrate it.
async fn fresh_migrated_database(name: &str) -> PgPool {
    assert!(
        name.starts_with(DISPOSABLE_PREFIX),
        "only disposable test databases may be created here"
    );
    let base = test_base_url();
    let maintenance_url = url_with_database(&base, "postgres");
    let admin = PgPool::connect(&maintenance_url)
        .await
        .expect("test postgres must be reachable");
    sqlx::query(&format!("DROP DATABASE IF EXISTS \"{name}\""))
        .execute(&admin)
        .await
        .expect("stale test database drops");
    sqlx::query(&format!("CREATE DATABASE \"{name}\""))
        .execute(&admin)
        .await
        .expect("fresh test database creates");
    admin.close().await;
    let pool = PgPool::connect(&url_with_database(&base, name))
        .await
        .expect("fresh test database connects");
    migrations::apply(&pool)
        .await
        .expect("foundation migration applies");
    pool
}

async fn journal_versions(pool: &PgPool) -> Vec<i64> {
    sqlx::query("SELECT version FROM _sqlx_migrations ORDER BY version")
        .fetch_all(pool)
        .await
        .expect("migration journal reads")
        .into_iter()
        .map(|row| row.get::<i64, _>("version"))
        .collect()
}

/// Pool against a freshly migrated database with caller-chosen limits.
async fn fresh_pool(name: &str, configure: impl FnOnce(&mut PoolConfig)) -> PgPool {
    let seed = fresh_migrated_database(name).await;
    seed.close().await;
    open_pool(name, configure).await
}

/// Pool against an already-migrated database: connects only, never recreates.
async fn open_pool(name: &str, configure: impl FnOnce(&mut PoolConfig)) -> PgPool {
    let url = test_base_url();
    let mut config = PoolConfig::default();
    configure(&mut config);
    connect_disposable(&url_with_database(&url, name), &config)
        .await
        .expect("bounded pool connects")
}

/// Version nibble (first char of the third UUID group) must be '7'.
fn assert_uuidv7(id: &str) {
    assert_eq!(id.len(), 36, "UUID text shape");
    assert_eq!(
        id.chars().nth(14),
        Some('7'),
        "server-generated id {id} is version 7"
    );
}

#[tokio::test]
async fn generated_ids_are_version_7_from_postgresql() {
    let pool = fresh_migrated_database("procurali_test_p02t03_uuidv7").await;
    // No client-side id: PostgreSQL applies the column DEFAULT uuidv7().
    let first: String =
        sqlx::query_scalar("INSERT INTO foundation_ids DEFAULT VALUES RETURNING id::text")
            .fetch_one(&pool)
            .await
            .expect("server-generated insert");
    let second: String =
        sqlx::query_scalar("INSERT INTO foundation_ids DEFAULT VALUES RETURNING id::text")
            .fetch_one(&pool)
            .await
            .expect("server-generated insert");
    assert_ne!(first, second);
    assert_uuidv7(&first);
    assert_uuidv7(&second);
    assert!(
        second > first,
        "monotonic UUIDv7 ordering from the server ({first} < {second})"
    );
    // The native generator itself, independent of any table default.
    let native: String = sqlx::query_scalar("SELECT uuidv7()::text")
        .fetch_one(&pool)
        .await
        .expect("native uuidv7() call");
    assert_uuidv7(&native);
    pool.close().await;
}

#[tokio::test]
async fn second_migration_run_changes_nothing() {
    let pool = fresh_migrated_database("procurali_test_p02t03_rerun").await;
    let before = journal_versions(&pool).await;
    assert!(
        before.contains(&1),
        "the foundation version applied among {before:?}"
    );
    migrations::apply(&pool)
        .await
        .expect("second run is a clean no-op");
    let after = journal_versions(&pool).await;
    assert_eq!(after, before, "journal and schema untouched by the rerun");
    let tables: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.tables WHERE table_name = 'foundation_ids'",
    )
    .fetch_one(&pool)
    .await
    .expect("schema introspection");
    assert_eq!(tables, 1);
    pool.close().await;
}

#[tokio::test]
async fn failing_migration_leaves_no_partial_version() {
    let pool = fresh_migrated_database("procurali_test_p02t03_rollback").await;
    let applied = journal_versions(&pool).await;
    let dir = std::env::temp_dir().join("procurali-p02t03-failing");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("stale failing-migration dir clears");
    }
    std::fs::create_dir_all(&dir).expect("failing-migration dir creates");
    std::fs::write(
        dir.join("0001_ok.sql"),
        "CREATE TABLE rollback_probe (id UUID PRIMARY KEY DEFAULT uuidv7());",
    )
    .expect("good migration writes");
    std::fs::write(
        dir.join("0002_broken.sql"),
        "CREATE TABLE rollback_half (id UUID PRIMARY KEY);\nTHIS IS NOT VALID SQL;",
    )
    .expect("broken migration writes");
    let migrator = sqlx::migrate::Migrator::new(dir.clone())
        .await
        .expect("runtime migrator loads");
    let outcome = migrator.run(&pool).await;
    assert!(outcome.is_err(), "broken migration must fail loudly");
    std::fs::remove_dir_all(&dir).expect("failing-migration dir cleans up");

    // The failing version recorded nothing; prior versions stand untouched.
    let versions = journal_versions(&pool).await;
    assert_eq!(versions, applied, "only pre-existing versions stand");
    for table in ["rollback_probe", "rollback_half"] {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM information_schema.tables WHERE table_name = $1",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("schema introspection");
        assert_eq!(count, 0, "no partial objects from the failed version");
    }
    pool.close().await;
}

#[tokio::test]
async fn new_connection_reads_persisted_foundation_data() {
    let writer = fresh_pool("procurali_test_p02t04_reload", |_| {}).await;
    let written: String =
        sqlx::query_scalar("INSERT INTO foundation_ids DEFAULT VALUES RETURNING id::text")
            .fetch_one(&writer)
            .await
            .expect("foundation row writes");
    writer.close().await;

    // A brand-new connection (separate pool) observes the persisted fact.
    let reader = open_pool("procurali_test_p02t04_reload", |_| {}).await;
    let seen: Vec<String> =
        sqlx::query_scalar("SELECT id::text FROM foundation_ids ORDER BY recorded_at")
            .fetch_all(&reader)
            .await
            .expect("foundation rows read through a new connection");
    assert_eq!(seen, vec![written]);
    reader.close().await;
}

#[tokio::test]
async fn unavailable_db_fails_readiness_and_connect() {
    // Valid disposable shape, nothing listening: deterministic typed failure.
    let dead_url =
        "postgres://procurali_test:canary-dead-pw@127.0.0.1:55999/procurali_test_p02t04_dead";
    let config = PoolConfig {
        acquire_timeout: Duration::from_secs(2),
        ..PoolConfig::default()
    };
    let error = connect_disposable(dead_url, &config)
        .await
        .expect_err("unreachable database must fail");
    assert_eq!(error, PoolError::ConnectionFailed);
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains("canary-dead-pw"));

    // The pool-backed readiness handler reports the outage with names only.
    let lazy = PgPool::connect_lazy(dead_url).expect("lazy pool builds");
    let app = axum::Router::new()
        .route("/ready", axum::routing::get(probe_pool))
        .with_state(lazy);
    let response = tower::ServiceExt::oneshot(
        app,
        axum::http::Request::get("/ready")
            .body(axum::body::Body::empty())
            .expect("test request builds"),
    )
    .await
    .expect("readiness responds");
    assert_eq!(
        response.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("body reads");
    assert_eq!(
        body.as_ref(),
        br#"{"status":"not_ready","unavailable":["database"]}"#
    );
}

#[tokio::test]
async fn refusals_carry_no_secrets() {
    // Guard fires before any connection: no database needed for this proof.
    let error = connect_disposable(
        "postgres://someone:canary-app-pw@127.0.0.1:5432/procurali_dev",
        &PoolConfig::default(),
    )
    .await
    .expect_err("non-disposable database refused");
    assert_eq!(error, PoolError::ForbiddenDatabase);
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains("canary-app-pw"));
    assert!(!rendered.contains("procurali_dev"));
}

#[tokio::test]
async fn exhausted_pool_is_bounded() {
    let pool = fresh_pool("procurali_test_p02t04_saturation", |config| {
        config.max_connections = 1;
        config.acquire_timeout = Duration::from_secs(2);
    })
    .await;
    // Hold the single connection for the whole test.
    let held = pool.acquire().await.expect("first connection acquires");
    let started = Instant::now();
    let outcome = tokio::time::timeout(Duration::from_secs(15), pool.acquire()).await;
    let waited = started.elapsed();
    assert!(
        waited < Duration::from_secs(10),
        "exhaustion surfaced in bounds ({waited:?}), never hangs"
    );
    assert!(
        outcome.expect("outer guard intact").is_err(),
        "second acquisition fails instead of false success"
    );
    drop(held);
    // The pool still serves after the pressure lifts.
    probe(&pool).await.expect("pool recovers after saturation");
    pool.close().await;
}

#[tokio::test]
async fn slow_query_is_bounded_by_statement_timeout() {
    let pool = fresh_pool("procurali_test_p02t04_querybound", |_| {}).await;
    let started = Instant::now();
    let outcome = tokio::time::timeout(Duration::from_secs(25), async {
        sqlx::query("SELECT pg_sleep(15)").execute(&pool).await
    })
    .await;
    let elapsed = started.elapsed();
    match outcome {
        Ok(Err(_)) => {}
        other => panic!("slow query must fail via statement timeout, got {other:?}"),
    }
    assert!(
        elapsed >= Duration::from_secs(9),
        "timeout fired after waiting, not instantly ({elapsed:?})"
    );
    assert!(
        elapsed < Duration::from_secs(20),
        "timeout fired in bounds ({elapsed:?})"
    );
    pool.close().await;
}
