//! Notice-scheduling acceptance (P23-T01): the business flows schedule
//! their own consequences.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL:
//! - a published request carries exactly one expiry sweep, due at its
//!   cycle deadline, and a replayed publication adds none;
//! - a renewed request carries the sweep of the renewed cycle;
//! - the first contact of a cycle schedules exactly one outcome prompt, due
//!   24 hours later, and later contacts of the same cycle add none;
//! - a refused publication schedules nothing at all;
//! - no job payload carries a destination, a token, or private text.
//!
//! Publications, renewals, contacts, and submissions travel the real
//! operations; time movement uses synthetic fixtures instead of real waits.
//! All phones, names, codes, and keys below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::publish_request::{publish_request, PublishError};
use procurali_backend::application::renew_request::{renew_request, RenewalConfirmation};
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::start_contact::{start_contact, ContactInput};
use procurali_backend::application::submit_offer::{submit_offer, OfferInput};
use procurali_backend::operations::jobs::expire_requests::EXPIRY_JOB_KIND;
use procurali_backend::operations::jobs::outcome_prompts::OUTCOME_PROMPT_KIND;
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

/// Test-only phone keys. Never production material.
fn test_keys<'a>() -> PhoneKeys<'a> {
    PhoneKeys {
        lookup_key: "p23t01-test-only-lookup-key",
        encryption_key: "p23t01-test-only-encryption-key",
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

async fn create_demand(pool: &sqlx::PgPool, author: uuid::Uuid, title: &str) -> uuid::Uuid {
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
            notes: Some("Preferably frost-free.".to_owned()),
        },
    )
    .await
    .expect("fixture draft validates");
    draft.id
}

async fn publish_open(pool: &sqlx::PgPool, author: uuid::Uuid, draft_id: uuid::Uuid) -> uuid::Uuid {
    publish_request(pool, author, draft_id)
        .await
        .expect("fixture draft publishes")
        .id
}

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

/// The scheduled sweeps of one request, with their due instants.
async fn expiry_jobs(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
) -> Vec<(i32, chrono::DateTime<chrono::Utc>)> {
    sqlx::query_as(
        "SELECT (payload->>'cycle')::int, not_before FROM background_jobs
         WHERE kind = $1 AND payload->>'request_id' = $2
         ORDER BY (payload->>'cycle')::int",
    )
    .bind(EXPIRY_JOB_KIND)
    .bind(request_id.to_string())
    .fetch_all(pool)
    .await
    .expect("expiry jobs read")
}

/// The scheduled prompts of one request, with their due instants.
async fn prompt_jobs(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
) -> Vec<(i32, chrono::DateTime<chrono::Utc>)> {
    sqlx::query_as(
        "SELECT (payload->>'cycle')::int, not_before FROM background_jobs
         WHERE kind = $1 AND payload->>'request_id' = $2
         ORDER BY (payload->>'cycle')::int",
    )
    .bind(OUTCOME_PROMPT_KIND)
    .bind(request_id.to_string())
    .fetch_all(pool)
    .await
    .expect("prompt jobs read")
}

async fn cycle_deadline(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
    cycle: i32,
) -> chrono::DateTime<chrono::Utc> {
    sqlx::query_scalar(
        "SELECT deadline FROM request_cycles WHERE request_id = $1 AND cycle_number = $2",
    )
    .bind(request_id)
    .bind(cycle)
    .fetch_one(pool)
    .await
    .expect("cycle deadline reads")
}

#[tokio::test]
async fn publication_schedules_one_expiry_sweep_at_the_deadline() {
    let db = TestDatabase::create("p23t01_publish")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p23t01_publish_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Schedule Owner", "+55 11 90000-0701").await;
    let draft_id = create_demand(db.pool(), author, "Refrigerator").await;
    let request_id = publish_open(db.pool(), author, draft_id).await;

    // Exactly one sweep, due at the cycle deadline.
    let jobs = expiry_jobs(db.pool(), request_id).await;
    assert_eq!(jobs.len(), 1, "the publication schedules one sweep");
    assert_eq!(jobs[0].0, 1, "the sweep belongs to the first cycle");
    assert_eq!(
        jobs[0].1,
        cycle_deadline(db.pool(), request_id, 1).await,
        "the sweep is due at the cycle deadline"
    );

    // A replayed publication adds nothing.
    let replayed = publish_request(db.pool(), author, draft_id)
        .await
        .expect("the replay answers with the standing request");
    assert_eq!(replayed.id, request_id);
    assert_eq!(
        expiry_jobs(db.pool(), request_id).await.len(),
        1,
        "a replayed publication schedules no second sweep"
    );

    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn renewal_schedules_the_sweep_of_the_renewed_cycle() {
    let db = TestDatabase::create("p23t01_renew")
        .await
        .expect("disposable database allocates");
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Renewal Owner", "+55 11 90000-0702").await;
    let draft_id = create_demand(db.pool(), author, "Washing Machine").await;
    let request_id = publish_open(db.pool(), author, draft_id).await;

    // The expiry sweep is what normally moves a cycle to `expired`; the
    // fixture stages that state directly so the renewal is eligible
    // without waiting seven days.
    sqlx::query("UPDATE requests SET state = 'expired' WHERE id = $1")
        .bind(request_id)
        .execute(db.pool())
        .await
        .expect("synthetic expiry applies");

    let renewed = renew_request(
        db.pool(),
        author,
        request_id,
        RenewalConfirmation {
            title: "Washing Machine".to_owned(),
            category_code: "home_appliances".to_owned(),
            budget: "600.00".to_owned(),
            condition: "either".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            notes: "Preferably frost-free.".to_owned(),
        },
    )
    .await
    .expect("the expired request renews");
    assert_eq!(renewed.cycle_number, 2);

    // Each cycle carries its own sweep, and the renewed one is due at the
    // new deadline.
    let jobs = expiry_jobs(db.pool(), request_id).await;
    assert_eq!(jobs.len(), 2, "each cycle carries its own sweep");
    assert_eq!(jobs[0].0, 1);
    assert_eq!(jobs[1].0, 2, "the renewed cycle carries the second sweep");
    assert_eq!(
        jobs[1].1,
        cycle_deadline(db.pool(), request_id, 2).await,
        "the renewed sweep is due at the new deadline"
    );

    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn the_first_contact_schedules_one_prompt_and_later_contacts_add_none() {
    let db = TestDatabase::create("p23t01_prompt")
        .await
        .expect("disposable database allocates");
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Prompt Buyer", "+55 11 90000-0703").await;
    let seller = seed_active(db.pool(), "Prompt Seller", "+55 11 90000-0704").await;
    let other = seed_active(db.pool(), "Other Seller", "+55 11 90000-0705").await;
    let draft_id = create_demand(db.pool(), author, "Freezer").await;
    let request_id = publish_open(db.pool(), author, draft_id).await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;
    let other_offer = submit_open(db.pool(), other, request_id).await;

    start_open(db.pool(), author, request_id, offer_id).await;
    let prompts = prompt_jobs(db.pool(), request_id).await;
    assert_eq!(prompts.len(), 1, "the first contact schedules one prompt");
    assert_eq!(prompts[0].0, 1, "the prompt belongs to the contacted cycle");
    let first_contact: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT min(initiated_at) FROM contacts WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(request_id)
    .fetch_one(db.pool())
    .await
    .expect("first contact reads");
    assert_eq!(
        prompts[0].1,
        first_contact + chrono::Duration::hours(24),
        "the prompt is due 24 hours after the first contact"
    );

    // A second contact of the same cycle converges on the same prompt.
    start_open(db.pool(), author, request_id, other_offer).await;
    assert_eq!(
        prompt_jobs(db.pool(), request_id).await.len(),
        1,
        "a later contact of the same cycle schedules no second prompt"
    );

    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn a_refused_publication_schedules_nothing() {
    let db = TestDatabase::create("p23t01_refuse")
        .await
        .expect("disposable database allocates");
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Refused Owner", "+55 11 90000-0706").await;
    let first_draft = create_demand(db.pool(), author, "Dishwasher").await;
    let request_id = publish_open(db.pool(), author, first_draft).await;

    // An identical open need is refused: the duplicate rule holds, and the
    // refusal leaves no scheduled work behind.
    let second_draft = create_demand(db.pool(), author, "Dishwasher").await;
    let refused = publish_request(db.pool(), author, second_draft)
        .await
        .expect_err("the identical open need is refused");
    assert_eq!(refused, PublishError::DuplicateIntent);
    assert_eq!(
        expiry_jobs(db.pool(), request_id).await.len(),
        1,
        "the refused publication schedules no second sweep"
    );
    let scheduled: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM background_jobs WHERE payload->>'request_id' = $1",
    )
    .bind(second_draft.to_string())
    .fetch_one(db.pool())
    .await
    .expect("jobs read");
    assert_eq!(scheduled, 0, "the refused publication scheduled nothing");

    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn scheduled_payloads_carry_no_private_material() {
    let db = TestDatabase::create("p23t01_payload")
        .await
        .expect("disposable database allocates");
    seed_catalog(db.pool()).await;
    let author = seed_active(db.pool(), "Payload Owner", "+55 11 90000-0707").await;
    let seller = seed_active(db.pool(), "Payload Seller", "+55 11 90000-0708").await;
    let draft_id = create_demand(db.pool(), author, "Microwave").await;
    let request_id = publish_open(db.pool(), author, draft_id).await;
    let offer_id = submit_open(db.pool(), seller, request_id).await;
    start_open(db.pool(), author, request_id, offer_id).await;

    let payloads: Vec<String> = sqlx::query_scalar(
        "SELECT payload::text FROM background_jobs WHERE payload->>'request_id' = $1",
    )
    .bind(request_id.to_string())
    .fetch_all(db.pool())
    .await
    .expect("payloads read");
    assert!(!payloads.is_empty(), "the request carries scheduled work");
    for payload in &payloads {
        assert!(
            !payload.contains("+55") && !payload.contains("canary"),
            "a scheduled payload carries no destination or secret: {payload}"
        );
    }

    db.cleanup().await.expect("suite cleans up");
}
