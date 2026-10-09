//! Retention cleanup sweep: redact due ordinary data, keep the rest.
//!
//! Canonical rules: INV-31 with INV-44 (identifiers and counts only pass
//! through this worker — no phone, reporter, destination, or secret value
//! is ever read into a variable, logged, or returned; redaction writes a
//! fixed empty value), INV-34 (one open hold per subject and class, row
//! locks serialize racers, conditional writes converge reruns —
//! sequential reruns, crash recoveries, and racing workers land on one
//! redaction and one release per hold), INV-42 (rows stay countable after
//! redaction: aggregates survive without user-level destination links),
//! AC-40 with EC-38 (ordinary contact destinations redact past due while
//! incident-held evidence stays byte-identical), and privacy-and-security
//! 15.3 (ordinary windows end; incident necessity stays staff-reviewed).
//!
//! Scope is deliberately narrow: due ordinary holds on contacts redact
//! their destination ciphertext, due ordinary holds on users release as
//! already scrubbed, and every other subject stays for explicit staff
//! review. Contacts covered by an open incident hold keep their
//! destination even under a due ordinary hold — incident necessity wins.
//! Scheduling belongs to operations follow-through; this sweep reads due
//! rows directly and is safe to run any time.

use crate::persistence::events::{record as record_event, NewEvent};

/// Job family for retention-cleanup sweeps.
pub const RETENTION_JOB_KIND: &str = "retention.cleanup";

/// Sweep bound (mirrors sibling bounded lists).
pub const CLEANUP_LIMIT_MAX: i64 = 100;

/// One bounded sweep: counts only, never identifiers or values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupReport {
    /// Contact destinations destroyed by this run.
    pub redacted: i64,
    /// Holds released by this run.
    pub released: i64,
    /// Due holds left for explicit staff review.
    pub skipped: i64,
    /// Due ordinary holds observed by this run.
    pub processed: i64,
}

/// Typed sweep failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupError {
    /// Out-of-range sweep bound.
    InvalidField,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for CleanupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid cleanup field"),
            Self::StorageFailed => f.write_str("retention cleanup failed"),
        }
    }
}

impl std::error::Error for CleanupError {}

/// Sweep due ordinary holds once, bounded: redact contact destinations,
/// release scrubbed users, and leave every other subject for explicit
/// staff review. Each hold claims its row lock inside its own
/// transaction, so racing workers serialize per hold, crashes lose
/// nothing half-written, and reruns converge on standing rows.
///
/// # Errors
///
/// Returns [`CleanupError::InvalidField`] for out-of-range bounds, else
/// [`CleanupError::StorageFailed`]. Reasons are static.
pub async fn run_cleanup(pool: &sqlx::PgPool, limit: i64) -> Result<CleanupReport, CleanupError> {
    if !(1..=CLEANUP_LIMIT_MAX).contains(&limit) {
        return Err(CleanupError::InvalidField);
    }
    // Candidate ids only: each hold is claimed below with a held row
    // lock, so racing workers serialize per hold instead of partitioning
    // a point-in-time list.
    let claimed: Vec<(uuid::Uuid, String, uuid::Uuid)> = sqlx::query_as(
        "SELECT id, subject_kind, subject_id FROM retention_holds
         WHERE status = 'open' AND class = 'ordinary' AND due_at <= now()
         ORDER BY due_at, id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|_| CleanupError::StorageFailed)?;
    let mut report = CleanupReport {
        redacted: 0,
        released: 0,
        skipped: 0,
        processed: 0,
    };
    for (hold_id, subject_kind, subject_id) in claimed {
        let mut tx = pool
            .begin()
            .await
            .map_err(|_| CleanupError::StorageFailed)?;
        // Claim the hold with a held row lock: losers wait here, then
        // observe the winner's settlement and converge without counting.
        let standing: Option<(String, String)> =
            sqlx::query_as("SELECT class, status FROM retention_holds WHERE id = $1 FOR UPDATE")
                .bind(hold_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| CleanupError::StorageFailed)?;
        match standing {
            Some((class, status)) if class == "ordinary" && status == "open" => {}
            _ => {
                tx.rollback()
                    .await
                    .map_err(|_| CleanupError::StorageFailed)?;
                continue;
            }
        }
        report.processed += 1;
        match subject_kind.as_str() {
            "contact" => {
                if incident_covered(&mut tx, subject_id).await? {
                    tx.rollback()
                        .await
                        .map_err(|_| CleanupError::StorageFailed)?;
                    report.skipped += 1;
                    continue;
                }
                // Fixed empty value only: the previous ciphertext is never
                // read, and the conditional write converges reruns.
                let cleared = sqlx::query(
                    "UPDATE contacts SET destination_ciphertext = '\\x'
                     WHERE id = $1 AND octet_length(destination_ciphertext) > 0",
                )
                .bind(subject_id)
                .execute(&mut *tx)
                .await
                .map_err(|_| CleanupError::StorageFailed)?
                .rows_affected();
                if cleared > 0 {
                    report.redacted += 1;
                }
                if release(&mut tx, hold_id, &subject_kind, subject_id, true).await? {
                    report.released += 1;
                }
            }
            "user" => {
                // Deletion already scrubbed identity and phone material:
                // nothing personal remains to redact, so the hold lifts
                // with its trail recorded.
                if release(&mut tx, hold_id, &subject_kind, subject_id, false).await? {
                    report.released += 1;
                }
            }
            _ => {
                tx.rollback()
                    .await
                    .map_err(|_| CleanupError::StorageFailed)?;
                report.skipped += 1;
                continue;
            }
        }
        tx.commit().await.map_err(|_| CleanupError::StorageFailed)?;
    }
    Ok(report)
}

/// True when an open incident hold covers the contact through a member
/// report on its request or offer: incident necessity wins over the
/// ordinary window, and the destination stays for policy review.
async fn incident_covered(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    contact_id: uuid::Uuid,
) -> Result<bool, CleanupError> {
    let found: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM contacts
         JOIN reports
           ON (reports.target_kind = 'request' AND reports.target_id = contacts.request_id)
           OR (reports.target_kind = 'offer' AND reports.target_id = contacts.offer_id)
         JOIN retention_holds AS hold
           ON hold.subject_kind = 'case' AND hold.subject_id = reports.case_id
          AND hold.class = 'incident' AND hold.status = 'open'
         WHERE contacts.id = $1 LIMIT 1",
    )
    .bind(contact_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| CleanupError::StorageFailed)?;
    Ok(found.is_some())
}

/// Release one open hold with its system fact: conditional on open, so
/// concurrent releases converge on the first writer. Returns whether this
/// call performed the release.
async fn release(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    hold_id: uuid::Uuid,
    subject_kind: &str,
    subject_id: uuid::Uuid,
    redacted: bool,
) -> Result<bool, CleanupError> {
    let released: Option<i32> = sqlx::query_scalar(
        "UPDATE retention_holds SET status = 'released', released_at = now()
         WHERE id = $1 AND status = 'open' RETURNING 1",
    )
    .bind(hold_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| CleanupError::StorageFailed)?;
    if released.is_none() {
        return Ok(false);
    }
    record_event(
        &mut **tx,
        NewEvent {
            actor_id: None,
            resource_kind: match subject_kind {
                "user" => "user",
                "request" => "request",
                "offer" => "offer",
                "contact" => "contact",
                "report" => "report",
                _ => "case",
            },
            resource_id: subject_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "retention.hold_released",
            policy: "mvp-free",
            source: "worker",
            payload: serde_json::json!({
                "class": "ordinary",
                "redacted": redacted,
            }),
        },
    )
    .await
    .map_err(|_| CleanupError::StorageFailed)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweep_bound_has_limits() {
        assert!((1..=CLEANUP_LIMIT_MAX).contains(&1));
        assert!((1..=CLEANUP_LIMIT_MAX).contains(&100));
        assert!(!(1..=CLEANUP_LIMIT_MAX).contains(&0));
        assert!(!(1..=CLEANUP_LIMIT_MAX).contains(&101));
        assert_eq!(RETENTION_JOB_KIND, "retention.cleanup");
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [CleanupError::InvalidField, CleanupError::StorageFailed] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
