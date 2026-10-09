//! Bounded appeals and accountable corrections: review without rewriting.
//!
//! Canonical rules: INV-34 (one appeal per appellant and decided case —
//! repeats converge, and later material evidence appends as its own rows),
//! INV-36 with INV-42 (allegations stay allegations; corrections travel as
//! separate business facts so derived reputation and metrics stay
//! traceable — the original allegation, decision, and outcome rows are
//! never rewritten here), INV-31 (appeal rows reference the grouped case
//! and the filing appellant only; other reporters never enter an appeal,
//! and appeal reads return the appeal row alone), INV-37 with AC-39
//! (actor, reason, scope, time, and previous/resulting standing recorded
//! through an inspection audit plus durable facts), INV-38 (reversal moves
//! the case back to review only — requests, outcomes, contacts, accounts,
//! and blocks stand untouched), INV-43 with EC-27 (no correction
//! fabricates verified transactions, reopens terminal requests, or
//! manufactures contact), and spec 16.4 (upheld, narrowed, or reversed
//! outcomes with the deciding staff member on record).
//!
//! Appellants are case parties only: a member reporter or a target owner.
//! Restricted accounts keep this path for their own history; pending,
//! deleted, and missing accounts refuse.

use serde::Serialize;

use crate::application::staff_permissions::{authorized_inspect, InspectInput, StaffError};
use crate::persistence::events::{record as record_event, NewEvent};

/// Appeal outcomes: the recorded reading of the challenged decision.
pub const APPEAL_OUTCOMES: [&str; 3] = ["upheld", "narrowed", "reversed"];
/// Decided case standings open to appeal (staff decisions only).
pub const APPEALABLE_CASE: [&str; 3] = ["valid", "invalid", "duplicate"];
/// Grounds bound in scalar values (mirrors the database backstop).
pub const APPEAL_GROUNDS_MAX_CHARS: usize = 1000;
/// Evidence-statement bound in scalar values (mirrors the backstop).
pub const APPEAL_EVIDENCE_MAX_CHARS: usize = 1000;
/// Decision-reason bound in scalar values.
pub const APPEAL_REASON_MAX_CHARS: usize = 1000;
/// Purpose bound in scalar values.
pub const APPEAL_PURPOSE_MAX_CHARS: usize = 500;
/// Policy-version bound in scalar values.
pub const APPEAL_POLICY_MAX_CHARS: usize = 32;

/// One appeal as supplied: which decided case is challenged and on what
/// basis.
#[derive(Debug, Clone)]
pub struct AppealInput {
    /// Decided case under challenge (must already stand decided).
    pub case_id: uuid::Uuid,
    /// Why the decision is challenged.
    pub grounds: String,
}

/// One appeal decision as supplied: the staff reading with its references.
#[derive(Debug, Clone)]
pub struct AppealDecisionInput {
    /// `upheld`, `narrowed`, or `reversed`.
    pub outcome: String,
    /// Why this reading was reached.
    pub reason: String,
    /// Rules version the decision applies.
    pub policy_version: String,
    /// Assigned safety purpose (recorded in the inspection audit).
    pub purpose: String,
}

/// One persisted appeal: the challenge and its reading, if decided.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Appeal {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Challenged case.
    pub case_id: uuid::Uuid,
    /// Filing party (a case reporter or target owner).
    pub appellant_id: uuid::Uuid,
    /// Initial challenge statement (immutable afterwards).
    pub grounds: String,
    /// `open`, `upheld`, `narrowed`, or `reversed`.
    pub status: String,
    /// Deciding staff member, once decided.
    pub decided_by: Option<uuid::Uuid>,
    /// Decision instant, once decided.
    pub decided_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Decision rationale, once decided.
    pub decision_reason: String,
    /// Filing instant.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// One persisted evidence statement on an open appeal.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppealEvidence {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Owning appeal.
    pub appeal_id: uuid::Uuid,
    /// Submitting appellant.
    pub submitted_by: uuid::Uuid,
    /// Bounded material statement.
    pub statement: String,
    /// Submission instant.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Typed appeal failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppealError {
    /// Unknown outcome or bad grounds/statement/reason/purpose/policy text.
    InvalidField,
    /// The account is missing, pending, or deleted.
    NotActive,
    /// No such case or appeal exists.
    NotFound,
    /// The caller is no party to the case, files on a foreign appeal, or
    /// holds no live moderator-or-better grant for decisions.
    NotPermitted,
    /// The case stands undecided, or the appeal already stands decided.
    InvalidState,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for AppealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid appeal field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("appeal target not found"),
            Self::NotPermitted => f.write_str("appeal not permitted"),
            Self::InvalidState => f.write_str("appeal is outside its standing"),
            Self::StorageFailed => f.write_str("appeal storage failed"),
        }
    }
}

impl std::error::Error for AppealError {}

fn check_bounded(value: &str, max: usize) -> Result<(), AppealError> {
    let count = value.chars().count();
    if value.trim().is_empty() || count > max {
        return Err(AppealError::InvalidField);
    }
    Ok(())
}

fn staff_error(error: StaffError) -> AppealError {
    match error {
        StaffError::InvalidField => AppealError::InvalidField,
        StaffError::NotActive => AppealError::NotActive,
        StaffError::NotFound => AppealError::NotFound,
        StaffError::NotPermitted => AppealError::NotPermitted,
        StaffError::StorageFailed => AppealError::StorageFailed,
    }
}

fn read_appeal(row: &sqlx::postgres::PgRow) -> Result<Appeal, AppealError> {
    use sqlx::Row;
    Ok(Appeal {
        id: row.try_get("id").map_err(|_| AppealError::StorageFailed)?,
        case_id: row
            .try_get("case_id")
            .map_err(|_| AppealError::StorageFailed)?,
        appellant_id: row
            .try_get("appellant_id")
            .map_err(|_| AppealError::StorageFailed)?,
        grounds: row
            .try_get("grounds")
            .map_err(|_| AppealError::StorageFailed)?,
        status: row
            .try_get("status")
            .map_err(|_| AppealError::StorageFailed)?,
        decided_by: row
            .try_get("decided_by")
            .map_err(|_| AppealError::StorageFailed)?,
        decided_at: row
            .try_get("decided_at")
            .map_err(|_| AppealError::StorageFailed)?,
        decision_reason: row
            .try_get("decision_reason")
            .map_err(|_| AppealError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| AppealError::StorageFailed)?,
    })
}

fn read_evidence(row: &sqlx::postgres::PgRow) -> Result<AppealEvidence, AppealError> {
    use sqlx::Row;
    Ok(AppealEvidence {
        id: row.try_get("id").map_err(|_| AppealError::StorageFailed)?,
        appeal_id: row
            .try_get("appeal_id")
            .map_err(|_| AppealError::StorageFailed)?,
        submitted_by: row
            .try_get("submitted_by")
            .map_err(|_| AppealError::StorageFailed)?,
        statement: row
            .try_get("statement")
            .map_err(|_| AppealError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| AppealError::StorageFailed)?,
    })
}

const APPEAL_COLUMNS: &str = "id, case_id, appellant_id, grounds, status, decided_by, decided_at, decision_reason, created_at";
const EVIDENCE_COLUMNS: &str = "id, appeal_id, submitted_by, statement, created_at";

/// Case-party standing: a member reporter or a target owner. Pending,
/// deleted, and missing accounts refuse before standing is even read —
/// restricted accounts keep this path for their own history.
async fn party_to_case(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    appellant_id: uuid::Uuid,
    case_id: uuid::Uuid,
) -> Result<bool, AppealError> {
    let caller: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1")
            .bind(appellant_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| AppealError::StorageFailed)?;
    match caller {
        Some((state, None)) if state == "active" || state == "suspended" || state == "banned" => {}
        _ => return Err(AppealError::NotActive),
    }
    let reporter: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM reports WHERE case_id = $1 AND reporter_id = $2")
            .bind(case_id)
            .bind(appellant_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| AppealError::StorageFailed)?;
    if reporter.is_some() {
        return Ok(true);
    }
    let targets: Vec<(String, uuid::Uuid)> =
        sqlx::query_as("SELECT target_kind, target_id FROM reports WHERE case_id = $1")
            .bind(case_id)
            .fetch_all(&mut **tx)
            .await
            .map_err(|_| AppealError::StorageFailed)?;
    for (kind, target) in targets {
        let owned: Option<i32> = match kind.as_str() {
            "request" => {
                sqlx::query_scalar("SELECT 1 FROM requests WHERE id = $1 AND author_id = $2")
                    .bind(target)
                    .bind(appellant_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|_| AppealError::StorageFailed)?
            }
            "offer" => sqlx::query_scalar("SELECT 1 FROM offers WHERE id = $1 AND seller_id = $2")
                .bind(target)
                .bind(appellant_id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(|_| AppealError::StorageFailed)?,
            _ => {
                if target == appellant_id {
                    Some(1)
                } else {
                    None
                }
            }
        };
        if owned.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// File one appeal against a decided case: the appellant must be a case
/// party with own history. Refiling converges on the standing appeal with
/// no new row and no new case — later material belongs to evidence.
///
/// # Errors
///
/// Returns [`AppealError::InvalidField`] for bad grounds text,
/// [`AppealError::NotActive`] for pending/deleted/missing appellants,
/// [`AppealError::NotFound`] for unknown cases,
/// [`AppealError::NotPermitted`] for non-party appellants,
/// [`AppealError::InvalidState`] unless the case stands decided, else
/// [`AppealError::StorageFailed`]. Reasons are static.
pub async fn file_appeal(
    pool: &sqlx::PgPool,
    appellant_id: uuid::Uuid,
    input: AppealInput,
) -> Result<Appeal, AppealError> {
    check_bounded(&input.grounds, APPEAL_GROUNDS_MAX_CHARS)?;
    let mut tx = pool.begin().await.map_err(|_| AppealError::StorageFailed)?;
    let standing: Option<String> =
        sqlx::query_scalar("SELECT status FROM report_cases WHERE id = $1")
            .bind(input.case_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| AppealError::StorageFailed)?;
    let standing = match standing {
        Some(standing) => standing,
        None => {
            tx.rollback()
                .await
                .map_err(|_| AppealError::StorageFailed)?;
            return Err(AppealError::NotFound);
        }
    };
    if !APPEALABLE_CASE.contains(&standing.as_str()) {
        tx.rollback()
            .await
            .map_err(|_| AppealError::StorageFailed)?;
        return Err(AppealError::InvalidState);
    }
    if !party_to_case(&mut tx, appellant_id, input.case_id).await? {
        tx.rollback()
            .await
            .map_err(|_| AppealError::StorageFailed)?;
        // Standing gate first: party_to_case already refused restricted
        // callers as NotActive, so this refusal names foreign hands.
        return Err(AppealError::NotPermitted);
    }
    let existing: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {APPEAL_COLUMNS} FROM appeals
         WHERE case_id = $1 AND appellant_id = $2 LIMIT 1"
    ))
    .bind(input.case_id)
    .bind(appellant_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| AppealError::StorageFailed)?;
    if let Some(row) = existing {
        let standing = read_appeal(&row)?;
        tx.rollback()
            .await
            .map_err(|_| AppealError::StorageFailed)?;
        return Ok(standing);
    }
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "INSERT INTO appeals (case_id, appellant_id, grounds)
         VALUES ($1, $2, $3) RETURNING {APPEAL_COLUMNS}"
    ))
    .bind(input.case_id)
    .bind(appellant_id)
    .bind(&input.grounds)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| AppealError::StorageFailed)?;
    let filed = read_appeal(&row)?;
    tx.commit().await.map_err(|_| AppealError::StorageFailed)?;
    Ok(filed)
}

/// Append one material evidence statement to an open appeal. The initial
/// grounds stay immutable; decided appeals refuse (their reading stands —
/// a later challenge is a new appeal, not a silent edit).
///
/// # Errors
///
/// Returns [`AppealError::InvalidField`] for bad statements,
/// [`AppealError::NotActive`] for pending/deleted/missing callers,
/// [`AppealError::NotFound`] for unknown appeals,
/// [`AppealError::NotPermitted`] for foreign appeals,
/// [`AppealError::InvalidState`] unless the appeal is open, else
/// [`AppealError::StorageFailed`]. Reasons are static.
pub async fn submit_evidence(
    pool: &sqlx::PgPool,
    appellant_id: uuid::Uuid,
    appeal_id: uuid::Uuid,
    statement: String,
) -> Result<AppealEvidence, AppealError> {
    check_bounded(&statement, APPEAL_EVIDENCE_MAX_CHARS)?;
    let mut tx = pool.begin().await.map_err(|_| AppealError::StorageFailed)?;
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {APPEAL_COLUMNS} FROM appeals WHERE id = $1"
    ))
    .bind(appeal_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| AppealError::StorageFailed)?;
    let appeal = match row {
        Some(row) => read_appeal(&row)?,
        None => {
            tx.rollback()
                .await
                .map_err(|_| AppealError::StorageFailed)?;
            return Err(AppealError::NotFound);
        }
    };
    if appeal.appellant_id != appellant_id {
        tx.rollback()
            .await
            .map_err(|_| AppealError::StorageFailed)?;
        return Err(AppealError::NotPermitted);
    }
    let caller: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1")
            .bind(appellant_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| AppealError::StorageFailed)?;
    match caller {
        Some((state, None)) if state == "active" || state == "suspended" || state == "banned" => {}
        _ => {
            tx.rollback()
                .await
                .map_err(|_| AppealError::StorageFailed)?;
            return Err(AppealError::NotActive);
        }
    }
    if appeal.status != "open" {
        tx.rollback()
            .await
            .map_err(|_| AppealError::StorageFailed)?;
        return Err(AppealError::InvalidState);
    }
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "INSERT INTO appeal_evidence (appeal_id, submitted_by, statement)
         VALUES ($1, $2, $3) RETURNING {EVIDENCE_COLUMNS}"
    ))
    .bind(appeal_id)
    .bind(appellant_id)
    .bind(&statement)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| AppealError::StorageFailed)?;
    let stored = read_evidence(&row)?;
    tx.commit().await.map_err(|_| AppealError::StorageFailed)?;
    Ok(stored)
}

/// Decide one open appeal with a recorded reading: upheld keeps the case
/// standing, narrowed records the reduced finding, and reversed returns
/// the case to review for a fresh decision. Every path records the
/// decision fact; narrowed and reversed additionally record a separate
/// correction fact for derived reputation and metrics. Requests,
/// outcomes, contacts, accounts, and blocks stand untouched — terminal
/// rows never reopen and no contact is fabricated here.
///
/// # Errors
///
/// Returns [`AppealError::InvalidField`] for unknown outcomes or bad
/// reason/purpose/policy text, [`AppealError::NotActive`] for restricted
/// staff, [`AppealError::NotFound`] for unknown appeals,
/// [`AppealError::NotPermitted`] for ungranted staff,
/// [`AppealError::InvalidState`] unless the appeal is open, else
/// [`AppealError::StorageFailed`]. Reasons are static.
pub async fn decide_appeal(
    pool: &sqlx::PgPool,
    staff_id: uuid::Uuid,
    appeal_id: uuid::Uuid,
    input: AppealDecisionInput,
) -> Result<Appeal, AppealError> {
    if !APPEAL_OUTCOMES.contains(&input.outcome.as_str()) {
        return Err(AppealError::InvalidField);
    }
    check_bounded(&input.reason, APPEAL_REASON_MAX_CHARS)?;
    check_bounded(&input.policy_version, APPEAL_POLICY_MAX_CHARS)?;
    check_bounded(&input.purpose, APPEAL_PURPOSE_MAX_CHARS)?;
    let mut tx = pool.begin().await.map_err(|_| AppealError::StorageFailed)?;
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {APPEAL_COLUMNS} FROM appeals WHERE id = $1"
    ))
    .bind(appeal_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| AppealError::StorageFailed)?;
    let appeal = match row {
        Some(row) => read_appeal(&row)?,
        None => {
            tx.rollback()
                .await
                .map_err(|_| AppealError::StorageFailed)?;
            return Err(AppealError::NotFound);
        }
    };
    if appeal.status != "open" {
        tx.rollback()
            .await
            .map_err(|_| AppealError::StorageFailed)?;
        return Err(AppealError::InvalidState);
    }
    tx.commit().await.map_err(|_| AppealError::StorageFailed)?;
    authorized_inspect(
        pool,
        staff_id,
        InspectInput {
            target_kind: "case".to_owned(),
            target_id: appeal.case_id,
            purpose: input.purpose,
            policy_version: input.policy_version.clone(),
        },
    )
    .await
    .map_err(staff_error)?;
    let mut tx = pool.begin().await.map_err(|_| AppealError::StorageFailed)?;
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "UPDATE appeals SET status = $2, decided_by = $3, decided_at = now(),
                decision_reason = $4
         WHERE id = $1 AND status = 'open' RETURNING {APPEAL_COLUMNS}"
    ))
    .bind(appeal_id)
    .bind(&input.outcome)
    .bind(staff_id)
    .bind(&input.reason)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| AppealError::StorageFailed)?;
    let decided = match row {
        Some(row) => read_appeal(&row)?,
        None => {
            tx.rollback()
                .await
                .map_err(|_| AppealError::StorageFailed)?;
            return Err(AppealError::InvalidState);
        }
    };
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(staff_id),
            resource_kind: "appeal",
            resource_id: appeal_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "appeal.decided",
            policy: "mvp-free",
            source: "staff",
            payload: serde_json::json!({
                "outcome": input.outcome,
                "reason": input.reason,
                "case_id": appeal.case_id,
                "policy_version": input.policy_version,
            }),
        },
    )
    .await
    .map_err(|_| AppealError::StorageFailed)?;
    if input.outcome == "narrowed" || input.outcome == "reversed" {
        record_event(
            &mut *tx,
            NewEvent {
                actor_id: Some(staff_id),
                resource_kind: "appeal",
                resource_id: appeal_id,
                cycle: None,
                revision: None,
                effective_at: chrono::Utc::now(),
                kind: "appeal.corrected",
                policy: "mvp-free",
                source: "staff",
                payload: serde_json::json!({
                    "outcome": input.outcome,
                    "reason": input.reason,
                    "case_id": appeal.case_id,
                    "policy_version": input.policy_version,
                    "note": "derived reputation and metrics correction",
                }),
            },
        )
        .await
        .map_err(|_| AppealError::StorageFailed)?;
    }
    if input.outcome == "reversed" {
        sqlx::query(
            "UPDATE report_cases SET status = 'review', updated_at = now()
             WHERE id = $1",
        )
        .bind(appeal.case_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| AppealError::StorageFailed)?;
        record_event(
            &mut *tx,
            NewEvent {
                actor_id: Some(staff_id),
                resource_kind: "case",
                resource_id: appeal.case_id,
                cycle: None,
                revision: None,
                effective_at: chrono::Utc::now(),
                kind: "case.reopened",
                policy: "mvp-free",
                source: "staff",
                payload: serde_json::json!({
                    "reason": input.reason,
                    "appeal_id": appeal_id,
                    "policy_version": input.policy_version,
                }),
            },
        )
        .await
        .map_err(|_| AppealError::StorageFailed)?;
    }
    tx.commit().await.map_err(|_| AppealError::StorageFailed)?;
    Ok(decided)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appeal_vocabulary_is_closed() {
        for outcome in ["upheld", "narrowed", "reversed"] {
            assert!(APPEAL_OUTCOMES.contains(&outcome));
        }
        assert!(!APPEAL_OUTCOMES.contains(&"dismissed"));
        for standing in ["valid", "invalid", "duplicate"] {
            assert!(APPEALABLE_CASE.contains(&standing));
        }
        assert!(!APPEALABLE_CASE.contains(&"open"));
        assert!(!APPEALABLE_CASE.contains(&"review"));
        assert!(!APPEALABLE_CASE.contains(&"withdrawn"));
    }

    #[test]
    fn appeal_text_has_bounds() {
        assert!(check_bounded("the finding missed context", APPEAL_GROUNDS_MAX_CHARS).is_ok());
        assert_eq!(
            check_bounded("   ", APPEAL_GROUNDS_MAX_CHARS),
            Err(AppealError::InvalidField)
        );
        assert_eq!(
            check_bounded(&"x".repeat(1001), APPEAL_GROUNDS_MAX_CHARS),
            Err(AppealError::InvalidField)
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            AppealError::InvalidField,
            AppealError::NotActive,
            AppealError::NotFound,
            AppealError::NotPermitted,
            AppealError::InvalidState,
            AppealError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
