//! Identity-history acceptance (P12-T05): moves keep history honest.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL with the real creation,
//! publication, submission, contact, profile, and proof-gated
//! phone-change operations:
//! - a default-city change moves the account label only, leaving every
//!   request, revision, and offer locality exactly as recorded;
//! - a verified number change redirects future handoffs to the current
//!   destination while prior contact terms stand byte-identical with one
//!   history entry;
//! - duplicate assignment and stranger history reads refuse without
//!   exposing previous-owner records or writing phone facts.
//!
//! Setup travels the real paths with real phone cryptography and the
//! deterministic fake provider. All phones, names, codes, and keys below
//! are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::auth_limits::AbuseLimits;
use procurali_backend::application::change_phone::{
    confirm_number_change, request_number_change, ChangeConfirmOutcome, ChangeRequestOutcome,
};
use procurali_backend::application::phone_verification::FakeVerifyProvider;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::start_contact::{start_contact, ContactError, ContactInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::application::update_profile::{update_profile, ProfileUpdate};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::phone_history::history_for;
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};
use std::time::Duration;

const FAKE_CODE: &str = "135790";
const WINDOW: Duration = Duration::from_secs(300);

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p12t05-test-only-lookup-key",
        encryption_key: "p12t05-test-only-encryption-key",
    }
}

fn open_limits() -> AbuseLimits {
    AbuseLimits {
        max_starts_per_hour: 10,
        resend_minimum_secs: 0,
        max_attempts_per_challenge: 5,
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

/// One live demand with one contacted offer through the real operations.
async fn live_contact(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    seller: uuid::Uuid,
) -> (uuid::Uuid, uuid::Uuid, String) {
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
    let demand = publish_request(pool, author, draft.id)
        .await
        .expect("fixture draft publishes")
        .id;
    let offer = submit_offer(
        pool,
        seller,
        demand,
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
        },
    )
    .await
    .expect("fixture submission submits")
    .id;
    let handoff_id = uuid::Uuid::now_v7().to_string();
    let handoff = start_contact(
        pool,
        &test_keys(),
        author,
        demand,
        offer,
        ContactInput {
            handoff_id: Some(handoff_id.clone()),
            expected_offer_terms: Some(1),
            entry_source: Some("offer_detail".to_owned()),
        },
    )
    .await
    .expect("fixture handoff starts");
    assert!(!handoff.repeat);
    (demand, offer, handoff_id)
}

async fn change_number(pool: &sqlx::PgPool, account: uuid::Uuid, new_phone: &str) {
    let provider = FakeVerifyProvider::for_tests(FAKE_CODE);
    assert_eq!(
        request_number_change(
            pool,
            &provider,
            account,
            new_phone,
            test_keys(),
            WINDOW,
            open_limits(),
        )
        .await
        .expect("request works"),
        ChangeRequestOutcome::Sent
    );
    assert_eq!(
        confirm_number_change(pool, &provider, account, new_phone, FAKE_CODE, test_keys(),)
            .await
            .expect("confirmation answers"),
        ChangeConfirmOutcome::Changed
    );
}

#[tokio::test]
async fn default_city_change_does_not_rewrite_locality() {
    let db = TestDatabase::create("p12t05_locality")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p12t05_locality_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Local Buyer", "+55 11 90000-1901").await;
    let seller = seed_active(db.pool(), "Local Seller", "+55 11 90000-1902").await;
    let (demand, offer, _) = live_contact(db.pool(), buyer, seller).await;
    let before_request: (String, String) =
        sqlx::query_as("SELECT city_code, region_code FROM requests WHERE id = $1")
            .bind(demand)
            .fetch_one(db.pool())
            .await
            .expect("request locality reads");
    let before_revision: (String, String) = sqlx::query_as(
        "SELECT city_code, region_code FROM request_revisions
         WHERE request_id = $1 AND revision_number = 1",
    )
    .bind(demand)
    .fetch_one(db.pool())
    .await
    .expect("revision locality reads");
    let before_offer: (String, String) =
        sqlx::query_as("SELECT city_code, region_code FROM offers WHERE id = $1")
            .bind(offer)
            .fetch_one(db.pool())
            .await
            .expect("offer locality reads");

    // The default-city label moves on the account only: every recorded
    // resource locality stands byte-identical.
    update_profile(
        db.pool(),
        buyer,
        ProfileUpdate {
            display_name: None,
            city: Some("Santos".to_owned()),
            region: None,
        },
    )
    .await
    .expect("city label updates");
    let account_city: String = sqlx::query_scalar("SELECT city FROM users WHERE id = $1")
        .bind(buyer)
        .fetch_one(db.pool())
        .await
        .expect("account reads");
    assert_eq!(account_city, "Santos");
    let after_request: (String, String) =
        sqlx::query_as("SELECT city_code, region_code FROM requests WHERE id = $1")
            .bind(demand)
            .fetch_one(db.pool())
            .await
            .expect("request locality re-reads");
    assert_eq!(before_request, after_request);
    let after_revision: (String, String) = sqlx::query_as(
        "SELECT city_code, region_code FROM request_revisions
         WHERE request_id = $1 AND revision_number = 1",
    )
    .bind(demand)
    .fetch_one(db.pool())
    .await
    .expect("revision locality re-reads");
    assert_eq!(before_revision, after_revision);
    let after_offer: (String, String) =
        sqlx::query_as("SELECT city_code, region_code FROM offers WHERE id = $1")
            .bind(offer)
            .fetch_one(db.pool())
            .await
            .expect("offer locality re-reads");
    assert_eq!(before_offer, after_offer);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn new_handoff_uses_new_destination_prior_terms_intact() {
    let db = TestDatabase::create("p12t05_newnumber")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p12t05_newnumber_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Number Buyer", "+55 11 90000-1903").await;
    let seller = seed_active(db.pool(), "Number Seller", "+55 11 90000-1904").await;
    let (demand, offer, _) = live_contact(db.pool(), buyer, seller).await;
    let before: (uuid::Uuid, i64, String, i32) = sqlx::query_as(
        "SELECT id, offer_price_cents, request_title, offer_terms_number FROM contacts
         WHERE offer_id = $1",
    )
    .bind(offer)
    .fetch_one(db.pool())
    .await
    .expect("contact reads");

    // A verified number change redirects the next handoff to the current
    // destination while the recorded terms stand byte-identical with one
    // history entry.
    change_number(db.pool(), seller, "+55 11 90000-1905").await;
    let history = history_for(db.pool(), seller).await.expect("history reads");
    assert_eq!(history.len(), 1);
    let retry = start_contact(
        db.pool(),
        &test_keys(),
        buyer,
        demand,
        offer,
        ContactInput {
            handoff_id: Some(uuid::Uuid::now_v7().to_string()),
            expected_offer_terms: Some(1),
            entry_source: Some("offer_detail".to_owned()),
        },
    )
    .await
    .expect("retry re-decrypts current destination");
    assert!(retry.repeat);
    assert!(
        retry.destination.contains("5511900001905"),
        "future handoff uses the new verified destination"
    );
    let after: (uuid::Uuid, i64, String, i32) = sqlx::query_as(
        "SELECT id, offer_price_cents, request_title, offer_terms_number FROM contacts
         WHERE offer_id = $1",
    )
    .bind(offer)
    .fetch_one(db.pool())
    .await
    .expect("contact re-reads");
    assert_eq!(before, after, "prior contact terms stand intact");
    let contacts: i64 = sqlx::query_scalar("SELECT count(*) FROM contacts")
        .fetch_one(db.pool())
        .await
        .expect("contacts read");
    assert_eq!(contacts, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn duplicate_phone_and_stranger_history_refuse() {
    let db = TestDatabase::create("p12t05_duplicate")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p12t05_duplicate_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Duplicate Buyer", "+55 11 90000-1906").await;
    let seller = seed_active(db.pool(), "Duplicate Seller", "+55 11 90000-1907").await;
    let stranger = seed_active(db.pool(), "Duplicate Stranger", "+55 11 90000-1908").await;
    let (demand, offer, _) = live_contact(db.pool(), buyer, seller).await;
    let seller_lookup_before: String =
        sqlx::query_scalar("SELECT phone_lookup FROM users WHERE id = $1")
            .bind(seller)
            .fetch_one(db.pool())
            .await
            .expect("lookup reads");
    let events_before: i64 = sqlx::query_scalar("SELECT count(*) FROM business_events")
        .fetch_one(db.pool())
        .await
        .expect("events read");

    // The held number refuses reassignment, and strangers read nobody
    // else's history: both refuse with zero writes and zero disclosure.
    let provider = FakeVerifyProvider::for_tests(FAKE_CODE);
    assert_eq!(
        request_number_change(
            db.pool(),
            &provider,
            stranger,
            "+55 11 90000-1907",
            test_keys(),
            WINDOW,
            open_limits(),
        )
        .await
        .expect("request answers"),
        ChangeRequestOutcome::NotEligible
    );
    assert_eq!(
        confirm_number_change(
            db.pool(),
            &provider,
            stranger,
            "+55 11 90000-1907",
            FAKE_CODE,
            test_keys(),
        )
        .await
        .expect("confirmation answers"),
        ChangeConfirmOutcome::Failed
    );
    assert!(matches!(
        start_contact(
            db.pool(),
            &test_keys(),
            stranger,
            demand,
            offer,
            ContactInput {
                handoff_id: Some(uuid::Uuid::now_v7().to_string()),
                expected_offer_terms: Some(1),
                entry_source: Some("offer_detail".to_owned()),
            },
        )
        .await,
        Err(ContactError::NotFound)
    ));
    let seller_lookup_after: String =
        sqlx::query_scalar("SELECT phone_lookup FROM users WHERE id = $1")
            .bind(seller)
            .fetch_one(db.pool())
            .await
            .expect("lookup re-reads");
    assert_eq!(seller_lookup_after, seller_lookup_before);
    let events_after: i64 = sqlx::query_scalar("SELECT count(*) FROM business_events")
        .fetch_one(db.pool())
        .await
        .expect("events re-read");
    assert_eq!(events_after, events_before, "refusals write no phone facts");
    assert!(history_for(db.pool(), stranger)
        .await
        .expect("stranger history reads")
        .is_empty());
    db.cleanup().await.expect("suite cleans up");
}
