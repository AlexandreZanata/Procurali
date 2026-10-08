//! Offer-edit acceptance (P07-T06): live terms with preserved engagement.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). These operations currently have no HTTP route (the
//! route-owning card lands them later), so the tests drive the application
//! operation directly over real PostgreSQL — the same direct-proof pattern
//! as the neighboring cards. Proves:
//! - price updates fit the budget or refuse, boundary inclusive;
//! - viewed engagement and historical snapshots survive edits, with buyer
//!   notices only where somebody looked;
//! - stale expected terms and stale demand refuse instead of adopting
//!   hidden terms.
//!
//! Setup submissions travel the real submission operation. All phones,
//! names, and codes below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::edit_offer::{edit_offer, EditError, EditTermsInput};
use procurali_backend::application::offer_reads::{buyer_offers, OfferSort};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::application::view_offer::view_offer;
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::offers::terms_for_offer;
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
            lookup_key: "p07t06-test-only-lookup-key",
            encryption_key: "p07t06-test-only-encryption-key",
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

/// One genuinely open either/600 demand through the real paths.
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

fn submit_input() -> OfferInput {
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

fn edit_input(expected: i32, price: &str) -> EditTermsInput {
    EditTermsInput {
        expected_terms_number: Some(expected),
        description: Some("Frost-free 300L".to_owned()),
        price: Some(price.to_owned()),
        condition: Some("used".to_owned()),
        city_code: Some("campinas".to_owned()),
        region_code: Some("centro".to_owned()),
        notes: None,
    }
}

async fn submit_open(
    pool: &sqlx::PgPool,
    seller: uuid::Uuid,
    request_id: uuid::Uuid,
) -> uuid::Uuid {
    submit_offer(pool, seller, request_id, submit_input())
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
async fn price_update_fits_budget_or_is_refused() {
    let db = TestDatabase::create("p07t06_price")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t06_price_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Price Owner", "+55 11 90000-0161").await;
    let seller = seed_active(db.pool(), "Price Seller", "+55 11 90000-0162").await;
    let request_id = publish_open(db.pool(), author).await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;

    // A within-budget move promotes current terms with history appended.
    let edited = edit_offer(db.pool(), seller, offer_id, edit_input(1, "550.00"))
        .await
        .expect("within-budget edit edits");
    assert_eq!(edited.price, "550.00");
    assert_eq!(
        terms_for_offer(db.pool(), offer_id)
            .await
            .expect("history reads")
            .len(),
        2
    );
    // A cent over the ceiling refuses with nothing appended; the exact
    // ceiling still fits.
    assert_eq!(
        edit_offer(db.pool(), seller, offer_id, edit_input(2, "600.01")).await,
        Err(EditError::OverBudget)
    );
    assert_eq!(
        terms_for_offer(db.pool(), offer_id)
            .await
            .expect("history reads")
            .len(),
        2
    );
    let edited = edit_offer(db.pool(), seller, offer_id, edit_input(2, "600.00"))
        .await
        .expect("ceiling price edits");
    assert_eq!(edited.price, "600.00");
    assert_eq!(
        terms_for_offer(db.pool(), offer_id)
            .await
            .expect("history reads")
            .len(),
        3
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn viewed_state_and_snapshot_survive_edits() {
    let db = TestDatabase::create("p07t06_viewed")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t06_viewed_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Viewed Owner", "+55 11 90000-0163").await;
    let seller = seed_active(db.pool(), "Viewed Seller", "+55 11 90000-0164").await;
    let request_id = publish_open(db.pool(), author).await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;
    view_offer(db.pool(), author, request_id, offer_id)
        .await
        .expect("fixture viewing views");

    // Editing viewed terms keeps engagement, history, and the single view
    // fact — and tells the buyer, who already looked.
    let edited = edit_offer(db.pool(), seller, offer_id, edit_input(1, "550.00"))
        .await
        .expect("viewed edit edits");
    assert_eq!(edited.state, "viewed", "edits never unsee");
    assert_eq!(offer_facts(db.pool(), offer_id, "offer.viewed").await, 1);
    assert_eq!(
        offer_facts(db.pool(), offer_id, "offer.terms_updated").await,
        1
    );
    let history = terms_for_offer(db.pool(), offer_id)
        .await
        .expect("history reads");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].price_cents, 52_000);
    assert_eq!(history[1].price_cents, 55_000);
    let notices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notices WHERE resource_id = $1 AND kind = 'offer.updated'",
    )
    .bind(offer_id)
    .fetch_one(db.pool())
    .await
    .expect("notices read");
    assert_eq!(notices, 1);

    // Editing unseen terms stays silent: no buyer notice for nobody.
    let other = seed_active(db.pool(), "Unseen Seller", "+55 11 90000-0165").await;
    let unseen = submit_open(db.pool(), other, request_id).await;
    edit_offer(db.pool(), other, unseen, edit_input(1, "550.00"))
        .await
        .expect("unseen edit edits");
    let notices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notices WHERE resource_id = $1 AND kind = 'offer.updated'",
    )
    .bind(unseen)
    .fetch_one(db.pool())
    .await
    .expect("notices read");
    assert_eq!(notices, 0);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn stale_expected_revision_refuses_edit_and_contact() {
    let db = TestDatabase::create("p07t06_stale")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t06_stale_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Stale Owner", "+55 11 90000-0166").await;
    let seller = seed_active(db.pool(), "Stale Seller", "+55 11 90000-0167").await;
    let request_id = publish_open(db.pool(), author).await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;

    // Composing against moved terms refuses instead of adopting them; the
    // next attempt with the current number succeeds exactly once more.
    assert_eq!(
        edit_offer(db.pool(), seller, offer_id, edit_input(99, "550.00")).await,
        Err(EditError::StaleTerms)
    );
    assert_eq!(
        terms_for_offer(db.pool(), offer_id)
            .await
            .expect("history reads")
            .len(),
        1
    );
    edit_offer(db.pool(), seller, offer_id, edit_input(1, "550.00"))
        .await
        .expect("current edit edits");
    assert_eq!(
        edit_offer(db.pool(), seller, offer_id, edit_input(1, "560.00")).await,
        Err(EditError::StaleTerms)
    );

    // A materially revised demand strands the offer: edits refuse as
    // resubmission scope, and comparison already disables contact.
    procurali_backend::application::revise_request::revise_request(
        db.pool(),
        author,
        request_id,
        procurali_backend::application::revise_request::ReviseInput {
            title: "Refrigerator".to_owned(),
            category_code: "home_appliances".to_owned(),
            budget: "600.00".to_owned(),
            condition: "either".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            notes: "Needs ice maker.".to_owned(),
        },
    )
    .await
    .expect("fixture revision revises");
    assert_eq!(
        edit_offer(db.pool(), seller, offer_id, edit_input(2, "560.00")).await,
        Err(EditError::ForbiddenState)
    );
    let list = buyer_offers(db.pool(), author, request_id, OfferSort::Newest)
        .await
        .expect("buyer reads");
    assert_eq!(list.live.len(), 1);
    assert!(!list.live[0].contact_allowed);

    // Terminal rows never edit, whatever the expected number claims.
    sqlx::query("UPDATE offers SET state = 'withdrawn' WHERE id = $1")
        .bind(offer_id)
        .execute(db.pool())
        .await
        .expect("synthetic withdrawal applies");
    assert_eq!(
        edit_offer(db.pool(), seller, offer_id, edit_input(2, "560.00")).await,
        Err(EditError::ForbiddenState)
    );
    db.cleanup().await.expect("suite cleans up");
}
