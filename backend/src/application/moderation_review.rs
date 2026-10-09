//! Prioritized case review and dispositions: human decisions on allegations.
//!
//! Canonical rules: INV-31 (queues and receipts carry counts, severities,
//! and business-context snapshots only — reporter identities, free-text
//! detail, phones, and secrets never enter a review projection), INV-36
//! with AC-38 and EC-31 (a disposition assesses the allegation; it changes
//! no trust label, ban, lifecycle, or visibility by itself, and raw volume
//! never bans — enforcement writers belong to later cards), INV-37 with
//! AC-39 (review start and disposition record actor, reason, scope, time,
//! and previous/resulting case standing through an inspection audit plus a
//! durable business fact), spec 16.2 (critical, high, normal, and low
//! triage with oldest-first age order inside each band) with 16.3 (the
//! decision identifies policy, evidence, reason, and appeal route; invalid
//! allegations penalize nobody, and ordinary uncertainty is never a
//! malicious-report finding — no such disposition exists here).
//!
//! Lifecycle is open to review to exactly one of valid, invalid, or
//! duplicate. Queue reads audit nothing: summaries hold no sensitive data
//! by construction.

use serde::Serialize;

use crate::application::staff_permissions::{authorized_inspect, require_moderator, InspectInput};
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::reports::REPORT_SEVERITIES;

/// Decided dispositions. `withdrawn` stays reporter-owned and never
/// appears here; no malicious-report disposition exists.
pub const DISPOSITIONS: [&str; 3] = ["valid", "invalid", "duplicate"];

/// Queue page bound (mirrors sibling bounded lists).
pub const QUEUE_LIMIT_MAX: i64 = 100;
/// Reason bound in scalar values.
pub const REVIEW_REASON_MAX_CHARS: usize = 1000;
/// Evidence-reference bound in scalar values.
pub const REVIEW_EVIDENCE_MAX_CHARS: usize = 1000;
/// Purpose bound in scalar values.
pub const REVIEW_PURPOSE_MAX_CHARS: usize = 500;
/// Policy-version bound in scalar values.
pub const REVIEW_POLICY_MAX_CHARS: usize = 32;
/// Sampled targets per queued case.
pub const QUEUE_TARGET_SAMPLE: i64 = 3;

/// One review opening as supplied: triage plus the audit purpose.
#[derive(Debug, Clone)]
pub struct OpenReviewInput {
    /// Triage severity (`critical`, `high`, `normal`, `low`).
    pub severity: String,
    /// Rules version the review runs under.
    pub policy_version: String,
    /// Assigned safety purpose (recorded in the inspection audit).
    pub purpose: String,
}

/// One disposition as supplied: the human decision with its references.
#[derive(Debug, Clone)]
pub struct DecideInput {
    /// `valid`, `invalid`, or `duplicate`.
    pub disposition: String,
    /// Why this decision was reached.
    pub reason: String,
    /// Optional evidence references (report/contact identifiers as text).
    pub evidence: Option<String>,
    /// Rules version the decision applies.
    pub policy_version: String,
    /// Assigned safety purpose (recorded in the inspection audit).
    pub purpose: String,
}

/// One queued case: standing plus sampled allegation context. No reporter,
/// detail, phone, or secret field exists here.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaseSummary {
    /// Grouped incident identity.
    pub case_id: uuid::Uuid,
    /// `open` or `review` (decided cases leave the queue).
    pub status: String,
    /// Triage severity, if triaged yet.
    pub severity: Option<String>,
    /// Allegation rows grouped here.
    pub report_count: i64,
    /// Distinct allegation reasons present.
    pub reasons: Vec<String>,
    /// Sampled targets with business-context titles.
    pub targets: Vec<TargetSample>,
    /// Intake instant.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last assessment instant.
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// One sampled review target: what was reported, never who reported it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TargetSample {
    /// Target family.
    pub target_kind: String,
    /// Target identifier.
    pub target_id: uuid::Uuid,
    /// Business-context title snapshot.
    pub title_snapshot: String,
}

/// One bounded queue page.
#[derive(Debug, Clone, PartialEq)]
pub struct QueuePage {
    /// Page rows in severity-then-age order.
    pub cases: Vec<CaseSummary>,
    /// Unresolved cases overall (filtering, not paging).
    pub total: i64,
    /// Applied limit.
    pub limit: i64,
    /// Applied offset.
    pub offset: i64,
}

/// One opened review: the case standing after triage.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenedReview {
    /// Reviewed case.
    pub case_id: uuid::Uuid,
    /// `review`.
    pub status: String,
    /// Assigned triage severity.
    pub severity: String,
}

/// One recorded disposition: the case standing after decision.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseDecision {
    /// Decided case.
    pub case_id: uuid::Uuid,
    /// The disposition.
    pub status: String,
    /// Recorded rationale.
    pub reason: String,
}

/// Typed review failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewError {
    /// Unknown severity/disposition, bad reason/evidence/purpose/policy
    /// text, or out-of-range paging.
    InvalidField,
    /// The account is missing, pending, suspended, banned, or deleted.
    NotActive,
    /// No such case exists.
    NotFound,
    /// The caller holds no live moderator-or-better grant.
    NotPermitted,
    /// The case is not in the standing this transition needs.
    InvalidState,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for ReviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid review field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("review case not found"),
            Self::NotPermitted => f.write_str("review not permitted"),
            Self::InvalidState => f.write_str("case is not in review standing"),
            Self::StorageFailed => f.write_str("review storage failed"),
        }
    }
}

impl std::error::Error for ReviewError {}

fn check_bounded(value: &str, max: usize, required: bool) -> Result<(), ReviewError> {
    let count = value.chars().count();
    if (required && value.trim().is_empty()) || count > max {
        return Err(ReviewError::InvalidField);
    }
    Ok(())
}

fn check_queue_page(limit: i64, offset: i64) -> Result<(), ReviewError> {
    if !(1..=QUEUE_LIMIT_MAX).contains(&limit) || offset < 0 {
        return Err(ReviewError::InvalidField);
    }
    Ok(())
}

#[derive(Debug, sqlx::FromRow)]
struct QueueRow {
    id: uuid::Uuid,
    status: String,
    severity: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    report_count: i64,
}

/// Staff queue: unresolved cases in severity order (critical, high,
/// normal, low, then untriaged), oldest first inside each band, with
/// sampled allegation context. Bounded and deterministic.
///
/// # Errors
///
/// Returns [`ReviewError::InvalidField`] for out-of-range paging,
/// [`ReviewError::NotActive`] for restricted callers,
/// [`ReviewError::NotPermitted`] for ungranted callers, else
/// [`ReviewError::StorageFailed`]. Reasons are static.
pub async fn queue_cases(
    pool: &sqlx::PgPool,
    moderator_id: uuid::Uuid,
    limit: i64,
    offset: i64,
) -> Result<QueuePage, ReviewError> {
    check_queue_page(limit, offset)?;
    require_moderator(pool, moderator_id)
        .await
        .map_err(|error| match error {
            crate::application::staff_permissions::StaffError::NotActive => ReviewError::NotActive,
            crate::application::staff_permissions::StaffError::NotPermitted => {
                ReviewError::NotPermitted
            }
            _ => ReviewError::StorageFailed,
        })?;
    let total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM report_cases WHERE status IN ('open', 'review')")
            .fetch_one(pool)
            .await
            .map_err(|_| ReviewError::StorageFailed)?;
    let rows: Vec<QueueRow> = sqlx::query_as(
        "SELECT c.id, c.status, c.severity, c.created_at, c.updated_at,
                count(r.id) AS report_count
         FROM report_cases c LEFT JOIN reports r ON r.case_id = c.id
         WHERE c.status IN ('open', 'review')
         GROUP BY c.id
         ORDER BY CASE c.severity
                    WHEN 'critical' THEN 0 WHEN 'high' THEN 1
                    WHEN 'normal' THEN 2 WHEN 'low' THEN 3 ELSE 4 END,
                  c.created_at, c.id
         LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(|_| ReviewError::StorageFailed)?;
    let mut cases = Vec::with_capacity(rows.len());
    for row in rows {
        let id = row.id;
        let reasons: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT reason FROM reports WHERE case_id = $1 ORDER BY reason",
        )
        .bind(id)
        .fetch_all(pool)
        .await
        .map_err(|_| ReviewError::StorageFailed)?;
        let targets: Vec<(String, uuid::Uuid, String)> = sqlx::query_as(
            "SELECT target_kind, target_id, target_title_snapshot FROM reports
             WHERE case_id = $1 ORDER BY created_at, id LIMIT $2",
        )
        .bind(id)
        .bind(QUEUE_TARGET_SAMPLE)
        .fetch_all(pool)
        .await
        .map_err(|_| ReviewError::StorageFailed)?;
        cases.push(CaseSummary {
            case_id: id,
            status: row.status,
            severity: row.severity,
            report_count: row.report_count,
            reasons,
            targets: targets
                .into_iter()
                .map(|(target_kind, target_id, title_snapshot)| TargetSample {
                    target_kind,
                    target_id,
                    title_snapshot,
                })
                .collect(),
            created_at: row.created_at,
            updated_at: row.updated_at,
        });
    }
    Ok(QueuePage {
        cases,
        total,
        limit,
        offset,
    })
}

/// Start review on one open case: triage the severity, move open to review,
/// audit the inspection, and record the fact. Assessment only — nothing
/// else moves.
///
/// # Errors
///
/// Returns [`ReviewError::InvalidField`] for unknown severity or bad
/// purpose/policy text, [`ReviewError::NotActive`] for restricted callers,
/// [`ReviewError::NotFound`] for unknown cases,
/// [`ReviewError::NotPermitted`] for ungranted callers,
/// [`ReviewError::InvalidState`] unless the case is open, else
/// [`ReviewError::StorageFailed`]. Reasons are static.
pub async fn open_review(
    pool: &sqlx::PgPool,
    moderator_id: uuid::Uuid,
    case_id: uuid::Uuid,
    input: OpenReviewInput,
) -> Result<OpenedReview, ReviewError> {
    if !REPORT_SEVERITIES.contains(&input.severity.as_str()) {
        return Err(ReviewError::InvalidField);
    }
    check_bounded(&input.policy_version, REVIEW_POLICY_MAX_CHARS, true)?;
    check_bounded(&input.purpose, REVIEW_PURPOSE_MAX_CHARS, true)?;
    authorized_inspect(
        pool,
        moderator_id,
        InspectInput {
            target_kind: "case".to_owned(),
            target_id: case_id,
            purpose: input.purpose,
            policy_version: input.policy_version.clone(),
        },
    )
    .await
    .map_err(|error| match error {
        crate::application::staff_permissions::StaffError::InvalidField => {
            ReviewError::InvalidField
        }
        crate::application::staff_permissions::StaffError::NotActive => ReviewError::NotActive,
        crate::application::staff_permissions::StaffError::NotFound => ReviewError::NotFound,
        crate::application::staff_permissions::StaffError::NotPermitted => {
            ReviewError::NotPermitted
        }
        _ => ReviewError::StorageFailed,
    })?;
    let mut tx = pool.begin().await.map_err(|_| ReviewError::StorageFailed)?;
    let standing: Option<String> =
        sqlx::query_scalar("SELECT status FROM report_cases WHERE id = $1")
            .bind(case_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| ReviewError::StorageFailed)?;
    match standing.as_deref() {
        Some("open") => {}
        Some(_) => {
            tx.rollback()
                .await
                .map_err(|_| ReviewError::StorageFailed)?;
            return Err(ReviewError::InvalidState);
        }
        None => {
            tx.rollback()
                .await
                .map_err(|_| ReviewError::StorageFailed)?;
            return Err(ReviewError::NotFound);
        }
    }
    let row: Option<(chrono::DateTime<chrono::Utc>,)> = sqlx::query_as(
        "UPDATE report_cases SET status = 'review', severity = $2, updated_at = now()
         WHERE id = $1 RETURNING updated_at",
    )
    .bind(case_id)
    .bind(&input.severity)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ReviewError::StorageFailed)?;
    if row.is_none() {
        tx.rollback()
            .await
            .map_err(|_| ReviewError::StorageFailed)?;
        return Err(ReviewError::StorageFailed);
    }
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(moderator_id),
            resource_kind: "case",
            resource_id: case_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "case.review_started",
            policy: "mvp-free",
            source: "staff",
            payload: serde_json::json!({
                "severity": input.severity,
                "policy_version": input.policy_version,
            }),
        },
    )
    .await
    .map_err(|_| ReviewError::StorageFailed)?;
    tx.commit().await.map_err(|_| ReviewError::StorageFailed)?;
    Ok(OpenedReview {
        case_id,
        status: "review".to_owned(),
        severity: input.severity,
    })
}

/// Decide one case under review: record the disposition with its reason,
/// evidence references, and policy version, audit the inspection, and
/// record the fact. The disposition assesses the allegation and nothing
/// else — later cards own enforcement.
///
/// # Errors
///
/// Returns [`ReviewError::InvalidField`] for unknown dispositions or bad
/// reason/evidence/purpose/policy text, [`ReviewError::NotActive`] for
/// restricted callers, [`ReviewError::NotFound`] for unknown cases,
/// [`ReviewError::NotPermitted`] for ungranted callers,
/// [`ReviewError::InvalidState`] unless the case is under review, else
/// [`ReviewError::StorageFailed`]. Reasons are static.
pub async fn decide_case(
    pool: &sqlx::PgPool,
    moderator_id: uuid::Uuid,
    case_id: uuid::Uuid,
    input: DecideInput,
) -> Result<CaseDecision, ReviewError> {
    if !DISPOSITIONS.contains(&input.disposition.as_str()) {
        return Err(ReviewError::InvalidField);
    }
    check_bounded(&input.reason, REVIEW_REASON_MAX_CHARS, true)?;
    if let Some(evidence) = &input.evidence {
        check_bounded(evidence, REVIEW_EVIDENCE_MAX_CHARS, false)?;
    }
    check_bounded(&input.policy_version, REVIEW_POLICY_MAX_CHARS, true)?;
    check_bounded(&input.purpose, REVIEW_PURPOSE_MAX_CHARS, true)?;
    authorized_inspect(
        pool,
        moderator_id,
        InspectInput {
            target_kind: "case".to_owned(),
            target_id: case_id,
            purpose: input.purpose,
            policy_version: input.policy_version.clone(),
        },
    )
    .await
    .map_err(|error| match error {
        crate::application::staff_permissions::StaffError::InvalidField => {
            ReviewError::InvalidField
        }
        crate::application::staff_permissions::StaffError::NotActive => ReviewError::NotActive,
        crate::application::staff_permissions::StaffError::NotFound => ReviewError::NotFound,
        crate::application::staff_permissions::StaffError::NotPermitted => {
            ReviewError::NotPermitted
        }
        _ => ReviewError::StorageFailed,
    })?;
    let mut tx = pool.begin().await.map_err(|_| ReviewError::StorageFailed)?;
    let standing: Option<String> =
        sqlx::query_scalar("SELECT status FROM report_cases WHERE id = $1")
            .bind(case_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| ReviewError::StorageFailed)?;
    match standing.as_deref() {
        Some("review") => {}
        Some(_) => {
            tx.rollback()
                .await
                .map_err(|_| ReviewError::StorageFailed)?;
            return Err(ReviewError::InvalidState);
        }
        None => {
            tx.rollback()
                .await
                .map_err(|_| ReviewError::StorageFailed)?;
            return Err(ReviewError::NotFound);
        }
    }
    let updated: Option<i32> = sqlx::query_scalar(
        "UPDATE report_cases SET status = $2, updated_at = now()
         WHERE id = $1 RETURNING 1",
    )
    .bind(case_id)
    .bind(&input.disposition)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ReviewError::StorageFailed)?;
    if updated.is_none() {
        tx.rollback()
            .await
            .map_err(|_| ReviewError::StorageFailed)?;
        return Err(ReviewError::StorageFailed);
    }
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(moderator_id),
            resource_kind: "case",
            resource_id: case_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "case.decided",
            policy: "mvp-free",
            source: "staff",
            payload: serde_json::json!({
                "disposition": input.disposition,
                "reason": input.reason,
                "evidence": input.evidence,
                "policy_version": input.policy_version,
            }),
        },
    )
    .await
    .map_err(|_| ReviewError::StorageFailed)?;
    tx.commit().await.map_err(|_| ReviewError::StorageFailed)?;
    Ok(CaseDecision {
        case_id,
        status: input.disposition,
        reason: input.reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_and_disposition_vocabularies_are_closed() {
        for severity in ["critical", "high", "normal", "low"] {
            assert!(REPORT_SEVERITIES.contains(&severity));
        }
        assert!(!REPORT_SEVERITIES.contains(&"urgent"));
        for disposition in ["valid", "invalid", "duplicate"] {
            assert!(DISPOSITIONS.contains(&disposition));
        }
        assert!(!DISPOSITIONS.contains(&"malicious"));
        assert!(!DISPOSITIONS.contains(&"withdrawn"));
    }

    #[test]
    fn queue_paging_has_bounds() {
        assert!(check_queue_page(20, 0).is_ok());
        assert!(check_queue_page(1, 0).is_ok());
        assert!(check_queue_page(100, 40).is_ok());
        assert_eq!(check_queue_page(0, 0), Err(ReviewError::InvalidField));
        assert_eq!(check_queue_page(101, 0), Err(ReviewError::InvalidField));
        assert_eq!(check_queue_page(20, -1), Err(ReviewError::InvalidField));
    }

    #[test]
    fn review_text_has_bounds() {
        assert!(check_bounded("triage note", REVIEW_REASON_MAX_CHARS, true).is_ok());
        assert_eq!(
            check_bounded("   ", REVIEW_REASON_MAX_CHARS, true),
            Err(ReviewError::InvalidField)
        );
        assert_eq!(
            check_bounded(&"x".repeat(1001), REVIEW_REASON_MAX_CHARS, true),
            Err(ReviewError::InvalidField)
        );
        assert!(check_bounded("", REVIEW_EVIDENCE_MAX_CHARS, false).is_ok());
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            ReviewError::InvalidField,
            ReviewError::NotActive,
            ReviewError::NotFound,
            ReviewError::NotPermitted,
            ReviewError::InvalidState,
            ReviewError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
