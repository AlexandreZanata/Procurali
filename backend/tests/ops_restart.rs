//! Operational recovery acceptance (P18-T05): restart, workers, outages.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL:
//! - a worker crash between claim and effect recovers idempotently, and
//!   repeated expiry sweeps converge on one transition, one event, and one
//!   owner notice (INV-09, INV-34);
//! - a provider outage yields a clear failed proof with no activation and no
//!   anchored challenge (AC-24);
//! - a database outage grants no contact and fabricates no saved mutation:
//!   every guarded operation fails with a static reason while persisted
//!   counts stay put (INV-28, AC-25).
//!
//! Crash and outage movement use synthetic fixtures (backdated leases and a
//! closed pool) instead of real process kills; the companion
//! `scripts/test-ops.sh` exercises the real container restart. All phones,
//! names, and codes below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::phone_verification::{VerificationError, VerificationProvider};
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::register_user::{
    confirm_challenge, register_account, start_challenge, RegistrationError,
};
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::application::start_contact::{start_contact, ContactError, ContactInput};
use procurali_backend::operations::jobs::expire_requests::{
    enqueue_expiry, process_expiry, EXPIRY_JOB_KIND,
};
use procurali_backend::operations::worker::{claim_jobs, complete_job, get_job, JobError};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::pool::{connect_disposable, PoolConfig};
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

const LEASE: std::time::Duration = std::time::Duration::from_secs(300);
const LOOKUP_KEY: &str = "p18t05-test-only-lookup-key";
const ENCRYPTION_KEY: &str = "p18t05-test-only-encryption-key";

fn keys() -> PhoneKeys<'static> {
    PhoneKeys {
        lookup_key: LOOKUP_KEY,
        encryption_key: ENCRYPTION_KEY,
    }
}

/// A provider outage: every call fails as unreachable. Static reasons only.
struct OutageProvider;

impl VerificationProvider for OutageProvider {
    async fn start_verification(
        &self,
        _phone: &str,
    ) -> Result<procurali_backend::application::phone_verification::StartOutcome, VerificationError>
    {
        Err(VerificationError::ConnectionFailed)
    }

    async fn check_verification(
        &self,
        _phone: &str,
        _code: &str,
    ) -> Result<procurali_backend::application::phone_verification::CheckOutcome, VerificationError>
    {
        Err(VerificationError::ConnectionFailed)
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

/// A real active account through the real creation path with real phone
/// cryptography; only the lifecycle flip (owned by verification) is staged.
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
        keys(),
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

/// Move cycle one into the past; only the deadline position matters.
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

async fn event_count(pool: &sqlx::PgPool, request_id: uuid::Uuid, kind: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'request' AND resource_id = $1 AND kind = $2",
    )
    .bind(request_id)
    .bind(kind)
    .fetch_one(pool)
    .await
    .expect("events read")
}

async fn notice_count(pool: &sqlx::PgPool, request_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM notices WHERE resource_id = $1")
        .bind(request_id)
        .fetch_one(pool)
        .await
        .expect("notices read")
}

#[tokio::test]
async fn crashed_worker_and_repeated_sweeps_yield_one_business_result() {
    let db = TestDatabase::create("p18t05_restart")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p18t05_restart_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Restart Owner", "+55 11 90000-0081").await;
    let id = publish_open(db.pool(), author).await;
    backdate_cycle(db.pool(), id).await;
    let deadline: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT deadline FROM request_cycles WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(id)
    .fetch_one(db.pool())
    .await
    .expect("deadline reads");

    // Bounded workload is claimed, then the worker dies before any effect:
    // the lease holder inspects nothing further and settles nothing.
    let job = enqueue_expiry(db.pool(), id, 1, deadline)
        .await
        .expect("expiry job enqueues");
    let claimed = claim_jobs(
        db.pool(),
        "worker-a",
        Some(EXPIRY_JOB_KIND),
        1,
        LEASE,
        chrono::Utc::now(),
    )
    .await
    .expect("crashed run claims");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, job.id);
    assert_eq!(event_count(db.pool(), id, "request.expired").await, 0);
    assert_eq!(notice_count(db.pool(), id).await, 0);

    // The restart is observed through state, not memory: the crashed claim
    // still holds its lease, so no twin can steal it yet.
    let held = get_job(db.pool(), job.id)
        .await
        .expect("job reads")
        .expect("job reads");
    assert_eq!(held.state, "claimed");
    assert_eq!(held.claimed_by.as_deref(), Some("worker-a"));
    assert!(claim_jobs(
        db.pool(),
        "worker-b",
        Some(EXPIRY_JOB_KIND),
        1,
        LEASE,
        chrono::Utc::now()
    )
    .await
    .expect("held lease polls")
    .is_empty());

    // Past the lease the recovered worker takes the same job exactly once,
    // processes the sweep, and completes: one transition, one fact.
    sqlx::query("UPDATE background_jobs SET lease_expires_at = $2 WHERE id = $1")
        .bind(job.id)
        .bind(chrono::Utc::now() - chrono::Duration::seconds(1))
        .execute(db.pool())
        .await
        .expect("synthetic lease expiry applies");
    let recovered = claim_jobs(
        db.pool(),
        "worker-b",
        Some(EXPIRY_JOB_KIND),
        1,
        LEASE,
        chrono::Utc::now(),
    )
    .await
    .expect("recovery polls");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].id, job.id);
    assert_eq!(recovered[0].attempts, 2);
    let report = process_expiry(db.pool(), id, chrono::Utc::now())
        .await
        .expect("recovery processes");
    assert!(report.transitioned);
    assert!(report.notice_created);
    assert_eq!(report.state, "expired");
    complete_job(db.pool(), job.id, "worker-b", chrono::Utc::now())
        .await
        .expect("recovery completes");

    // Repeated sweeps after recovery create nothing further: the rerun
    // observes the settled row, and a replayed completion is refused.
    let rerun = process_expiry(db.pool(), id, chrono::Utc::now())
        .await
        .expect("rerun processes");
    assert!(!rerun.transitioned, "no second transition");
    assert!(!rerun.notice_created, "no second notice");
    assert_eq!(rerun.state, "expired");
    assert_eq!(
        complete_job(db.pool(), job.id, "worker-b", chrono::Utc::now()).await,
        Err(JobError::StaleClaim),
        "completed work never completes twice"
    );
    assert_eq!(event_count(db.pool(), id, "request.expired").await, 1);
    assert_eq!(notice_count(db.pool(), id).await, 1);
    let cycle: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT ended_at FROM request_cycles WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(id)
    .fetch_one(db.pool())
    .await
    .expect("cycle reads");
    assert!(cycle.is_some(), "the ended cycle stays stamped");
    let settled = get_job(db.pool(), job.id)
        .await
        .expect("job reads")
        .expect("job reads");
    assert_eq!(settled.state, "completed");
    assert!(settled.claimed_by.is_none(), "no lease survives completion");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn provider_outage_is_a_failed_proof_with_no_activation() {
    let db = TestDatabase::create("p18t05_provider")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p18t05_provider_"),
        "known suite identity in the database name"
    );
    let provider = OutageProvider;
    let phone = "+55 11 90000-0082";
    let user = register_account(
        db.pool(),
        NewUser {
            display_name: "Outage Holder".to_owned(),
            city: "Campinas".to_owned(),
            region: "SP".to_owned(),
            policy_version: "v1".to_owned(),
            policy_accepted_at: chrono::Utc::now(),
            phone: phone.to_owned(),
        },
        keys(),
    )
    .await
    .expect("pending account registers");

    // The outage surfaces as one clear failed proof: challenge dispatch and
    // code confirmation both refuse, and the refusal names no values.
    let start = start_challenge(
        db.pool(),
        &provider,
        phone,
        keys(),
        std::time::Duration::from_secs(300),
    )
    .await;
    assert_eq!(start, Err(RegistrationError::ProviderUnavailable));
    let anchors_after_outage_start: i64 =
        sqlx::query_scalar("SELECT count(*) FROM phone_challenges")
            .fetch_one(db.pool())
            .await
            .expect("anchors read");
    assert_eq!(
        anchors_after_outage_start, 0,
        "the outage anchors no challenge"
    );
    let confirm_without_anchor =
        confirm_challenge(db.pool(), &provider, phone, "135790", keys()).await;
    assert_eq!(
        confirm_without_anchor,
        Ok(procurali_backend::application::register_user::ConfirmationOutcome::Failed),
        "no anchor means no provider call and no activation"
    );
    // A live anchor followed by an outage at confirmation time is the clear
    // failed proof: the provider is consulted, fails loudly, and the holder
    // stays pending with no activation fact.
    {
        let mut tx = db.pool().begin().await.expect("anchor tx begins");
        procurali_backend::persistence::users::create_challenge(
            &mut tx,
            procurali_backend::persistence::users::NewChallenge {
                phone: phone.to_owned(),
                code: "0123456789abcdef0123456789abcdef".to_owned(),
                expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
            },
            LOOKUP_KEY,
        )
        .await
        .expect("anchor stores");
        tx.commit().await.expect("anchor commits");
    }
    let confirm = confirm_challenge(db.pool(), &provider, phone, "135790", keys()).await;
    assert_eq!(confirm, Err(RegistrationError::ProviderUnavailable));
    for rendered in [
        format!(
            "{:?} {}",
            RegistrationError::ProviderUnavailable,
            RegistrationError::ProviderUnavailable
        ),
        format!(
            "{:?} {}",
            VerificationError::ConnectionFailed,
            VerificationError::ConnectionFailed
        ),
    ] {
        assert!(!rendered.contains("+55"), "no destination leaks");
        assert!(!rendered.contains("90000"), "no destination leaks");
        assert!(!rendered.contains("135790"), "no code leaks");
    }

    // No activation survives the outage: the holder stays pending with no
    // new fact, and a retry still needs the provider.
    let state: String = sqlx::query_scalar("SELECT state FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(db.pool())
        .await
        .expect("account reads");
    assert_eq!(state, "pending");
    let anchors: i64 = sqlx::query_scalar("SELECT count(*) FROM phone_challenges")
        .fetch_one(db.pool())
        .await
        .expect("anchors read");
    assert_eq!(anchors, 1, "only the test fixture anchor exists");
    let activations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'account' AND resource_id = $1 AND kind = 'account.activated'",
    )
    .bind(user.id)
    .fetch_one(db.pool())
    .await
    .expect("events read");
    assert_eq!(activations, 0, "the outage activates nothing");
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn database_outage_grants_no_contact_and_fabricates_nothing() {
    let db = TestDatabase::create("p18t05_dboutage")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p18t05_dboutage_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Outage Owner", "+55 11 90000-0083").await;
    let id = publish_open(db.pool(), author).await;
    let contacts_before: i64 = sqlx::query_scalar("SELECT count(*) FROM contacts")
        .fetch_one(db.pool())
        .await
        .expect("contacts read");
    assert_eq!(contacts_before, 0);
    let events_before: i64 = sqlx::query_scalar("SELECT count(*) FROM business_events")
        .fetch_one(db.pool())
        .await
        .expect("events read");

    // A live database still refuses contact without an eligible offer: no
    // grant, no disclosure, no saved row.
    let unknown_offer = uuid::Uuid::now_v7();
    let refused = start_contact(
        db.pool(),
        &keys(),
        author,
        id,
        unknown_offer,
        ContactInput {
            handoff_id: Some(uuid::Uuid::now_v7().to_string()),
            expected_offer_terms: Some(1),
            entry_source: Some("test".to_owned()),
        },
    )
    .await;
    assert_eq!(refused, Err(ContactError::NotFound));

    // The outage itself: every guarded operation fails with a static reason
    // once connections are gone — contact, worker claim, and sweep alike.
    db.pool().close().await;
    let contact_during = start_contact(
        db.pool(),
        &keys(),
        author,
        id,
        unknown_offer,
        ContactInput {
            handoff_id: Some(uuid::Uuid::now_v7().to_string()),
            expected_offer_terms: Some(1),
            entry_source: Some("test".to_owned()),
        },
    )
    .await;
    assert_eq!(contact_during, Err(ContactError::StorageFailed));
    let claim_during = claim_jobs(
        db.pool(),
        "worker-a",
        Some(EXPIRY_JOB_KIND),
        1,
        LEASE,
        chrono::Utc::now(),
    )
    .await;
    assert_eq!(claim_during, Err(JobError::StorageFailed));
    let sweep_during = process_expiry(db.pool(), id, chrono::Utc::now()).await;
    assert!(sweep_during.is_err(), "the sweep fails without a database");
    let rendered = format!("{contact_during:?} {contact_during:?}");
    assert!(
        !rendered.contains("90000"),
        "no destination leaks on outage"
    );

    // The restored connection proves nothing was granted or fabricated while
    // the database was unreachable: identical counts, live eligibility back.
    let base = std::env::var("TEST_DATABASE_URL").expect("test url set");
    let (head, _) = base.rsplit_once('/').expect("test url has a database");
    let reopened = connect_disposable(&format!("{head}/{}", db.name()), &PoolConfig::default())
        .await
        .expect("connection restores");
    let contacts_after: i64 = sqlx::query_scalar("SELECT count(*) FROM contacts")
        .fetch_one(&reopened)
        .await
        .expect("contacts read");
    let events_after: i64 = sqlx::query_scalar("SELECT count(*) FROM business_events")
        .fetch_one(&reopened)
        .await
        .expect("events read");
    assert_eq!(contacts_after, contacts_before, "no contact was granted");
    assert_eq!(events_after, events_before, "no mutation was fabricated");
    let state: String = sqlx::query_scalar("SELECT state FROM requests WHERE id = $1")
        .bind(id)
        .fetch_one(&reopened)
        .await
        .expect("request reads");
    assert_eq!(state, "active", "the outage flips no lifecycle");
    reopened.close().await;
    db.cleanup().await.expect("suite cleans up");
}
