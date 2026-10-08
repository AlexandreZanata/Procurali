//! PostgreSQL foundation acceptance: real 18.6 features, never substitutes.
//!
//! Requires an explicit `TEST_DATABASE_URL` pointing at a disposable
//! `procurali_test_*` database (see `scripts/infra.sh up-test`). A missing or
//! non-disposable URL fails loudly — database absence is an error, never a skip.
//! Each test owns a freshly created database, so parallel execution is safe.

use procurali_backend::persistence::migrations;
use sqlx::{PgPool, Row};

/// This file's queryset: every name starts with `procurali_test_p02t03_`.
const DISPOSABLE_PREFIX: &str = "procurali_test_p02t03_";

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
    assert_eq!(before, vec![1], "exactly the foundation version applied");
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

    // Version 2 recorded nothing; its partial objects rolled back.
    let versions = journal_versions(&pool).await;
    assert_eq!(versions, vec![1], "only the foundation version stands");
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
