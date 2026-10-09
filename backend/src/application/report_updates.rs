//! Grouped duplicates and reporter withdrawal: evidence without weight.
//!
//! Canonical rules: INV-34 (repeats duplicate nothing — a refiling lands as
//! one information record inside the standing incident case, and a repeated
//! withdrawal returns the standing row), INV-36 with AC-38 and EC-31
//! (grouped allegations stay allegations — grouping and withdrawal change no
//! assessment, trust label, ban, lifecycle, or visibility, and raw volume
//! alone never bans), spec 16.1 (same reporter/target/incident becomes
//! additional information; different reporters group with sources retained;
//! withdrawal is recorded while serious review continues independently).
//!
//! One incident is one target kind plus identifier plus reason. Per-incident
//! writers serialize on a transaction-scoped advisory lock keyed by exactly
//! that triple, so concurrent refilings converge on one case without a
//! schema change; the lock releases with the transaction, crash-safe by
//! construction. Snapshots are never updated, the case row is never moved by
//! these writers, and withdrawal touches only the report status.
//!
//! Resolution mirrors `create_report.rs` by per-operation ownership: each
//! application operation owns its standing checks against the shared
//! persistence vocabulary, and the grouped-submission tests pin both to the
//! same observable rules.

use crate::application::create_report::ReportInput;
use crate::persistence::reports::{
    create_case, create_report, report as read_report, update_report_status, NewReport, Report,
    ReportError, REPORT_DETAIL_MAX_CHARS, REPORT_REASONS, TARGET_KINDS,
};

/// Typed grouped-update failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupError {
    /// Unknown kind/reason, overlong detail, or `other` without explanation.
    InvalidField,
    /// The caller is missing, pending, or deleted.
    NotActive,
    /// No such target or report exists.
    NotFound,
    /// The target is real but this caller has no relevant
    /// view-or-history standing for it.
    NotRelevant,
    /// The caller reported their own content, or touches a foreign report.
    NotPermitted,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for GroupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid report field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("report target not found"),
            Self::NotRelevant => f.write_str("no relevant standing for this target"),
            Self::NotPermitted => f.write_str("report update not permitted"),
            Self::StorageFailed => f.write_str("report update failed"),
        }
    }
}

impl std::error::Error for GroupError {}

fn validate_reason_detail(reason: &str, detail: &str) -> Result<(), GroupError> {
    if !REPORT_REASONS.contains(&reason) {
        return Err(GroupError::InvalidField);
    }
    if detail.chars().count() > REPORT_DETAIL_MAX_CHARS {
        return Err(GroupError::InvalidField);
    }
    if reason == "other" && detail.trim().is_empty() {
        return Err(GroupError::InvalidField);
    }
    Ok(())
}

/// Deterministic per-incident lock identity: kind plus identifier plus
/// reason. Same incident always serializes together; distinct incidents
/// never block each other.
fn incident_key(kind: &str, target_id: uuid::Uuid, reason: &str) -> String {
    format!("report-group:{kind}:{target_id}:{reason}")
}

/// File one allegation with grouping: the first filing for an incident opens
/// its case; a refiling by the same reporter lands as one `duplicate`
/// information record in the standing case; a filing by a different reporter
/// with standing groups into the standing case as `open`, keeping its own
/// source and context. Assessment, trust, bans, lifecycles, and visibility
/// stand untouched.
///
/// # Errors
///
/// Returns [`GroupError::InvalidField`] for unknown kind/reason or bad
/// detail, [`GroupError::NotActive`] for missing/pending/deleted callers,
/// [`GroupError::NotFound`] for fabricated targets,
/// [`GroupError::NotRelevant`] for real but inaccessible targets,
/// [`GroupError::NotPermitted`] for self-reports, else
/// [`GroupError::StorageFailed`]. Reasons are static.
pub async fn submit_grouped_report(
    pool: &sqlx::PgPool,
    reporter_id: uuid::Uuid,
    input: ReportInput,
) -> Result<Report, GroupError> {
    if !TARGET_KINDS.contains(&input.target_kind.as_str()) {
        return Err(GroupError::InvalidField);
    }
    validate_reason_detail(&input.reason, &input.detail)?;
    let mut tx = pool.begin().await.map_err(|_| GroupError::StorageFailed)?;
    // Serialize per incident: concurrent refilings queue here, and the loser
    // finds the winner's case below instead of opening a second intake.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(incident_key(
            &input.target_kind,
            input.target_id,
            &input.reason,
        ))
        .execute(&mut *tx)
        .await
        .map_err(|_| GroupError::StorageFailed)?;
    let caller: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1")
            .bind(reporter_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| GroupError::StorageFailed)?;
    let (caller_state, caller_deleted) = match caller {
        Some(standing) => standing,
        None => {
            tx.rollback().await.map_err(|_| GroupError::StorageFailed)?;
            return Err(GroupError::NotActive);
        }
    };
    if caller_deleted.is_some() {
        tx.rollback().await.map_err(|_| GroupError::StorageFailed)?;
        return Err(GroupError::NotActive);
    }
    let history_only = matches!(caller_state.as_str(), "suspended" | "banned");
    if caller_state != "active" && !history_only {
        tx.rollback().await.map_err(|_| GroupError::StorageFailed)?;
        return Err(GroupError::NotActive);
    }
    let snapshots = match resolve_target(&mut tx, reporter_id, &input, history_only).await? {
        TargetResolution::Resolved(snapshots) => snapshots,
        TargetResolution::OwnContent => {
            tx.rollback().await.map_err(|_| GroupError::StorageFailed)?;
            return Err(GroupError::NotPermitted);
        }
        TargetResolution::Missing => {
            tx.rollback().await.map_err(|_| GroupError::StorageFailed)?;
            return Err(GroupError::NotFound);
        }
        TargetResolution::Irrelevant => {
            tx.rollback().await.map_err(|_| GroupError::StorageFailed)?;
            return Err(GroupError::NotRelevant);
        }
    };
    // The standing intake for this incident, if any: the earliest
    // still-assessable case already holding it.
    let standing: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT c.id FROM report_cases c JOIN reports r ON r.case_id = c.id
         WHERE r.target_kind = $1 AND r.target_id = $2 AND r.reason = $3
           AND c.status IN ('open', 'review')
         ORDER BY c.created_at, c.id LIMIT 1",
    )
    .bind(&input.target_kind)
    .bind(input.target_id)
    .bind(&input.reason)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| GroupError::StorageFailed)?;
    let stored = match standing {
        Some(case_id) => {
            // Same reporter already on this intake: information, not weight.
            let repeat: Option<i32> = sqlx::query_scalar(
                "SELECT 1 FROM reports
                 WHERE case_id = $1 AND reporter_id = $2
                   AND target_kind = $3 AND target_id = $4 AND reason = $5",
            )
            .bind(case_id)
            .bind(reporter_id)
            .bind(&input.target_kind)
            .bind(input.target_id)
            .bind(&input.reason)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| GroupError::StorageFailed)?;
            let filed = create_report(
                &mut tx,
                NewReport {
                    case_id,
                    reporter_id,
                    target_kind: input.target_kind,
                    target_id: input.target_id,
                    reason: input.reason,
                    detail: input.detail,
                    target_title_snapshot: snapshots.title,
                    target_context_snapshot: snapshots.context,
                },
            )
            .await
            .map_err(|error| match error {
                ReportError::InvalidField => GroupError::InvalidField,
                _ => GroupError::StorageFailed,
            })?;
            if repeat.is_some() {
                update_report_status(&mut tx, filed.id, "duplicate")
                    .await
                    .map_err(|_| GroupError::StorageFailed)?
            } else {
                filed
            }
        }
        None => {
            let case = create_case(&mut tx, None)
                .await
                .map_err(|error| match error {
                    ReportError::InvalidField => GroupError::InvalidField,
                    _ => GroupError::StorageFailed,
                })?;
            create_report(
                &mut tx,
                NewReport {
                    case_id: case.id,
                    reporter_id,
                    target_kind: input.target_kind,
                    target_id: input.target_id,
                    reason: input.reason,
                    detail: input.detail,
                    target_title_snapshot: snapshots.title,
                    target_context_snapshot: snapshots.context,
                },
            )
            .await
            .map_err(|error| match error {
                ReportError::InvalidField => GroupError::InvalidField,
                _ => GroupError::StorageFailed,
            })?
        }
    };
    tx.commit().await.map_err(|_| GroupError::StorageFailed)?;
    Ok(stored)
}

/// Withdraw one own allegation: the report moves to `withdrawn` with its
/// evidence intact, and its case keeps whatever assessment standing it
/// held — review of a serious allegation continues independently, and no
/// restriction is lifted by this writer. Withdrawing an already-withdrawn
/// report returns the standing row with no new write.
///
/// # Errors
///
/// Returns [`GroupError::NotActive`] for missing/pending/deleted callers,
/// [`GroupError::NotFound`] for unknown reports,
/// [`GroupError::NotPermitted`] for foreign reports, else
/// [`GroupError::StorageFailed`]. Reasons are static.
pub async fn withdraw_report(
    pool: &sqlx::PgPool,
    reporter_id: uuid::Uuid,
    report_id: uuid::Uuid,
) -> Result<Report, GroupError> {
    let mut tx = pool.begin().await.map_err(|_| GroupError::StorageFailed)?;
    let caller: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1")
            .bind(reporter_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| GroupError::StorageFailed)?;
    match caller {
        Some((state, deleted)) => {
            let history_only = matches!(state.as_str(), "suspended" | "banned");
            if deleted.is_some() || (state != "active" && !history_only) {
                tx.rollback().await.map_err(|_| GroupError::StorageFailed)?;
                return Err(GroupError::NotActive);
            }
        }
        None => {
            tx.rollback().await.map_err(|_| GroupError::StorageFailed)?;
            return Err(GroupError::NotActive);
        }
    }
    let stored = read_report(&mut *tx, report_id)
        .await
        .map_err(|_| GroupError::StorageFailed)?
        .ok_or(GroupError::NotFound);
    let stored = match stored {
        Ok(stored) => stored,
        Err(error) => {
            tx.rollback().await.map_err(|_| GroupError::StorageFailed)?;
            return Err(error);
        }
    };
    if stored.reporter_id != reporter_id {
        tx.rollback().await.map_err(|_| GroupError::StorageFailed)?;
        return Err(GroupError::NotPermitted);
    }
    if stored.status == "withdrawn" {
        tx.rollback().await.map_err(|_| GroupError::StorageFailed)?;
        return Ok(stored);
    }
    let withdrawn = update_report_status(&mut tx, report_id, "withdrawn")
        .await
        .map_err(|_| GroupError::StorageFailed)?;
    tx.commit().await.map_err(|_| GroupError::StorageFailed)?;
    Ok(withdrawn)
}

struct Snapshots {
    title: String,
    context: String,
}

enum TargetResolution {
    Resolved(Snapshots),
    OwnContent,
    Missing,
    Irrelevant,
}

async fn has_offer_on_request(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    reporter_id: uuid::Uuid,
    request_id: uuid::Uuid,
) -> Result<bool, GroupError> {
    let found: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM offers WHERE request_id = $1 AND seller_id = $2")
            .bind(request_id)
            .bind(reporter_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| GroupError::StorageFailed)?;
    Ok(found.is_some())
}

async fn has_contact_on_request(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    reporter_id: uuid::Uuid,
    request_id: uuid::Uuid,
) -> Result<bool, GroupError> {
    let found: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM contacts WHERE request_id = $1 AND (buyer_id = $2 OR seller_id = $2)",
    )
    .bind(request_id)
    .bind(reporter_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| GroupError::StorageFailed)?;
    Ok(found.is_some())
}

async fn has_shared_pair_history(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    reporter_id: uuid::Uuid,
    target_id: uuid::Uuid,
) -> Result<bool, GroupError> {
    let contact: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM contacts
         WHERE (buyer_id = $1 AND seller_id = $2) OR (buyer_id = $2 AND seller_id = $1)",
    )
    .bind(reporter_id)
    .bind(target_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| GroupError::StorageFailed)?;
    if contact.is_some() {
        return Ok(true);
    }
    let offer: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM offers JOIN requests ON offers.request_id = requests.id
         WHERE (offers.seller_id = $1 AND requests.author_id = $2)
            OR (offers.seller_id = $2 AND requests.author_id = $1)",
    )
    .bind(reporter_id)
    .bind(target_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| GroupError::StorageFailed)?;
    Ok(offer.is_some())
}

async fn resolve_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    reporter_id: uuid::Uuid,
    input: &ReportInput,
    history_only: bool,
) -> Result<TargetResolution, GroupError> {
    match input.target_kind.as_str() {
        "request" => {
            let row: Option<(uuid::Uuid, String, String, String)> = sqlx::query_as(
                "SELECT author_id, title, notes, visibility FROM requests WHERE id = $1",
            )
            .bind(input.target_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| GroupError::StorageFailed)?;
            let (author_id, title, notes, visibility) = match row {
                Some(row) => row,
                None => return Ok(TargetResolution::Missing),
            };
            if author_id == reporter_id {
                return Ok(TargetResolution::OwnContent);
            }
            let history = has_offer_on_request(tx, reporter_id, input.target_id).await?
                || has_contact_on_request(tx, reporter_id, input.target_id).await?;
            let relevant = history || (!history_only && visibility == "public");
            if !relevant {
                return Ok(TargetResolution::Irrelevant);
            }
            Ok(TargetResolution::Resolved(Snapshots {
                title,
                context: notes,
            }))
        }
        "offer" => {
            let row: Option<(uuid::Uuid, uuid::Uuid, String, String)> = sqlx::query_as(
                "SELECT seller_id, request_id, description, notes FROM offers WHERE id = $1",
            )
            .bind(input.target_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| GroupError::StorageFailed)?;
            let (seller_id, request_id, description, notes) = match row {
                Some(row) => row,
                None => return Ok(TargetResolution::Missing),
            };
            if seller_id == reporter_id {
                return Ok(TargetResolution::OwnContent);
            }
            let author: Option<uuid::Uuid> =
                sqlx::query_scalar("SELECT author_id FROM requests WHERE id = $1")
                    .bind(request_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|_| GroupError::StorageFailed)?;
            match author {
                Some(author) if author == reporter_id => {
                    Ok(TargetResolution::Resolved(Snapshots {
                        title: description,
                        context: notes,
                    }))
                }
                Some(_) => Ok(TargetResolution::Irrelevant),
                None => Err(GroupError::StorageFailed),
            }
        }
        "user" => {
            let row: Option<(String, String, String)> =
                sqlx::query_as("SELECT display_name, city, region FROM users WHERE id = $1")
                    .bind(input.target_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|_| GroupError::StorageFailed)?;
            let (display_name, city, region) = match row {
                Some(row) => row,
                None => return Ok(TargetResolution::Missing),
            };
            if input.target_id == reporter_id {
                return Ok(TargetResolution::OwnContent);
            }
            if !has_shared_pair_history(tx, reporter_id, input.target_id).await? {
                return Ok(TargetResolution::Irrelevant);
            }
            Ok(TargetResolution::Resolved(Snapshots {
                title: display_name,
                context: format!("{city} / {region}"),
            }))
        }
        _ => Err(GroupError::InvalidField),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incident_lock_identity_is_exact() {
        let target = uuid::Uuid::now_v7();
        assert_eq!(
            incident_key("offer", target, "spam"),
            incident_key("offer", target, "spam")
        );
        assert_ne!(
            incident_key("offer", target, "spam"),
            incident_key("offer", target, "fraud")
        );
        assert_ne!(
            incident_key("offer", target, "spam"),
            incident_key("request", target, "spam")
        );
        assert_ne!(
            incident_key("offer", target, "spam"),
            incident_key("offer", uuid::Uuid::now_v7(), "spam")
        );
    }

    #[test]
    fn other_requires_its_explanation() {
        assert!(validate_reason_detail("spam", "").is_ok());
        assert!(validate_reason_detail("other", "saw a prohibited listing").is_ok());
        assert_eq!(
            validate_reason_detail("other", "   "),
            Err(GroupError::InvalidField)
        );
        assert_eq!(
            validate_reason_detail("guilty", "words"),
            Err(GroupError::InvalidField)
        );
        assert_eq!(
            validate_reason_detail("spam", &"x".repeat(1001)),
            Err(GroupError::InvalidField)
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            GroupError::InvalidField,
            GroupError::NotActive,
            GroupError::NotFound,
            GroupError::NotRelevant,
            GroupError::NotPermitted,
            GroupError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
