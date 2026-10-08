//! Effective-expiry acceptance (P06-T02): deadline eligibility.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL:
//! - offer/contact eligibility holds strictly before the exclusive deadline
//!   and refuses the exact-deadline instant, whatever the recorded state;
//! - repeated and concurrent expiry transitions record exactly one
//!   `request.expired` fact;
//! - suspension never pauses or extends the deadline, and expiry of a
//!   hidden suspended row keeps it hidden.
//!
//! Setup publishes travel the real draft and publication operations. All
//! phones, names, and codes below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::request_eligibility::{
    check_request_actionable, expire_if_elapsed, RequestEligibilityError,
};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::transaction::{run_serializable, AttemptError};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p06t02-test-only-lookup-key",
        encryption_key: "p06t02-test-only-encryption-key",
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

/// A real account, activated through a synthetic state fixture: identity
/// itself travels the real creation path with real phone cryptography, and
/// only the lifecycle flip (owned by the verification flow) is staged.
async fn seed_author(pool: &sqlx::PgPool, display: &str, phone: &str) -> uuid::Uuid {
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
    .expect("fixture author stores");
    tx.commit().await.expect("author commits");
    sqlx::query("UPDATE users SET state = 'active' WHERE id = $1")
        .bind(user.id)
        .execute(pool)
        .await
        .expect("synthetic activation applies");
    user.id
}

/// One genuinely open request through the real draft and publication paths.
async fn publish_open(pool: &sqlx::PgPool, author: uuid::Uuid) -> uuid::Uuid {
    let draft = create_draft(
        pool,
        author,
        DraftInput {
            title: Some("Refrigerator".to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some("520.00".to_owned()),
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

/// Move one cycle into the past, honoring the cycle CHECK (start and
/// deadline travel together; only the deadline position matters).
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

async fn current_deadline(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
) -> chrono::DateTime<chrono::Utc> {
    sqlx::query_scalar(
        "SELECT deadline FROM request_cycles WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(request_id)
    .fetch_one(pool)
    .await
    .expect("deadline reads")
}

/// Eligibility verdict without mutating: check inside a transaction, roll back.
async fn eligible(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<
    procurali_backend::application::request_eligibility::ActionableRequest,
    RequestEligibilityError,
> {
    let mut tx = pool.begin().await.expect("transaction begins");
    let verdict = check_request_actionable(&mut tx, request_id, now).await;
    tx.rollback().await.expect("probe rolls back");
    match verdict {
        Ok(actionable) => Ok(actionable),
        Err(AttemptError::Abort(reason)) => Err(reason),
        Err(AttemptError::Db(_)) => panic!("probe storage failed"),
    }
}

async fn expired_facts(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
) -> Vec<(String, chrono::DateTime<chrono::Utc>)> {
    sqlx::query_as(
        "SELECT kind, effective_at FROM business_events
         WHERE resource_kind = 'request' AND resource_id = $1 AND kind = 'request.expired'
         ORDER BY occurred_at, id",
    )
    .bind(request_id)
    .fetch_all(pool)
    .await
    .expect("events read")
}

#[tokio::test]
async fn eligibility_denies_the_exact_deadline_despite_recorded_state() {
    let db = TestDatabase::create("p06t02_boundary")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t02_boundary_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Boundary Owner", "+55 11 90000-0061").await;
    let id = publish_open(db.pool(), author).await;

    // Whole-second deadline: strictly before is eligible, the exact instant
    // and everything after are expired — recorded `active` notwithstanding.
    let base = chrono::DateTime::from_timestamp(chrono::Utc::now().timestamp(), 0)
        .expect("whole-second instant builds");
    let deadline = base + chrono::Duration::days(7);
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(id)
    .bind(base)
    .bind(deadline)
    .execute(db.pool())
    .await
    .expect("exact deadline stages");
    let actionable = eligible(db.pool(), id, deadline - chrono::Duration::seconds(1))
        .await
        .expect("before the deadline is eligible");
    assert_eq!(actionable.id, id);
    assert_eq!(actionable.cycle_number, 1);
    assert_eq!(actionable.revision_number, 1);
    assert_eq!(actionable.deadline, deadline);
    assert_eq!(
        eligible(db.pool(), id, deadline).await,
        Err(RequestEligibilityError::Expired),
        "the exact deadline instant is already expired"
    );
    assert_eq!(
        eligible(db.pool(), id, deadline + chrono::Duration::hours(1)).await,
        Err(RequestEligibilityError::Expired)
    );

    // Lifecycle and visibility gate before time: drafts, terminal rows,
    // suspended rows, and hidden rows are refused without reaching the clock.
    let draft = create_draft(
        db.pool(),
        author,
        DraftInput {
            title: Some("Spare Fridge".to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some("100.00".to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
        },
    )
    .await
    .expect("fixture draft validates");
    assert_eq!(
        eligible(db.pool(), draft.id, deadline - chrono::Duration::seconds(1)).await,
        Err(RequestEligibilityError::NotActionable)
    );
    sqlx::query("UPDATE requests SET state = 'suspended' WHERE id = $1")
        .bind(id)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    assert_eq!(
        eligible(db.pool(), id, deadline - chrono::Duration::seconds(1)).await,
        Err(RequestEligibilityError::NotActionable)
    );
    sqlx::query("UPDATE requests SET state = 'completed', visibility = 'public' WHERE id = $1")
        .bind(id)
        .execute(db.pool())
        .await
        .expect("synthetic completion applies");
    assert_eq!(
        eligible(db.pool(), id, deadline - chrono::Duration::seconds(1)).await,
        Err(RequestEligibilityError::NotActionable)
    );
    sqlx::query("UPDATE requests SET state = 'active', visibility = 'hidden' WHERE id = $1")
        .bind(id)
        .execute(db.pool())
        .await
        .expect("synthetic concealment applies");
    assert_eq!(
        eligible(db.pool(), id, deadline - chrono::Duration::seconds(1)).await,
        Err(RequestEligibilityError::NotActionable)
    );
    assert_eq!(
        eligible(db.pool(), uuid::Uuid::now_v7(), deadline).await,
        Err(RequestEligibilityError::NotFound)
    );
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn repeated_expiry_records_exactly_one_fact() {
    let db = TestDatabase::create("p06t02_repeat")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t02_repeat_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Repeat Owner", "+55 11 90000-0062").await;
    let id = publish_open(db.pool(), author).await;
    backdate_cycle(db.pool(), id).await;
    let deadline = current_deadline(db.pool(), id).await;
    let now = chrono::Utc::now();

    // First transition moves state and records the fact effective at the
    // deadline — the instant expiry took effect, not the worker's arrival.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let outcome = expire_if_elapsed(&mut tx, id, now)
        .await
        .expect("expiry transitions");
    tx.commit().await.expect("transition commits");
    assert_eq!(
        outcome,
        procurali_backend::application::request_eligibility::ExpiryOutcome::Transitioned
    );
    let facts = expired_facts(db.pool(), id).await;
    assert_eq!(
        facts,
        [("request.expired".to_owned(), deadline)],
        "one fact effective at the deadline"
    );

    // A repeat settles without writing.
    let mut tx = db.pool().begin().await.expect("transaction begins");
    let outcome = expire_if_elapsed(&mut tx, id, now)
        .await
        .expect("repeat settles");
    tx.commit().await.expect("probe commits");
    assert_eq!(
        outcome,
        procurali_backend::application::request_eligibility::ExpiryOutcome::AlreadySettled
    );
    assert_eq!(expired_facts(db.pool(), id).await.len(), 1);
    let live = publish_open(db.pool(), author).await;
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert_eq!(
        expire_if_elapsed(&mut tx, live, now)
            .await
            .expect("live row is not due"),
        procurali_backend::application::request_eligibility::ExpiryOutcome::NotDue
    );
    tx.rollback().await.expect("probe rolls back");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_expiry_racers_record_one_fact() {
    let db = TestDatabase::create("p06t02_race")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t02_race_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Expiry Race Owner", "+55 11 90000-0063").await;
    let id = publish_open(db.pool(), author).await;
    backdate_cycle(db.pool(), id).await;

    // Two workers race the same elapsed row in serializable transactions:
    // the row lock serializes them, so one transitions and the other
    // observes the settled row — exactly one fact either way.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let expire = |barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            run_serializable(&pool, |tx, _attempt| {
                Box::pin(async move { expire_if_elapsed(tx, id, chrono::Utc::now()).await })
            })
            .await
        }
    };
    let (first, second) = tokio::join!(expire(std::sync::Arc::clone(&barrier)), expire(barrier),);
    let mut outcomes = [first, second]
        .into_iter()
        .map(|outcome| {
            outcome.unwrap_or_else(|error| panic!("expiry failed unexpectedly: {error:?}"))
        })
        .collect::<Vec<_>>();
    outcomes.sort_by_key(|outcome| *outcome as u8);
    assert_eq!(
        outcomes,
        [
            procurali_backend::application::request_eligibility::ExpiryOutcome::Transitioned,
            procurali_backend::application::request_eligibility::ExpiryOutcome::AlreadySettled,
        ]
    );
    assert_eq!(expired_facts(db.pool(), id).await.len(), 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn suspension_never_pauses_or_extends_the_deadline() {
    let db = TestDatabase::create("p06t02_suspend")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t02_suspend_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Suspend Owner", "+55 11 90000-0064").await;
    let id = publish_open(db.pool(), author).await;
    let deadline = current_deadline(db.pool(), id).await;

    // Suspension changes state only: the deadline stands exactly where the
    // cycle set it, and eligibility already refuses the row.
    sqlx::query("UPDATE requests SET state = 'suspended' WHERE id = $1")
        .bind(id)
        .execute(db.pool())
        .await
        .expect("synthetic suspension applies");
    assert_eq!(current_deadline(db.pool(), id).await, deadline);
    assert_eq!(
        eligible(db.pool(), id, deadline - chrono::Duration::seconds(1)).await,
        Err(RequestEligibilityError::NotActionable)
    );

    // Past the deadline, the suspended row is expired business-wise, and the
    // transition keeps it hidden: restriction first, timing untouched.
    sqlx::query("UPDATE requests SET visibility = 'hidden' WHERE id = $1")
        .bind(id)
        .execute(db.pool())
        .await
        .expect("synthetic concealment applies");
    backdate_cycle(db.pool(), id).await;
    assert_eq!(
        eligible(db.pool(), id, chrono::Utc::now()).await,
        Err(RequestEligibilityError::NotActionable),
        "restriction outranks elapsed time in the reported refusal"
    );
    let mut tx = db.pool().begin().await.expect("transaction begins");
    assert_eq!(
        expire_if_elapsed(&mut tx, id, chrono::Utc::now())
            .await
            .expect("hidden suspended row expires"),
        procurali_backend::application::request_eligibility::ExpiryOutcome::Transitioned
    );
    tx.commit().await.expect("transition commits");
    let row: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM requests WHERE id = $1")
            .bind(id)
            .fetch_one(db.pool())
            .await
            .expect("row reads");
    assert_eq!(row, ("expired".to_owned(), "hidden".to_owned()));
    assert_eq!(expired_facts(db.pool(), id).await.len(), 1);
    db.cleanup().await.expect("suite cleans up");
}
