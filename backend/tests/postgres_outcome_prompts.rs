//! Outcome-prompt acceptance (P09-T02): ask once, mutate nothing.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL:
//! - no prompt exists before its delay, and exactly one stands at it;
//! - repeated contacts and repeated runs converge on that single prompt;
//! - answering Not yet and processing expired or terminal demand changes
//!   no deadline, cycle, or lifecycle.
//!
//! Contacts, publications, and submissions travel the real operations;
//! time movement uses synthetic fixtures instead of real waits. All
//! phones, names, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::close_request::{close_request, BuyerOutcome, OutcomeSource};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::record_outcome::{record_outcome, OutcomeAnswer};
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::start_contact::{start_contact, ContactInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::operations::jobs::outcome_prompts::{
    process_prompt, schedule_contact_prompt, schedule_expiry_prompt, PromptOutcome,
};
use procurali_backend::operations::worker::{claim_jobs, complete_job};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p09t02-test-only-lookup-key",
        encryption_key: "p09t02-test-only-encryption-key",
    }
}

const LEASE: std::time::Duration = std::time::Duration::from_secs(300);

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

async fn start_open(
    pool: &sqlx::PgPool,
    buyer: uuid::Uuid,
    request_id: uuid::Uuid,
    offer_id: uuid::Uuid,
) {
    start_contact(
        pool,
        &test_keys(),
        buyer,
        request_id,
        offer_id,
        ContactInput {
            handoff_id: Some(uuid::Uuid::now_v7().to_string()),
            expected_offer_terms: Some(1),
            entry_source: Some("offer_detail".to_owned()),
        },
    )
    .await
    .expect("fixture handoff starts");
}

async fn prompt_notices(pool: &sqlx::PgPool, request_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM notices
         WHERE resource_kind = 'request' AND resource_id = $1 AND kind = 'outcome.prompt'",
    )
    .bind(request_id)
    .fetch_one(pool)
    .await
    .expect("notices read")
}

async fn prompt_jobs(pool: &sqlx::PgPool, request_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM background_jobs
         WHERE kind = 'outcome.prompt' AND payload->>'request_id' = $1",
    )
    .bind(request_id.to_string())
    .fetch_one(pool)
    .await
    .expect("jobs read")
}

#[tokio::test]
async fn prompt_before_delay_absent_at_delay_present_once() {
    let db = TestDatabase::create("p09t02_timing")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p09t02_timing_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Timing Owner", "+55 11 90000-0501").await;
    let seller = seed_active(db.pool(), "Timing Seller", "+55 11 90000-0502").await;
    let request_id = publish_open(db.pool(), author, "Refrigerator").await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;
    start_open(db.pool(), author, request_id, offer_id).await;

    // Scheduling twice yields one pending job due a full day after first
    // contact; polling now finds nothing due.
    let first = schedule_contact_prompt(db.pool(), request_id, 1)
        .await
        .expect("prompt schedules");
    let scheduled = first.expect("contact exists to ask about");
    let second = schedule_contact_prompt(db.pool(), request_id, 1)
        .await
        .expect("reschedule probes");
    assert_eq!(second.map(|job| job.id), Some(scheduled.id));
    assert_eq!(prompt_jobs(db.pool(), request_id).await, 1);
    let initiated: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT min(initiated_at) FROM contacts WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(request_id)
    .fetch_one(db.pool())
    .await
    .expect("contact time reads");
    assert_eq!(
        scheduled.not_before,
        initiated + chrono::Duration::hours(24)
    );
    assert!(claim_jobs(
        db.pool(),
        "worker-a",
        Some("outcome.prompt"),
        10,
        LEASE,
        chrono::Utc::now()
    )
    .await
    .expect("early poll responds")
    .is_empty());
    assert_eq!(prompt_notices(db.pool(), request_id).await, 0);

    // Past the delay the claim lands and processing records exactly one
    // prompt; a second scheduled run observes it and stands down.
    sqlx::query("UPDATE background_jobs SET not_before = $2 WHERE id = $1")
        .bind(scheduled.id)
        .bind(chrono::Utc::now() - chrono::Duration::seconds(1))
        .execute(db.pool())
        .await
        .expect("synthetic delay elapses");
    let claimed = claim_jobs(
        db.pool(),
        "worker-a",
        Some("outcome.prompt"),
        10,
        LEASE,
        chrono::Utc::now(),
    )
    .await
    .expect("due poll responds");
    assert_eq!(claimed.len(), 1);
    assert_eq!(
        process_prompt(db.pool(), &claimed[0]).await,
        Ok(PromptOutcome::Recorded)
    );
    complete_job(db.pool(), claimed[0].id, "worker-a", chrono::Utc::now())
        .await
        .expect("run completes");
    assert_eq!(prompt_notices(db.pool(), request_id).await, 1);
    let rerun = schedule_contact_prompt(db.pool(), request_id, 1)
        .await
        .expect("reschedule probes");
    let rerun = rerun.expect("a fresh job may schedule");
    sqlx::query("UPDATE background_jobs SET not_before = $2 WHERE id = $1")
        .bind(rerun.id)
        .bind(chrono::Utc::now() - chrono::Duration::seconds(1))
        .execute(db.pool())
        .await
        .expect("synthetic delay elapses");
    let claimed = claim_jobs(
        db.pool(),
        "worker-b",
        Some("outcome.prompt"),
        10,
        LEASE,
        chrono::Utc::now(),
    )
    .await
    .expect("due poll responds");
    assert_eq!(claimed.len(), 1);
    assert_eq!(
        process_prompt(db.pool(), &claimed[0]).await,
        Ok(PromptOutcome::AlreadyPrompted)
    );
    assert_eq!(prompt_notices(db.pool(), request_id).await, 1);
    // Prompting mutates nothing else: the demand still runs its cycle.
    let row: (String, i32) =
        sqlx::query_as("SELECT state, current_cycle_number FROM requests WHERE id = $1")
            .bind(request_id)
            .fetch_one(db.pool())
            .await
            .expect("row reads");
    assert_eq!(row, ("active".to_owned(), 1));
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn repeated_contacts_generate_no_repeated_prompts() {
    let db = TestDatabase::create("p09t02_repeats")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p09t02_repeats_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Repeat Owner", "+55 11 90000-0503").await;
    let seller = seed_active(db.pool(), "Repeat Seller", "+55 11 90000-0504").await;
    let request_id = publish_open(db.pool(), author, "Refrigerator").await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;
    start_open(db.pool(), author, request_id, offer_id).await;
    start_open(db.pool(), author, request_id, offer_id).await;

    // Two contacts, two schedules, one processing wave: a single prompt.
    let first = schedule_contact_prompt(db.pool(), request_id, 1)
        .await
        .expect("first schedule probes");
    assert!(first.is_some());
    let second = schedule_contact_prompt(db.pool(), request_id, 1)
        .await
        .expect("second schedule probes");
    assert_eq!(second.map(|job| job.id), first.map(|job| job.id));
    assert_eq!(prompt_jobs(db.pool(), request_id).await, 1);
    sqlx::query("UPDATE background_jobs SET not_before = $1 WHERE kind = 'outcome.prompt'")
        .bind(chrono::Utc::now() - chrono::Duration::seconds(1))
        .execute(db.pool())
        .await
        .expect("synthetic delay elapses");
    let claimed = claim_jobs(
        db.pool(),
        "worker-a",
        Some("outcome.prompt"),
        10,
        LEASE,
        chrono::Utc::now(),
    )
    .await
    .expect("due poll responds");
    assert_eq!(claimed.len(), 1);
    assert_eq!(
        process_prompt(db.pool(), &claimed[0]).await,
        Ok(PromptOutcome::Recorded)
    );
    assert_eq!(prompt_notices(db.pool(), request_id).await, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn not_yet_keeps_deadline_and_expired_stays_inactive() {
    let db = TestDatabase::create("p09t02_stillness")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p09t02_stillness_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Still Owner", "+55 11 90000-0505").await;
    let request_id = publish_open(db.pool(), author, "Refrigerator").await;
    let deadline: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT deadline FROM request_cycles WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(request_id)
    .fetch_one(db.pool())
    .await
    .expect("deadline reads");

    // Answering Not yet records the unresolved outcome with the deadline,
    // cycle, and lifecycle exactly as they were.
    record_outcome(db.pool(), author, request_id, OutcomeAnswer::Unresolved)
        .await
        .expect("unresolved records");
    let row: (String, i32, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        "SELECT state, current_cycle_number, deadline FROM requests
         JOIN request_cycles ON request_cycles.request_id = requests.id
          AND request_cycles.cycle_number = requests.current_cycle_number
         WHERE requests.id = $1",
    )
    .bind(request_id)
    .fetch_one(db.pool())
    .await
    .expect("row reads");
    assert_eq!(row.0, "active");
    assert_eq!(row.1, 1);
    assert_eq!(row.2, deadline);

    // An expired demand still takes its expiry prompt without waking up:
    // notice recorded, deadline and inactivity untouched.
    let lapsed = publish_open(db.pool(), author, "Lapsed Fridge").await;
    let now = chrono::Utc::now();
    sqlx::query(
        "UPDATE request_cycles SET started_at = $2, deadline = $3
         WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(lapsed)
    .bind(now - chrono::Duration::days(8))
    .bind(now - chrono::Duration::days(1))
    .execute(db.pool())
    .await
    .expect("synthetic expiry applies");
    let past_deadline = now - chrono::Duration::days(1);
    let job = schedule_expiry_prompt(db.pool(), lapsed, 1, past_deadline)
        .await
        .expect("expiry prompt schedules");
    assert!(job.is_some());
    let claimed = claim_jobs(
        db.pool(),
        "worker-a",
        Some("outcome.prompt"),
        10,
        LEASE,
        now,
    )
    .await
    .expect("due poll responds");
    assert_eq!(claimed.len(), 1);
    assert_eq!(
        process_prompt(db.pool(), &claimed[0]).await,
        Ok(PromptOutcome::Recorded)
    );
    complete_job(db.pool(), claimed[0].id, "worker-a", now)
        .await
        .expect("run completes");
    assert_eq!(prompt_notices(db.pool(), lapsed).await, 1);
    let row: (String, i32, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        "SELECT state, current_cycle_number, deadline FROM requests
         JOIN request_cycles ON request_cycles.request_id = requests.id
          AND request_cycles.cycle_number = requests.current_cycle_number
         WHERE requests.id = $1",
    )
    .bind(lapsed)
    .fetch_one(db.pool())
    .await
    .expect("row reads");
    assert_eq!(row.0, "active", "prompting never transitions lifecycle");
    assert_eq!(row.1, 1, "prompting never opens cycles");
    // Microsecond database precision versus nanosecond memory values.
    assert_eq!(row.2.timestamp_micros(), past_deadline.timestamp_micros());

    // Terminal demands stop prompting entirely.
    close_request(
        db.pool(),
        author,
        lapsed,
        BuyerOutcome::Found(OutcomeSource::Elsewhere),
    )
    .await
    .expect("closure closes");
    let job = schedule_expiry_prompt(db.pool(), lapsed, 1, past_deadline)
        .await
        .expect("terminal schedule probes");
    assert!(job.is_some(), "scheduling stays permissive");
    let claimed = claim_jobs(
        db.pool(),
        "worker-b",
        Some("outcome.prompt"),
        10,
        LEASE,
        now,
    )
    .await
    .expect("due poll responds");
    assert_eq!(claimed.len(), 1);
    assert_eq!(
        process_prompt(db.pool(), &claimed[0]).await,
        Ok(PromptOutcome::SkippedTerminal)
    );
    assert_eq!(prompt_notices(db.pool(), lapsed).await, 1);
    db.cleanup().await.expect("suite cleans up");
}
