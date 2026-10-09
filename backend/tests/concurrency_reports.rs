//! Report-grouping race acceptance (P10-T05): one intake under contention.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL with barrier-coordinated
//! racers (no sleeps as synchronization, no mocked persistence):
//! - two concurrent refilings of one incident converge on one case, with
//!   the loser landing as a `duplicate` information record;
//! - snapshots stay identical across the race, and volume alone changes no
//!   account standing.
//!
//! Setup travels the real draft, publication, and submission operations.
//! All phones, names, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::create_report::ReportInput;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::report_updates::submit_grouped_report;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p10t05-test-only-lookup-key",
        encryption_key: "p10t05-test-only-encryption-key",
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

/// One genuinely open either/600 demand with one sent used/520 offer through
/// the real operations.
async fn live_offer(pool: &sqlx::PgPool, author: uuid::Uuid, seller: uuid::Uuid) -> uuid::Uuid {
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
    submit_offer(
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
    .id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_repeats_yield_one_case_plus_information() {
    let db = TestDatabase::create("p10t05_race")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t05_race_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let buyer = seed_active(db.pool(), "Race Buyer", "+55 11 90000-0607").await;
    let seller = seed_active(db.pool(), "Race Seller", "+55 11 90000-0608").await;
    let offer = live_offer(db.pool(), buyer, seller).await;

    // Two identical refilings race in separate transactions: the
    // per-incident advisory lock serializes them, so exactly one intake
    // opens and the loser lands as a duplicate information record.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let attempt = |barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            submit_grouped_report(
                &pool,
                buyer,
                ReportInput {
                    target_kind: "offer".to_owned(),
                    target_id: offer,
                    reason: "spam".to_owned(),
                    detail: "race noise".to_owned(),
                },
            )
            .await
        }
    };
    let (first, second) = tokio::join!(attempt(std::sync::Arc::clone(&barrier)), attempt(barrier));
    let (first, second) = (
        first.expect("racer responds"),
        second.expect("racer responds"),
    );
    assert_eq!(first.case_id, second.case_id, "one incident intake stands");
    let mut statuses = [first.status.clone(), second.status.clone()];
    statuses.sort();
    assert_eq!(statuses, ["duplicate", "open"]);
    assert_eq!(first.target_title_snapshot, "Frost-free 300L");
    assert_eq!(second.target_title_snapshot, "Frost-free 300L");

    let cases: i64 = sqlx::query_scalar("SELECT count(*) FROM report_cases")
        .fetch_one(db.pool())
        .await
        .expect("cases read");
    assert_eq!(cases, 1);
    let reports: i64 = sqlx::query_scalar("SELECT count(*) FROM reports")
        .fetch_one(db.pool())
        .await
        .expect("reports read");
    assert_eq!(reports, 2, "intake plus one information record");
    let states: Vec<String> = sqlx::query_scalar("SELECT state FROM users ORDER BY created_at")
        .fetch_all(db.pool())
        .await
        .expect("account states read");
    assert!(states.iter().all(|state| state == "active"));
    db.cleanup().await.expect("suite cleans up");
}
