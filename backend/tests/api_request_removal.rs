//! Removal acceptance (P06-T07): concealment with immutable history.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). The removal operation currently has no HTTP route (the
//! route-owning card lands it later), so these tests drive the application
//! operation directly over real PostgreSQL — the same direct-proof pattern
//! as the publication, renewal, and closure cards. Proves:
//! - active removal stops new activity while quota history stands;
//! - completed and cancelled removals hide content with outcomes intact;
//! - repeats converge without new facts, and strangers remove nothing.
//!
//! Setup publishes travel the real draft and publication operations. All
//! phones, names, and codes below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::close_request::{close_request, BuyerOutcome, OutcomeSource};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::remove_request::{remove_request, RemoveError};
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::request_eligibility::{
    check_request_actionable, RequestEligibilityError,
};
use procurali_backend::application::request_limits::{activation_count, open_request_count};
use procurali_backend::application::revise_request::{revise_request, ReviseError, ReviseInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::requests::{cycles_for_request, request as read_request};
use procurali_backend::persistence::transaction::AttemptError;
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

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
        PhoneKeys {
            lookup_key: "p06t07-test-only-lookup-key",
            encryption_key: "p06t07-test-only-encryption-key",
        },
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

/// One genuinely open request through the real draft and publication paths.
async fn publish_open(pool: &sqlx::PgPool, author: uuid::Uuid, title: &str) -> uuid::Uuid {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some(title.to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some("520.00".to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
        },
    )
    .await
    .expect("fixture draft validates");
    publish_request(pool, author, draft.id)
        .await
        .expect("fixture draft publishes")
        .id
}

/// Live counters inside one probe transaction.
async fn counts(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> (u32, u32) {
    fn readable(
        result: Result<
            u32,
            AttemptError<procurali_backend::application::request_limits::LimitError>,
        >,
    ) -> u32 {
        match result {
            Ok(count) => count,
            Err(AttemptError::Abort(_)) => panic!("probe refusal impossible"),
            Err(AttemptError::Db(_)) => panic!("probe storage failed"),
        }
    }
    let mut tx = pool.begin().await.expect("transaction begins");
    let open = readable(open_request_count(&mut tx, author, now).await);
    let activations = readable(activation_count(&mut tx, author, now).await);
    tx.rollback().await.expect("probe rolls back");
    (open, activations)
}

async fn request_kinds(pool: &sqlx::PgPool, request_id: uuid::Uuid) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT kind, payload::text FROM business_events
         WHERE resource_kind = 'request' AND resource_id = $1
         ORDER BY occurred_at, id",
    )
    .bind(request_id)
    .fetch_all(pool)
    .await
    .expect("events read")
}

#[tokio::test]
async fn active_removal_stops_activity_and_keeps_quota_history() {
    let db = TestDatabase::create("p06t07_active")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t07_active_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Active Owner", "+55 11 90000-0101").await;
    let removed = publish_open(db.pool(), author, "Gone Fridge").await;
    publish_open(db.pool(), author, "Kept Fridge").await;
    let now = chrono::Utc::now();
    assert_eq!(counts(db.pool(), author, now).await, (2, 2));

    let outcome = remove_request(db.pool(), author, removed)
        .await
        .expect("owner removes");
    assert!(outcome.transitioned);
    assert_eq!(outcome.state, "cancelled");
    assert_eq!(outcome.visibility, "hidden");
    let facts = request_kinds(db.pool(), removed).await;
    let removal = facts
        .iter()
        .find(|(kind, _)| kind == "request.removed")
        .expect("removal fact records");
    assert!(removal.1.contains("active"), "prior state stays recorded");
    assert!(removal.1.contains("owner_removal"));

    // The open slot frees while the activation history stands exactly.
    assert_eq!(counts(db.pool(), author, now).await, (1, 2));
    // New activity stops: eligibility, revision, renewal scope, and closure
    // all refuse the concealed row through their existing guards.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert!(matches!(
        check_request_actionable(&mut tx, removed, now).await,
        Err(AttemptError::Abort(RequestEligibilityError::NotActionable))
    ));
    tx.rollback().await.expect("probe rolls back");
    assert_eq!(
        revise_request(
            db.pool(),
            author,
            removed,
            ReviseInput {
                title: "Gone Fridge".to_owned(),
                category_code: "home_appliances".to_owned(),
                budget: "520.00".to_owned(),
                condition: "either".to_owned(),
                city_code: "campinas".to_owned(),
                region_code: "centro".to_owned(),
                notes: "Still there.".to_owned(),
            },
        )
        .await,
        Err(ReviseError::ForbiddenState)
    );
    let cycles = cycles_for_request(db.pool(), removed)
        .await
        .expect("cycles read");
    assert_eq!(cycles.len(), 1);
    assert!(cycles[0].ended_at.is_some(), "removal ends the live cycle");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn terminal_removal_hides_without_converting_outcomes() {
    let db = TestDatabase::create("p06t07_terminal")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t07_terminal_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Terminal Owner", "+55 11 90000-0102").await;

    // A completed row hides with its completion intact: no abandonment
    // appears anywhere, and the completion fact stands.
    let completed = publish_open(db.pool(), author, "Found Fridge").await;
    close_request(
        db.pool(),
        author,
        completed,
        BuyerOutcome::Found(OutcomeSource::Elsewhere),
    )
    .await
    .expect("completion closes");
    let outcome = remove_request(db.pool(), author, completed)
        .await
        .expect("owner removes completed");
    assert!(outcome.transitioned);
    assert_eq!(outcome.state, "completed", "completion is not converted");
    assert_eq!(outcome.visibility, "hidden");
    let facts = request_kinds(db.pool(), completed).await;
    assert!(facts.iter().any(|(kind, _)| kind == "request.completed"));
    assert!(!facts.iter().any(|(kind, _)| kind == "request.cancelled"));
    let removal = facts
        .iter()
        .find(|(kind, _)| kind == "request.removed")
        .expect("removal fact records");
    assert!(
        removal.1.contains("completed"),
        "prior outcome stays recorded"
    );

    // A cancelled row hides the same way, keeping its own reason.
    let cancelled = publish_open(db.pool(), author, "Dropped Fridge").await;
    close_request(db.pool(), author, cancelled, BuyerOutcome::NotNeeded)
        .await
        .expect("cancellation closes");
    let outcome = remove_request(db.pool(), author, cancelled)
        .await
        .expect("owner removes cancelled");
    assert!(outcome.transitioned);
    assert_eq!(outcome.state, "cancelled");
    assert_eq!(outcome.visibility, "hidden");
    let facts = request_kinds(db.pool(), cancelled).await;
    assert!(facts.iter().any(|(kind, _)| kind == "request.cancelled"));
    assert!(!facts.iter().any(|(kind, _)| kind == "request.completed"));
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn repeated_remove_converges_and_strangers_remove_nothing() {
    let db = TestDatabase::create("p06t07_repeat")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t07_repeat_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Repeat Owner", "+55 11 90000-0103").await;
    let stranger = seed_author(db.pool(), "Remove Stranger", "+55 11 90000-0104").await;
    let id = publish_open(db.pool(), author, "Twice Fridge").await;

    // Repeats return the same hidden row with no new fact: no restore, no
    // reopen, no duplication.
    let first = remove_request(db.pool(), author, id)
        .await
        .expect("first removal removes");
    assert!(first.transitioned);
    let second = remove_request(db.pool(), author, id)
        .await
        .expect("repeat removal converges");
    assert!(!second.transitioned);
    assert_eq!(
        (second.state, second.visibility),
        (first.state, first.visibility)
    );
    assert_eq!(
        request_kinds(db.pool(), id)
            .await
            .iter()
            .filter(|(kind, _)| kind == "request.removed")
            .count(),
        1
    );

    // Strangers remove nothing, and the owner's row stands exactly as the
    // first removal left it.
    assert_eq!(
        remove_request(db.pool(), stranger, id).await,
        Err(RemoveError::NotFound)
    );
    assert_eq!(
        remove_request(db.pool(), stranger, uuid::Uuid::now_v7()).await,
        Err(RemoveError::NotFound)
    );
    let stored = read_request(db.pool(), id)
        .await
        .expect("request reads")
        .expect("request reads");
    assert_eq!(stored.state, "cancelled");
    assert_eq!(stored.visibility, "hidden");
    db.cleanup().await.expect("suite cleans up");
}
