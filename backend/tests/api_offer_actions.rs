//! Offer-action acceptance (P07-T05): first view and buyer decline.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). These operations currently have no HTTP route (the
//! route-owning card lands them later), so the tests drive the application
//! operations directly over real PostgreSQL — the same direct-proof
//! pattern as the publication, renewal, and closure cards. Proves:
//! - repeat detail openings convert once with a single fact;
//! - strangers view and decline nothing;
//! - declined offers stay unavailable with their slot held and history
//!   intact, scoring nobody.
//!
//! Setup submissions travel the real submission operation. All phones,
//! names, and codes below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::offer_limits::{check_submission_limits, OfferLimitError};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::reject_offer::{reject_offer, RejectError};
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::application::view_offer::{view_offer, ViewError};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::offers;
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

/// A real active account: identity travels the real creation path with real
/// phone cryptography, and only the lifecycle flip (owned by the
/// verification flow) is staged.
async fn seed_active(pool: &sqlx::PgPool, display: &str, phone: &str) -> uuid::Uuid {
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
            lookup_key: "p07t05-test-only-lookup-key",
            encryption_key: "p07t05-test-only-encryption-key",
        },
    )
    .await
    .expect("fixture account stores");
    tx.commit().await.expect("account commits");
    sqlx::query("UPDATE users SET state = 'active' WHERE id = $1")
        .bind(user.id)
        .execute(pool)
        .await
        .expect("synthetic activation applies");
    user.id
}

fn offer_input() -> OfferInput {
    OfferInput {
        revision_number: Some(1),
        cycle_number: Some(1),
        description: Some("Frost-free 300L".to_owned()),
        price: Some("520.00".to_owned()),
        condition: Some("used".to_owned()),
        city_code: Some("campinas".to_owned()),
        region_code: Some("centro".to_owned()),
        notes: None,
        available: Some(true),
        available_in_city: Some(true),
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
            budget: Some("600.00".to_owned()),
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

/// One sent offer through the real submission operation.
async fn submit_open(
    pool: &sqlx::PgPool,
    seller: uuid::Uuid,
    request_id: uuid::Uuid,
) -> uuid::Uuid {
    submit_offer(pool, seller, request_id, offer_input())
        .await
        .expect("fixture submission submits")
        .id
}

async fn offer_facts(pool: &sqlx::PgPool, offer_id: uuid::Uuid, kind: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'offer' AND resource_id = $1 AND kind = $2",
    )
    .bind(offer_id)
    .bind(kind)
    .fetch_one(pool)
    .await
    .expect("events read")
}

#[tokio::test]
async fn repeated_details_create_one_first_view_conversion() {
    let db = TestDatabase::create("p07t05_view")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t05_view_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "View Owner", "+55 11 90000-0151").await;
    let seller = seed_active(db.pool(), "View Seller", "+55 11 90000-0152").await;
    let request_id = publish_open(db.pool(), author).await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;

    // First opening converts sent to viewed with exactly one fact; repeats
    // read the same viewed row with nothing new recorded.
    let viewed = view_offer(db.pool(), author, request_id, offer_id)
        .await
        .expect("first opening views");
    assert_eq!(viewed.state, "viewed");
    assert_eq!(viewed.seller_name, "View Seller");
    assert_eq!(offer_facts(db.pool(), offer_id, "offer.viewed").await, 1);
    for _ in 0..2 {
        let repeated = view_offer(db.pool(), author, request_id, offer_id)
            .await
            .expect("repeat opening reads");
        assert_eq!(repeated.state, "viewed");
    }
    assert_eq!(offer_facts(db.pool(), offer_id, "offer.viewed").await, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn strangers_view_and_decline_nothing() {
    let db = TestDatabase::create("p07t05_stranger")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t05_stranger_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Stranger Owner", "+55 11 90000-0153").await;
    let seller = seed_active(db.pool(), "Stranger Seller", "+55 11 90000-0154").await;
    let stranger = seed_active(db.pool(), "View Stranger", "+55 11 90000-0155").await;
    let request_id = publish_open(db.pool(), author).await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;

    // Strangers share the missing-row refusal on every action, and the offer
    // stands untouched with no fact recorded.
    assert_eq!(
        view_offer(db.pool(), stranger, request_id, offer_id).await,
        Err(ViewError::NotFound)
    );
    assert_eq!(
        reject_offer(db.pool(), stranger, request_id, offer_id, None).await,
        Err(RejectError::NotFound)
    );
    assert_eq!(
        view_offer(db.pool(), stranger, request_id, uuid::Uuid::now_v7()).await,
        Err(ViewError::NotFound)
    );
    let stored = offers::offer(db.pool(), offer_id)
        .await
        .expect("offer reads")
        .expect("offer reads");
    assert_eq!(stored.state, "sent");
    assert_eq!(offer_facts(db.pool(), offer_id, "offer.viewed").await, 0);
    assert_eq!(offer_facts(db.pool(), offer_id, "offer.rejected").await, 0);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn declined_offers_stay_unavailable_without_resubmission() {
    let db = TestDatabase::create("p07t05_decline")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t05_decline_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Decline Owner", "+55 11 90000-0156").await;
    let seller = seed_active(db.pool(), "Decline Seller", "+55 11 90000-0157").await;
    let request_id = publish_open(db.pool(), author).await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;

    // A viewed offer declines with an optional note: one fact, static
    // terminal reason, history intact, nobody scored.
    view_offer(db.pool(), author, request_id, offer_id)
        .await
        .expect("fixture viewing views");
    let declined = reject_offer(
        db.pool(),
        author,
        request_id,
        offer_id,
        Some("Found a better fit.".to_owned()),
    )
    .await
    .expect("decline declines");
    assert_eq!(declined.state, "rejected");
    assert_eq!(offer_facts(db.pool(), offer_id, "offer.viewed").await, 1);
    assert_eq!(offer_facts(db.pool(), offer_id, "offer.rejected").await, 1);
    let stored = offers::offer(db.pool(), offer_id)
        .await
        .expect("offer reads")
        .expect("offer reads");
    assert_eq!(stored.terminal_reason.as_deref(), Some("declined"));

    // Repeats refuse with nothing new; the held slot blocks same-cycle
    // resubmission through both the guard and the durable key.
    assert_eq!(
        reject_offer(db.pool(), author, request_id, offer_id, None).await,
        Err(RejectError::ForbiddenState)
    );
    assert_eq!(offer_facts(db.pool(), offer_id, "offer.rejected").await, 1);
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let verdict = check_submission_limits(&mut tx, request_id, 1, seller, chrono::Utc::now()).await;
    tx.rollback().await.expect("probe rolls back");
    assert!(
        matches!(
            verdict,
            Err(AttemptError::Abort(OfferLimitError::SlotOccupied))
        ),
        "the held slot blocks resubmission"
    );
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert!(offers::create_offer(
        &mut tx,
        offers::NewOffer {
            request_id,
            cycle_number: 1,
            revision_number: 1,
            seller_id: seller,
            description: "Frost-free 300L".to_owned(),
            price_cents: 52_000,
            condition: "used".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            notes: String::new(),
        },
    )
    .await
    .is_err());
    tx.rollback().await.expect("refusal rolls back");

    // Phone-shaped notes refuse without touching the row.
    let second = submit_open_for(db.pool(), seller, author).await;
    assert_eq!(
        reject_offer(
            db.pool(),
            author,
            second.0,
            second.1,
            Some("Call +55 11 98765-4321".to_owned()),
        )
        .await,
        Err(RejectError::InvalidNote)
    );
    db.cleanup().await.expect("suite cleans up");
}

/// A second demand with one offer, for note-validation fixtures.
async fn submit_open_for(
    pool: &sqlx::PgPool,
    seller: uuid::Uuid,
    author: uuid::Uuid,
) -> (uuid::Uuid, uuid::Uuid) {
    let request_id = publish_open(pool, author).await;
    let offer_id = submit_open(pool, seller, request_id).await;
    (request_id, offer_id)
}
