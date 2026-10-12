//! Transaction atomicity under injected faults (P17-T03): exhaustion,
//! post-event failure, and snapshot isolation over real PostgreSQL.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Genuine serialization conflicts come from PostgreSQL
//! itself (`RAISE ... ERRCODE 40001`); assertions pin exact attempt
//! counts and row counts so a half-state or duplicate fact cannot hide.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::persistence::{
    events::{record as record_event, NewEvent},
    transaction::{
        run_serializable, AttemptError, TransactionError, MAX_TRANSACTION_ATTEMPTS,
    },
};
use serde_json::json;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

fn probe_event(resource: uuid::Uuid) -> NewEvent {
    NewEvent {
        actor_id: None,
        resource_kind: "request",
        resource_id: resource,
        cycle: Some(1),
        revision: None,
        effective_at: chrono::Utc::now(),
        kind: "request.published",
        policy: "mvp-free",
        source: "api",
        payload: json!({"cycle": 1}),
    }
}

async fn raise_serialization(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DO $$ BEGIN RAISE EXCEPTION 'injected serialization conflict for P17-T03'
         USING ERRCODE = '40001'; END $$;",
    )
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

async fn table_count(pool: &sqlx::PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count reads")
}

#[derive(Debug, PartialEq, Eq)]
struct Never;

impl std::fmt::Display for Never {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("unreachable business refusal")
    }
}

#[tokio::test]
async fn exhausted_serialization_retries_commit_nothing() {
    let db = TestDatabase::create("p17t03_exhausted")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p17t03_exhausted_"),
        "known suite identity in the database name"
    );
    let attempts = Arc::new(AtomicU32::new(0));
    let resource = uuid::Uuid::now_v7();
    let outcome: Result<(), TransactionError<Never>> = run_serializable(db.pool(), |tx, _| {
        let attempts = Arc::clone(&attempts);
        let event = probe_event(resource);
        Box::pin(async move {
            attempts.fetch_add(1, Ordering::SeqCst);
            sqlx::query("INSERT INTO foundation_ids DEFAULT VALUES")
                .execute(&mut **tx)
                .await
                .map_err(AttemptError::Db)?;
            record_event(&mut **tx, event)
                .await
                .map_err(|_| AttemptError::Db(sqlx::Error::Protocol("event seam failed".into())))?;
            raise_serialization(tx).await.map_err(AttemptError::Db)?;
            Ok(())
        })
    })
    .await;
    // Every attempt hits the injected conflict: bounded exhaustion with
    // an explicit count, and no partial row anywhere.
    assert_eq!(
        outcome,
        Err(TransactionError::RetryExhausted {
            attempts: MAX_TRANSACTION_ATTEMPTS
        })
    );
    assert_eq!(attempts.load(Ordering::SeqCst), MAX_TRANSACTION_ATTEMPTS);
    assert_eq!(table_count(db.pool(), "foundation_ids").await, 0);
    assert_eq!(table_count(db.pool(), "business_events").await, 0);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn post_event_failure_retries_to_exactly_one_fact_set() {
    let db = TestDatabase::create("p17t03_post_event")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p17t03_post_event_"),
        "known suite identity in the database name"
    );
    let attempts = Arc::new(AtomicU32::new(0));
    let resource = uuid::Uuid::now_v7();
    let outcome: Result<(), TransactionError<Never>> = run_serializable(db.pool(), |tx, _| {
        let attempts = Arc::clone(&attempts);
        let event = probe_event(resource);
        Box::pin(async move {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            sqlx::query("INSERT INTO foundation_ids DEFAULT VALUES")
                .execute(&mut **tx)
                .await
                .map_err(AttemptError::Db)?;
            record_event(&mut **tx, event)
                .await
                .map_err(|_| AttemptError::Db(sqlx::Error::Protocol("event seam failed".into())))?;
            // The first attempt dies after the event lands; the retry
            // must not duplicate either row.
            if attempt == 0 {
                raise_serialization(tx).await.map_err(AttemptError::Db)?;
            }
            Ok(())
        })
    })
    .await;
    assert_eq!(outcome, Ok(()));
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(table_count(db.pool(), "foundation_ids").await, 1);
    assert_eq!(table_count(db.pool(), "business_events").await, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uncommitted_writes_stay_invisible_until_commit() {
    let db = TestDatabase::create("p17t03_snapshot")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p17t03_snapshot_"),
        "known suite identity in the database name"
    );
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let writer = {
        let pool = db.pool().clone();
        let barrier = Arc::clone(&barrier);
        async move {
            let mut tx = pool.begin().await.expect("writer begins");
            sqlx::query("INSERT INTO foundation_ids DEFAULT VALUES")
                .execute(&mut *tx)
                .await
                .expect("writer writes");
            // Hold the write uncommitted while the reader looks twice.
            barrier.wait().await;
            barrier.wait().await;
            tx.commit().await.expect("writer commits");
        }
    };
    let reader = {
        let pool = db.pool().clone();
        let barrier = Arc::clone(&barrier);
        async move {
            barrier.wait().await;
            let before: i64 = sqlx::query_scalar("SELECT count(*) FROM foundation_ids")
                .fetch_one(&pool)
                .await
                .expect("reader reads");
            barrier.wait().await;
            // Wait past the commit, then read the settled state.
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let after: i64 = sqlx::query_scalar("SELECT count(*) FROM foundation_ids")
                .fetch_one(&pool)
                .await
                .expect("reader re-reads");
            (before, after)
        }
    };
    let (writer_done, reader_done) = tokio::join!(
        tokio::spawn(writer),
        tokio::spawn(reader),
    );
    writer_done.expect("writer joins");
    let (before, after) = reader_done.expect("reader joins");
    assert_eq!(before, 0, "no dirty read of the uncommitted row");
    assert_eq!(after, 1, "committed row becomes visible exactly once");
    db.cleanup().await.expect("suite cleans up");
}
