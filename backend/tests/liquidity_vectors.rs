//! Liquidity-classification vectors (P14-T05): history separate from live.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL`. Drives real
//! publication/offer/withdrawal operations over real PostgreSQL and proves:
//! - the 24-hour boundary and three live sellers classify correctly;
//! - a late first offer never erases the missed early target;
//! - withdrawn-all-supply reads low despite a prior timely response.
//!
//! All names, numbers, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::liquidity::{classify_request, Liquidity};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::application::withdraw_offer::{withdraw_offer, WithdrawReason};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p14t05-test-only-lookup-key",
        encryption_key: "p14t05-test-only-encryption-key",
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

async fn live_offer(
    pool: &sqlx::PgPool,
    seller: uuid::Uuid,
    demand: uuid::Uuid,
    price: &str,
) -> uuid::Uuid {
    submit_offer(
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
    .id
}

async fn backdate_publication(pool: &sqlx::PgPool, demand: uuid::Uuid, hours_ago: i64) {
    sqlx::query("UPDATE requests SET original_published_at = $1 WHERE id = $2")
        .bind(chrono::Utc::now() - chrono::Duration::hours(hours_ago))
        .bind(demand)
        .execute(pool)
        .await
        .expect("publication staging applies");
}

#[tokio::test]
async fn boundary_and_three_live_sellers_classify() {
    let db = TestDatabase::create("p14t05_boundary")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t05_boundary_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Liquid Buyer", "+55 11 90000-2940").await;
    let seller_one = seed_active(db.pool(), "Liquid Seller One", "+55 11 90000-2941").await;
    let seller_two = seed_active(db.pool(), "Liquid Seller Two", "+55 11 90000-2942").await;
    let seller_three = seed_active(db.pool(), "Liquid Seller Three", "+55 11 90000-2943").await;
    let now = chrono::Utc::now() + chrono::Duration::seconds(10);

    let awaiting = publish_demand(db.pool(), buyer, "Awaiting Fridge").await;
    let awaiting_report = classify_request(db.pool(), awaiting, now)
        .await
        .expect("awaiting classifies");
    assert_eq!(awaiting_report.state, Liquidity::AwaitingSupply);

    let gap = publish_demand(db.pool(), buyer, "Gap Fridge").await;
    backdate_publication(db.pool(), gap, 30).await;
    let gap_report = classify_request(db.pool(), gap, now)
        .await
        .expect("gap classifies");
    assert_eq!(gap_report.state, Liquidity::NoOfferGap);

    let healthy = publish_demand(db.pool(), buyer, "Healthy Fridge").await;
    backdate_publication(db.pool(), healthy, 2).await;
    live_offer(db.pool(), seller_one, healthy, "520.00").await;
    let healthy_report = classify_request(db.pool(), healthy, now)
        .await
        .expect("healthy classifies");
    assert_eq!(healthy_report.state, Liquidity::Healthy);
    assert_eq!(healthy_report.live_sellers, 1);

    let crowded = publish_demand(db.pool(), buyer, "Crowded Fridge").await;
    backdate_publication(db.pool(), crowded, 2).await;
    live_offer(db.pool(), seller_one, crowded, "520.00").await;
    live_offer(db.pool(), seller_two, crowded, "510.00").await;
    live_offer(db.pool(), seller_three, crowded, "500.00").await;
    let crowded_report = classify_request(db.pool(), crowded, now)
        .await
        .expect("crowded classifies");
    assert_eq!(crowded_report.state, Liquidity::HighCompetition);
    assert_eq!(crowded_report.live_sellers, 3);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn late_first_offer_keeps_missed_target() {
    let db = TestDatabase::create("p14t05_late")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t05_late_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Late Buyer", "+55 11 90000-2950").await;
    let seller = seed_active(db.pool(), "Late Seller", "+55 11 90000-2951").await;
    let demand = publish_demand(db.pool(), buyer, "Late Fridge").await;
    backdate_publication(db.pool(), demand, 30).await;
    live_offer(db.pool(), seller, demand, "520.00").await;
    let now = chrono::Utc::now() + chrono::Duration::seconds(10);
    let report = classify_request(db.pool(), demand, now)
        .await
        .expect("late demand classifies");
    assert!(report.first_offer_at.is_some());
    assert_eq!(report.live_sellers, 1);
    assert_eq!(report.state, Liquidity::Low);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn withdrawn_all_supply_reads_low_despite_timely_history() {
    let db = TestDatabase::create("p14t05_withdrawn")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p14t05_withdrawn_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Gone Buyer", "+55 11 90000-2960").await;
    let seller = seed_active(db.pool(), "Gone Seller", "+55 11 90000-2961").await;
    let demand = publish_demand(db.pool(), buyer, "Gone Fridge").await;
    backdate_publication(db.pool(), demand, 2).await;
    let offer = live_offer(db.pool(), seller, demand, "520.00").await;
    withdraw_offer(db.pool(), seller, offer, WithdrawReason::Withdrawn)
        .await
        .expect("withdrawal applies");
    let now = chrono::Utc::now() + chrono::Duration::seconds(10);
    let report = classify_request(db.pool(), demand, now)
        .await
        .expect("withdrawn demand classifies");
    assert!(
        report.first_offer_at.is_some(),
        "history survives withdrawal"
    );
    assert_eq!(report.live_sellers, 0);
    assert_eq!(report.state, Liquidity::Low);
    db.cleanup().await.expect("suite cleans up");
}
