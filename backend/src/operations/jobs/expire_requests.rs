//! Expiry sweep worker: ended cycles with exactly one owner notice.
//!
//! Canonical rules: INV-09 (elapsed rows accept no offers or contact —
//! this worker moves them to `expired` and stamps their cycle ended, so no
//! stale availability survives), INV-12 (renewal never rewrites first
//! publication history — nothing here touches `original_published_at`),
//! INV-13 (expiry is its own distinct event, never a silent state flip),
//! AC-10 (deadline reached refuses submissions and drops discovery before
//! any job runs; the sweep records the bookkeeping), AC-12 (an unanswered
//! prompt leaves the row expired and counts no new activation — answering
//! without renewal writes nothing at all), AC-49 with EC-19 (a suspended
//! row expires while staying hidden; restoration never reactivates it —
//! only lifecycle state moves here, never visibility or timing).
//!
//! One `request.expire` job names one request. Processing is idempotent end
//! to end: the lifecycle transition itself serializes concurrent workers on
//! the request row lock, the cycle stamp is conditional on `NULL`, and the
//! owner notice deduplicates per (event, recipient) — so sequential reruns,
//! crash recoveries, and racing workers converge on one transition, one
//! ended cycle, one event, and one notice.
//!
//! Offer expiry is a real pending cascade, not a closed question: facts and
//! jobs already scope work per cycle (`cycle` travels in both), so the
//! offer writers invalidate exactly the ended cycle's live offers when they
//! land. Nothing here pretends that cascade is unnecessary; nothing here
//! touches offer rows that do not exist yet.

use crate::application::request_eligibility::{expire_if_elapsed, ExpiryOutcome};
use crate::operations::worker::{enqueue, get_job, Job, NewJob};
use crate::persistence::notices::{record as record_notice, NewNotice, NoticeError};
use serde_json::{json, Value};

/// Job family for request-expiry sweeps.
pub const EXPIRY_JOB_KIND: &str = "request.expire";

/// Owner notice kind for an ended cycle.
pub const EXPIRY_NOTICE_KIND: &str = "request.expired";

/// Static owner prompt: renew, found, or close — never automatic renewal.
/// No names, values, or contact material take parameters here.
pub const EXPIRY_NOTICE_BODY: &str = "Your request expired at the end of its 7-day cycle. Renew it for another cycle, mark it as found, or close it. Leaving this prompt unanswered never renews automatically.";

/// One processed sweep: what moved and what already stood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpiryReport {
    /// True when this run performed the lifecycle transition.
    pub transitioned: bool,
    /// True when this run created the owner notice.
    pub notice_created: bool,
    /// The expiry fact backing the notice, when one exists.
    pub event_id: Option<uuid::Uuid>,
    /// Lifecycle state after this run.
    pub state: String,
}

/// Typed sweep failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpireJobError {
    /// The payload names no usable request or cycle.
    InvalidJob,
    /// No such request exists.
    UnknownRequest,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for ExpireJobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJob => f.write_str("invalid expiry job"),
            Self::UnknownRequest => f.write_str("unknown expiry request"),
            Self::StorageFailed => f.write_str("expiry sweep failed"),
        }
    }
}

impl std::error::Error for ExpireJobError {}

/// Parse one `request.expire` payload into its request and advisory cycle.
/// The cycle scopes future offer invalidation; current behavior always keys
/// on the request's live current cycle instead.
pub fn parse_expiry_payload(payload: &Value) -> Option<(uuid::Uuid, i32)> {
    let request_id = payload.get("request_id")?.as_str()?.parse().ok()?;
    let cycle = payload.get("cycle")?.as_i64()?;
    let cycle = i32::try_from(cycle).ok()?;
    if cycle < 1 {
        return None;
    }
    Some((request_id, cycle))
}

/// Enqueue one expiry sweep due at `deadline`: the worker only wakes when
/// there is something to expire.
///
/// # Errors
///
/// Returns [`ExpireJobError::InvalidJob`] for a non-positive cycle, else
/// the worker's enqueue refusal.
pub async fn enqueue_expiry(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
    cycle: i32,
    deadline: chrono::DateTime<chrono::Utc>,
) -> Result<Job, ExpireJobError> {
    if cycle < 1 {
        return Err(ExpireJobError::InvalidJob);
    }
    let job = enqueue(
        pool,
        NewJob {
            kind: EXPIRY_JOB_KIND.to_owned(),
            payload: json!({"request_id": request_id, "cycle": cycle}),
            max_attempts: 3,
        },
    )
    .await
    .map_err(|_| ExpireJobError::InvalidJob)?;
    sqlx::query("UPDATE background_jobs SET not_before = $2 WHERE id = $1")
        .bind(job.id)
        .bind(deadline)
        .execute(pool)
        .await
        .map_err(|_| ExpireJobError::StorageFailed)?;
    get_job(pool, job.id)
        .await
        .map_err(|_| ExpireJobError::StorageFailed)?
        .ok_or(ExpireJobError::StorageFailed)
}

/// Process one expiry sweep for `request_id`, atomically: transition the
/// elapsed row (or observe its settled state), stamp the ended cycle, and
/// record the owner's notice exactly once. Unanswered prompts write nothing
/// further — no renewal, no activation, no republication.
///
/// # Errors
///
/// Returns [`ExpireJobError::UnknownRequest`] for a missing row, else
/// [`ExpireJobError::StorageFailed`]. Reasons are static.
pub async fn process_expiry(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<ExpiryReport, ExpireJobError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ExpireJobError::StorageFailed)?;
    let outcome = expire_if_elapsed(&mut tx, request_id, now)
        .await
        .map_err(|error| {
            use crate::persistence::transaction::AttemptError;
            match error {
                AttemptError::Abort(
                    crate::application::request_eligibility::ExpiryError::NotFound,
                ) => ExpireJobError::UnknownRequest,
                _ => ExpireJobError::StorageFailed,
            }
        })?;
    if outcome == ExpiryOutcome::NotDue {
        tx.rollback()
            .await
            .map_err(|_| ExpireJobError::StorageFailed)?;
        return current_report(pool, request_id).await;
    }
    // The expiry fact for this cycle: just created, or standing from an
    // earlier run or a direct transition. Terminal rows have none, and then
    // there is nothing to stamp or notify.
    let event: Option<(uuid::Uuid, chrono::DateTime<chrono::Utc>, i32, uuid::Uuid)> =
        sqlx::query_as(
            "SELECT event.id, event.effective_at, event.cycle, request.author_id
         FROM business_events AS event
         JOIN requests AS request ON request.id = event.resource_id
         WHERE event.resource_kind = 'request'
           AND event.resource_id = $1
           AND event.kind = 'request.expired'
         ORDER BY event.occurred_at, event.id
         LIMIT 1",
        )
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ExpireJobError::StorageFailed)?;
    let mut notice_created = false;
    let mut event_id = None;
    if let Some((found_id, effective_at, cycle, author_id)) = event {
        event_id = Some(found_id);
        sqlx::query(
            "UPDATE request_cycles SET ended_at = $3
             WHERE request_id = $1 AND cycle_number = $2 AND ended_at IS NULL",
        )
        .bind(request_id)
        .bind(cycle)
        .bind(effective_at)
        .execute(&mut *tx)
        .await
        .map_err(|_| ExpireJobError::StorageFailed)?;
        match record_notice(
            &mut *tx,
            NewNotice {
                account_id: author_id,
                kind: EXPIRY_NOTICE_KIND,
                resource_kind: "request",
                resource_id: request_id,
                event_id: found_id,
                body: EXPIRY_NOTICE_BODY.to_owned(),
            },
        )
        .await
        {
            Ok(_) => notice_created = true,
            Err(NoticeError::Duplicate) => {}
            Err(_) => {
                tx.rollback()
                    .await
                    .map_err(|_| ExpireJobError::StorageFailed)?;
                return Err(ExpireJobError::StorageFailed);
            }
        }
    }
    tx.commit()
        .await
        .map_err(|_| ExpireJobError::StorageFailed)?;
    let mut report = current_report(pool, request_id).await?;
    report.transitioned = outcome == ExpiryOutcome::Transitioned;
    report.notice_created = notice_created;
    report.event_id = event_id;
    Ok(report)
}

/// Current lifecycle state for one request, without mutating.
async fn current_report(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
) -> Result<ExpiryReport, ExpireJobError> {
    let state: Option<String> = sqlx::query_scalar("SELECT state FROM requests WHERE id = $1")
        .bind(request_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| ExpireJobError::StorageFailed)?;
    match state {
        Some(state) => Ok(ExpiryReport {
            transitioned: false,
            notice_created: false,
            event_id: None,
            state,
        }),
        None => Err(ExpireJobError::UnknownRequest),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_parses_request_and_advisory_cycle() {
        let id = uuid::Uuid::now_v7();
        assert_eq!(
            parse_expiry_payload(&json!({"request_id": id, "cycle": 1})),
            Some((id, 1))
        );
        assert_eq!(parse_expiry_payload(&json!({"request_id": id})), None);
        assert_eq!(
            parse_expiry_payload(&json!({"request_id": id, "cycle": 0})),
            None
        );
        assert_eq!(
            parse_expiry_payload(&json!({"request_id": "nope", "cycle": 1})),
            None
        );
    }

    #[test]
    fn notice_body_names_the_owner_options() {
        for option in ["Renew", "found", "close"] {
            assert!(EXPIRY_NOTICE_BODY.contains(option), "prompt names {option}");
        }
        assert!(EXPIRY_NOTICE_BODY.chars().count() <= 1000);
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            ExpireJobError::InvalidJob,
            ExpireJobError::UnknownRequest,
            ExpireJobError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
