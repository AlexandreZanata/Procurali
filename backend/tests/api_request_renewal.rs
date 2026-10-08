//! Renewal acceptance (P06-T05): explicit confirmed renewal only.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). The renewal operation currently has no HTTP route (the
//! route-owning card lands it later), so these tests drive the application
//! operation directly over real PostgreSQL — the same direct-proof pattern
//! as the publication card. Proves:
//! - early renewal and unconfirmed interactions produce no new cycle;
//! - valid renewal increments cycle and activation count exactly once,
//!   retaining identity and first-publication age;
//! - terminal, suspended, and hidden rows cannot reactivate, and renewal
//!   records no new publication fact.
//!
//! Setup publishes travel the real draft and publication operations. All
//! phones, names, and codes below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::renew_request::{
    renew_request, RenewError, RenewalConfirmation,
};
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::requests::{
    cycles_for_request, request as read_request, requests_for_author,
};
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
            lookup_key: "p06t05-test-only-lookup-key",
            encryption_key: "p06t05-test-only-encryption-key",
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

fn confirmation() -> RenewalConfirmation {
    RenewalConfirmation {
        title: "Refrigerator".to_owned(),
        category_code: "home_appliances".to_owned(),
        budget: "520.00".to_owned(),
        condition: "either".to_owned(),
        city_code: "campinas".to_owned(),
        region_code: "centro".to_owned(),
        notes: "Preferably frost-free.".to_owned(),
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

/// Move one cycle into the past, honoring the cycle CHECK (start and
/// deadline travel together; only the deadline position matters).
async fn backdate_cycle(pool: &sqlx::PgPool, request_id: uuid::Uuid) {
    let now = chrono::Utc::now();
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(request_id)
    .bind(now - chrono::Duration::days(8))
    .bind(now - chrono::Duration::days(1))
    .execute(pool)
    .await
    .expect("synthetic expiry applies");
}

async fn event_count(pool: &sqlx::PgPool, author: uuid::Uuid, kind: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE actor_id = $1 AND resource_kind = 'request' AND kind = $2",
    )
    .bind(author)
    .bind(kind)
    .fetch_one(pool)
    .await
    .expect("events read")
}

#[tokio::test]
async fn early_renewal_and_unconfirmed_paths_produce_no_cycle() {
    let db = TestDatabase::create("p06t05_early")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t05_early_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Early Owner", "+55 11 90000-0081").await;
    let id = publish_open(db.pool(), author).await;

    // A fresh seven-day cycle is not renewable: early refusal, one cycle,
    // no renewal fact, no activation consumed.
    assert_eq!(
        renew_request(db.pool(), author, id, confirmation()).await,
        Err(RenewError::EarlyRenewal)
    );
    assert_eq!(
        cycles_for_request(db.pool(), id)
            .await
            .expect("cycles read")
            .len(),
        1
    );
    assert_eq!(event_count(db.pool(), author, "request.renewed").await, 0);

    // Neither a material revision nor a stale confirmation renews: the
    // expired row gains history but no cycle, and the refusal names terms.
    backdate_cycle(db.pool(), id).await;
    let stale = RenewalConfirmation {
        budget: "800.00".to_owned(),
        ..confirmation()
    };
    assert_eq!(
        renew_request(db.pool(), author, id, stale).await,
        Err(RenewError::StaleRequirements)
    );
    assert_eq!(
        cycles_for_request(db.pool(), id)
            .await
            .expect("cycles read")
            .len(),
        1
    );
    assert_eq!(event_count(db.pool(), author, "request.renewed").await, 0);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn valid_renewal_increments_cycle_and_activation_once() {
    let db = TestDatabase::create("p06t05_valid")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t05_valid_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Renew Owner", "+55 11 90000-0082").await;
    let id = publish_open(db.pool(), author).await;
    let published_at = read_request(db.pool(), id)
        .await
        .expect("request reads")
        .expect("request reads")
        .original_published_at;
    backdate_cycle(db.pool(), id).await;

    let renewed = renew_request(db.pool(), author, id, confirmation())
        .await
        .expect("eligible renewal renews");
    assert_eq!(renewed.id, id);
    assert_eq!(renewed.cycle_number, 2);
    assert_eq!(renewed.revision_number, 1, "renewal revises nothing");
    assert_eq!(renewed.state, "active");
    assert_eq!(renewed.original_published_at, published_at);
    assert_eq!(
        renewed.deadline.signed_duration_since(renewed.started_at),
        chrono::Duration::days(7)
    );
    let cycles = cycles_for_request(db.pool(), id)
        .await
        .expect("cycles read");
    assert_eq!(cycles.len(), 2);
    assert!(
        cycles[0].ended_at.is_some(),
        "the prior cycle ends instead of lingering"
    );
    assert_eq!(event_count(db.pool(), author, "request.renewed").await, 1);
    assert_eq!(event_count(db.pool(), author, "request.published").await, 1);

    // An immediate repeat is early again: still two cycles, still one fact.
    assert_eq!(
        renew_request(db.pool(), author, id, confirmation()).await,
        Err(RenewError::EarlyRenewal)
    );
    assert_eq!(
        cycles_for_request(db.pool(), id)
            .await
            .expect("cycles read")
            .len(),
        2
    );
    assert_eq!(event_count(db.pool(), author, "request.renewed").await, 1);

    // An active row inside its final day renews without waiting for expiry.
    let second = publish_open(db.pool(), author).await;
    let now = chrono::Utc::now();
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(second)
    .bind(now - chrono::Duration::days(6) - chrono::Duration::hours(12))
    .bind(now + chrono::Duration::hours(12))
    .execute(db.pool())
    .await
    .expect("synthetic final day applies");
    let renewed = renew_request(db.pool(), author, second, confirmation())
        .await
        .expect("final-day renewal renews");
    assert_eq!(renewed.cycle_number, 2);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn terminal_suspended_and_hidden_rows_never_reactivate() {
    let db = TestDatabase::create("p06t05_terminal")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t05_terminal_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Terminal Owner", "+55 11 90000-0083").await;

    for (title, state, visibility) in [
        ("Terminal One", "completed", "public"),
        ("Terminal Two", "cancelled", "hidden"),
        ("Terminal Three", "suspended", "public"),
        ("Terminal Four", "expired", "hidden"),
    ] {
        let draft = create_draft(
            db.pool(),
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
        let published = publish_request(db.pool(), author, draft.id)
            .await
            .expect("fixture draft publishes")
            .id;
        sqlx::query("UPDATE requests SET state = $2, visibility = $3 WHERE id = $1")
            .bind(published)
            .bind(state)
            .bind(visibility)
            .execute(db.pool())
            .await
            .expect("synthetic terminal state applies");
        assert_eq!(
            renew_request(db.pool(), author, published, confirmation()).await,
            Err(RenewError::ForbiddenState),
            "{state}/{visibility} never reactivates"
        );
        assert_eq!(event_count(db.pool(), author, "request.renewed").await, 0);
    }

    // Strangers renew nothing, and renewal is never a publication metric:
    // one row, one publication fact, retained first-publication age.
    let other = seed_author(db.pool(), "Renew Stranger", "+55 11 90000-0084").await;
    let id = publish_open(db.pool(), author).await;
    backdate_cycle(db.pool(), id).await;
    assert_eq!(
        renew_request(db.pool(), other, id, confirmation()).await,
        Err(RenewError::NotFound)
    );
    let before = read_request(db.pool(), id)
        .await
        .expect("request reads")
        .expect("request reads")
        .original_published_at;
    renew_request(db.pool(), author, id, confirmation())
        .await
        .expect("eligible renewal renews");
    let after = read_request(db.pool(), id)
        .await
        .expect("request reads")
        .expect("request reads");
    assert_eq!(after.original_published_at, before);
    assert_eq!(
        requests_for_author(db.pool(), author)
            .await
            .expect("owner reads")
            .len(),
        5,
        "renewal creates cycles, not requests"
    );
    assert_eq!(event_count(db.pool(), author, "request.published").await, 5);
    assert_eq!(event_count(db.pool(), author, "request.renewed").await, 1);
    db.cleanup().await.expect("suite cleans up");
}
