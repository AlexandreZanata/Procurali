//! Withdrawal acceptance (P07-T07): terminal honesty, history intact.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). These operations currently have no HTTP route (the
//! route-owning card lands them later), so the tests drive the application
//! operation directly over real PostgreSQL — the same direct-proof pattern
//! as the neighboring cards. Proves:
//! - withdrawn offers leave the actionable set and contact with history
//!   intact;
//! - repeat withdrawals record no duplicate terminal fact;
//! - withdrawal after contact preserves history and infers nothing
//!   elsewhere.
//!
//! Setup submissions travel the real submission operation. All phones,
//! names, and codes below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::offer_reads::{buyer_offers, OfferSort};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::application::withdraw_offer::{
    withdraw_offer, WithdrawError, WithdrawReason,
};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::offers::{self, terms_for_offer};
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
            lookup_key: "p07t07-test-only-lookup-key",
            encryption_key: "p07t07-test-only-encryption-key",
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
async fn publish_open(pool: &sqlx::PgPool, author: uuid::Uuid, title: &str) -> uuid::Uuid {
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
async fn withdrawn_offers_leave_action_and_contact() {
    let db = TestDatabase::create("p07t07_live")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t07_live_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Live Owner", "+55 11 90000-0171").await;
    let seller = seed_active(db.pool(), "Live Seller", "+55 11 90000-0172").await;
    let other = seed_active(db.pool(), "Other Seller", "+55 11 90000-0173").await;
    let request_id = publish_open(db.pool(), author, "Refrigerator").await;
    let withdrawn_id = submit_open(db.pool(), seller, request_id).await;
    let kept_id = submit_open(db.pool(), other, request_id).await;

    let withdrawn = withdraw_offer(db.pool(), seller, withdrawn_id, WithdrawReason::Withdrawn)
        .await
        .expect("withdrawal withdraws");
    assert_eq!(withdrawn.state, "withdrawn");
    assert!(!withdrawn.live);
    assert!(!withdrawn.contact_allowed);
    assert_eq!(
        offer_facts(db.pool(), withdrawn_id, "offer.withdrawn").await,
        1
    );

    // The comparison set drops the withdrawn row while its history stands;
    // the sibling offer never notices.
    let list = buyer_offers(db.pool(), author, request_id, OfferSort::Newest)
        .await
        .expect("buyer reads");
    assert_eq!(list.live.len(), 1);
    assert_eq!(list.live[0].id, kept_id);
    assert!(list.live[0].contact_allowed);
    assert_eq!(list.history.len(), 1);
    assert_eq!(list.history[0].id, withdrawn_id);
    assert_eq!(
        terms_for_offer(db.pool(), withdrawn_id)
            .await
            .expect("history reads")
            .len(),
        1
    );
    let stored = offers::offer(db.pool(), kept_id)
        .await
        .expect("offer reads")
        .expect("offer reads");
    assert_eq!(stored.state, "sent");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn repeat_withdrawal_records_no_duplicate_fact() {
    let db = TestDatabase::create("p07t07_repeat")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t07_repeat_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Repeat Owner", "+55 11 90000-0174").await;
    let seller = seed_active(db.pool(), "Repeat Seller", "+55 11 90000-0175").await;
    let request_id = publish_open(db.pool(), author, "Refrigerator").await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;

    withdraw_offer(db.pool(), seller, offer_id, WithdrawReason::Unavailable)
        .await
        .expect("first withdrawal withdraws");
    let stored = offers::offer(db.pool(), offer_id)
        .await
        .expect("offer reads")
        .expect("offer reads");
    assert_eq!(stored.terminal_reason.as_deref(), Some("unavailable"));
    // Repeats refuse with nothing new: one fact, one notice, same row.
    assert_eq!(
        withdraw_offer(db.pool(), seller, offer_id, WithdrawReason::Unavailable).await,
        Err(WithdrawError::ForbiddenState)
    );
    assert_eq!(offer_facts(db.pool(), offer_id, "offer.withdrawn").await, 1);
    let notices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notices WHERE resource_id = $1 AND kind = 'offer.withdrawn'",
    )
    .bind(offer_id)
    .fetch_one(db.pool())
    .await
    .expect("notices read");
    assert_eq!(notices, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn withdrawal_after_contact_preserves_history_without_inference() {
    let db = TestDatabase::create("p07t07_contact")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t07_contact_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Contact Owner", "+55 11 90000-0176").await;
    let seller = seed_active(db.pool(), "Contact Seller", "+55 11 90000-0177").await;
    let other = seed_active(db.pool(), "Contact Other", "+55 11 90000-0178").await;
    let request_id = publish_open(db.pool(), author, "Refrigerator").await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;
    let sibling_id = submit_open(db.pool(), other, request_id).await;

    // Contact writers do not exist yet, so the contacted engagement is a
    // synthetic fixture; withdrawal from it must still preserve everything
    // and infer nothing elsewhere.
    sqlx::query("UPDATE offers SET state = 'contacted' WHERE id = $1")
        .bind(offer_id)
        .execute(db.pool())
        .await
        .expect("synthetic contact applies");
    let withdrawn = withdraw_offer(db.pool(), seller, offer_id, WithdrawReason::Withdrawn)
        .await
        .expect("withdrawal after contact withdraws");
    assert_eq!(withdrawn.state, "withdrawn");
    assert_eq!(offer_facts(db.pool(), offer_id, "offer.withdrawn").await, 1);
    assert_eq!(
        terms_for_offer(db.pool(), offer_id)
            .await
            .expect("history reads")
            .len(),
        1,
        "terms history stands"
    );
    // No cross-request or sibling inference: the other offer, its facts,
    // and the demand itself are byte-identical to before.
    let sibling = offers::offer(db.pool(), sibling_id)
        .await
        .expect("offer reads")
        .expect("offer reads");
    assert_eq!(sibling.state, "sent");
    assert_eq!(
        offer_facts(db.pool(), sibling_id, "offer.withdrawn").await,
        0
    );
    let demand = procurali_backend::persistence::requests::request(db.pool(), request_id)
        .await
        .expect("request reads")
        .expect("request reads");
    assert_eq!(demand.state, "active");
    assert_eq!(demand.current_cycle_number, 1);
    db.cleanup().await.expect("suite cleans up");
}
