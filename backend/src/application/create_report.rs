//! Relevant report submission: useful reasons and past-interaction reports.
//!
//! Canonical rules: INV-05 (identifiers plus ownership/state/history checks
//! decide — no role label, owner claim, or session boolean exists anywhere
//! here), INV-31 (the reporter never reaches target-facing surfaces — this
//! writer stores the reporter on the private row only and returns the row
//! for staff-scoped callers, never a public projection), INV-36 with AC-38
//! and EC-31 (an allegation, never a finding — this writer opens one fresh
//! case per submission and changes no trust label, ban, lifecycle, or
//! visibility state; grouping and volume policy belong to later cards),
//! AC-37 with EC-14 (a contacted closed request stays reportable from
//! retained history — closure never dismisses the allegation or reopens
//! contact).
//!
//! Suspended and banned callers keep only their own-history path: they may
//! file when an offer/contact pairing ties them to the target, never from
//! mere public visibility, and this writer grants no marketplace power in
//! return (it writes exactly one case plus one report). Pending, deleted,
//! and missing callers cannot file at all. The cookie-session HTTP boundary
//! serves active callers through the shared session contract; the
//! suspended/banned own-history branch below exists for the
//! appeal/staff-assisted intake and is proven by direct tests.

use crate::persistence::reports::{
    create_case, create_report, NewReport, Report, ReportError, REPORT_DETAIL_MAX_CHARS,
    REPORT_REASONS, TARGET_KINDS,
};

/// One report submission as supplied: target plus allegation category.
/// Snapshots resolve server-side from the target row at submission.
#[derive(Debug, Clone)]
pub struct ReportInput {
    /// Target family (`request`, `offer`, or `user`).
    pub target_kind: String,
    /// Target identifier (must already exist).
    pub target_id: uuid::Uuid,
    /// Allegation reason (one of the specification reasons).
    pub reason: String,
    /// Bounded reporter context (`other` must explain itself here).
    pub detail: String,
}

/// Typed report-submission failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    /// Unknown kind/reason, overlong detail, or `other` without explanation.
    InvalidField,
    /// The caller is missing, pending, or deleted.
    NotActive,
    /// No such target exists.
    NotFound,
    /// The target is real but this caller has no relevant
    /// view-or-history standing for it.
    NotRelevant,
    /// The caller reported their own content.
    OwnContent,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid report field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("report target not found"),
            Self::NotRelevant => f.write_str("no relevant standing for this target"),
            Self::OwnContent => f.write_str("cannot report own content"),
            Self::StorageFailed => f.write_str("report submission failed"),
        }
    }
}

impl std::error::Error for SubmitError {}

fn validate_reason_detail(reason: &str, detail: &str) -> Result<(), SubmitError> {
    if !REPORT_REASONS.contains(&reason) {
        return Err(SubmitError::InvalidField);
    }
    if detail.chars().count() > REPORT_DETAIL_MAX_CHARS {
        return Err(SubmitError::InvalidField);
    }
    if reason == "other" && detail.trim().is_empty() {
        return Err(SubmitError::InvalidField);
    }
    Ok(())
}

/// File one allegation for the authenticated caller: resolve the target,
/// refuse own/fabricated/irrelevant targets, then record one fresh case
/// plus one report atomically. Nothing else moves — no lifecycle, no
/// visibility, no trust, no ban.
///
/// # Errors
///
/// Returns [`SubmitError::InvalidField`] for unknown kind/reason or bad
/// detail, [`SubmitError::NotActive`] for missing/pending/deleted callers,
/// [`SubmitError::NotFound`] for fabricated targets,
/// [`SubmitError::NotRelevant`] for real but inaccessible targets,
/// [`SubmitError::OwnContent`] for self-reports, else
/// [`SubmitError::StorageFailed`]. Reasons are static.
pub async fn submit_report(
    pool: &sqlx::PgPool,
    reporter_id: uuid::Uuid,
    input: ReportInput,
) -> Result<Report, SubmitError> {
    if !TARGET_KINDS.contains(&input.target_kind.as_str()) {
        return Err(SubmitError::InvalidField);
    }
    validate_reason_detail(&input.reason, &input.detail)?;
    let mut tx = pool.begin().await.map_err(|_| SubmitError::StorageFailed)?;
    let caller: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1")
            .bind(reporter_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| SubmitError::StorageFailed)?;
    let (caller_state, caller_deleted) = match caller {
        Some(standing) => standing,
        None => {
            tx.rollback()
                .await
                .map_err(|_| SubmitError::StorageFailed)?;
            return Err(SubmitError::NotActive);
        }
    };
    if caller_deleted.is_some() {
        tx.rollback()
            .await
            .map_err(|_| SubmitError::StorageFailed)?;
        return Err(SubmitError::NotActive);
    }
    let history_only = matches!(caller_state.as_str(), "suspended" | "banned");
    if caller_state != "active" && !history_only {
        tx.rollback()
            .await
            .map_err(|_| SubmitError::StorageFailed)?;
        return Err(SubmitError::NotActive);
    }
    let snapshots = resolve_target(&mut tx, reporter_id, &input, history_only).await?;
    let outcome = match snapshots {
        TargetResolution::Refused(error) => {
            tx.rollback()
                .await
                .map_err(|_| SubmitError::StorageFailed)?;
            return Err(error);
        }
        TargetResolution::Resolved(snapshots) => {
            let case = create_case(&mut tx, None)
                .await
                .map_err(|error| match error {
                    ReportError::InvalidField => SubmitError::InvalidField,
                    _ => SubmitError::StorageFailed,
                })?;
            let stored = create_report(
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
                ReportError::InvalidField => SubmitError::InvalidField,
                _ => SubmitError::StorageFailed,
            })?;
            stored
        }
    };
    tx.commit().await.map_err(|_| SubmitError::StorageFailed)?;
    Ok(outcome)
}

struct Snapshots {
    title: String,
    context: String,
}

enum TargetResolution {
    Resolved(Snapshots),
    Refused(SubmitError),
}

async fn has_offer_on_request(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    reporter_id: uuid::Uuid,
    request_id: uuid::Uuid,
) -> Result<bool, SubmitError> {
    let found: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM offers WHERE request_id = $1 AND seller_id = $2")
            .bind(request_id)
            .bind(reporter_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| SubmitError::StorageFailed)?;
    Ok(found.is_some())
}

async fn has_contact_on_request(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    reporter_id: uuid::Uuid,
    request_id: uuid::Uuid,
) -> Result<bool, SubmitError> {
    let found: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM contacts WHERE request_id = $1 AND (buyer_id = $2 OR seller_id = $2)",
    )
    .bind(request_id)
    .bind(reporter_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| SubmitError::StorageFailed)?;
    Ok(found.is_some())
}

async fn has_shared_pair_history(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    reporter_id: uuid::Uuid,
    target_id: uuid::Uuid,
) -> Result<bool, SubmitError> {
    let contact: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM contacts
         WHERE (buyer_id = $1 AND seller_id = $2) OR (buyer_id = $2 AND seller_id = $1)",
    )
    .bind(reporter_id)
    .bind(target_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| SubmitError::StorageFailed)?;
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
    .map_err(|_| SubmitError::StorageFailed)?;
    Ok(offer.is_some())
}

async fn resolve_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    reporter_id: uuid::Uuid,
    input: &ReportInput,
    history_only: bool,
) -> Result<TargetResolution, SubmitError> {
    match input.target_kind.as_str() {
        "request" => {
            let row: Option<(uuid::Uuid, String, String, String)> = sqlx::query_as(
                "SELECT author_id, title, notes, visibility FROM requests WHERE id = $1",
            )
            .bind(input.target_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| SubmitError::StorageFailed)?;
            let (author_id, title, notes, visibility) = match row {
                Some(row) => row,
                None => return Ok(TargetResolution::Refused(SubmitError::NotFound)),
            };
            if author_id == reporter_id {
                return Ok(TargetResolution::Refused(SubmitError::OwnContent));
            }
            let history = has_offer_on_request(tx, reporter_id, input.target_id).await?
                || has_contact_on_request(tx, reporter_id, input.target_id).await?;
            let relevant = history || (!history_only && visibility == "public");
            if !relevant {
                return Ok(TargetResolution::Refused(SubmitError::NotRelevant));
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
            .map_err(|_| SubmitError::StorageFailed)?;
            let (seller_id, request_id, description, notes) = match row {
                Some(row) => row,
                None => return Ok(TargetResolution::Refused(SubmitError::NotFound)),
            };
            if seller_id == reporter_id {
                return Ok(TargetResolution::Refused(SubmitError::OwnContent));
            }
            let author: Option<uuid::Uuid> =
                sqlx::query_scalar("SELECT author_id FROM requests WHERE id = $1")
                    .bind(request_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|_| SubmitError::StorageFailed)?;
            match author {
                Some(author) if author == reporter_id => {
                    Ok(TargetResolution::Resolved(Snapshots {
                        title: description,
                        context: notes,
                    }))
                }
                Some(_) => Ok(TargetResolution::Refused(SubmitError::NotRelevant)),
                None => Err(SubmitError::StorageFailed),
            }
        }
        "user" => {
            let row: Option<(String, String, String)> =
                sqlx::query_as("SELECT display_name, city, region FROM users WHERE id = $1")
                    .bind(input.target_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|_| SubmitError::StorageFailed)?;
            let (display_name, city, region) = match row {
                Some(row) => row,
                None => return Ok(TargetResolution::Refused(SubmitError::NotFound)),
            };
            if input.target_id == reporter_id {
                return Ok(TargetResolution::Refused(SubmitError::OwnContent));
            }
            if !has_shared_pair_history(tx, reporter_id, input.target_id).await? {
                return Ok(TargetResolution::Refused(SubmitError::NotRelevant));
            }
            Ok(TargetResolution::Resolved(Snapshots {
                title: display_name,
                context: format!("{city} / {region}"),
            }))
        }
        _ => Err(SubmitError::InvalidField),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_requires_its_explanation() {
        assert!(validate_reason_detail("spam", "").is_ok());
        assert!(validate_reason_detail("other", "saw a weapon listing").is_ok());
        assert_eq!(
            validate_reason_detail("other", "   "),
            Err(SubmitError::InvalidField)
        );
        assert_eq!(
            validate_reason_detail("guilty", "words"),
            Err(SubmitError::InvalidField)
        );
        assert_eq!(
            validate_reason_detail("spam", &"x".repeat(1001)),
            Err(SubmitError::InvalidField)
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            SubmitError::InvalidField,
            SubmitError::NotActive,
            SubmitError::NotFound,
            SubmitError::NotRelevant,
            SubmitError::OwnContent,
            SubmitError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
