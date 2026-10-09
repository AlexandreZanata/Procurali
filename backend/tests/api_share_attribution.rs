//! Share-attribution acceptance (P13-T05): visits without tracking.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). There is no attribution HTTP surface yet, so these tests
//! drive the application operations directly over real PostgreSQL — the
//! same direct-proof pattern as the staff-grant cards ("api" here is the
//! audited Rust operation surface). Proves:
//! - attributable registration and offer submission use the correct
//!   window with the known source, rechecking live availability;
//! - expired windows, missing markers, direct visits, and fabricated
//!   markers all answer unknown, never a guessed conversion;
//! - a request closed after registration refuses the stale offer with
//!   nothing written.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::close_request::{close_request, BuyerOutcome, OutcomeSource};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::share_attribution::{
    attribute_registration, record_landing, Attribution,
};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput, SubmitError};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p13t05-test-only-lookup-key",
        encryption_key: "p13t05-test-only-encryption-key",
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
        test_keys(),
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

/// One published demand through the real operations.
async fn publish_demand(pool: &sqlx::PgPool, author: uuid::Uuid, title: &str) -> uuid::Uuid {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some(title.to_owned()),
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

#[tokio::test]
async fn attributable_uses_correct_window_and_source() {
    let db = TestDatabase::create("p13t05_window")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p13t05_window_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Window Buyer", "+55 11 90000-2401").await;
    let demand = publish_demand(db.pool(), buyer, "Refrigerator").await;
    let landing = record_landing(db.pool(), Some(demand), None)
        .await
        .expect("landing records");
    assert_eq!(landing.request_id, Some(demand));
    // A registration after the visit attributes to the known source, and
    // the carried intent submits under rechecked live availability.
    let seller = seed_active(db.pool(), "Window Seller", "+55 11 90000-2402").await;
    assert!(matches!(
        attribute_registration(db.pool(), seller, Some(landing.id)).await,
        Ok(Attribution::Attributed { request_id, .. }) if request_id == demand
    ));
    submit_offer(db.pool(), seller, demand, offer_input())
        .await
        .expect("eligible submission succeeds on recheck");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn expired_or_missing_marker_gives_unknown() {
    let db = TestDatabase::create("p13t05_unknown")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p13t05_unknown_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Unknown Buyer", "+55 11 90000-2403").await;
    let demand = publish_demand(db.pool(), buyer, "Refrigerator").await;
    let stale = record_landing(
        db.pool(),
        Some(demand),
        Some(chrono::Utc::now() - chrono::Duration::days(8)),
    )
    .await
    .expect("stale landing records");
    let direct = record_landing(db.pool(), None, None)
        .await
        .expect("direct visit records");
    assert_eq!(direct.request_id, None);
    let seller = seed_active(db.pool(), "Unknown Seller", "+55 11 90000-2404").await;

    // Expired windows, missing markers, direct visits, fabricated
    // markers, and missing accounts all answer unknown — never a guess.
    for marker in [
        Some(stale.id),
        None,
        Some(direct.id),
        Some(uuid::Uuid::now_v7()),
    ] {
        assert_eq!(
            attribute_registration(db.pool(), seller, marker).await,
            Ok(Attribution::Unknown)
        );
    }
    assert_eq!(
        attribute_registration(db.pool(), uuid::Uuid::now_v7(), Some(stale.id)).await,
        Ok(Attribution::Unknown)
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn closed_request_after_registration_refuses_stale_offer() {
    let db = TestDatabase::create("p13t05_stale")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p13t05_stale_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Stale Buyer", "+55 11 90000-2405").await;
    let demand = publish_demand(db.pool(), buyer, "Refrigerator").await;
    let landing = record_landing(db.pool(), Some(demand), None)
        .await
        .expect("landing records");
    let seller = seed_active(db.pool(), "Stale Seller", "+55 11 90000-2406").await;
    assert!(matches!(
        attribute_registration(db.pool(), seller, Some(landing.id)).await,
        Ok(Attribution::Attributed { .. })
    ));
    // The demand closes after registration: the carried intent meets
    // current requirements and refuses, writing no offer row.
    close_request(
        db.pool(),
        buyer,
        demand,
        BuyerOutcome::Found(OutcomeSource::Elsewhere),
    )
    .await
    .expect("demand completes");
    assert_eq!(
        submit_offer(db.pool(), seller, demand, offer_input()).await,
        Err(SubmitError::ForbiddenState)
    );
    let offers: i64 = sqlx::query_scalar("SELECT count(*) FROM offers")
        .fetch_one(db.pool())
        .await
        .expect("offers read");
    assert_eq!(offers, 0);
    db.cleanup().await.expect("suite cleans up");
}
