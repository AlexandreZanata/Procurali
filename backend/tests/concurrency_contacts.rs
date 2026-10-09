//! Contact race linearization (P08-T06): declared schedules, exact states.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Coordinates handoffs against every restriction kind over
//! real PostgreSQL and proves, per declared winning order, the exact
//! persisted rows and facts. Ordering uses barriers and explicit
//! sequencing only — no arbitrary sleeps anywhere: a schedule wins by
//! construction, never by timing luck.
//!
//! Setup handoffs travel the real initiation operation. All phones, names,
//! codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::block_user::block_user;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::request_eligibility::check_request_actionable;
use procurali_backend::application::start_contact::{start_contact, ContactError, ContactInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::application::withdraw_offer::{withdraw_offer, WithdrawReason};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::contacts::contacts_for_offer;
use procurali_backend::persistence::transaction::{
    run_serializable, AttemptError, TransactionError,
};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p08t06-test-only-lookup-key",
        encryption_key: "p08t06-test-only-encryption-key",
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

/// One sent used/520 offer through the real submission operation.
async fn submit_open(
    pool: &sqlx::PgPool,
    seller: uuid::Uuid,
    request_id: uuid::Uuid,
) -> uuid::Uuid {
    submit_offer(
        pool,
        seller,
        request_id,
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

async fn start_handoff(
    pool: &sqlx::PgPool,
    buyer: uuid::Uuid,
    request: uuid::Uuid,
    offer: uuid::Uuid,
) -> Result<procurali_backend::application::start_contact::Handoff, ContactError> {
    start_contact(
        pool,
        &test_keys(),
        buyer,
        request,
        offer,
        ContactInput {
            handoff_id: Some(uuid::Uuid::now_v7().to_string()),
            expected_offer_terms: Some(1),
            entry_source: Some("offer_detail".to_owned()),
        },
    )
    .await
}

async fn contact_rows(pool: &sqlx::PgPool, offer_id: uuid::Uuid) -> usize {
    contacts_for_offer(pool, offer_id)
        .await
        .expect("contacts read")
        .len()
}

#[tokio::test]
async fn restriction_first_disables_handoff() {
    let db = TestDatabase::create("p08t06_restricted")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p08t06_restricted_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Restricted Owner", "+55 11 90000-0401").await;
    let seller = seed_active(db.pool(), "Restricted Seller", "+55 11 90000-0402").await;

    // Declared order one: every restriction lands before the handoff, so
    // each attempt refuses with its defined error and records nothing.
    let withdrawn = demand_with_offer(db.pool(), author, seller).await;
    withdraw_offer(db.pool(), seller, withdrawn.1, WithdrawReason::Withdrawn)
        .await
        .expect("fixture withdrawal withdraws");
    assert_eq!(
        start_handoff(db.pool(), author, withdrawn.0, withdrawn.1).await,
        Err(ContactError::ForbiddenState)
    );
    assert_eq!(contact_rows(db.pool(), withdrawn.1).await, 0);

    let blocked = demand_with_offer(db.pool(), author, seller).await;
    sqlx::query("INSERT INTO user_blocks (blocker_id, blocked_id) VALUES ($1, $2)")
        .bind(author)
        .bind(seller)
        .execute(db.pool())
        .await
        .expect("synthetic block applies");
    assert_eq!(
        start_handoff(db.pool(), author, blocked.0, blocked.1).await,
        Err(ContactError::Blocked)
    );
    assert_eq!(contact_rows(db.pool(), blocked.1).await, 0);
    sqlx::query("DELETE FROM user_blocks WHERE blocker_id = $1 AND blocked_id = $2")
        .bind(author)
        .bind(seller)
        .execute(db.pool())
        .await
        .expect("synthetic unblock applies");

    let suspended = demand_with_offer(db.pool(), author, seller).await;
    sqlx::query("UPDATE users SET state = 'suspended' WHERE id = $1")
        .bind(seller)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    assert_eq!(
        start_handoff(db.pool(), author, suspended.0, suspended.1).await,
        Err(ContactError::ForbiddenState)
    );
    assert_eq!(contact_rows(db.pool(), suspended.1).await, 0);
    sqlx::query("UPDATE users SET state = 'active' WHERE id = $1")
        .bind(seller)
        .execute(db.pool())
        .await
        .expect("synthetic restoration applies");

    let elapsed = demand_with_offer(db.pool(), author, seller).await;
    backdate_cycle(db.pool(), elapsed.0).await;
    assert_eq!(
        start_handoff(db.pool(), author, elapsed.0, elapsed.1).await,
        Err(ContactError::ExpiredRequest)
    );
    assert_eq!(contact_rows(db.pool(), elapsed.1).await, 0);

    let revised = demand_with_offer(db.pool(), author, seller).await;
    revise_material(db.pool(), author, revised.0).await;
    assert_eq!(
        start_handoff(db.pool(), author, revised.0, revised.1).await,
        Err(ContactError::StaleTerms)
    );
    assert_eq!(contact_rows(db.pool(), revised.1).await, 0);

    let elapsed = demand_with_offer(db.pool(), author, seller).await;
    backdate_cycle(db.pool(), elapsed.0).await;
    assert_eq!(
        start_handoff(db.pool(), author, elapsed.0, elapsed.1).await,
        Err(ContactError::ExpiredRequest)
    );
    assert_eq!(contact_rows(db.pool(), elapsed.1).await, 0);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn contact_first_preserves_valid_initiation() {
    let db = TestDatabase::create("p08t06_contactwins")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p08t06_contactwins_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Winner Owner", "+55 11 90000-0403").await;
    let seller = seed_active(db.pool(), "Winner Seller", "+55 11 90000-0404").await;
    let (request_id, offer_id) = demand_with_offer(db.pool(), author, seller).await;

    // Declared order two: the handoff commits first, so the row stands with
    // its frozen snapshot; the later withdrawal ends the offer without
    // recalling history or touching the contact.
    let handoff = start_handoff(db.pool(), author, request_id, offer_id)
        .await
        .expect("handoff starts");
    assert!(!handoff.repeat);
    withdraw_offer(db.pool(), seller, offer_id, WithdrawReason::Withdrawn)
        .await
        .expect("later withdrawal withdraws");
    assert_eq!(contact_rows(db.pool(), offer_id).await, 1);
    let stored = contacts_for_offer(db.pool(), offer_id)
        .await
        .expect("contacts read")[0]
        .clone();
    assert_eq!(stored.id, handoff.contact_id);
    assert_eq!(stored.offer_price_cents, 52_000);
    let offer: (String,) = sqlx::query_as("SELECT state FROM offers WHERE id = $1")
        .bind(offer_id)
        .fetch_one(db.pool())
        .await
        .expect("offer reads");
    assert_eq!(offer.0, "withdrawn");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn open_withdrawal_contact_race_converges() {
    let db = TestDatabase::create("p08t06_openrace")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p08t06_openrace_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Open Owner", "+55 11 90000-0405").await;
    let seller = seed_active(db.pool(), "Open Seller", "+55 11 90000-0406").await;
    let (request_id, offer_id) = demand_with_offer(db.pool(), author, seller).await;

    // True concurrency with no ordering tricks: whichever path wins, the
    // end state is exact — a standing contact with the pre-race snapshot
    // plus a withdrawn offer, or a clean refusal with nothing recorded.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let withdraw = |barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            withdraw_offer(&pool, seller, offer_id, WithdrawReason::Withdrawn).await
        }
    };
    let handoff = |barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            start_handoff(&pool, author, request_id, offer_id).await
        }
    };
    let (withdrawn, started) =
        tokio::join!(withdraw(std::sync::Arc::clone(&barrier)), handoff(barrier),);
    match (withdrawn, started) {
        (Ok(_), Ok(handoff)) => {
            assert!(!handoff.repeat);
            assert_eq!(contact_rows(db.pool(), offer_id).await, 1);
            let stored = contacts_for_offer(db.pool(), offer_id)
                .await
                .expect("contacts read")[0]
                .clone();
            assert_eq!(stored.id, handoff.contact_id);
            assert_eq!(stored.offer_price_cents, 52_000);
        }
        (Ok(_), Err(ContactError::ForbiddenState)) => {
            assert_eq!(contact_rows(db.pool(), offer_id).await, 0);
        }
        (left, right) => panic!("unexpected race outcome: {left:?} / {right:?}"),
    }
    // Immutable terms and single-submission facts hold in every branch.
    let terms: i64 = sqlx::query_scalar("SELECT count(*) FROM offer_terms WHERE offer_id = $1")
        .bind(offer_id)
        .fetch_one(db.pool())
        .await
        .expect("terms read");
    assert_eq!(terms, 1);
    let submitted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'offer' AND resource_id = $1 AND kind = 'offer.submitted'",
    )
    .bind(offer_id)
    .fetch_one(db.pool())
    .await
    .expect("events read");
    assert_eq!(submitted, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn stale_deadline_and_retry_policy_have_defined_errors() {
    let db = TestDatabase::create("p08t06_policy")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p08t06_policy_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Policy Owner", "+55 11 90000-0407").await;
    let seller = seed_active(db.pool(), "Policy Seller", "+55 11 90000-0408").await;
    let (request_id, _) = demand_with_offer(db.pool(), author, seller).await;

    // Deadline equality is already expired: the check at the exact instant
    // refuses without depending on any background job.
    let base = chrono::DateTime::from_timestamp(chrono::Utc::now().timestamp(), 0)
        .expect("whole-second instant builds");
    let deadline = base + chrono::Duration::days(7);
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(request_id)
    .bind(base)
    .bind(deadline)
    .execute(db.pool())
    .await
    .expect("exact deadline stages");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let verdict = check_request_actionable(&mut tx, request_id, deadline).await;
    tx.rollback().await.expect("probe rolls back");
    assert!(matches!(
        verdict,
        Err(AttemptError::Abort(
            procurali_backend::application::request_eligibility::RequestEligibilityError::Expired
        ))
    ));

    // The retry runner never retries business refusals and never masks
    // storage failures: both surface once, with their defined errors.
    let attempts = Arc::new(AtomicU32::new(0));
    let runner_attempts = Arc::clone(&attempts);
    let refused = run_serializable(db.pool(), |_, attempt| {
        runner_attempts.fetch_max(attempt, Ordering::SeqCst);
        Box::pin(async move {
            Err::<(), AttemptError<ContactError>>(AttemptError::Abort(ContactError::ForbiddenState))
        })
    })
    .await;
    assert!(matches!(
        refused,
        Err(TransactionError::Aborted(ContactError::ForbiddenState))
    ));
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
    let attempts = Arc::new(AtomicU32::new(0));
    let runner_attempts = Arc::clone(&attempts);
    let broken = run_serializable(db.pool(), |tx, attempt| {
        runner_attempts.fetch_max(attempt, Ordering::SeqCst);
        Box::pin(async move {
            sqlx::query("SELECT 1 FROM no_such_relation_xyz")
                .fetch_one(&mut **tx)
                .await
                .map_err(AttemptError::Db)?;
            Ok::<_, AttemptError<ContactError>>(())
        })
    })
    .await;
    assert!(matches!(broken, Err(TransactionError::StorageFailed)));
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
    db.cleanup().await.expect("suite cleans up");
}

/// One demand plus its offer for schedule fixtures.
async fn demand_with_offer(
    pool: &sqlx::PgPool,
    author: uuid::Uuid,
    seller: uuid::Uuid,
) -> (uuid::Uuid, uuid::Uuid) {
    let request_id = publish_open(pool, author, "Refrigerator").await;
    let offer_id = submit_open(pool, seller, request_id).await;
    (request_id, offer_id)
}

/// Move one cycle into the past, honoring the cycle CHECK.
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

/// One material revision through the real operation.
async fn revise_material(pool: &sqlx::PgPool, author: uuid::Uuid, request_id: uuid::Uuid) {
    procurali_backend::application::revise_request::revise_request(
        pool,
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn block_winning_contact_race_yields_no_destination() {
    let db = TestDatabase::create("p10t06_blockrace")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p10t06_blockrace_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Race Owner", "+55 11 90000-0701").await;
    let seller = seed_active(db.pool(), "Race Seller", "+55 11 90000-0702").await;
    let (request_id, offer_id) = demand_with_offer(db.pool(), author, seller).await;

    // Blocking races with contact initiation through the P10 writers: the
    // barrier releases both at once, and the end state is exact in every
    // branch. A block-winning race records no contact and no destination;
    // a contact-winning race keeps exactly one unique initiation before
    // the block invalidates its offer.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let block = |barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            block_user(&pool, author, seller).await
        }
    };
    let handoff = |barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            start_contact(
                &pool,
                &test_keys(),
                author,
                request_id,
                offer_id,
                ContactInput {
                    handoff_id: Some(uuid::Uuid::now_v7().to_string()),
                    expected_offer_terms: Some(1),
                    entry_source: Some("offer_detail".to_owned()),
                },
            )
            .await
        }
    };
    let (blocked, started) =
        tokio::join!(block(std::sync::Arc::clone(&barrier)), handoff(barrier),);
    match (blocked, started) {
        (Ok(_), Ok(handoff)) => {
            assert!(!handoff.repeat);
            assert_eq!(contact_rows(db.pool(), offer_id).await, 1);
            let stored = contacts_for_offer(db.pool(), offer_id)
                .await
                .expect("contacts read")[0]
                .clone();
            assert_eq!(stored.id, handoff.contact_id);
            assert_eq!(stored.offer_price_cents, 52_000);
        }
        (Ok(_), Err(ContactError::Blocked | ContactError::ForbiddenState)) => {
            assert_eq!(contact_rows(db.pool(), offer_id).await, 0);
        }
        (left, right) => panic!("unexpected race outcome: {left:?} / {right:?}"),
    }
    // The block row stands in every branch: the restriction is never lost
    // to the race. Refusals carry no destination by construction — the
    // error path holds no handoff value at all — and the offer is terminal
    // afterwards, so no later contact can reopen it.
    let stands: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM user_blocks WHERE blocker_id = $1 AND blocked_id = $2")
            .bind(author)
            .bind(seller)
            .fetch_optional(db.pool())
            .await
            .expect("block reads");
    assert!(stands.is_some());
    let state: String = sqlx::query_scalar("SELECT state FROM offers WHERE id = $1")
        .bind(offer_id)
        .fetch_one(db.pool())
        .await
        .expect("offer reads");
    assert_eq!(state, "invalidated");
    db.cleanup().await.expect("suite cleans up");
}
