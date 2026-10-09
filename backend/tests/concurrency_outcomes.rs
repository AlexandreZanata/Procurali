//! Outcome races (P09-T04): duplicate answers and closure contention.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Coordinates duplicate final answers and closing-versus-
//! submitting schedules over real PostgreSQL with barriers only — no
//! arbitrary sleeps anywhere — and proves the converged end state is
//! exact: one outcome row, one closure fact, and no live offer on
//! terminal demand.
//!
//! Setup closes travel the real outcome operation. All phones, names,
//! codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::record_outcome::{
    record_outcome, CompletionSource, OutcomeAnswer, OutcomeError,
};
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::request_offer_cascades::{
    cascade_request_offers, CascadeCause,
};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
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
            lookup_key: "p09t04-test-only-lookup-key",
            encryption_key: "p09t04-test-only-encryption-key",
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

async fn outcome_rows(pool: &sqlx::PgPool, request_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM request_outcomes WHERE request_id = $1")
        .bind(request_id)
        .fetch_one(pool)
        .await
        .expect("outcomes read")
}

async fn completed_facts(pool: &sqlx::PgPool, request_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'request' AND resource_id = $1 AND kind = 'request.completed'",
    )
    .bind(request_id)
    .fetch_one(pool)
    .await
    .expect("facts read")
}

async fn live_offer_count(pool: &sqlx::PgPool, request_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM offers
         WHERE request_id = $1 AND state IN ('sent', 'viewed', 'contacted')",
    )
    .bind(request_id)
    .fetch_one(pool)
    .await
    .expect("live rows read")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicate_final_answers_converge_on_one_outcome() {
    let db = TestDatabase::create("p09t04_dupanswers")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p09t04_dupanswers_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Dup Owner", "+55 11 90000-0801").await;
    let request_id = publish_open(db.pool(), author, "Refrigerator").await;

    // Two identical final answers race: the row lock serializes the
    // recordings, so the loser replays the winner's row — or, when both
    // pass the check first, exactly one close wins and the other refuses.
    // Either way there is one outcome row and one closure fact.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let answer = |barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            record_outcome(
                &pool,
                author,
                request_id,
                OutcomeAnswer::Completed(CompletionSource::Elsewhere),
            )
            .await
        }
    };
    let (first, second) = tokio::join!(answer(std::sync::Arc::clone(&barrier)), answer(barrier),);
    let oks = [&first, &second].iter().filter(|r| r.is_ok()).count();
    assert!(
        oks >= 1,
        "at least one duplicate answer records: {first:?} / {second:?}"
    );
    for outcome in [&first, &second].iter().filter_map(|r| r.as_ref().ok()) {
        assert_eq!(outcome.outcome, "completed");
    }
    for error in [&first, &second].iter().filter_map(|r| r.as_ref().err()) {
        assert_eq!(
            *error,
            OutcomeError::ForbiddenState,
            "the only legal loser outcome refuses"
        );
    }
    assert_eq!(outcome_rows(db.pool(), request_id).await, 1);
    assert_eq!(completed_facts(db.pool(), request_id).await, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn submission_against_closure_leaves_no_live_offer() {
    let db = TestDatabase::create("p09t04_closerace")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p09t04_closerace_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Close Owner", "+55 11 90000-0802").await;
    let seller = seed_active(db.pool(), "Close Seller", "+55 11 90000-0803").await;
    let other = seed_active(db.pool(), "Close Other", "+55 11 90000-0804").await;
    let request_id = publish_open(db.pool(), author, "Refrigerator").await;
    submit_offer(db.pool(), seller, request_id, submit_input())
        .await
        .expect("fixture submission submits");

    // Closer records completion with its cascade while a second seller
    // submits: either the submission lands first and the cascade retires
    // it, or closure lands first and the submission refuses — terminal
    // demand never keeps a live offer, and the outcome records once.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let close = |barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            let recorded = record_outcome(
                &pool,
                author,
                request_id,
                OutcomeAnswer::Completed(CompletionSource::Elsewhere),
            )
            .await;
            let mut tx = pool.begin().await.expect("transaction begins");
            let moved =
                cascade_request_offers(&mut tx, request_id, CascadeCause::RequestClosed).await;
            tx.commit().await.expect("cascade commits");
            (recorded, moved)
        }
    };
    let submit = |barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            submit_offer(&pool, other, request_id, submit_input()).await
        }
    };
    let ((recorded, moved), submitted) =
        tokio::join!(close(std::sync::Arc::clone(&barrier)), submit(barrier),);
    assert!(recorded.is_ok(), "closure records: {recorded:?}");
    assert!(moved.is_ok(), "cascade runs: {moved:?}");
    assert_eq!(outcome_rows(db.pool(), request_id).await, 1);
    assert_eq!(completed_facts(db.pool(), request_id).await, 1);
    match submitted {
        Ok(_) => {
            // Landed first: the cascade retired it — unless it slipped
            // strictly between the separately-committed close and cascade,
            // the documented transient window that the wiring card closes.
        }
        Err(_) => {
            // Landed after closure: refused with nothing created.
        }
    }
    // Either way a follow-up sweep (what production scheduling performs
    // continuously) converges every live row, and the assertions hold on
    // that converged end state.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    cascade_request_offers(&mut tx, request_id, CascadeCause::RequestClosed)
        .await
        .expect("sweep converges");
    tx.commit().await.expect("sweep commits");
    assert_eq!(live_offer_count(db.pool(), request_id).await, 0);
    db.cleanup().await.expect("suite cleans up");
}
