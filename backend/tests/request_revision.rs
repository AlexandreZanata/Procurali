//! Request-revision acceptance (P06-T01): material classification.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL:
//! - a spelling-only edit preserves compatibility with no new revision,
//!   cycle, deadline, or fact;
//! - budget moves up and down are both material with byte-stable history,
//!   bounded by three successful material revisions per rolling day;
//! - terminal rows refuse edits while expired and suspended rows revise
//!   without republishing or lifting restrictions.
//!
//! Setup publishes travel the real draft and publication operations. Offer
//! rows do not exist yet: the no-resurrection proof is structural — prior
//! revisions are never rewritten, cycles never restart, and deadlines never
//! move. All phones, names, and codes below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::revise_request::{revise_request, ReviseError, ReviseInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::requests::{
    cycles_for_request, request as read_request, revision, revisions_for_request,
};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p06t01-test-only-lookup-key",
        encryption_key: "p06t01-test-only-encryption-key",
    }
}

async fn seed_catalog(pool: &sqlx::PgPool) {
    let mut tx = pool.begin().await.expect("transaction begins");
    upsert_city(&mut tx, "campinas", "Campinas", true)
        .await
        .expect("fixture city stores");
    upsert_region(&mut tx, "campinas", "centro", "Centro")
        .await
        .expect("fixture region stores");
    tx.commit().await.expect("fixtures commit");
}

/// A real account, activated through a synthetic state fixture: identity
/// itself travels the real creation path with real phone cryptography, and
/// only the lifecycle flip (owned by the verification flow) is staged.
async fn seed_author(pool: &sqlx::PgPool, display: &str, phone: &str) -> uuid::Uuid {
    let mut tx = pool.begin().await.expect("transaction begins");
    let user = create_user(
        &mut tx,
        NewUser {
            display_name: display.to_owned(),
            city: "Campinas".to_owned(),
            region: "SP".to_owned(),
            policy_version: "v1".to_owned(),
            policy_accepted_at: chrono::Utc::now(),
            phone: phone.to_owned(),
        },
        test_keys(),
    )
    .await
    .expect("fixture author stores");
    tx.commit().await.expect("author commits");
    sqlx::query("UPDATE users SET state = 'active' WHERE id = $1")
        .bind(user.id)
        .execute(pool)
        .await
        .expect("synthetic activation applies");
    user.id
}

fn revise_input(title: &str, budget: &str, notes: &str) -> ReviseInput {
    ReviseInput {
        title: title.to_owned(),
        category_code: "home_appliances".to_owned(),
        budget: budget.to_owned(),
        condition: "either".to_owned(),
        city_code: "campinas".to_owned(),
        region_code: "centro".to_owned(),
        notes: notes.to_owned(),
    }
}

/// One genuinely open request through the real draft and publication paths.
async fn publish_open(pool: &sqlx::PgPool, author: uuid::Uuid) -> uuid::Uuid {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some("Refrigerator".to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some("520.00".to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: Some("Preferably frost-free.".to_owned()),
        },
    )
    .await
    .expect("fixture draft validates");
    publish_request(pool, author, draft.id)
        .await
        .expect("fixture draft publishes")
        .id
}

async fn event_count(pool: &sqlx::PgPool, request_id: uuid::Uuid, kind: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'request' AND resource_id = $1 AND kind = $2",
    )
    .bind(request_id)
    .bind(kind)
    .fetch_one(pool)
    .await
    .expect("events read")
}

#[tokio::test]
async fn spelling_only_edit_preserves_compatibility() {
    let db = TestDatabase::create("p06t01_spelling")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t01_spelling_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Spelling Owner", "+55 11 90000-0051").await;
    let id = publish_open(db.pool(), author).await;
    let before = read_request(db.pool(), id)
        .await
        .expect("request reads")
        .expect("request reads");

    // Spacing and casing alone change nothing structural: same revision,
    // same cycle, same deadline, no new fact — the wording stays compatible.
    let revised = revise_request(
        db.pool(),
        author,
        id,
        revise_input("  REFRIGERATOR ", "520.00", "Preferably   FROST-free."),
    )
    .await
    .expect("spelling correction revises");
    assert!(!revised.material);
    assert_eq!(revised.revision_number, 1);
    assert_eq!(revised.title, "  REFRIGERATOR ");
    assert_eq!(
        revisions_for_request(db.pool(), id)
            .await
            .expect("revisions read")
            .len(),
        1
    );
    assert_eq!(
        cycles_for_request(db.pool(), id)
            .await
            .expect("cycles read")
            .len(),
        1
    );
    let after = read_request(db.pool(), id)
        .await
        .expect("request reads")
        .expect("request reads");
    assert_eq!(after.current_revision_number, 1);
    assert_eq!(after.current_cycle_number, before.current_cycle_number);
    assert_eq!(after.original_published_at, before.original_published_at);
    assert_eq!(event_count(db.pool(), id, "request.published").await, 1);
    assert_eq!(event_count(db.pool(), id, "request.revised").await, 0);
    // The preserved revision still describes the current need.
    let first = revision(db.pool(), id, 1)
        .await
        .expect("revision reads")
        .expect("first revision reads");
    assert_eq!(first.title, "Refrigerator");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn budget_up_and_down_are_material_within_three_a_day() {
    let db = TestDatabase::create("p06t01_budget")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t01_budget_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Budget Owner", "+55 11 90000-0052").await;
    let id = publish_open(db.pool(), author).await;
    let published_deadline = cycles_for_request(db.pool(), id)
        .await
        .expect("cycles read")[0]
        .deadline;

    // Upwards is material...
    let revised = revise_request(
        db.pool(),
        author,
        id,
        revise_input("Refrigerator", "800.00", "Preferably frost-free."),
    )
    .await
    .expect("budget increase revises");
    assert!(revised.material);
    assert_eq!(revised.revision_number, 2);
    assert_eq!(revised.budget, "800.00");
    // ...and downwards is material too, back at the original ceiling.
    let revised = revise_request(
        db.pool(),
        author,
        id,
        revise_input("Refrigerator", "520.00", "Preferably frost-free."),
    )
    .await
    .expect("budget decrease revises");
    assert!(revised.material);
    assert_eq!(revised.revision_number, 3);
    assert_eq!(revised.budget, "520.00");

    // History is byte-stable: each past price stands exactly as recorded,
    // so no later resubmission can silently resurrect or rewrite one.
    let history = revisions_for_request(db.pool(), id)
        .await
        .expect("revisions read");
    assert_eq!(history.len(), 3);
    assert_eq!(
        history
            .iter()
            .map(|entry| entry.budget_cents)
            .collect::<Vec<_>>(),
        [52_000, 80_000, 52_000]
    );
    // No cycle restarted, no deadline moved, no republication happened.
    assert_eq!(
        cycles_for_request(db.pool(), id)
            .await
            .expect("cycles read")
            .len(),
        1
    );
    assert_eq!(
        cycles_for_request(db.pool(), id)
            .await
            .expect("cycles read")[0]
            .deadline,
        published_deadline
    );
    let stored = read_request(db.pool(), id)
        .await
        .expect("request reads")
        .expect("request reads");
    assert_eq!(stored.state, "active");
    assert_eq!(stored.current_cycle_number, 1);
    assert_eq!(event_count(db.pool(), id, "request.published").await, 1);
    assert_eq!(event_count(db.pool(), id, "request.revised").await, 2);

    // The third successful material revision is still allowed...
    let revised = revise_request(
        db.pool(),
        author,
        id,
        revise_input(
            "Double-door refrigerator",
            "520.00",
            "Preferably frost-free.",
        ),
    )
    .await
    .expect("third material revision revises");
    assert!(revised.material);
    assert_eq!(revised.revision_number, 4);
    // ...while the fourth inside the rolling day is refused, recording
    // nothing itself.
    assert_eq!(
        revise_request(
            db.pool(),
            author,
            id,
            revise_input("Double-door refrigerator", "520.00", "Needs ice maker."),
        )
        .await,
        Err(ReviseError::RevisionQuotaExhausted)
    );
    assert_eq!(
        revisions_for_request(db.pool(), id)
            .await
            .expect("revisions read")
            .len(),
        4
    );
    assert_eq!(event_count(db.pool(), id, "request.revised").await, 3);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn terminal_rows_refuse_while_expired_and_suspended_revise_privately() {
    let db = TestDatabase::create("p06t01_terminal")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t01_terminal_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Terminal Owner", "+55 11 90000-0053").await;

    // Completed and cancelled rows are read-only, even for spelling.
    let completed = publish_open(db.pool(), author).await;
    sqlx::query("UPDATE requests SET state = 'completed' WHERE id = $1")
        .bind(completed)
        .execute(db.pool())
        .await
        .expect("synthetic completion applies");
    assert_eq!(
        revise_request(
            db.pool(),
            author,
            completed,
            revise_input("  Refrigerator ", "520.00", "Preferably frost-free."),
        )
        .await,
        Err(ReviseError::ForbiddenState)
    );
    let other = seed_author(db.pool(), "Other Owner", "+55 11 90000-0054").await;
    assert_eq!(
        revise_request(
            db.pool(),
            other,
            completed,
            revise_input("Refrigerator", "520.00", "Preferably frost-free."),
        )
        .await,
        Err(ReviseError::NotFound)
    );

    // An expired row revises without republishing: same cycle, same past
    // deadline, same publication history, one new revision and fact.
    let expired = publish_open(db.pool(), author).await;
    let now = chrono::Utc::now();
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(expired)
    .bind(now - chrono::Duration::days(8))
    .bind(now - chrono::Duration::days(1))
    .execute(db.pool())
    .await
    .expect("synthetic expiry applies");
    let revised = revise_request(
        db.pool(),
        author,
        expired,
        revise_input("Refrigerator", "800.00", "Preferably frost-free."),
    )
    .await
    .expect("expired row revises");
    assert!(revised.material);
    assert_eq!(revised.revision_number, 2);
    let stored = read_request(db.pool(), expired)
        .await
        .expect("request reads")
        .expect("request reads");
    assert_eq!(stored.current_cycle_number, 1, "no new cycle starts");
    assert_eq!(
        event_count(db.pool(), expired, "request.published").await,
        1
    );
    assert_eq!(event_count(db.pool(), expired, "request.revised").await, 1);
    let cycle = &cycles_for_request(db.pool(), expired)
        .await
        .expect("cycles read")[0];
    assert!(cycle.deadline < now, "the deadline stays elapsed");

    // A suspended correction stores its private revision and keeps the
    // restriction: state, visibility, cycle, and deadline are untouched.
    let suspended = publish_open(db.pool(), author).await;
    sqlx::query("UPDATE requests SET state = 'suspended' WHERE id = $1")
        .bind(suspended)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    let before = read_request(db.pool(), suspended)
        .await
        .expect("request reads")
        .expect("request reads");
    let revised = revise_request(
        db.pool(),
        author,
        suspended,
        revise_input("Refrigerator", "520.00", "Still needed; violation removed."),
    )
    .await
    .expect("suspended row revises");
    assert!(revised.material);
    let after = read_request(db.pool(), suspended)
        .await
        .expect("request reads")
        .expect("request reads");
    assert_eq!(after.state, "suspended", "the restriction stays");
    assert_eq!(after.visibility, before.visibility, "nothing republicizes");
    assert_eq!(after.current_cycle_number, 1);
    assert_eq!(
        event_count(db.pool(), suspended, "request.revised").await,
        1
    );
    db.cleanup().await.expect("suite cleans up");
}
