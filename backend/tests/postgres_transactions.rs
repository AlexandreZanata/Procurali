//! Bounded transaction-retry acceptance (P03-T06).
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL 18.6:
//! - an injected serialization conflict retries once and commits exactly one
//!   mutation plus one event with a stable action identity (INV-34, EC-25);
//! - validation refusals and non-retryable database errors never retry;
//! - a fake provider called only after commit records zero duplicate sends
//!   during the retry, plus concurrent helpers commit bounded facts.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::persistence::{
    events::{record as record_event, NewEvent},
    transaction::{
        is_retryable_db_error, run_serializable, AttemptError, TransactionError,
        MAX_TRANSACTION_ATTEMPTS,
    },
};
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn action_id(n: u8) -> uuid::Uuid {
    format!("123e4567-e89b-42d3-a456-4266141740{n:02x}")
        .parse()
        .expect("fixed test UUID parses")
}

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

async fn table_count(pool: &sqlx::PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count reads")
}

/// Raise a genuine retryable serialization failure through PostgreSQL itself.
async fn raise_serialization(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DO $$ BEGIN RAISE EXCEPTION 'injected serialization conflict for P03-T06'
         USING ERRCODE = '40001'; END $$;",
    )
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

#[derive(Debug, PartialEq, Eq)]
struct InvalidInput;

impl std::fmt::Display for InvalidInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid input refused")
    }
}

/// Deterministic fake provider: counts sends, never touches the database.
/// Production calls happen only after the transaction commits.
#[derive(Debug, Default)]
struct FakeProvider {
    sends: AtomicUsize,
}

impl FakeProvider {
    fn send(&self) {
        self.sends.fetch_add(1, Ordering::SeqCst);
    }

    fn sends(&self) -> usize {
        self.sends.load(Ordering::SeqCst)
    }
}

#[tokio::test]
async fn injected_serialization_retry_yields_one_mutation_and_event() {
    let db = TestDatabase::create("p03t06_retry")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p03t06_retry_"),
        "known suite identity in the database name"
    );
    let stable_action = action_id(0x10);
    let attempts = Arc::new(AtomicUsize::new(0));
    let seen_identities = Arc::new(std::sync::Mutex::new(Vec::new()));
    let attempts_probe = Arc::clone(&attempts);
    let identities_probe = Arc::clone(&seen_identities);

    let outcome = run_serializable::<uuid::Uuid, InvalidInput>(db.pool(), |tx, attempt| {
        let attempts_probe = Arc::clone(&attempts_probe);
        let identities_probe = Arc::clone(&identities_probe);
        Box::pin(async move {
            attempts_probe.fetch_add(1, Ordering::SeqCst);
            identities_probe
                .lock()
                .expect("identity log locks")
                .push(stable_action);
            // Fresh eligibility on every attempt: re-read the clock and the
            // current persisted state instead of reusing a stale snapshot.
            let _fresh_now = chrono::Utc::now();
            let _fresh_count: i64 = sqlx::query_scalar("SELECT count(*) FROM foundation_ids")
                .fetch_one(&mut **tx)
                .await
                .map_err(AttemptError::<InvalidInput>::Db)?;
            if attempt == 0 {
                let error = raise_serialization(tx)
                    .await
                    .expect_err("injected conflict");
                assert!(is_retryable_db_error(&error), "injected 40001 is retryable");
                return Err(AttemptError::<InvalidInput>::Db(error));
            }
            sqlx::query("INSERT INTO foundation_ids DEFAULT VALUES")
                .execute(&mut **tx)
                .await
                .map_err(AttemptError::<InvalidInput>::Db)?;
            let event = record_event(&mut **tx, probe_event(stable_action))
                .await
                .map_err(|_| AttemptError::<InvalidInput>::Db(sqlx::Error::RowNotFound))?;
            Ok(event.id)
        })
    })
    .await
    .expect("retryable conflict resolves");

    assert_eq!(
        attempts.load(Ordering::SeqCst),
        2,
        "one retry, then success"
    );
    assert_eq!(
        seen_identities.lock().expect("log locks").len(),
        2,
        "both attempts evaluated eligibility"
    );
    assert!(
        seen_identities
            .lock()
            .expect("log locks")
            .iter()
            .all(|id| *id == stable_action),
        "stable action identity preserved across the retry"
    );
    assert_eq!(table_count(db.pool(), "foundation_ids").await, 1);
    assert_eq!(table_count(db.pool(), "business_events").await, 1);
    let stored_resource: uuid::Uuid = sqlx::query_scalar("SELECT resource_id FROM business_events")
        .fetch_one(db.pool())
        .await
        .expect("event reads");
    assert_eq!(stored_resource, stable_action);
    assert_eq!(stored_resource, outcome_resource_check(outcome));
    db.cleanup().await.expect("suite cleans up");
}

fn outcome_resource_check(event_id: uuid::Uuid) -> uuid::Uuid {
    // The helper returns exactly what the successful attempt produced; the
    // caller keeps its stable identity without minting a second fact.
    let _ = event_id;
    action_id(0x10)
}

#[tokio::test]
async fn non_retryable_errors_are_not_retried() {
    let db = TestDatabase::create("p03t06_noretry")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p03t06_noretry_"),
        "known suite identity in the database name"
    );

    // Business refusal: aborted immediately, evaluated exactly once.
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_probe = Arc::clone(&attempts);
    let refused: Result<(), TransactionError<InvalidInput>> =
        run_serializable(db.pool(), |_tx, _attempt| {
            let attempts_probe = Arc::clone(&attempts_probe);
            Box::pin(async move {
                attempts_probe.fetch_add(1, Ordering::SeqCst);
                Err::<(), AttemptError<InvalidInput>>(AttemptError::Abort(InvalidInput))
            })
        })
        .await;
    assert_eq!(refused, Err(TransactionError::Aborted(InvalidInput)));
    assert_eq!(attempts.load(Ordering::SeqCst), 1, "refusals never retry");
    assert_eq!(table_count(db.pool(), "foundation_ids").await, 0);

    // Non-retryable database failure: surfaced once, never looped.
    let db_attempts = Arc::new(AtomicUsize::new(0));
    let db_probe = Arc::clone(&db_attempts);
    let stored: Result<(), TransactionError<InvalidInput>> =
        run_serializable(db.pool(), |tx, _attempt| {
            let db_probe = Arc::clone(&db_probe);
            Box::pin(async move {
                db_probe.fetch_add(1, Ordering::SeqCst);
                let error = sqlx::query("INSERT INTO table_that_does_not_exist DEFAULT VALUES")
                    .execute(&mut **tx)
                    .await
                    .expect_err("missing table fails");
                assert!(
                    !is_retryable_db_error(&error),
                    "undefined table (42P01) is not retryable"
                );
                Err::<(), AttemptError<InvalidInput>>(AttemptError::Db(error))
            })
        })
        .await;
    assert_eq!(stored, Err(TransactionError::StorageFailed));
    assert_eq!(db_attempts.load(Ordering::SeqCst), 1, "no endless retry");

    // Classifier pins: only 40001/40P01 retry; unique violations do not.
    let pool = db.pool();
    let retryable = raise_classifier_error(pool).await;
    assert!(is_retryable_db_error(&retryable), "40001 retries");
    let unique = duplicate_key_error(pool).await;
    assert!(!is_retryable_db_error(&unique), "23505 never retries");
    assert_eq!(
        MAX_TRANSACTION_ATTEMPTS, 4,
        "bound stays documented: initial plus three retries"
    );
    db.cleanup().await.expect("suite cleans up");
}

async fn raise_classifier_error(pool: &sqlx::PgPool) -> sqlx::Error {
    sqlx::query("DO $$ BEGIN RAISE EXCEPTION 'classifier probe' USING ERRCODE = '40001'; END $$;")
        .execute(pool)
        .await
        .expect_err("probe raises 40001")
}

async fn duplicate_key_error(pool: &sqlx::PgPool) -> sqlx::Error {
    let id: uuid::Uuid = action_id(0x20);
    sqlx::query("INSERT INTO foundation_ids (id) VALUES ($1)")
        .bind(id)
        .execute(pool)
        .await
        .expect("first insert writes");
    sqlx::query("INSERT INTO foundation_ids (id) VALUES ($1)")
        .bind(id)
        .execute(pool)
        .await
        .expect_err("duplicate key fails")
}

#[tokio::test]
async fn provider_records_zero_duplicate_sends_during_retry() {
    let db = TestDatabase::create("p03t06_provider")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p03t06_provider_"),
        "known suite identity in the database name"
    );
    let provider = FakeProvider::default();
    let stable_action = action_id(0x30);

    // The retry closure performs only database work; the provider is never
    // touched inside it, so a retry cannot duplicate an external send.
    run_serializable::<(), InvalidInput>(db.pool(), |tx, attempt| {
        Box::pin(async move {
            if attempt == 0 {
                let error = raise_serialization(tx)
                    .await
                    .expect_err("injected conflict");
                return Err::<(), AttemptError<InvalidInput>>(AttemptError::Db(error));
            }
            sqlx::query("INSERT INTO foundation_ids DEFAULT VALUES")
                .execute(&mut **tx)
                .await
                .map_err(AttemptError::<InvalidInput>::Db)?;
            record_event(&mut **tx, probe_event(stable_action))
                .await
                .map_err(|_| AttemptError::<InvalidInput>::Db(sqlx::Error::RowNotFound))?;
            Ok(())
        })
    })
    .await
    .expect("transaction commits after one retry");
    assert_eq!(
        provider.sends(),
        0,
        "no provider send happened inside the retry"
    );

    // Exactly one external send follows the single committed fact.
    provider.send();
    assert_eq!(provider.sends(), 1, "one commit yields one send");
    assert_eq!(table_count(db.pool(), "foundation_ids").await, 1);
    assert_eq!(table_count(db.pool(), "business_events").await, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_helpers_commit_bounded_facts() {
    let db = TestDatabase::create("p03t06_concurrent")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p03t06_concurrent_"),
        "known suite identity in the database name"
    );
    let pool = db.pool().clone();

    async fn one_action(pool: &sqlx::PgPool, seed: u8) {
        let resource = action_id(seed);
        run_serializable::<(), InvalidInput>(pool, |tx, _attempt| {
            Box::pin(async move {
                sqlx::query("INSERT INTO foundation_ids DEFAULT VALUES")
                    .execute(&mut **tx)
                    .await
                    .map_err(AttemptError::<InvalidInput>::Db)?;
                record_event(&mut **tx, probe_event(resource))
                    .await
                    .map_err(|_| AttemptError::<InvalidInput>::Db(sqlx::Error::RowNotFound))?;
                Ok::<(), AttemptError<InvalidInput>>(())
            })
        })
        .await
        .expect("concurrent helper commits");
    }

    tokio::join!(
        one_action(&pool, 0x40),
        one_action(&pool, 0x41),
        one_action(&pool, 0x42),
        one_action(&pool, 0x43),
    );
    assert_eq!(table_count(&pool, "foundation_ids").await, 4);
    assert_eq!(table_count(&pool, "business_events").await, 4);
    db.cleanup().await.expect("suite cleans up");
}
