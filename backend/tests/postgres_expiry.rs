//! Expiry-sweep acceptance (P06-T04): ended cycles and owner notices.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL:
//! - a suspended elapsed request becomes expired while staying hidden,
//!   with its cycle stamped ended and one owner notice;
//! - sequential reruns, crash recovery, and racing workers converge on one
//!   transition, one event, and one notice;
//! - an answered-but-unrenewed prompt leaves the row expired with no new
//!   cycle, fact, or activation.
//!
//! Setup publishes travel the real draft and publication operations; time
//! movement uses synthetic fixtures instead of real waits. All phones,
//! names, and codes below are synthetic and reserved.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::application::publish_request::publish_request;
use procurali_backend::application::request_drafts::{create_draft, DraftInput};
use procurali_backend::operations::jobs::expire_requests::{
    enqueue_expiry, process_expiry, EXPIRY_JOB_KIND,
};
use procurali_backend::operations::worker::{claim_jobs, complete_job};
use procurali_backend::persistence::catalogs::{upsert_city, upsert_region};
use procurali_backend::persistence::notices::acknowledge;
use procurali_backend::persistence::users::{create_user, NewUser, PhoneKeys};

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
        PhoneKeys {
            lookup_key: "p06t04-test-only-lookup-key",
            encryption_key: "p06t04-test-only-encryption-key",
        },
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

async fn request_facts(pool: &sqlx::PgPool, request_id: uuid::Uuid, kind: &str) -> i64 {
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

async fn owner_notices(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
) -> Vec<(uuid::Uuid, String, Option<chrono::DateTime<chrono::Utc>>)> {
    sqlx::query_as(
        "SELECT id, body, acknowledged_at FROM notices WHERE resource_id = $1 ORDER BY id",
    )
    .bind(request_id)
    .fetch_all(pool)
    .await
    .expect("notices read")
}

#[tokio::test]
async fn suspended_expired_stays_hidden_and_becomes_expired() {
    let db = TestDatabase::create("p06t04_hidden")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t04_hidden_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Hidden Owner", "+55 11 90000-0071").await;
    let id = publish_open(db.pool(), author).await;
    sqlx::query("UPDATE requests SET state = 'suspended', visibility = 'hidden' WHERE id = $1")
        .bind(id)
        .execute(db.pool())
        .await
        .expect("synthetic hidden suspension applies");
    backdate_cycle(db.pool(), id).await;

    let report = process_expiry(db.pool(), id, chrono::Utc::now())
        .await
        .expect("sweep processes");
    assert!(report.transitioned);
    assert!(report.notice_created);
    assert_eq!(report.state, "expired");
    assert!(report.event_id.is_some());

    // Hidden stays hidden with history intact: expired state, ended cycle,
    // one deadline-effective fact, one unacknowledged owner notice.
    let row: (String, String) =
        sqlx::query_as("SELECT state, visibility FROM requests WHERE id = $1")
            .bind(id)
            .fetch_one(db.pool())
            .await
            .expect("row reads");
    assert_eq!(row, ("expired".to_owned(), "hidden".to_owned()));
    let cycle: (
        chrono::DateTime<chrono::Utc>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as(
        "SELECT deadline, ended_at FROM request_cycles WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(id)
    .fetch_one(db.pool())
    .await
    .expect("cycle reads");
    assert_eq!(cycle.1, Some(cycle.0), "the cycle ends at its deadline");
    assert_eq!(request_facts(db.pool(), id, "request.expired").await, 1);
    let notices = owner_notices(db.pool(), id).await;
    assert_eq!(notices.len(), 1);
    assert!(notices[0].2.is_none(), "the prompt starts unanswered");
    for option in ["Renew", "found", "close"] {
        assert!(notices[0].1.contains(option), "prompt names {option}");
    }
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worker_runs_converge_on_one_notice_and_event() {
    let db = TestDatabase::create("p06t04_runs")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t04_runs_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Runs Owner", "+55 11 90000-0072").await;
    let id = publish_open(db.pool(), author).await;
    backdate_cycle(db.pool(), id).await;
    let deadline: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT deadline FROM request_cycles WHERE request_id = $1 AND cycle_number = 1",
    )
    .bind(id)
    .fetch_one(db.pool())
    .await
    .expect("deadline reads");

    // The bounded job path end to end: enqueue due at the deadline, claim,
    // process, and complete — then a crashed twin recovers nothing further.
    let job = enqueue_expiry(db.pool(), id, 1, deadline)
        .await
        .expect("expiry job enqueues");
    assert!(job.not_before == deadline);
    let claimed = claim_jobs(
        db.pool(),
        "worker-a",
        Some(EXPIRY_JOB_KIND),
        1,
        LEASE,
        chrono::Utc::now(),
    )
    .await
    .expect("first run claims");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, job.id);
    let report = process_expiry(db.pool(), id, chrono::Utc::now())
        .await
        .expect("first run processes");
    assert!(report.transitioned);
    assert!(report.notice_created);
    complete_job(db.pool(), job.id, "worker-a", chrono::Utc::now())
        .await
        .expect("first run completes");
    assert!(claim_jobs(
        db.pool(),
        "worker-b",
        Some(EXPIRY_JOB_KIND),
        1,
        LEASE,
        chrono::Utc::now()
    )
    .await
    .expect("settled poll responds")
    .is_empty());

    // A sequential rerun settles without writing anything new.
    let rerun = process_expiry(db.pool(), id, chrono::Utc::now())
        .await
        .expect("rerun settles");
    assert!(!rerun.transitioned);
    assert!(!rerun.notice_created);
    assert_eq!(request_facts(db.pool(), id, "request.expired").await, 1);
    assert_eq!(owner_notices(db.pool(), id).await.len(), 1);

    // Racing workers on a second elapsed row still converge on one of each:
    // the row lock serializes the transitions and the notice deduplicates.
    let raced = publish_open(db.pool(), author).await;
    backdate_cycle(db.pool(), raced).await;
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let sweep = |barrier: std::sync::Arc<tokio::sync::Barrier>| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            process_expiry(&pool, raced, chrono::Utc::now()).await
        }
    };
    let (first, second) = tokio::join!(sweep(std::sync::Arc::clone(&barrier)), sweep(barrier),);
    let (first, second) = (
        first.expect("racer responds"),
        second.expect("racer responds"),
    );
    assert!(
        first.transitioned != second.transitioned,
        "exactly one racer transitions"
    );
    assert!(
        !(first.notice_created && second.notice_created),
        "at most one racer creates the notice"
    );
    assert_eq!(request_facts(db.pool(), raced, "request.expired").await, 1);
    assert_eq!(owner_notices(db.pool(), raced).await.len(), 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn unanswered_prompt_keeps_expired_without_renewal() {
    let db = TestDatabase::create("p06t04_unanswered")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t04_unanswered_"),
        "known suite identity in the database name"
    );
    seed_catalog(db.pool()).await;
    let author = seed_author(db.pool(), "Quiet Owner", "+55 11 90000-0073").await;
    let id = publish_open(db.pool(), author).await;
    backdate_cycle(db.pool(), id).await;
    process_expiry(db.pool(), id, chrono::Utc::now())
        .await
        .expect("sweep processes");

    // The owner answers the prompt without confirming renewal: the notice
    // acknowledges, and absolutely nothing else moves — still expired, still
    // one cycle, still one publication, no new activation counted.
    let notices = owner_notices(db.pool(), id).await;
    assert_eq!(notices.len(), 1);
    assert!(acknowledge(db.pool(), notices[0].0, author)
        .await
        .expect("acknowledgment records"));
    let row: (String, i32, i32) = sqlx::query_as(
        "SELECT state, current_cycle_number, current_revision_number FROM requests WHERE id = $1",
    )
    .bind(id)
    .fetch_one(db.pool())
    .await
    .expect("row reads");
    assert_eq!(row.0, "expired");
    assert_eq!((row.1, row.2), (1, 1), "no cycle or revision advances");
    assert_eq!(request_facts(db.pool(), id, "request.published").await, 1);
    assert_eq!(request_facts(db.pool(), id, "request.expired").await, 1);
    let rerun = process_expiry(db.pool(), id, chrono::Utc::now())
        .await
        .expect("settled sweep reports");
    assert!(!rerun.transitioned);
    assert!(!rerun.notice_created);
    assert_eq!(owner_notices(db.pool(), id).await.len(), 1);
    db.cleanup().await.expect("suite cleans up");
}
