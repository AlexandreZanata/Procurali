//! Outcome-prompt scheduling: ask once, never nag, never mutate.
//!
//! Canonical rules: INV-12 (prompts never extend time — processing writes
//! no lifecycle, cycle, deadline, or publication state whatsoever),
//! INV-13 (the prompt is its own business, never a silent outcome —
//! unanswered prompts leave unknown exactly unknown), INV-34 with AC-31
//! (repeats duplicate nothing — one prompt notice per request and cycle no
//! matter how many contacts, jobs, or workers converge on it).
//!
//! Two triggers share one prompt per request cycle: the first contact
//! (due 24 hours later) and the cycle deadline. Whichever worker fires
//! first records the single `outcome.prompt` notice; the other observes it
//! and stands down. Terminal requests stop prompting entirely, and rows
//! without any contact have nothing to ask about. The prompt body offers
//! the buyer's three answers and states plainly that silence changes
//! nothing — it links nowhere and verifies nothing.

use crate::operations::worker::{enqueue, get_job, Job, NewJob, DEFAULT_MAX_ATTEMPTS};
use crate::persistence::notices::{record as record_notice, NewNotice};
use serde_json::{json, Value};

/// Job family for outcome prompts.
pub const OUTCOME_PROMPT_KIND: &str = "outcome.prompt";

/// Owner notice kind for an outcome prompt.
pub const OUTCOME_PROMPT_NOTICE_KIND: &str = "outcome.prompt";

/// Hours after the first contact in a cycle before asking.
pub const POST_CONTACT_PROMPT_HOURS: i64 = 24;

/// Static owner prompt: the three answers with silence as a valid choice.
/// No names, values, links, or contact material take parameters here.
pub const OUTCOME_PROMPT_BODY: &str = "How did your request go? If you found what you needed, mark it found; if not yet, leave it open; if you no longer need it, close it. No answer needed — silence keeps everything exactly as is.";

/// Due instant for a post-contact prompt: one full day after first contact.
#[must_use]
pub fn contact_prompt_due(
    first_contact_at: chrono::DateTime<chrono::Utc>,
) -> chrono::DateTime<chrono::Utc> {
    first_contact_at + chrono::Duration::hours(POST_CONTACT_PROMPT_HOURS)
}

/// What one prompt-processing run did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptOutcome {
    /// The single prompt notice now exists because of this run.
    Recorded,
    /// A prompt notice already stood: combined, not duplicated.
    AlreadyPrompted,
    /// Terminal requests stop prompting: completed, cancelled, or draft.
    SkippedTerminal,
    /// Nothing to ask about (no contact in cycle for contact prompts).
    NothingToAsk,
}

/// Typed prompt failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptError {
    /// The payload names no usable request or cycle.
    InvalidJob,
    /// No such request exists.
    UnknownRequest,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for PromptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJob => f.write_str("invalid prompt job"),
            Self::UnknownRequest => f.write_str("unknown prompt request"),
            Self::StorageFailed => f.write_str("prompt sweep failed"),
        }
    }
}

impl std::error::Error for PromptError {}

/// Parse one `outcome.prompt` payload into its request, cycle, and trigger.
pub fn parse_prompt_payload(payload: &Value) -> Option<(uuid::Uuid, i32, String)> {
    let request_id = payload.get("request_id")?.as_str()?.parse().ok()?;
    let cycle = payload.get("cycle")?.as_i64()?;
    let cycle = i32::try_from(cycle).ok()?;
    let trigger = payload.get("trigger")?.as_str()?.to_owned();
    if cycle < 1 || (trigger != "post_contact" && trigger != "expiry") {
        return None;
    }
    Some((request_id, cycle, trigger))
}

/// Schedule the post-contact prompt for one request cycle: due 24 hours
/// after its first contact, or nothing when the cycle has no contact to
/// ask about. An already-scheduled pending job returns instead of
/// duplicating.
///
/// # Errors
///
/// Returns [`PromptError::StorageFailed`] on database failure only.
pub async fn schedule_contact_prompt(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
    cycle_number: i32,
) -> Result<Option<Job>, PromptError> {
    let first: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT min(initiated_at) FROM contacts WHERE request_id = $1 AND cycle_number = $2",
    )
    .bind(request_id)
    .bind(cycle_number)
    .fetch_one(pool)
    .await
    .map_err(|_| PromptError::StorageFailed)?;
    let first = match first {
        Some(first) => first,
        None => return Ok(None),
    };
    schedule_prompt(
        pool,
        request_id,
        cycle_number,
        "post_contact",
        contact_prompt_due(first),
    )
    .await
}

/// Schedule the expiry prompt for one request cycle, due at its deadline.
/// An already-scheduled pending job returns instead of duplicating.
///
/// # Errors
///
/// Returns [`PromptError::StorageFailed`] on database failure only.
pub async fn schedule_expiry_prompt(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
    cycle_number: i32,
    deadline: chrono::DateTime<chrono::Utc>,
) -> Result<Option<Job>, PromptError> {
    schedule_prompt(pool, request_id, cycle_number, "expiry", deadline).await
}

async fn schedule_prompt(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
    cycle_number: i32,
    trigger: &str,
    not_before: chrono::DateTime<chrono::Utc>,
) -> Result<Option<Job>, PromptError> {
    let pending: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM background_jobs
         WHERE kind = $1 AND state IN ('queued', 'claimed')
           AND payload->>'request_id' = $2
           AND (payload->>'cycle')::int = $3",
    )
    .bind(OUTCOME_PROMPT_KIND)
    .bind(request_id.to_string())
    .bind(cycle_number)
    .fetch_optional(pool)
    .await
    .map_err(|_| PromptError::StorageFailed)?;
    if let Some(id) = pending {
        return get_job(pool, id)
            .await
            .map_err(|_| PromptError::StorageFailed)?
            .ok_or(PromptError::StorageFailed)
            .map(Some);
    }
    let job = enqueue(
        pool,
        NewJob {
            kind: OUTCOME_PROMPT_KIND.to_owned(),
            payload: json!({"request_id": request_id, "cycle": cycle_number, "trigger": trigger}),
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        },
    )
    .await
    .map_err(|_| PromptError::StorageFailed)?;
    sqlx::query("UPDATE background_jobs SET not_before = $2 WHERE id = $1")
        .bind(job.id)
        .bind(not_before)
        .execute(pool)
        .await
        .map_err(|_| PromptError::StorageFailed)?;
    get_job(pool, job.id)
        .await
        .map_err(|_| PromptError::StorageFailed)?
        .ok_or(PromptError::StorageFailed)
        .map(Some)
}

/// Process one claimed prompt job: record the single outcome prompt when
/// due and eligible, otherwise stand down with the reason. Terminal
/// requests stop prompting; simultaneous triggers combine into the one
/// standing notice; silence is never an answer. Lifecycle, cycles,
/// deadlines, and publication state are never written here.
///
/// # Errors
///
/// Returns [`PromptError::InvalidJob`] for an unparsable payload,
/// [`PromptError::UnknownRequest`] for a missing row, else
/// [`PromptError::StorageFailed`].
pub async fn process_prompt(pool: &sqlx::PgPool, job: &Job) -> Result<PromptOutcome, PromptError> {
    // The trigger selected scheduling only; processing keys on the live
    // row, so simultaneous triggers converge instead of duplicating.
    let (request_id, _cycle_number, _trigger) =
        parse_prompt_payload(&job.payload).ok_or(PromptError::InvalidJob)?;
    let mut tx = pool.begin().await.map_err(|_| PromptError::StorageFailed)?;
    // Lock the demand row first: concurrent prompt workers serialize here,
    // so the second observes the first's notice instead of duplicating it.
    let row: Option<(String, uuid::Uuid)> =
        sqlx::query_as("SELECT state, author_id FROM requests WHERE id = $1 FOR UPDATE")
            .bind(request_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| PromptError::StorageFailed)?;
    let (state, author_id) = row.ok_or(PromptError::UnknownRequest)?;
    if state == "completed" || state == "cancelled" || state == "draft" {
        tx.rollback()
            .await
            .map_err(|_| PromptError::StorageFailed)?;
        return Ok(PromptOutcome::SkippedTerminal);
    }
    let existing: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM notices
         WHERE resource_kind = 'request' AND resource_id = $1 AND kind = 'outcome.prompt'",
    )
    .bind(request_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| PromptError::StorageFailed)?;
    if existing.is_some() {
        tx.rollback()
            .await
            .map_err(|_| PromptError::StorageFailed)?;
        return Ok(PromptOutcome::AlreadyPrompted);
    }
    // Anchor the notice on the latest request fact for a stable recipient
    // trail; without any fact yet, the row stands on its own history.
    let anchor: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM business_events
         WHERE resource_kind = 'request' AND resource_id = $1
         ORDER BY occurred_at DESC, id DESC LIMIT 1",
    )
    .bind(request_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| PromptError::StorageFailed)?;
    let anchor = anchor.ok_or(PromptError::StorageFailed)?;
    record_notice(
        &mut *tx,
        NewNotice {
            account_id: author_id,
            kind: OUTCOME_PROMPT_NOTICE_KIND,
            resource_kind: "request",
            resource_id: request_id,
            event_id: anchor,
            body: OUTCOME_PROMPT_BODY.to_owned(),
        },
    )
    .await
    .map_err(|_| PromptError::StorageFailed)?;
    tx.commit().await.map_err(|_| PromptError::StorageFailed)?;
    Ok(PromptOutcome::Recorded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_due_is_one_full_day() {
        let first =
            chrono::DateTime::from_timestamp(1_791_000_000, 0).expect("fixture instant builds");
        assert_eq!(
            contact_prompt_due(first).signed_duration_since(first),
            chrono::Duration::hours(24)
        );
        assert_eq!(POST_CONTACT_PROMPT_HOURS, 24);
    }

    #[test]
    fn payload_parses_triggers_only() {
        let id = uuid::Uuid::now_v7();
        assert_eq!(
            parse_prompt_payload(&json!({"request_id": id, "cycle": 1, "trigger": "expiry"})),
            Some((id, 1, "expiry".to_owned()))
        );
        assert_eq!(
            parse_prompt_payload(&json!({"request_id": id, "cycle": 0, "trigger": "expiry"})),
            None
        );
        assert_eq!(
            parse_prompt_payload(&json!({"request_id": id, "cycle": 1, "trigger": "nudge"})),
            None
        );
        assert_eq!(parse_prompt_payload(&json!({"request_id": id})), None);
    }

    #[test]
    fn notice_body_names_answers_not_links() {
        for option in ["found", "close"] {
            assert!(
                OUTCOME_PROMPT_BODY.contains(option),
                "prompt names {option}"
            );
        }
        assert!(OUTCOME_PROMPT_BODY.chars().count() <= 1000);
        for absent in ["http", "wa.me", "phone", "+55"] {
            assert!(!OUTCOME_PROMPT_BODY.contains(absent));
        }
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            PromptError::InvalidJob,
            PromptError::UnknownRequest,
            PromptError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
