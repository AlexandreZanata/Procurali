//! Offer-quota acceptance (P07-T03): daily allowance and slot permanence.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL:
//! - the eleventh successful submission in the rolling window is denied
//!   while failed submissions consume nothing;
//! - concurrent final-quota contenders cannot exceed ten;
//! - withdrawn and rejected slots stay taken, and ordinary term edits move
//!   no counter.
//!
//! Successful setup submissions travel the real submission operation;
//! quota attempts compose the transactional guard with submission
//! mechanics until the wiring card calls the guard inside submission
//! itself. Quota tables do not exist by design: the allowance derives from
//! immutable facts, so nothing can reset it. All phones, names, and codes
//! below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::offer_limits::{
    check_submission_limits, submission_count, OfferLimitError,
};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::offers::{insert_terms, terms_for_offer, update_current_terms};
use procurali_backend::persistence::transaction::{run_serializable, AttemptError};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p07t03-test-only-lookup-key",
        encryption_key: "p07t03-test-only-encryption-key",
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
/// verification flow) is staged, because submission requires eligibility.
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

/// One genuinely open request through the real draft and publication paths.
/// Publication carries no quota wiring yet, so fixtures may exceed nominal
/// allowances; the guard under test is the offer allowance, not activation.
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

/// One successful submission through the real operation.
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

/// Guard verdict without mutating: check inside a transaction, roll back.
async fn guard(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
    cycle_number: i32,
    seller_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), OfferLimitError> {
    let mut tx = pool.begin().await.expect("transaction begins");
    let verdict = check_submission_limits(&mut tx, request_id, cycle_number, seller_id, now).await;
    tx.rollback().await.expect("probe rolls back");
    match verdict {
        Ok(()) => Ok(()),
        Err(AttemptError::Abort(reason)) => Err(reason),
        Err(AttemptError::Db(_)) => panic!("probe storage failed"),
    }
}

/// Successful-submission count inside one probe transaction.
async fn counted(
    pool: &sqlx::PgPool,
    seller_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> u32 {
    let mut tx = pool.begin().await.expect("transaction begins");
    let count = submission_count(&mut tx, seller_id, now).await;
    tx.rollback().await.expect("probe rolls back");
    match count {
        Ok(count) => count,
        Err(AttemptError::Abort(_)) => panic!("probe refusal impossible"),
        Err(AttemptError::Db(_)) => panic!("probe storage failed"),
    }
}

/// Submission mechanics for guarded compositions: a genuine slot row, its
/// first terms, and its counted fact, written with direct SQL so the
/// original `sqlx::Error` reaches the serializable runner — persistence
/// writers erase it into flat errors, which would turn a retriable
/// statement-level conflict into a wrongful terminal refusal. Values are
/// known-valid fixtures (the writers' own validation is proven by their
/// suites); this helper stands in until the wiring card calls the guard
/// inside submission itself.
async fn submit_live_offer(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    seller: uuid::Uuid,
    request_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<uuid::Uuid, AttemptError<OfferLimitError>> {
    let id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO offers
            (id, request_id, cycle_number, revision_number, seller_id,
             description, price_cents, \"condition\", city_code, region_code,
             notes, current_terms_number)
         VALUES ($1, $2, 1, 1, $3, 'Frost-free 300L', 52000, 'used',
                 'campinas', 'centro', '', 1)",
    )
    .bind(id)
    .bind(request_id)
    .bind(seller)
    .execute(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    sqlx::query(
        "INSERT INTO offer_terms
            (offer_id, terms_number, description, price_cents, \"condition\",
             city_code, region_code, notes)
         VALUES ($1, 1, 'Frost-free 300L', 52000, 'used', 'campinas',
                 'centro', '')",
    )
    .bind(id)
    .execute(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    sqlx::query(
        "INSERT INTO business_events
            (actor_id, resource_kind, resource_id, cycle, effective_at, kind,
             policy, source, payload)
         VALUES ($1, 'offer', $2, 1, $3, 'offer.submitted', 'mvp-free', 'api',
                 '{\"state\": \"sent\"}')",
    )
    .bind(seller)
    .bind(id)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    Ok(id)
}

#[tokio::test]
async fn eleventh_successful_submission_is_denied() {
    let db = TestDatabase::create("p07t03_quota")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t03_quota_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Quota Owner", "+55 11 90000-0131").await;
    let seller = seed_active(db.pool(), "Quota Seller", "+55 11 90000-0132").await;
    let now = chrono::Utc::now();
    for index in 0..10 {
        let request_id = publish_open(db.pool(), author, &format!("Quota Demand {index}")).await;
        submit_open(db.pool(), seller, request_id).await;
    }
    assert_eq!(counted(db.pool(), seller, now).await, 10);

    // Failed submissions consume nothing: over-budget and malformed terms
    // refuse through the real operation with the count untouched.
    let extra = publish_open(db.pool(), author, "Quota Extra").await;
    assert!(submit_offer(
        db.pool(),
        seller,
        extra,
        OfferInput {
            price: Some("600.01".to_owned()),
            ..offer_input()
        }
    )
    .await
    .is_err());
    assert!(submit_offer(
        db.pool(),
        seller,
        extra,
        OfferInput {
            condition: Some("refurbished".to_owned()),
            ..offer_input()
        }
    )
    .await
    .is_err());
    assert_eq!(counted(db.pool(), seller, now).await, 10);

    // The eleventh success is denied with no new slot row.
    assert_eq!(
        guard(db.pool(), extra, 1, seller, now).await,
        Err(OfferLimitError::QuotaExhausted)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM offers WHERE request_id = $1")
            .bind(extra)
            .fetch_one(db.pool())
            .await
            .expect("offers read"),
        0
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_final_quota_contenders_cannot_exceed() {
    let db = TestDatabase::create("p07t03_race")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t03_race_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Race Owner", "+55 11 90000-0133").await;
    let seller = seed_active(db.pool(), "Race Seller", "+55 11 90000-0134").await;
    for index in 0..9 {
        let request_id = publish_open(db.pool(), author, &format!("Race Demand {index}")).await;
        submit_open(db.pool(), seller, request_id).await;
    }
    let first = publish_open(db.pool(), author, "Race Final One").await;
    let second = publish_open(db.pool(), author, "Race Final Two").await;

    // Both contenders observe nine successes and race for the tenth in
    // serializable transactions: one commits, the other retries, re-reads
    // ten, and takes the quota refusal.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let contend = |barrier: std::sync::Arc<tokio::sync::Barrier>, request_id: uuid::Uuid| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            run_serializable(&pool, |tx, _attempt| {
                Box::pin(async move {
                    let now = chrono::Utc::now();
                    check_submission_limits(tx, request_id, 1, seller, now).await?;
                    submit_live_offer(tx, seller, request_id, now).await?;
                    Ok::<_, AttemptError<OfferLimitError>>(())
                })
            })
            .await
        }
    };
    let (first_outcome, second_outcome) = tokio::join!(
        contend(std::sync::Arc::clone(&barrier), first),
        contend(barrier, second),
    );
    let outcomes = [first_outcome, second_outcome];
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
        1,
        "exactly one contender takes the final allowance"
    );
    assert!(
        outcomes.iter().any(|outcome| matches!(
            outcome,
            Err(
                procurali_backend::persistence::transaction::TransactionError::Aborted(
                    OfferLimitError::QuotaExhausted
                )
            )
        )),
        "the loser takes the quota refusal, not a silent success"
    );
    assert_eq!(counted(db.pool(), seller, chrono::Utc::now()).await, 10);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn withdrawn_and_rejected_slots_stay_taken() {
    let db = TestDatabase::create("p07t03_slots")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p07t03_slots_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Slot Owner", "+55 11 90000-0135").await;
    let seller = seed_active(db.pool(), "Slot Seller", "+55 11 90000-0136").await;
    let now = chrono::Utc::now();

    // A withdrawn slot stays taken: the guard names it, and a direct
    // resubmission conflicts on the durable key with no new row.
    let withdrawn = publish_open(db.pool(), author, "Withdrawn Demand").await;
    submit_open(db.pool(), seller, withdrawn).await;
    sqlx::query("UPDATE offers SET state = 'withdrawn' WHERE request_id = $1")
        .bind(withdrawn)
        .execute(db.pool())
        .await
        .expect("synthetic withdrawal applies");
    assert_eq!(
        guard(db.pool(), withdrawn, 1, seller, now).await,
        Err(OfferLimitError::SlotOccupied)
    );
    assert!(submit_offer(db.pool(), seller, withdrawn, offer_input())
        .await
        .is_err());
    // A rejected slot behaves identically.
    let rejected = publish_open(db.pool(), author, "Rejected Demand").await;
    submit_open(db.pool(), seller, rejected).await;
    sqlx::query("UPDATE offers SET state = 'rejected' WHERE request_id = $1")
        .bind(rejected)
        .execute(db.pool())
        .await
        .expect("synthetic rejection applies");
    assert_eq!(
        guard(db.pool(), rejected, 1, seller, now).await,
        Err(OfferLimitError::SlotOccupied)
    );

    // Ordinary term edits move no counter: history grows, facts do not.
    let before = counted(db.pool(), seller, now).await;
    assert_eq!(before, 2);
    let offer: uuid::Uuid = sqlx::query_scalar("SELECT id FROM offers WHERE request_id = $1")
        .bind(withdrawn)
        .fetch_one(db.pool())
        .await
        .expect("offer reads");
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let edited = procurali_backend::persistence::offers::TermsSnapshot {
        description: "Frost-free 350L".to_owned(),
        price_cents: 48_000,
        condition: "used".to_owned(),
        city_code: "campinas".to_owned(),
        region_code: "centro".to_owned(),
        notes: String::new(),
    };
    insert_terms(&mut tx, offer, 2, &edited)
        .await
        .expect("terms store");
    update_current_terms(&mut tx, offer, 2, &edited)
        .await
        .expect("current promotes");
    tx.commit().await.expect("edit commits");
    assert_eq!(counted(db.pool(), seller, now).await, before);
    assert_eq!(
        terms_for_offer(db.pool(), offer)
            .await
            .expect("history reads")
            .len(),
        2
    );
    // A stranger holds no slot anywhere here.
    let stranger = seed_active(db.pool(), "Slot Stranger", "+55 11 90000-0137").await;
    assert!(guard(db.pool(), withdrawn, 1, stranger, now).await.is_ok());
    db.cleanup().await.expect("suite cleans up");
}
