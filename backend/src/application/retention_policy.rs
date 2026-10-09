//! Retention classification and reviewed holds: windows with owners.
//!
//! Canonical rules: privacy-and-security 15.3 proposed defaults (ordinary
//! removed-content and contact context up to 90 days after closure or
//! deletion; open safety incidents with necessity reviews every 90 days;
//! closed safety evidence 180 days maximum after decision; the minimum
//! explainable moderation trail; commercial-record retention future-only
//! with no permanent default; every exception naming basis, scope, owner,
//! and review/end condition), INV-34 (one open hold per subject and class
//! — repeats converge), INV-31 with INV-44 (identifiers, classes,
//! purposes, due dates, and decision metadata only — no reporter, phone,
//! or secret material anywhere here), and EC-38 (incident rows stay for
//! policy review under holds; the cleanup worker later reads open holds
//! past due).
//!
//! No silent permanence exists: statuses are open or released only, expiry
//! changes nothing by itself, and only explicit review or release writes
//! move a hold. Reviews extend incident holds by one 90-day window and
//! otherwise record without moving the due date.

use serde::Serialize;

use crate::application::staff_permissions::StaffError;
use crate::persistence::events::{record as record_event, NewEvent};

/// Ordinary removed-content and contact context.
pub const CLASS_ORDINARY: &str = "ordinary";
/// Open safety incident evidence under necessity review.
pub const CLASS_INCIDENT: &str = "incident";
/// Minimum explainable moderation decision trail.
pub const CLASS_AUDIT: &str = "audit";
/// Documented obligation-based exception with owner and review condition.
pub const CLASS_EXCEPTION: &str = "exception";

/// Ordinary window in days after closure or deletion.
pub const ORDINARY_WINDOW_DAYS: i64 = 90;
/// Incident necessity-review cadence in days.
pub const INCIDENT_REVIEW_DAYS: i64 = 90;
/// Closed-evidence and audit maximum in days.
pub const CLOSED_MAX_DAYS: i64 = 180;
/// Purpose bound in scalar values (mirrors the database backstop).
pub const HOLD_PURPOSE_MAX_CHARS: usize = 500;
/// Basis bound in scalar values (mirrors the backstop).
pub const HOLD_BASIS_MAX_CHARS: usize = 500;
/// Review-condition bound in scalar values (mirrors the backstop).
pub const HOLD_REVIEW_MAX_CHARS: usize = 500;
/// Review-note bound in scalar values.
pub const HOLD_NOTE_MAX_CHARS: usize = 500;
/// Release-reason bound in scalar values.
pub const HOLD_RELEASE_MAX_CHARS: usize = 1000;

/// One hold classification as supplied: what is retained, why, and until
/// when.
#[derive(Debug, Clone)]
pub struct ClassifyInput {
    /// `user`, `request`, `offer`, `contact`, `report`, or `case`.
    pub subject_kind: String,
    /// Retained identifier (must already exist).
    pub subject_id: uuid::Uuid,
    /// `ordinary`, `incident`, `audit`, or `exception`.
    pub class: String,
    /// Stated retention purpose.
    pub purpose: String,
    /// Reference instant (closure/deletion/decision time, or now for open
    /// incident intake).
    pub anchor_at: chrono::DateTime<chrono::Utc>,
    /// Explicit end for exceptions (required, future-dated).
    pub ends_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Obligation basis (exceptions only).
    pub basis: Option<String>,
    /// Review or end condition (exceptions only).
    pub review_condition: Option<String>,
    /// Accountable owner (exceptions only).
    pub owner_id: Option<uuid::Uuid>,
}

/// One persisted retention hold.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RetentionHold {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Retained family.
    pub subject_kind: String,
    /// Retained identifier.
    pub subject_id: uuid::Uuid,
    /// Retention class.
    pub class: String,
    /// Stated purpose.
    pub purpose: String,
    /// Review-or-removal deadline.
    pub due_at: chrono::DateTime<chrono::Utc>,
    /// Obligation basis (exceptions only).
    pub basis: Option<String>,
    /// Review or end condition (exceptions only).
    pub review_condition: Option<String>,
    /// Accountable owner (exceptions only).
    pub owner_id: Option<uuid::Uuid>,
    /// `open` or `released`.
    pub status: String,
    /// Classifying staff member, if any (automatic paths record none).
    pub created_by: Option<uuid::Uuid>,
    /// Last necessity-review instant, if reviewed.
    pub reviewed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Last reviewer, if reviewed.
    pub reviewed_by: Option<uuid::Uuid>,
    /// Release instant, if released.
    pub released_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Classification instant.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Typed retention failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionError {
    /// Unknown kind/class or bad purpose/basis/review/note/reason text.
    InvalidField,
    /// The caller's account is missing, pending, suspended, banned, or deleted.
    NotActive,
    /// No such subject or hold exists.
    NotFound,
    /// The caller holds no live moderator-or-better grant.
    NotPermitted,
    /// The hold already stands released, or is not open for this action.
    InvalidState,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for RetentionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid retention field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("retention target not found"),
            Self::NotPermitted => f.write_str("retention not permitted"),
            Self::InvalidState => f.write_str("hold is outside its standing"),
            Self::StorageFailed => f.write_str("retention storage failed"),
        }
    }
}

impl std::error::Error for RetentionError {}

fn check_bounded(value: &str, max: usize) -> Result<(), RetentionError> {
    let count = value.chars().count();
    if value.trim().is_empty() || count > max {
        return Err(RetentionError::InvalidField);
    }
    Ok(())
}

fn staff_error(error: StaffError) -> RetentionError {
    match error {
        StaffError::InvalidField => RetentionError::InvalidField,
        StaffError::NotActive => RetentionError::NotActive,
        StaffError::NotFound => RetentionError::NotFound,
        StaffError::NotPermitted => RetentionError::NotPermitted,
        StaffError::StorageFailed => RetentionError::StorageFailed,
    }
}

fn read_hold(row: &sqlx::postgres::PgRow) -> Result<RetentionHold, RetentionError> {
    use sqlx::Row;
    Ok(RetentionHold {
        id: row
            .try_get("id")
            .map_err(|_| RetentionError::StorageFailed)?,
        subject_kind: row
            .try_get("subject_kind")
            .map_err(|_| RetentionError::StorageFailed)?,
        subject_id: row
            .try_get("subject_id")
            .map_err(|_| RetentionError::StorageFailed)?,
        class: row
            .try_get("class")
            .map_err(|_| RetentionError::StorageFailed)?,
        purpose: row
            .try_get("purpose")
            .map_err(|_| RetentionError::StorageFailed)?,
        due_at: row
            .try_get("due_at")
            .map_err(|_| RetentionError::StorageFailed)?,
        basis: row
            .try_get("basis")
            .map_err(|_| RetentionError::StorageFailed)?,
        review_condition: row
            .try_get("review_condition")
            .map_err(|_| RetentionError::StorageFailed)?,
        owner_id: row
            .try_get("owner_id")
            .map_err(|_| RetentionError::StorageFailed)?,
        status: row
            .try_get("status")
            .map_err(|_| RetentionError::StorageFailed)?,
        created_by: row
            .try_get("created_by")
            .map_err(|_| RetentionError::StorageFailed)?,
        reviewed_at: row
            .try_get("reviewed_at")
            .map_err(|_| RetentionError::StorageFailed)?,
        reviewed_by: row
            .try_get("reviewed_by")
            .map_err(|_| RetentionError::StorageFailed)?,
        released_at: row
            .try_get("released_at")
            .map_err(|_| RetentionError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| RetentionError::StorageFailed)?,
    })
}

const HOLD_COLUMNS: &str = "id, subject_kind, subject_id, class, purpose, due_at, basis, review_condition, owner_id, status, created_by, reviewed_at, reviewed_by, released_at, created_at";

/// Canonical due date for a class: ordinary runs 90 days from the anchor,
/// incident and audit review from now (90/180 days), and exceptions end
/// exactly at their documented future instant.
fn due_for(
    class: &str,
    anchor_at: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
    ends_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<chrono::DateTime<chrono::Utc>, RetentionError> {
    match class {
        CLASS_ORDINARY => Ok(anchor_at + chrono::Duration::days(ORDINARY_WINDOW_DAYS)),
        CLASS_INCIDENT => Ok(now + chrono::Duration::days(INCIDENT_REVIEW_DAYS)),
        CLASS_AUDIT => Ok(now + chrono::Duration::days(CLOSED_MAX_DAYS)),
        CLASS_EXCEPTION => match ends_at {
            Some(ends_at) if ends_at > now => Ok(ends_at),
            _ => Err(RetentionError::InvalidField),
        },
        _ => Err(RetentionError::InvalidField),
    }
}

async fn subject_exists(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    kind: &str,
    subject_id: uuid::Uuid,
) -> Result<bool, RetentionError> {
    let table = match kind {
        "user" => "users",
        "request" => "requests",
        "offer" => "offers",
        "contact" => "contacts",
        "report" => "reports",
        "case" => "report_cases",
        _ => return Err(RetentionError::InvalidField),
    };
    let found: Option<i32> = sqlx::query_scalar(&format!("SELECT 1 FROM {table} WHERE id = $1"))
        .bind(subject_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| RetentionError::StorageFailed)?;
    Ok(found.is_some())
}

/// Classify one subject for retention with its canonical due date: staff
/// callers need a live moderator-or-better grant, while the automatic
/// deletion/closure paths classify with no actor. Repeats converge on the
/// standing open hold with no new fact.
///
/// # Errors
///
/// Returns [`RetentionError::InvalidField`] for unknown kinds/classes or
/// bad text and dates, [`RetentionError::NotActive`] for restricted
/// staff, [`RetentionError::NotFound`] for missing subjects,
/// [`RetentionError::NotPermitted`] for ungranted staff, else
/// [`RetentionError::StorageFailed`]. Reasons are static.
pub async fn classify_hold(
    pool: &sqlx::PgPool,
    actor_id: Option<uuid::Uuid>,
    input: ClassifyInput,
) -> Result<RetentionHold, RetentionError> {
    if ![CLASS_ORDINARY, CLASS_INCIDENT, CLASS_AUDIT, CLASS_EXCEPTION]
        .contains(&input.class.as_str())
    {
        return Err(RetentionError::InvalidField);
    }
    check_bounded(&input.purpose, HOLD_PURPOSE_MAX_CHARS)?;
    if input.class == CLASS_EXCEPTION {
        match (&input.basis, &input.review_condition, input.owner_id) {
            (Some(basis), Some(review), Some(_)) => {
                check_bounded(basis, HOLD_BASIS_MAX_CHARS)?;
                check_bounded(review, HOLD_REVIEW_MAX_CHARS)?;
            }
            _ => return Err(RetentionError::InvalidField),
        }
    }
    let now = chrono::Utc::now();
    let due_at = due_for(&input.class, input.anchor_at, now, input.ends_at)?;
    // Obligation exceptions are administrative-only (moderators decide
    // and suspend, but only administrators bind the product to an
    // external obligation); every other class takes either grant, and the
    // automatic deletion/closure paths classify with no actor.
    if input.class == CLASS_EXCEPTION {
        match actor_id {
            Some(actor) => {
                crate::application::staff_permissions::require_admin(pool, actor)
                    .await
                    .map_err(staff_error)?;
            }
            None => return Err(RetentionError::NotPermitted),
        }
    } else if let Some(actor) = actor_id {
        crate::application::staff_permissions::require_moderator(pool, actor)
            .await
            .map_err(staff_error)?;
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| RetentionError::StorageFailed)?;
    if !subject_exists(&mut tx, &input.subject_kind, input.subject_id).await? {
        tx.rollback()
            .await
            .map_err(|_| RetentionError::StorageFailed)?;
        return Err(RetentionError::NotFound);
    }
    let existing: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {HOLD_COLUMNS} FROM retention_holds
         WHERE subject_kind = $1 AND subject_id = $2 AND class = $3 AND status = 'open'
         LIMIT 1"
    ))
    .bind(&input.subject_kind)
    .bind(input.subject_id)
    .bind(&input.class)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| RetentionError::StorageFailed)?;
    if let Some(row) = existing {
        let standing = read_hold(&row)?;
        tx.rollback()
            .await
            .map_err(|_| RetentionError::StorageFailed)?;
        return Ok(standing);
    }
    let row: Result<sqlx::postgres::PgRow, sqlx::Error> = sqlx::query(&format!(
        "INSERT INTO retention_holds
            (subject_kind, subject_id, class, purpose, due_at, basis,
             review_condition, owner_id, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         RETURNING {HOLD_COLUMNS}"
    ))
    .bind(&input.subject_kind)
    .bind(input.subject_id)
    .bind(&input.class)
    .bind(&input.purpose)
    .bind(due_at)
    .bind(input.basis.as_deref())
    .bind(input.review_condition.as_deref())
    .bind(input.owner_id)
    .bind(actor_id)
    .fetch_one(&mut *tx)
    .await;
    let row = match row {
        Ok(row) => row,
        Err(error)
            if error
                .as_database_error()
                .is_some_and(|database| database.code().as_deref() == Some("23505")) =>
        {
            // A concurrent classification won the race: converge on its
            // row instead of duplicating the hold.
            let winner: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
                "SELECT {HOLD_COLUMNS} FROM retention_holds
                 WHERE subject_kind = $1 AND subject_id = $2 AND class = $3
                   AND status = 'open' LIMIT 1"
            ))
            .bind(&input.subject_kind)
            .bind(input.subject_id)
            .bind(&input.class)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| RetentionError::StorageFailed)?;
            let winner = winner.ok_or(RetentionError::StorageFailed)?;
            let standing = read_hold(&winner)?;
            tx.rollback()
                .await
                .map_err(|_| RetentionError::StorageFailed)?;
            return Ok(standing);
        }
        Err(_) => return Err(RetentionError::StorageFailed),
    };
    let hold = read_hold(&row)?;
    record_event(
        &mut *tx,
        NewEvent {
            actor_id,
            resource_kind: match input.subject_kind.as_str() {
                "user" => "user",
                "request" => "request",
                "offer" => "offer",
                "contact" => "contact",
                "report" => "report",
                _ => "case",
            },
            resource_id: input.subject_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "retention.hold_classified",
            policy: "mvp-free",
            source: if actor_id.is_some() { "staff" } else { "api" },
            payload: serde_json::json!({
                "class": hold.class,
                "purpose": hold.purpose,
                "due_at": hold.due_at,
            }),
        },
    )
    .await
    .map_err(|_| RetentionError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| RetentionError::StorageFailed)?;
    Ok(hold)
}

/// Record one necessity review on an open hold: incident holds extend by
/// one 90-day window, other classes keep their due date, and every class
/// records reviewer and instant. Expired holds review exactly like live
/// ones — expiry never decides anything by itself.
///
/// # Errors
///
/// Returns [`RetentionError::InvalidField`] for bad notes,
/// [`RetentionError::NotActive`] for restricted staff,
/// [`RetentionError::NotFound`] for unknown holds,
/// [`RetentionError::NotPermitted`] for ungranted staff,
/// [`RetentionError::InvalidState`] unless the hold is open, else
/// [`RetentionError::StorageFailed`]. Reasons are static.
pub async fn review_hold(
    pool: &sqlx::PgPool,
    staff_id: uuid::Uuid,
    hold_id: uuid::Uuid,
    note: String,
) -> Result<RetentionHold, RetentionError> {
    check_bounded(&note, HOLD_NOTE_MAX_CHARS)?;
    crate::application::staff_permissions::require_moderator(pool, staff_id)
        .await
        .map_err(staff_error)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| RetentionError::StorageFailed)?;
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {HOLD_COLUMNS} FROM retention_holds WHERE id = $1"
    ))
    .bind(hold_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| RetentionError::StorageFailed)?;
    let hold = match row {
        Some(row) => read_hold(&row)?,
        None => {
            tx.rollback()
                .await
                .map_err(|_| RetentionError::StorageFailed)?;
            return Err(RetentionError::NotFound);
        }
    };
    if hold.status != "open" {
        tx.rollback()
            .await
            .map_err(|_| RetentionError::StorageFailed)?;
        return Err(RetentionError::InvalidState);
    }
    let due_at = if hold.class == CLASS_INCIDENT {
        chrono::Utc::now() + chrono::Duration::days(INCIDENT_REVIEW_DAYS)
    } else {
        hold.due_at
    };
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "UPDATE retention_holds SET reviewed_at = now(), reviewed_by = $2, due_at = $3
         WHERE id = $1 AND status = 'open' RETURNING {HOLD_COLUMNS}"
    ))
    .bind(hold_id)
    .bind(staff_id)
    .bind(due_at)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| RetentionError::StorageFailed)?
    .ok_or(RetentionError::InvalidState)?;
    let reviewed = read_hold(&row)?;
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(staff_id),
            resource_kind: match hold.subject_kind.as_str() {
                "user" => "user",
                "request" => "request",
                "offer" => "offer",
                "contact" => "contact",
                "report" => "report",
                _ => "case",
            },
            resource_id: hold.subject_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "retention.hold_reviewed",
            policy: "mvp-free",
            source: "staff",
            payload: serde_json::json!({
                "class": hold.class,
                "due_at": reviewed.due_at,
                "note": note,
            }),
        },
    )
    .await
    .map_err(|_| RetentionError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| RetentionError::StorageFailed)?;
    Ok(reviewed)
}

/// Release one open hold with a recorded rationale, making its subject
/// eligible for the cleanup worker. Releasing twice converges.
///
/// # Errors
///
/// Returns [`RetentionError::InvalidField`] for bad reasons,
/// [`RetentionError::NotActive`] for restricted staff,
/// [`RetentionError::NotFound`] for unknown holds,
/// [`RetentionError::NotPermitted`] for ungranted staff, else
/// [`RetentionError::StorageFailed`]. Reasons are static.
pub async fn release_hold(
    pool: &sqlx::PgPool,
    staff_id: uuid::Uuid,
    hold_id: uuid::Uuid,
    reason: String,
) -> Result<RetentionHold, RetentionError> {
    check_bounded(&reason, HOLD_RELEASE_MAX_CHARS)?;
    crate::application::staff_permissions::require_moderator(pool, staff_id)
        .await
        .map_err(staff_error)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| RetentionError::StorageFailed)?;
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {HOLD_COLUMNS} FROM retention_holds WHERE id = $1"
    ))
    .bind(hold_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| RetentionError::StorageFailed)?;
    let hold = match row {
        Some(row) => read_hold(&row)?,
        None => {
            tx.rollback()
                .await
                .map_err(|_| RetentionError::StorageFailed)?;
            return Err(RetentionError::NotFound);
        }
    };
    if hold.status == "released" {
        tx.rollback()
            .await
            .map_err(|_| RetentionError::StorageFailed)?;
        return Ok(hold);
    }
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "UPDATE retention_holds SET status = 'released', released_at = now()
         WHERE id = $1 AND status = 'open' RETURNING {HOLD_COLUMNS}"
    ))
    .bind(hold_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| RetentionError::StorageFailed)?
    .ok_or(RetentionError::InvalidState)?;
    let released = read_hold(&row)?;
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(staff_id),
            resource_kind: match hold.subject_kind.as_str() {
                "user" => "user",
                "request" => "request",
                "offer" => "offer",
                "contact" => "contact",
                "report" => "report",
                _ => "case",
            },
            resource_id: hold.subject_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "retention.hold_released",
            policy: "mvp-free",
            source: "staff",
            payload: serde_json::json!({
                "class": hold.class,
                "reason": reason,
            }),
        },
    )
    .await
    .map_err(|_| RetentionError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| RetentionError::StorageFailed)?;
    Ok(released)
}

/// Open holds for one subject, oldest due first. Worker and staff-tooling
/// read; callers own their authorization for reads.
pub async fn holds_for_subject(
    pool: &sqlx::PgPool,
    subject_kind: &str,
    subject_id: uuid::Uuid,
) -> Result<Vec<RetentionHold>, RetentionError> {
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {HOLD_COLUMNS} FROM retention_holds
         WHERE subject_kind = $1 AND subject_id = $2 AND status = 'open'
         ORDER BY due_at, id"
    ))
    .bind(subject_kind)
    .bind(subject_id)
    .fetch_all(pool)
    .await
    .map_err(|_| RetentionError::StorageFailed)?;
    rows.iter().map(read_hold).collect()
}

/// Open holds past due, oldest first, bounded for the cleanup worker.
/// Expiry is reported here — never acted on by this read.
pub async fn due_holds(
    pool: &sqlx::PgPool,
    limit: i64,
) -> Result<Vec<RetentionHold>, RetentionError> {
    if !(1..=100).contains(&limit) {
        return Err(RetentionError::InvalidField);
    }
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {HOLD_COLUMNS} FROM retention_holds
         WHERE status = 'open' AND due_at <= now()
         ORDER BY due_at, id LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|_| RetentionError::StorageFailed)?;
    rows.iter().map(read_hold).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_windows_apply_per_class() {
        let now = chrono::Utc::now();
        let anchor = now - chrono::Duration::days(10);
        assert_eq!(
            due_for(CLASS_ORDINARY, anchor, now, None).expect("ordinary dues"),
            anchor + chrono::Duration::days(90)
        );
        assert_eq!(
            due_for(CLASS_INCIDENT, anchor, now, None).expect("incident dues"),
            now + chrono::Duration::days(90)
        );
        assert_eq!(
            due_for(CLASS_AUDIT, anchor, now, None).expect("audit dues"),
            now + chrono::Duration::days(180)
        );
        let ends = now + chrono::Duration::days(30);
        assert_eq!(
            due_for(CLASS_EXCEPTION, anchor, now, Some(ends)).expect("exception dues"),
            ends
        );
        assert_eq!(
            due_for(CLASS_EXCEPTION, anchor, now, None),
            Err(RetentionError::InvalidField)
        );
        assert_eq!(
            due_for(CLASS_EXCEPTION, anchor, now, Some(now)),
            Err(RetentionError::InvalidField)
        );
        assert_eq!(
            due_for("archival", anchor, now, None),
            Err(RetentionError::InvalidField)
        );
    }

    #[test]
    fn retention_text_has_bounds() {
        assert!(check_bounded("recent dispute cover", HOLD_PURPOSE_MAX_CHARS).is_ok());
        assert_eq!(
            check_bounded("   ", HOLD_PURPOSE_MAX_CHARS),
            Err(RetentionError::InvalidField)
        );
        assert_eq!(
            check_bounded(&"x".repeat(501), HOLD_PURPOSE_MAX_CHARS),
            Err(RetentionError::InvalidField)
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            RetentionError::InvalidField,
            RetentionError::NotActive,
            RetentionError::NotFound,
            RetentionError::NotPermitted,
            RetentionError::InvalidState,
            RetentionError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
