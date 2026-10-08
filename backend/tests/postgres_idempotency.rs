//! Durable action-idempotency acceptance (P03-T07).
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL 18.6:
//! - concurrent duplicate actions commit exactly one fact (INV-34, EC-25);
//! - the same key with a different input is refused with `idempotency_key_reuse`;
//! - a rolled-back attempt reserves no key and the retry succeeds;
//! - the record holds a private event reference only (no phone/destination).

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::idempotency::{
    claim, find, ClaimOutcome, CompletedClaim, IdempotencyError, NewClaim,
};
use procurali_backend::persistence::{
    events::{record as record_event, NewEvent},
    transaction::{run_serializable, AttemptError, TransactionError},
};
use serde_json::json;
use std::sync::Arc;

fn actor(n: u8) -> uuid::Uuid {
    format!("123e4567-e89b-42d3-a456-4266141741{n:02x}")
        .parse()
        .expect("fixed test UUID parses")
}

fn resource(n: u8) -> uuid::Uuid {
    format!("123e4567-e89b-42d3-a456-4266141742{n:02x}")
        .parse()
        .expect("fixed test UUID parses")
}

fn probe_event(resource: uuid::Uuid) -> NewEvent {
    NewEvent {
        actor_id: None,
        resource_kind: "outcome",
        resource_id: resource,
        cycle: Some(1),
        revision: None,
        effective_at: chrono::Utc::now(),
        kind: "outcome.recorded",
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

/// Perform one idempotent action with caller-side race convergence.
///
/// Each attempt runs serializable: a visible record replays (same digest) or
/// conflicts (changed digest) without new writes; otherwise the mutation, its
/// event, and its claim commit atomically. A race lost under concurrency
/// surfaces as a rolled-back storage failure and converges on retry in a fresh
/// transaction, which then replays the winner. Bounded: at most 8 outer tries.
async fn perform(
    pool: &sqlx::PgPool,
    actor_id: uuid::Uuid,
    operation: &'static str,
    key: &str,
    digest: &str,
) -> CompletedClaim {
    for _ in 0..8 {
        let key_owned = key.to_owned();
        let digest_owned = digest.to_owned();
        let outcome: Result<CompletedClaim, TransactionError<IdempotencyError>> =
            run_serializable(pool, |tx, _attempt| {
                let key_owned = key_owned.clone();
                let digest_owned = digest_owned.clone();
                Box::pin(async move {
                    if let Some(existing) = find(&mut **tx, actor_id, operation, &key_owned)
                        .await
                        .map_err(|error| match error {
                        IdempotencyError::StorageFailed => {
                            AttemptError::Abort(IdempotencyError::StorageFailed)
                        }
                        other => AttemptError::Abort(other),
                    })? {
                        if existing.input_digest == digest_owned {
                            return Ok(existing);
                        }
                        return Err(AttemptError::Abort(IdempotencyError::Conflict));
                    }
                    sqlx::query("INSERT INTO foundation_ids DEFAULT VALUES")
                        .execute(&mut **tx)
                        .await
                        .map_err(|_| AttemptError::Abort(IdempotencyError::StorageFailed))?;
                    let event = record_event(&mut **tx, probe_event(resource(0x01)))
                        .await
                        .map_err(|_| AttemptError::Abort(IdempotencyError::StorageFailed))?;
                    match claim(
                        &mut *tx,
                        NewClaim {
                            actor_id,
                            operation,
                            key: key_owned,
                            input_digest: digest_owned,
                            result_event: event.id,
                        },
                    )
                    .await
                    {
                        Ok(ClaimOutcome::Created(record) | ClaimOutcome::Replayed(record)) => {
                            Ok(record)
                        }
                        Err(IdempotencyError::Conflict) => {
                            Err(AttemptError::Abort(IdempotencyError::Conflict))
                        }
                        Err(other) => Err(AttemptError::Abort(other)),
                    }
                })
            })
            .await;
        match outcome {
            Ok(record) => return record,
            // Lost a concurrent race: the attempt rolled back, retry fresh.
            Err(TransactionError::Aborted(IdempotencyError::StorageFailed))
            | Err(TransactionError::StorageFailed) => continue,
            Err(TransactionError::Aborted(other)) => {
                panic!("idempotent action refused unexpectedly: {other:?}")
            }
            Err(TransactionError::RetryExhausted { .. }) => continue,
        }
    }
    panic!("idempotent action did not converge in bounds")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_duplicate_actions_create_one_fact() {
    let db = TestDatabase::create("p03t07_race")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p03t07_race_"),
        "known suite identity in the database name"
    );
    let pool = db.pool().clone();
    let barrier = Arc::new(tokio::sync::Barrier::new(4));
    let runner = |barrier: Arc<tokio::sync::Barrier>, pool: sqlx::PgPool| async move {
        barrier.wait().await;
        perform(
            &pool,
            actor(0x10),
            "record_outcome",
            "outcome-key-1",
            "digest-v1",
        )
        .await
    };
    let (first, second, third, fourth) = tokio::join!(
        runner(Arc::clone(&barrier), pool.clone()),
        runner(Arc::clone(&barrier), pool.clone()),
        runner(Arc::clone(&barrier), pool.clone()),
        runner(Arc::clone(&barrier), pool),
    );
    assert_eq!(first.result_event, second.result_event);
    assert_eq!(first.result_event, third.result_event);
    assert_eq!(first.result_event, fourth.result_event);
    assert_eq!(first.key, "outcome-key-1");
    assert_eq!(table_count(db.pool(), "foundation_ids").await, 1);
    assert_eq!(table_count(db.pool(), "business_events").await, 1);
    assert_eq!(table_count(db.pool(), "idempotency_records").await, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn same_key_with_different_input_returns_defined_conflict() {
    let db = TestDatabase::create("p03t07_conflict")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p03t07_conflict_"),
        "known suite identity in the database name"
    );
    let first = perform(
        db.pool(),
        actor(0x20),
        "record_outcome",
        "outcome-key-2",
        "digest-v1",
    )
    .await;
    // Same key with the same body replays the existing business result.
    let replayed = perform(
        db.pool(),
        actor(0x20),
        "record_outcome",
        "outcome-key-2",
        "digest-v1",
    )
    .await;
    assert_eq!(replayed.id, first.id);
    assert_eq!(replayed.result_event, first.result_event);
    assert_eq!(table_count(db.pool(), "business_events").await, 1);
    assert_eq!(table_count(db.pool(), "idempotency_records").await, 1);

    // Same key with a changed body is refused; nothing new is stored.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    sqlx::query("INSERT INTO foundation_ids DEFAULT VALUES")
        .execute(&mut *tx)
        .await
        .expect("mutation writes");
    let event = record_event(&mut *tx, probe_event(resource(0x02)))
        .await
        .expect("event records");
    let conflict = claim(
        &mut tx,
        NewClaim {
            actor_id: actor(0x20),
            operation: "record_outcome",
            key: "outcome-key-2".to_owned(),
            input_digest: "digest-CHANGED".to_owned(),
            result_event: event.id,
        },
    )
    .await
    .expect_err("changed body under one key must conflict");
    assert_eq!(conflict, IdempotencyError::Conflict);
    assert_eq!(conflict.code(), "idempotency_key_reuse");
    assert!(format!("{conflict}").contains("idempotency_key_reuse"));
    tx.rollback()
        .await
        .expect("rollback clears the refused attempt");
    assert_eq!(table_count(db.pool(), "business_events").await, 1);
    assert_eq!(table_count(db.pool(), "idempotency_records").await, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn failure_rollback_never_reserves_an_unrecoverable_key() {
    let db = TestDatabase::create("p03t07_rollback")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p03t07_rollback_"),
        "known suite identity in the database name"
    );
    // A claim committed inside a transaction that then fails rolls back fully.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    sqlx::query("INSERT INTO foundation_ids DEFAULT VALUES")
        .execute(&mut *tx)
        .await
        .expect("mutation writes");
    let event = record_event(&mut *tx, probe_event(resource(0x03)))
        .await
        .expect("event records");
    claim(
        &mut tx,
        NewClaim {
            actor_id: actor(0x30),
            operation: "record_outcome",
            key: "outcome-key-3".to_owned(),
            input_digest: "digest-v1".to_owned(),
            result_event: event.id,
        },
    )
    .await
    .expect("claim records");
    sqlx::query("INSERT INTO table_that_does_not_exist DEFAULT VALUES")
        .execute(&mut *tx)
        .await
        .expect_err("injected failure surfaces");
    tx.rollback().await.expect("rollback after failure");
    assert_eq!(table_count(db.pool(), "foundation_ids").await, 0);
    assert_eq!(table_count(db.pool(), "business_events").await, 0);
    assert_eq!(table_count(db.pool(), "idempotency_records").await, 0);
    assert!(
        find(db.pool(), actor(0x30), "record_outcome", "outcome-key-3")
            .await
            .expect("lookup queries")
            .is_none(),
        "rolled-back key remains usable"
    );

    // The genuine retry after rollback succeeds exactly once.
    let completed = perform(
        db.pool(),
        actor(0x30),
        "record_outcome",
        "outcome-key-3",
        "digest-v1",
    )
    .await;
    assert_eq!(completed.key, "outcome-key-3");
    assert_eq!(table_count(db.pool(), "foundation_ids").await, 1);
    assert_eq!(table_count(db.pool(), "business_events").await, 1);
    assert_eq!(table_count(db.pool(), "idempotency_records").await, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn storage_holds_no_private_material() {
    let db = TestDatabase::create("p03t07_privacy")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p03t07_privacy_"),
        "known suite identity in the database name"
    );
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns
          WHERE table_name = 'idempotency_records' ORDER BY column_name",
    )
    .fetch_all(db.pool())
    .await
    .expect("schema introspection");
    assert_eq!(
        columns,
        [
            "actor_id",
            "created_at",
            "id",
            "input_digest",
            "key",
            "operation",
            "result_event"
        ],
        "record columns are exactly the allowlist"
    );
    // The stored reference is an event identifier, never a destination.
    let completed = perform(
        db.pool(),
        actor(0x40),
        "start_contact",
        "contact-key-4",
        "digest-v1",
    )
    .await;
    let stored_event: uuid::Uuid =
        sqlx::query_scalar("SELECT result_event FROM idempotency_records")
            .fetch_one(db.pool())
            .await
            .expect("reference reads");
    assert_eq!(stored_event, completed.result_event);
    // Error values render nothing sensitive by construction (static reasons).
    let rendered = format!(
        "{:?} {} {}",
        IdempotencyError::StorageFailed,
        IdempotencyError::Conflict,
        IdempotencyError::InvalidKey
    );
    assert!(!rendered.contains("phone"));
    assert!(!rendered.contains("sess"));
    assert!(!rendered.contains("whatsapp"));
    db.cleanup().await.expect("suite cleans up");
}
