//! Event/notice atomicity acceptance: one mutation with its facts, full
//! rollback on injected failure, private-column absence, scoped acknowledgment.
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly otherwise).

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::persistence::{
    events::{record as record_event, EventError, NewEvent},
    notices::{acknowledge, record as record_notice, NewNotice, NoticeError},
};
use serde_json::json;

fn test_actor() -> uuid::Uuid {
    "123e4567-e89b-42d3-a456-426614174001"
        .parse()
        .expect("fixed test UUID parses")
}

fn test_account() -> uuid::Uuid {
    "123e4567-e89b-42d3-a456-426614174002"
        .parse()
        .expect("fixed test UUID parses")
}

fn test_resource() -> uuid::Uuid {
    "123e4567-e89b-42d3-a456-426614174003"
        .parse()
        .expect("fixed test UUID parses")
}

fn probe_event(resource: uuid::Uuid) -> NewEvent {
    NewEvent {
        actor_id: Some(test_actor()),
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

fn probe_notice(account: uuid::Uuid, resource: uuid::Uuid, event: uuid::Uuid) -> NewNotice {
    NewNotice {
        account_id: account,
        kind: "request.published",
        resource_kind: "request",
        resource_id: resource,
        event_id: event,
        body: "A request entered its first cycle.".to_owned(),
    }
}

async fn table_count(pool: &sqlx::PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count reads")
}

async fn table_columns(db: &TestDatabase, table: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns
          WHERE table_name = $1 ORDER BY column_name",
    )
    .bind(table)
    .fetch_all(db.pool())
    .await
    .expect("schema introspection")
}

#[tokio::test]
async fn committed_mutation_has_exactly_one_fact_and_notice() {
    let db = TestDatabase::create("p03t05_atomic")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p03t05_atomic_"),
        "known suite identity in the database name"
    );
    let resource = test_resource();
    let mut tx = db.pool().begin().await.expect("transaction begins");
    sqlx::query("INSERT INTO foundation_ids DEFAULT VALUES")
        .execute(&mut *tx)
        .await
        .expect("mutation writes");
    let event = record_event(&mut *tx, probe_event(resource))
        .await
        .expect("event records");
    let notice = record_notice(&mut *tx, probe_notice(test_account(), resource, event.id))
        .await
        .expect("notice records");
    tx.commit().await.expect("atomic unit commits");

    assert_eq!(table_count(db.pool(), "foundation_ids").await, 1);
    assert_eq!(table_count(db.pool(), "business_events").await, 1);
    assert_eq!(table_count(db.pool(), "notices").await, 1);
    assert_eq!(notice.event_id, event.id);
    assert_eq!(event.resource_id, resource);
    assert_eq!(event.payload, json!({"cycle": 1}));
    assert_eq!(
        event.id.to_string().chars().nth(14),
        Some('7'),
        "server-generated event id is version 7"
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn injected_failure_rolls_back_mutation_and_event() {
    let db = TestDatabase::create("p03t05_rollback")
        .await
        .expect("disposable database allocates");
    let resource = test_resource();
    let mut tx = db.pool().begin().await.expect("transaction begins");
    sqlx::query("INSERT INTO foundation_ids DEFAULT VALUES")
        .execute(&mut *tx)
        .await
        .expect("mutation writes");
    record_event(&mut *tx, probe_event(resource))
        .await
        .expect("event records");
    record_notice(
        &mut *tx,
        probe_notice(test_account(), resource, uuid::Uuid::nil()),
    )
    .await
    .expect_err("notice for a missing event must fail");
    tx.rollback().await.expect("explicit rollback");
    // The notice failure above already poisoned nothing; rollback clears all.
    assert_eq!(table_count(db.pool(), "foundation_ids").await, 0);
    assert_eq!(table_count(db.pool(), "business_events").await, 0);
    assert_eq!(table_count(db.pool(), "notices").await, 0);

    // A later failure after successful writes also commits nothing.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    sqlx::query("INSERT INTO foundation_ids DEFAULT VALUES")
        .execute(&mut *tx)
        .await
        .expect("mutation writes");
    let failed: Result<_, sqlx::Error> =
        sqlx::query("INSERT INTO table_that_does_not_exist DEFAULT VALUES")
            .execute(&mut *tx)
            .await;
    assert!(failed.is_err(), "injected failure surfaces");
    tx.rollback().await.expect("rollback after failure");
    assert_eq!(table_count(db.pool(), "foundation_ids").await, 0);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn storage_holds_no_private_columns() {
    let db = TestDatabase::create("p03t05_privacy")
        .await
        .expect("disposable database allocates");
    assert_eq!(
        table_columns(&db, "business_events").await,
        [
            "actor_id",
            "cycle",
            "effective_at",
            "id",
            "kind",
            "occurred_at",
            "payload",
            "policy",
            "resource_id",
            "resource_kind",
            "revision",
            "source"
        ],
        "event columns are exactly the allowlist"
    );
    assert_eq!(
        table_columns(&db, "notices").await,
        [
            "account_id",
            "acknowledged_at",
            "body",
            "created_at",
            "event_id",
            "id",
            "kind",
            "resource_id",
            "resource_kind"
        ],
        "notice columns are exactly the allowlist"
    );
    // Error values render nothing sensitive by construction (static reasons).
    let rendered = format!(
        "{:?} {} {:?}",
        EventError::StorageFailed,
        NoticeError::Duplicate,
        NoticeError::StorageFailed
    );
    assert!(!rendered.contains("phone"));
    assert!(!rendered.contains("sess"));
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn duplicate_notice_for_same_event_is_refused() {
    let db = TestDatabase::create("p03t05_dedup")
        .await
        .expect("disposable database allocates");
    let resource = test_resource();
    let event = record_event(db.pool(), probe_event(resource))
        .await
        .expect("event records");
    record_notice(db.pool(), probe_notice(test_account(), resource, event.id))
        .await
        .expect("first notice records");
    let duplicate = record_notice(db.pool(), probe_notice(test_account(), resource, event.id))
        .await
        .expect_err("same event+recipient twice must fail");
    assert_eq!(duplicate, NoticeError::Duplicate, "typed refusal, no panic");
    // A different recipient's notice for the same event is legitimate.
    let other: uuid::Uuid = "123e4567-e89b-42d3-a456-426614174004"
        .parse()
        .expect("fixed UUID parses");
    record_notice(db.pool(), probe_notice(other, resource, event.id))
        .await
        .expect("other recipient records");
    assert_eq!(table_count(db.pool(), "notices").await, 2);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn acknowledge_is_owner_scoped() {
    let db = TestDatabase::create("p03t05_ack")
        .await
        .expect("disposable database allocates");
    let resource = test_resource();
    let event = record_event(db.pool(), probe_event(resource))
        .await
        .expect("event records");
    let notice = record_notice(db.pool(), probe_notice(test_account(), resource, event.id))
        .await
        .expect("notice records");
    let stranger: uuid::Uuid = "123e4567-e89b-42d3-a456-426614174005"
        .parse()
        .expect("fixed UUID parses");
    assert!(
        !acknowledge(db.pool(), notice.id, stranger)
            .await
            .expect("ack queries"),
        "strangers acknowledge nothing"
    );
    assert!(
        acknowledge(db.pool(), notice.id, test_account())
            .await
            .expect("ack queries"),
        "recipient acknowledges"
    );
    assert!(
        !acknowledge(db.pool(), notice.id, test_account())
            .await
            .expect("ack queries"),
        "double acknowledgment is not a second transition"
    );
    let at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT acknowledged_at FROM notices WHERE id = $1")
            .bind(notice.id)
            .fetch_one(db.pool())
            .await
            .expect("acknowledgment reads");
    assert!(at.is_some(), "acknowledgment instant recorded");
    db.cleanup().await.expect("suite cleans up");
}
