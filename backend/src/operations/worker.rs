//! Durable bounded background-job claims over PostgreSQL.
//!
//! Canonical rules: INV-34 (repeats duplicate nothing — claims are atomic
//! single statements with `SKIP LOCKED`, completion is conditional on
//! ownership, and every terminal transition happens once).
//!
//! Lifecycle: `queued` (due when `not_before` passes) → `claimed` (one
//! `claimed_by` worker holds the row until `lease_expires_at`) → either
//! `completed`, back to `queued` with a later `not_before` on retryable
//! failure, or `failed` with a static reason on permanent failure or
//! exhaustion. `attempts` counts claims against `max_attempts`; a claim
//! whose lease lapsed without settlement is reclaimable by any worker, so
//! a crashed worker's eligible work returns instead of stalling. Rows that
//! can never be claimed again move to `failed` through
//! [`sweep_exhausted`] — never silently, never resurrected.
//!
//! Payloads carry identifiers, numbers, and timestamps only: enqueue
//! refuses phone-shaped digit runs and secret-named keys, and failure
//! reasons are static strings, so no destination, token, or secret ever
//! lands in a job row or an error.

use serde_json::Value;

/// Maximum rows claimed by one poll: claims stay bounded per worker.
pub const MAX_CLAIM_BATCH: u32 = 10;

/// Default claim budget per job when the producer names none.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Backoff base in seconds: attempt N waits `30 * 2^(N-1)`.
pub const RETRY_BACKOFF_BASE_SECS: u64 = 30;

/// Backoff ceiling in seconds.
pub const RETRY_BACKOFF_MAX_SECS: u64 = 1800;

/// Payload ceiling in serialized bytes, matching the business-event bound.
pub const PAYLOAD_MAX_BYTES: usize = 8192;

/// Retry backoff after `attempts` counted claims: exponential from the base
/// with a hard ceiling. Pure and overflow-safe.
#[must_use]
pub fn retry_backoff(attempts: u32) -> std::time::Duration {
    let shift = attempts.saturating_sub(1).min(6);
    let seconds = RETRY_BACKOFF_BASE_SECS
        .saturating_mul(1 << shift)
        .min(RETRY_BACKOFF_MAX_SECS);
    std::time::Duration::from_secs(seconds)
}

/// Key names that never belong in a job payload.
const FORBIDDEN_PAYLOAD_KEYS: [&str; 7] = [
    "phone",
    "token",
    "secret",
    "password",
    "authorization",
    "cookie",
    "session",
];

/// Minimum consecutive-digit run refused as phone-shaped (E.164 minimum),
/// mirroring the profile-text rule.
const PHONE_SHAPED_DIGITS: usize = 8;

fn contains_phone_shaped_digits(value: &str) -> bool {
    let mut run = 0usize;
    for char in value.chars() {
        if char.is_ascii_digit() {
            run += 1;
            if run >= PHONE_SHAPED_DIGITS {
                return true;
            }
        } else if matches!(char, ' ' | '-' | '(' | ')' | '.' | '+') {
            continue;
        } else {
            run = 0;
        }
    }
    false
}

/// True when no payload string carries a phone-shaped run and no object key
/// names a secret. Values shaped exactly as identifiers (UUIDs, the system's
/// only id shape — phone destinations never parse as one) are exempt, so
/// digit-heavy ids never trip the phone rule while free text still does.
fn payload_redacted(payload: &Value) -> bool {
    match payload {
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
        Value::String(text) => {
            uuid::Uuid::parse_str(text.trim()).is_ok() || !contains_phone_shaped_digits(text)
        }
        Value::Array(items) => items.iter().all(payload_redacted),
        Value::Object(fields) => fields.iter().all(|(key, value)| {
            !FORBIDDEN_PAYLOAD_KEYS.contains(&key.as_str()) && payload_redacted(value)
        }),
    }
}

/// One job to enqueue: kind, redacted payload, and a claim budget.
#[derive(Debug, Clone, PartialEq)]
pub struct NewJob {
    /// Job family (`request.expire`, notice fan-out, ...), 1..=64 chars.
    pub kind: String,
    /// Identifiers, numbers, and timestamps only — never secrets.
    pub payload: Value,
    /// Claim budget; at least 1.
    pub max_attempts: u32,
}

/// One persisted job with its lease and attempt state.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Job family.
    pub kind: String,
    /// Stored payload.
    pub payload: Value,
    /// Lifecycle state (`queued`, `claimed`, `completed`, `failed`).
    pub state: String,
    /// Claims consumed so far.
    pub attempts: u32,
    /// Claim budget.
    pub max_attempts: u32,
    /// Earliest instant the row is due for claiming.
    pub not_before: chrono::DateTime<chrono::Utc>,
    /// Current lease holder, if claimed.
    pub claimed_by: Option<String>,
    /// Lease expiry, if claimed.
    pub lease_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Static failure reason, once failed.
    pub last_error: Option<String>,
    /// Recording instant (database clock).
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Completion instant, once completed.
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Typed job failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobError {
    /// The job description is malformed or carries secrets.
    InvalidJob,
    /// The row is not claimed by this worker (lost race, lost lease, or
    /// already settled).
    StaleClaim,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJob => f.write_str("invalid job"),
            Self::StaleClaim => f.write_str("stale job claim"),
            Self::StorageFailed => f.write_str("job storage failed"),
        }
    }
}

impl std::error::Error for JobError {}

fn read_job(row: &sqlx::postgres::PgRow) -> Result<Job, JobError> {
    use sqlx::Row;
    Ok(Job {
        id: row.try_get("id").map_err(|_| JobError::StorageFailed)?,
        kind: row.try_get("kind").map_err(|_| JobError::StorageFailed)?,
        payload: row
            .try_get("payload")
            .map_err(|_| JobError::StorageFailed)?,
        state: row.try_get("state").map_err(|_| JobError::StorageFailed)?,
        attempts: row
            .try_get::<i32, _>("attempts")
            .map_err(|_| JobError::StorageFailed)? as u32,
        max_attempts: row
            .try_get::<i32, _>("max_attempts")
            .map_err(|_| JobError::StorageFailed)? as u32,
        not_before: row
            .try_get("not_before")
            .map_err(|_| JobError::StorageFailed)?,
        claimed_by: row
            .try_get("claimed_by")
            .map_err(|_| JobError::StorageFailed)?,
        lease_expires_at: row
            .try_get("lease_expires_at")
            .map_err(|_| JobError::StorageFailed)?,
        last_error: row
            .try_get("last_error")
            .map_err(|_| JobError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| JobError::StorageFailed)?,
        completed_at: row
            .try_get("completed_at")
            .map_err(|_| JobError::StorageFailed)?,
    })
}

const JOB_COLUMNS: &str = "id, kind, payload, state, attempts, max_attempts, not_before, claimed_by, lease_expires_at, last_error, created_at, completed_at";

/// Same columns qualified for the claiming join, where the candidate set
/// also exposes `id`.
const JOB_QUALIFIED_COLUMNS: &str = "job.id, job.kind, job.payload, job.state, job.attempts, job.max_attempts, job.not_before, job.claimed_by, job.lease_expires_at, job.last_error, job.created_at, job.completed_at";

/// Enqueue one job as due immediately with a fresh claim budget.
///
/// # Errors
///
/// Returns [`JobError::InvalidJob`] for a malformed kind, zero budget,
/// oversized payload, or secret-shaped content, else
/// [`JobError::StorageFailed`]. Reasons are static.
pub async fn enqueue(pool: &sqlx::PgPool, job: NewJob) -> Result<Job, JobError> {
    if job.kind.trim().is_empty()
        || job.kind.chars().count() > 64
        || job.max_attempts < 1
        || job.payload.to_string().len() > PAYLOAD_MAX_BYTES
        || !payload_redacted(&job.payload)
    {
        return Err(JobError::InvalidJob);
    }
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "INSERT INTO background_jobs (kind, payload, max_attempts)
         VALUES ($1, $2, $3) RETURNING {JOB_COLUMNS}"
    ))
    .bind(&job.kind)
    .bind(&job.payload)
    .bind(i64::from(job.max_attempts))
    .fetch_one(pool)
    .await
    .map_err(|_| JobError::StorageFailed)?;
    read_job(&row)
}

/// Atomically claim up to `limit` due jobs of one family for `worker_id`.
///
/// Due means queued past `not_before`, or claimed with a lapsed lease and
/// remaining budget (crash recovery). Skipped locked rows belong to live
/// workers and are never stolen. The lease lasts `lease`; the worker must
/// settle before it lapses or lose the claim.
///
/// Runtime-checked SQL keeps `cargo build` offline-capable.
///
/// # Errors
///
/// Returns [`JobError::InvalidJob`] for an empty worker id, a zero batch,
/// or a batch past [`MAX_CLAIM_BATCH`], else [`JobError::StorageFailed`].
pub async fn claim_jobs(
    pool: &sqlx::PgPool,
    worker_id: &str,
    kind: Option<&str>,
    limit: u32,
    lease: std::time::Duration,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<Job>, JobError> {
    if worker_id.trim().is_empty() || !(1..=MAX_CLAIM_BATCH).contains(&limit) {
        return Err(JobError::InvalidJob);
    }
    let lease_length = chrono::Duration::from_std(lease).map_err(|_| JobError::InvalidJob)?;
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "UPDATE background_jobs AS job
         SET state = 'claimed', claimed_by = $1,
             lease_expires_at = $4 + ($5 * interval '1 second'),
             attempts = job.attempts + 1, updated_at = $4
         FROM (
             SELECT id FROM background_jobs
             WHERE ((state = 'queued' AND not_before <= $4 AND attempts < max_attempts)
                 OR (state = 'claimed' AND lease_expires_at <= $4
                     AND attempts < max_attempts))
               AND ($3::text IS NULL OR kind = $3)
             ORDER BY not_before, created_at, id
             LIMIT $2
             FOR UPDATE SKIP LOCKED
         ) AS due
         WHERE job.id = due.id
         RETURNING {JOB_QUALIFIED_COLUMNS}"
    ))
    .bind(worker_id)
    .bind(i64::from(limit))
    .bind(kind)
    .bind(now)
    .bind(lease_length.num_seconds())
    .fetch_all(pool)
    .await
    .map_err(|_| JobError::StorageFailed)?;
    rows.iter().map(read_job).collect()
}

/// Complete one owned claim: exactly the holding worker flips its own
/// `claimed` row to `completed`. Anybody else — a race loser, an expired
/// lease holder, a replay — takes [`JobError::StaleClaim`] and changes
/// nothing, so the same claim can never complete twice.
///
/// # Errors
///
/// Returns [`JobError::StaleClaim`] unless the row is still claimed by this
/// worker, else [`JobError::StorageFailed`].
pub async fn complete_job(
    pool: &sqlx::PgPool,
    job_id: uuid::Uuid,
    worker_id: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Job, JobError> {
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "UPDATE background_jobs
         SET state = 'completed', claimed_by = NULL, lease_expires_at = NULL,
             completed_at = $3, updated_at = $3
         WHERE id = $1 AND state = 'claimed' AND claimed_by = $2
         RETURNING {JOB_COLUMNS}"
    ))
    .bind(job_id)
    .bind(worker_id)
    .bind(now)
    .fetch_optional(pool)
    .await
    .map_err(|_| JobError::StorageFailed)?;
    match row {
        Some(row) => read_job(&row),
        None => Err(JobError::StaleClaim),
    }
}

/// Settle one owned claim as failed: retryable failures with remaining
/// budget return the row to `queued` past a backoff (pending truthfully —
/// never lost, never due early), while permanent failures or exhausted
/// budgets move it to `failed` with the static reason.
///
/// # Errors
///
/// Returns [`JobError::StaleClaim`] unless the row is still claimed by this
/// worker, else [`JobError::StorageFailed`]. Reasons stay static: values
/// never render into `last_error`.
pub async fn fail_job(
    pool: &sqlx::PgPool,
    job_id: uuid::Uuid,
    worker_id: &str,
    retryable: bool,
    reason: &'static str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Job, JobError> {
    if reason.trim().is_empty() || reason.chars().count() > 64 {
        return Err(JobError::InvalidJob);
    }
    let current: Option<Job> = get_job(pool, job_id).await?;
    let current = match current {
        Some(current)
            if current.state == "claimed" && current.claimed_by.as_deref() == Some(worker_id) =>
        {
            current
        }
        _ => return Err(JobError::StaleClaim),
    };
    let backoff = chrono::Duration::from_std(retry_backoff(current.attempts))
        .map_err(|_| JobError::StorageFailed)?;
    let row: Option<sqlx::postgres::PgRow> = if retryable && current.attempts < current.max_attempts
    {
        sqlx::query(&format!(
            "UPDATE background_jobs
             SET state = 'queued', claimed_by = NULL, lease_expires_at = NULL,
                 not_before = $4, updated_at = $3
             WHERE id = $1 AND state = 'claimed' AND claimed_by = $2
             RETURNING {JOB_COLUMNS}"
        ))
        .bind(job_id)
        .bind(worker_id)
        .bind(now)
        .bind(now + backoff)
        .fetch_optional(pool)
        .await
        .map_err(|_| JobError::StorageFailed)?
    } else {
        sqlx::query(&format!(
            "UPDATE background_jobs
             SET state = 'failed', claimed_by = NULL, lease_expires_at = NULL,
                 last_error = $4, updated_at = $3
             WHERE id = $1 AND state = 'claimed' AND claimed_by = $2
             RETURNING {JOB_COLUMNS}"
        ))
        .bind(job_id)
        .bind(worker_id)
        .bind(now)
        .bind(reason)
        .fetch_optional(pool)
        .await
        .map_err(|_| JobError::StorageFailed)?
    };
    match row {
        Some(row) => read_job(&row),
        None => Err(JobError::StaleClaim),
    }
}

/// One job by id, if it exists.
///
/// # Errors
///
/// Returns [`JobError::StorageFailed`] on database failure only.
pub async fn get_job(pool: &sqlx::PgPool, job_id: uuid::Uuid) -> Result<Option<Job>, JobError> {
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {JOB_COLUMNS} FROM background_jobs WHERE id = $1"
    ))
    .bind(job_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| JobError::StorageFailed)?;
    row.map(|row| read_job(&row)).transpose()
}

/// Move unclaimable rows to `failed`: queued or lease-lapsed rows whose
/// budget is spent. Returns the swept count; terminal rows are untouched.
///
/// # Errors
///
/// Returns [`JobError::StorageFailed`] on database failure only.
pub async fn sweep_exhausted(
    pool: &sqlx::PgPool,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<u64, JobError> {
    let swept = sqlx::query(
        "UPDATE background_jobs
         SET state = 'failed', claimed_by = NULL, lease_expires_at = NULL,
             last_error = 'exhausted', updated_at = $1
         WHERE attempts >= max_attempts
           AND ((state = 'queued')
             OR (state = 'claimed' AND lease_expires_at <= $1))",
    )
    .bind(now)
    .execute(pool)
    .await
    .map_err(|_| JobError::StorageFailed)?;
    Ok(swept.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn backoff_grows_exponentially_with_a_ceiling() {
        assert_eq!(retry_backoff(1), std::time::Duration::from_secs(30));
        assert_eq!(retry_backoff(2), std::time::Duration::from_secs(60));
        assert_eq!(retry_backoff(3), std::time::Duration::from_secs(120));
        assert_eq!(retry_backoff(0), std::time::Duration::from_secs(30));
        assert_eq!(
            retry_backoff(u32::MAX),
            std::time::Duration::from_secs(RETRY_BACKOFF_MAX_SECS)
        );
    }

    #[test]
    fn redaction_refuses_secrets_and_phone_shapes() {
        assert!(payload_redacted(
            &json!({"request_id": "01a11c", "cycle": 1})
        ));
        // A digit-heavy identifier parses as a UUID, so it stays exempt.
        assert!(payload_redacted(
            &json!({"request_id": "12345678-1234-1234-1234-123456789012"})
        ));
        assert!(payload_redacted(&json!({"items": [{"code": "abc"}]})));
        assert!(!payload_redacted(&json!({"phone": "+5511987654321"})));
        assert!(!payload_redacted(
            &json!({"note": "call +55 11 98765-4321"})
        ));
        assert!(!payload_redacted(&json!({"token": "abc"})));
        assert!(!payload_redacted(&json!({"nested": {"session": "x"}})));
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            JobError::InvalidJob,
            JobError::StaleClaim,
            JobError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
            assert!(!rendered.contains("+55"));
        }
    }
}
