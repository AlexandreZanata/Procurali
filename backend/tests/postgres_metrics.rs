//! Cohort-metric acceptance (P14-T03): exact ratios, honest unknowns.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). There is no metrics HTTP surface yet, so these tests drive
//! the application operations directly over real PostgreSQL — the same
//! direct-proof pattern as the staff-grant cards ("api" here is the
//! audited Rust operation surface). Proves:
//! - one demand with two distinct seller contacts measures North Star 2
//!   with 100% coverage and no inferred sale;
//! - elsewhere and unknown-source resolutions stay separate from
//!   platform-attributed outcomes;
//! - empty and immature cohorts report not-applicable instead of a false
//!   zero success rate.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::metrics::{compute_cohort, CohortScope, Maturity};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::record_outcome::{
    record_outcome, CompletionSource, OutcomeAnswer,
};
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::start_contact::{start_contact, ContactInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::application::view_offer::view_offer;
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p14t03-test-only-lookup-key",
        encryption_key: "p14t03-test-only-encryption-key",
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

/// One viewed and contacted offer through the real operations.
async fn contacted_offer(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    seller: uuid::Uuid,
    demand: uuid::Uuid,
    price: &str,
) -> uuid::Uuid {
    let offer = submit_offer(
        pool,
        seller,
        demand,
        OfferInput {
            revision_number: Some(1),
            cycle_number: Some(1),
            description: Some("Frost-free 300L".to_owned()),
            price: Some(price.to_owned()),
            condition: Some("used".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
            available: Some(true),
            available_in_city: Some(true),
        },
    )
    .await
    .expect("fixture submission submits")
    .id;
    view_offer(pool, author, demand, offer)
        .await
        .expect("fixture view records");
    start_contact(
        pool,
        &test_keys(),
        author,
        demand,
        offer,
        ContactInput {
            handoff_id: Some(uuid::Uuid::now_v7().to_string()),
            expected_offer_terms: Some(1),
            entry_source: Some("offer_detail".to_owned()),
        },
    )
    .await
    .expect("fixture handoff starts");
    offer
}

fn cohort_window(
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
) -> CohortScope {
    CohortScope {
        city_code: Some("campinas".to_owned()),
        category_code: Some("home_appliances".to_owned()),
        from,
        to,
    }
}

#[tokio::test]
async fn one_request_two_contacts_north_star_two() {
    let db = TestDatabase::create("p14t03_northstar")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t03_northstar_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "North Buyer", "+55 11 90000-2901").await;
    let seller_one = seed_active(db.pool(), "North Seller One", "+55 11 90000-2902").await;
    let seller_two = seed_active(db.pool(), "North Seller Two", "+55 11 90000-2903").await;
    let from = chrono::Utc::now() - chrono::Duration::hours(1);
    let demand = publish_demand(db.pool(), buyer, "Refrigerator").await;
    contacted_offer(db.pool(), buyer, seller_one, demand, "520.00").await;
    contacted_offer(db.pool(), buyer, seller_two, demand, "480.00").await;
    let to = chrono::Utc::now() + chrono::Duration::hours(1);

    // One demand with two distinct seller contacts: North Star 2 with
    // 100% coverage, full offer ratios, zero completions — and no sale,
    // success-percentage, or trust vocabulary anywhere near the numbers.
    let metrics = compute_cohort(db.pool(), cohort_window(from, to))
        .await
        .expect("cohort computes");
    assert_eq!(metrics.published, 1);
    assert_eq!(metrics.north_star.numerator, 2);
    assert_eq!(metrics.north_star.denominator, 1);
    assert_eq!(metrics.north_star.value, Some(2.0));
    assert_eq!(metrics.coverage.value, Some(1.0));
    assert_eq!(metrics.offer_coverage.value, Some(1.0));
    assert_eq!(metrics.offers_per_request.value, Some(2.0));
    assert_eq!(metrics.offers_per_request_median, Some(2.0));
    assert_eq!(metrics.offer_view_rate.value, Some(1.0));
    assert_eq!(metrics.offer_to_contact.value, Some(1.0));
    assert_eq!(metrics.completed, 0);
    assert_eq!(metrics.unknown_outcomes, 1);
    let rendered = serde_json::to_string(&metrics).expect("metrics serialize");
    for absent in ["sale", "sold", "trust", "score", "verified"] {
        assert!(!rendered.contains(absent), "no {absent} in metrics");
    }
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn elsewhere_differs_from_platform_attributed() {
    let db = TestDatabase::create("p14t03_attribution")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t03_attribution_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Split Buyer", "+55 11 90000-2904").await;
    let seller = seed_active(db.pool(), "Split Seller", "+55 11 90000-2905").await;
    let from = chrono::Utc::now() - chrono::Duration::hours(1);
    let platform_demand = publish_demand(db.pool(), buyer, "Platform Fridge").await;
    let platform_offer = contacted_offer(db.pool(), buyer, seller, platform_demand, "520.00").await;
    record_outcome(
        db.pool(),
        buyer,
        platform_demand,
        OutcomeAnswer::Completed(CompletionSource::Platform {
            offer_id: platform_offer,
        }),
    )
    .await
    .expect("platform outcome records");
    let elsewhere_demand = publish_demand(db.pool(), buyer, "Elsewhere Fridge").await;
    record_outcome(
        db.pool(),
        buyer,
        elsewhere_demand,
        OutcomeAnswer::Completed(CompletionSource::Elsewhere),
    )
    .await
    .expect("elsewhere outcome records");
    let undisclosed_demand = publish_demand(db.pool(), buyer, "Quiet Fridge").await;
    record_outcome(
        db.pool(),
        buyer,
        undisclosed_demand,
        OutcomeAnswer::Completed(CompletionSource::Unknown),
    )
    .await
    .expect("undisclosed outcome records");
    let to = chrono::Utc::now() + chrono::Duration::hours(1);

    // Three completions split one platform against two elsewhere-source:
    // the platform rate counts only the contact-linked finding.
    let metrics = compute_cohort(db.pool(), cohort_window(from, to))
        .await
        .expect("cohort computes");
    assert_eq!(metrics.published, 3);
    assert_eq!(metrics.completed, 3);
    assert_eq!(metrics.completed_platform, 1);
    assert_eq!(metrics.completed_elsewhere, 2);
    assert_ne!(metrics.completed_platform, metrics.completed);
    assert_eq!(metrics.resolution_rate.value, Some(1.0));
    assert_eq!(metrics.platform_resolution_rate.numerator, 1);
    assert_eq!(metrics.platform_resolution_rate.denominator, 3);
    assert_eq!(metrics.respondent_success.value, Some(1.0));
    assert_eq!(metrics.unknown_outcomes, 0);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn empty_and_immature_are_not_false_zero() {
    let db = TestDatabase::create("p14t03_empty")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t03_empty_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Empty Buyer", "+55 11 90000-2906").await;
    let seller = seed_active(db.pool(), "Empty Seller", "+55 11 90000-2907").await;
    publish_demand(db.pool(), buyer, "Refrigerator").await;
    let now = chrono::Utc::now();

    // An empty cohort reports not-applicable everywhere instead of zeros.
    let empty = compute_cohort(
        db.pool(),
        cohort_window(
            now - chrono::Duration::days(60),
            now - chrono::Duration::days(30),
        ),
    )
    .await
    .expect("empty cohort computes");
    assert_eq!(empty.published, 0);
    assert_eq!(empty.maturity, Maturity::Mature);
    for value in [
        empty.north_star.value,
        empty.coverage.value,
        empty.offer_coverage.value,
        empty.offers_per_request.value,
        empty.offer_view_rate.value,
        empty.offer_to_contact.value,
        empty.resolution_rate.value,
        empty.platform_resolution_rate.value,
        empty.respondent_success.value,
        empty.outcome_response.value,
    ] {
        assert_eq!(value, None);
    }
    assert_eq!(empty.offers_per_request_median, None);
    assert_eq!(empty.completed, 0);
    assert_eq!(empty.unknown_outcomes, 0);

    // A fresh cohort is immature with its observation still counting: the
    // unresolved demand reads unknown, never a false zero success rate.
    let fresh = compute_cohort(
        db.pool(),
        cohort_window(
            now - chrono::Duration::hours(1),
            now + chrono::Duration::hours(1),
        ),
    )
    .await
    .expect("fresh cohort computes");
    assert_eq!(fresh.published, 1);
    assert_eq!(fresh.maturity, Maturity::Immature);
    assert_eq!(fresh.unknown_outcomes, 1);
    assert_eq!(fresh.respondent_success.value, None);
    let _ = seller;
    db.cleanup().await.expect("suite cleans up");
}
