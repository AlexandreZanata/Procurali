//! Background-job acceptance (P06-T03): durable bounded claims.
//!
//! Requires an explicit disposable `TEST_DATABASE_URL` (fails loudly
//! otherwise). Proves against real PostgreSQL:
//! - two workers racing one claim produce a single completion and a single
//!   business fact;
//! - a crashed worker's eligible work returns on lease expiry with no
//!   duplicate fact;
//! - a failed dependency keeps the job pending (backoff) then failed
//!   (exhaustion or permanence) truthfully, with secret-shaped payloads
//!   refused at enqueue.
//!
//! Crash and clock movement use synthetic time fixtures (backdated leases
//! and due times) instead of real sleeps; every value below is synthetic.

#[path = "support/database.rs"]
mod database;

use database::TestDatabase;
use procurali_backend::operations::worker::{
    claim_jobs, complete_job, enqueue, fail_job, get_job, sweep_exhausted, JobError, NewJob,
};
use serde_json::json;

const LEASE: std::time::Duration = std::time::Duration::from_secs(300);

async fn enqueue_proof(pool: &sqlx::PgPool, max_attempts: u32) -> uuid::Uuid {
    enqueue(
        pool,
        NewJob {
            kind: "test.proof".to_owned(),
            payload: json!({"n": 1}),
            max_attempts,
        },
    )
    .await
    .expect("fixture job enqueues")
    .id
}

/// Business effect of one execution: exactly one fact per completed claim.
async fn execute_once(pool: &sqlx::PgPool, job_id: uuid::Uuid) {
    sqlx::query(
        "INSERT INTO business_events
            (resource_kind, resource_id, effective_at, kind, policy, source, payload)
         VALUES ('job', $1, now(), 'job.executed', 'mvp-free', 'worker', '{}')",
    )
    .bind(job_id)
    .execute(pool)
    .await
    .expect("execution fact records");
}

async fn executed_facts(pool: &sqlx::PgPool, job_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM business_events
         WHERE resource_kind = 'job' AND resource_id = $1 AND kind = 'job.executed'",
    )
    .bind(job_id)
    .fetch_one(pool)
    .await
    .expect("facts read")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_workers_do_not_complete_the_same_claim_twice() {
    let db = TestDatabase::create("p06t03_race")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t03_race_"),
        "known suite identity in the database name"
    );
    let job_id = enqueue_proof(db.pool(), 3).await;

    // Both workers poll the single due row at once: exactly one claim wins.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let poll = |barrier: std::sync::Arc<tokio::sync::Barrier>, worker: &'static str| {
        let pool = db.pool().clone();
        async move {
            barrier.wait().await;
            claim_jobs(
                &pool,
                worker,
                Some("test.proof"),
                1,
                LEASE,
                chrono::Utc::now(),
            )
            .await
        }
    };
    let (first, second) = tokio::join!(
        poll(std::sync::Arc::clone(&barrier), "worker-a"),
        poll(barrier, "worker-b"),
    );
    let (first, second) = (
        first.expect("poll responds"),
        second.expect("poll responds"),
    );
    assert_eq!(
        first.len() + second.len(),
        1,
        "exactly one worker holds the claim"
    );
    let (winner, winner_id) = if first.len() == 1 {
        (first, "worker-a")
    } else {
        (second, "worker-b")
    };
    assert_eq!(winner[0].id, job_id);
    assert_eq!(winner[0].attempts, 1);

    // Only the holder executes and completes; every other completion is a
    // stale refusal that changes nothing.
    execute_once(db.pool(), job_id).await;
    complete_job(db.pool(), job_id, winner_id, chrono::Utc::now())
        .await
        .expect("holder completes");
    for impostor in ["worker-a", "worker-b", "worker-c"] {
        assert_eq!(
            complete_job(db.pool(), job_id, impostor, chrono::Utc::now()).await,
            Err(JobError::StaleClaim),
            "{impostor} cannot complete twice"
        );
    }
    assert_eq!(executed_facts(db.pool(), job_id).await, 1);
    let stored = get_job(db.pool(), job_id)
        .await
        .expect("job reads")
        .expect("job reads");
    assert_eq!(stored.state, "completed");
    assert_eq!(stored.attempts, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn crash_restart_recovers_eligible_work_without_duplicate_facts() {
    let db = TestDatabase::create("p06t03_crash")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t03_crash_"),
        "known suite identity in the database name"
    );
    let job_id = enqueue_proof(db.pool(), 3).await;
    let now = chrono::Utc::now();

    // The first worker claims and then dies without settling: while its
    // lease holds, nobody else may take the row.
    let claimed = claim_jobs(db.pool(), "worker-a", None, 1, LEASE, now)
        .await
        .expect("first claim polls");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, job_id);
    assert!(claim_jobs(db.pool(), "worker-b", None, 1, LEASE, now)
        .await
        .expect("held lease polls")
        .is_empty());

    // Past the lease, the row is eligible again for any worker: the same
    // job returns with its attempts honestly counted, executes once, and
    // completes exactly once.
    sqlx::query("UPDATE background_jobs SET lease_expires_at = $2 WHERE id = $1")
        .bind(job_id)
        .bind(now - chrono::Duration::seconds(1))
        .execute(db.pool())
        .await
        .expect("synthetic lease expiry applies");
    let recovered = claim_jobs(db.pool(), "worker-b", None, 1, LEASE, now)
        .await
        .expect("recovery polls");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].id, job_id);
    assert_eq!(recovered[0].attempts, 2);
    execute_once(db.pool(), job_id).await;
    complete_job(db.pool(), job_id, "worker-b", now)
        .await
        .expect("recovery completes");
    assert_eq!(executed_facts(db.pool(), job_id).await, 1);
    db.cleanup().await.expect("suite cleans up");
}

#[tokio::test]
async fn failed_dependency_keeps_job_pending_then_failed_truthfully() {
    let db = TestDatabase::create("p06t03_failure")
        .await
        .expect("disposable database allocates");
    assert!(
        db.name().starts_with("procurali_test_p06t03_failure_"),
        "known suite identity in the database name"
    );
    // Every operation reads a fresh effective instant: rows default
    // `not_before` to the database clock at insertion, so any client instant
    // captured before a write is already stale for due comparisons.
    let job_id = enqueue_proof(db.pool(), 2).await;
    let t0 = chrono::Utc::now();

    // A retryable dependency failure parks the job past a backoff: pending
    // with attempts counted, invisible to polling until due, error unset.
    let claimed = claim_jobs(db.pool(), "worker-a", None, 1, LEASE, chrono::Utc::now())
        .await
        .expect("first claim polls");
    assert_eq!(claimed.len(), 1);
    let parked = fail_job(
        db.pool(),
        job_id,
        "worker-a",
        true,
        "dependency_unavailable",
        chrono::Utc::now(),
    )
    .await
    .expect("retryable failure parks");
    assert_eq!(parked.state, "queued");
    assert_eq!(parked.attempts, 1);
    assert!(parked.not_before > t0, "backoff delays redelivery");
    assert_eq!(parked.last_error, None);
    assert!(
        claim_jobs(db.pool(), "worker-a", None, 1, LEASE, chrono::Utc::now())
            .await
            .expect("early poll responds")
            .is_empty()
    );

    // Past the backoff the job is due again; spending its last attempt on
    // another retryable failure exhausts it into `failed` with the reason.
    sqlx::query("UPDATE background_jobs SET not_before = $2 WHERE id = $1")
        .bind(job_id)
        .bind(chrono::Utc::now() - chrono::Duration::seconds(1))
        .execute(db.pool())
        .await
        .expect("synthetic backoff elapses");
    let reclaimed = claim_jobs(db.pool(), "worker-a", None, 1, LEASE, chrono::Utc::now())
        .await
        .expect("due poll responds");
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].attempts, 2);
    let failed = fail_job(
        db.pool(),
        job_id,
        "worker-a",
        true,
        "dependency_unavailable",
        chrono::Utc::now(),
    )
    .await
    .expect("exhaustion settles");
    assert_eq!(failed.state, "failed");
    assert_eq!(failed.last_error.as_deref(), Some("dependency_unavailable"));
    assert!(
        claim_jobs(db.pool(), "worker-a", None, 10, LEASE, chrono::Utc::now())
            .await
            .expect("settled poll responds")
            .is_empty()
    );

    // A permanent failure settles at once, and the sweep collects rows whose
    // budget lapsed without settlement — completed work is never touched.
    let doomed = enqueue_proof(db.pool(), 3).await;
    let doomed_claim = claim_jobs(db.pool(), "worker-a", None, 10, LEASE, chrono::Utc::now())
        .await
        .expect("claim polls");
    assert_eq!(doomed_claim.len(), 1);
    let failed = fail_job(
        db.pool(),
        doomed,
        "worker-a",
        false,
        "permanent",
        chrono::Utc::now(),
    )
    .await
    .expect("permanent failure settles");
    assert_eq!(failed.state, "failed");
    assert_eq!(failed.last_error.as_deref(), Some("permanent"));
    let stranded = enqueue_proof(db.pool(), 1).await;
    let stranded_claim = claim_jobs(db.pool(), "worker-a", None, 10, LEASE, chrono::Utc::now())
        .await
        .expect("single-budget claim polls");
    assert_eq!(stranded_claim.len(), 1);
    sqlx::query("UPDATE background_jobs SET lease_expires_at = $2 WHERE id = $1")
        .bind(stranded)
        .bind(chrono::Utc::now() - chrono::Duration::seconds(1))
        .execute(db.pool())
        .await
        .expect("synthetic lease expiry applies");
    assert!(
        claim_jobs(db.pool(), "worker-a", None, 10, LEASE, chrono::Utc::now())
            .await
            .expect("exhausted poll responds")
            .is_empty()
    );
    assert_eq!(
        sweep_exhausted(db.pool(), chrono::Utc::now())
            .await
            .expect("sweep runs"),
        1
    );
    let swept = get_job(db.pool(), stranded)
        .await
        .expect("job reads")
        .expect("job reads");
    assert_eq!(swept.state, "failed");
    assert_eq!(swept.last_error.as_deref(), Some("exhausted"));

    // Secret-shaped payloads never enqueue: phones, tokens, and sessions.
    for payload in [
        serde_json::json!({"phone": "+5511987654321"}),
        serde_json::json!({"note": "call +55 11 98765-4321"}),
        serde_json::json!({"token": "abc"}),
    ] {
        assert_eq!(
            enqueue(
                db.pool(),
                NewJob {
                    kind: "test.proof".to_owned(),
                    payload,
                    max_attempts: 1,
                },
            )
            .await,
            Err(JobError::InvalidJob)
        );
    }
    // Over-wide batches and empty workers are refused, not silently clamped.
    assert_eq!(
        claim_jobs(db.pool(), "worker-a", None, 0, LEASE, chrono::Utc::now()).await,
        Err(JobError::InvalidJob)
    );
    assert_eq!(
        claim_jobs(db.pool(), "worker-a", None, 11, LEASE, chrono::Utc::now()).await,
        Err(JobError::InvalidJob)
    );
    db.cleanup().await.expect("suite cleans up");
}
