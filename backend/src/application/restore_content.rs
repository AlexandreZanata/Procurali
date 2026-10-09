//! Explicit resource restoration precedence: current facts decide.
//!
//! Canonical rules: INV-25 (terminal offers take no contact —
//! invalidation, withdrawal, expiry, and rejection stay terminal here),
//! INV-26 (restoration never makes previous-cycle offers live again —
//! stale cycle/revision rows retain), INV-37 with AC-39 (actor, reason,
//! scope, time, and previous/resulting standing recorded through an
//! inspection audit plus a durable fact; the original suspension fact is
//! never rewritten), INV-38 with EC-36 (lifting the content restriction
//! overrides no independent restriction — banned/suspended accounts, pair
//! blocks, prohibitions, and elapsed deadlines each retain with a named
//! reason), AC-49 with EC-19 (a past-deadline row resolves to expired
//! hidden/history context instead of reviving), EC-28 (a corrected
//! revision waiting under suspension restores onto current terms only).
//!
//! Restoration returns its verdict instead of refusing: `Restored` moves
//! exactly one row to its prior live standing with a new fact, while
//! `Retained` names every still-effective restriction and writes nothing
//! but the already-recorded inspection audit — except past-deadline rows,
//! which settle into expiry through the shared expiry writer. Terminal
//! and draft rows are outside restoration entirely and refuse.

use crate::application::request_eligibility::expire_if_elapsed;
use crate::application::staff_permissions::{authorized_inspect, InspectInput, StaffError};
use crate::persistence::catalogs::{category as category_standing, CategoryStatus};
use crate::persistence::eligibility::blocked_either_direction;
use crate::persistence::events::{record as record_event, NewEvent};

/// Restorable target families.
pub const RESTORABLE_KINDS: [&str; 2] = ["request", "offer"];
/// Reason bound in scalar values.
pub const RESTORE_REASON_MAX_CHARS: usize = 1000;
/// Purpose bound in scalar values.
pub const RESTORE_PURPOSE_MAX_CHARS: usize = 500;
/// Policy-version bound in scalar values.
pub const RESTORE_POLICY_MAX_CHARS: usize = 32;

/// One resource restoration as supplied: what may go live again and why.
#[derive(Debug, Clone)]
pub struct RestoreContentInput {
    /// `request` or `offer`.
    pub target_kind: String,
    /// Content identifier (must stand suspended).
    pub target_id: uuid::Uuid,
    /// Why this content returns.
    pub reason: String,
    /// Rules version the restoration applies.
    pub policy_version: String,
    /// Assigned safety purpose (recorded in the inspection audit).
    pub purpose: String,
}

/// One restored row: standing after the write.
#[derive(Debug, Clone, PartialEq)]
pub struct RestoredContent {
    /// Restored family.
    pub target_kind: String,
    /// Restored identifier.
    pub target_id: uuid::Uuid,
    /// Restored state (the prior live standing).
    pub state: String,
    /// Restored visibility (the prior live visibility).
    pub visibility: String,
    /// False when the row already stood live (no new fact).
    pub restored: bool,
}

/// One retained row: standing plus every still-effective restriction.
#[derive(Debug, Clone, PartialEq)]
pub struct RetainedContent {
    /// Retained family.
    pub target_kind: String,
    /// Retained identifier.
    pub target_id: uuid::Uuid,
    /// Standing kept (`suspended`, or `expired` after deadline settlement).
    pub state: String,
    /// Visibility kept.
    pub visibility: String,
    /// Restriction codes, e.g. `account_banned`, `pair_blocked`,
    /// `deadline_elapsed`, `parent_restricted`, `stale_terms`,
    /// `prohibited_category`, `retired_category`.
    pub reasons: Vec<String>,
}

/// Restoration verdict: exactly one row moved, or none with reasons.
#[derive(Debug, Clone, PartialEq)]
pub enum RestoreOutcome {
    /// The row returned to its prior live standing (or already stood there).
    Restored(RestoredContent),
    /// The row stays restricted under the named reasons.
    Retained(RetainedContent),
}

/// Typed restoration failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreContentError {
    /// Unknown kind or bad reason/purpose/policy text.
    InvalidField,
    /// The caller's account is missing, pending, suspended, banned, or deleted.
    NotActive,
    /// No such content exists.
    NotFound,
    /// The caller holds no live moderator-or-better grant.
    NotPermitted,
    /// The row is a draft, terminal, or otherwise outside restoration.
    InvalidState,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for RestoreContentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid restoration field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("restoration target not found"),
            Self::NotPermitted => f.write_str("restoration not permitted"),
            Self::InvalidState => f.write_str("content is outside restoration"),
            Self::StorageFailed => f.write_str("restoration storage failed"),
        }
    }
}

impl std::error::Error for RestoreContentError {}

fn check_bounded(value: &str, max: usize) -> Result<(), RestoreContentError> {
    let count = value.chars().count();
    if value.trim().is_empty() || count > max {
        return Err(RestoreContentError::InvalidField);
    }
    Ok(())
}

fn staff_error(error: StaffError) -> RestoreContentError {
    match error {
        StaffError::InvalidField => RestoreContentError::InvalidField,
        StaffError::NotActive => RestoreContentError::NotActive,
        StaffError::NotFound => RestoreContentError::NotFound,
        StaffError::NotPermitted => RestoreContentError::NotPermitted,
        StaffError::StorageFailed => RestoreContentError::StorageFailed,
    }
}

/// Restore one suspended request or offer to its prior live standing when
/// every current restriction permits it: owner/seller account active, pair
/// unblocked (offers), category allowed, deadline unelapsed, parent demand
/// live and public (offers), and terms on the current cycle/revision
/// (offers). Past-deadline rows settle into expired hidden/history context
/// instead. Anything else retains with its restriction reasons.
///
/// # Errors
///
/// Returns [`RestoreContentError::InvalidField`] for unknown kinds or bad
/// text, [`RestoreContentError::NotActive`] for restricted callers,
/// [`RestoreContentError::NotFound`] for missing content,
/// [`RestoreContentError::NotPermitted`] for ungranted callers,
/// [`RestoreContentError::InvalidState`] outside suspended/live standing,
/// else [`RestoreContentError::StorageFailed`]. Reasons are static.
pub async fn restore_content(
    pool: &sqlx::PgPool,
    moderator_id: uuid::Uuid,
    input: RestoreContentInput,
) -> Result<RestoreOutcome, RestoreContentError> {
    if !RESTORABLE_KINDS.contains(&input.target_kind.as_str()) {
        return Err(RestoreContentError::InvalidField);
    }
    check_bounded(&input.reason, RESTORE_REASON_MAX_CHARS)?;
    check_bounded(&input.policy_version, RESTORE_POLICY_MAX_CHARS)?;
    check_bounded(&input.purpose, RESTORE_PURPOSE_MAX_CHARS)?;
    authorized_inspect(
        pool,
        moderator_id,
        InspectInput {
            target_kind: input.target_kind.clone(),
            target_id: input.target_id,
            purpose: input.purpose.clone(),
            policy_version: input.policy_version.clone(),
        },
    )
    .await
    .map_err(staff_error)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| RestoreContentError::StorageFailed)?;
    let table = match input.target_kind.as_str() {
        "request" => "requests",
        "offer" => "offers",
        _ => {
            tx.rollback()
                .await
                .map_err(|_| RestoreContentError::StorageFailed)?;
            return Err(RestoreContentError::InvalidField);
        }
    };
    // Lock the row first: concurrent restorations serialize here.
    let locked: Option<(String, String)> = sqlx::query_as(&format!(
        "SELECT state, visibility FROM {table} WHERE id = $1 FOR UPDATE"
    ))
    .bind(input.target_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| RestoreContentError::StorageFailed)?;
    let (state, visibility) = match locked {
        Some(standing) => standing,
        None => {
            tx.rollback()
                .await
                .map_err(|_| RestoreContentError::StorageFailed)?;
            return Err(RestoreContentError::NotFound);
        }
    };
    if state != "suspended" {
        let live = match input.target_kind.as_str() {
            "request" => state == "active",
            _ => matches!(state.as_str(), "sent" | "viewed" | "contacted"),
        };
        tx.rollback()
            .await
            .map_err(|_| RestoreContentError::StorageFailed)?;
        if live {
            return Ok(RestoreOutcome::Restored(RestoredContent {
                target_kind: input.target_kind,
                target_id: input.target_id,
                state,
                visibility,
                restored: false,
            }));
        }
        return Err(RestoreContentError::InvalidState);
    }
    let mut reasons = Vec::new();
    // Only a directly targeted request settles into expiry here; an offer
    // keeps its parent's settlement for the expiry writer (or a later
    // request restoration) and simply retains.
    let mut settle_expired = false;
    match input.target_kind.as_str() {
        "request" => {
            settle_expired =
                collect_request_restrictions(&mut tx, input.target_id, &mut reasons).await?;
        }
        _ => {
            collect_offer_restrictions(&mut tx, input.target_id, &mut reasons).await?;
        }
    }
    if !reasons.is_empty() {
        if settle_expired {
            // Past-deadline rows resolve to expired hidden/history context
            // through the shared expiry writer — never back to live. This
            // settlement persists: commit the expiry fact, then report the
            // retained standing.
            expire_if_elapsed(&mut tx, input.target_id, chrono::Utc::now())
                .await
                .map_err(|_| RestoreContentError::StorageFailed)?;
            let standing = standing_of(&mut tx, &input).await?;
            tx.commit()
                .await
                .map_err(|_| RestoreContentError::StorageFailed)?;
            return Ok(RestoreOutcome::Retained(RetainedContent {
                target_kind: input.target_kind,
                target_id: input.target_id,
                state: standing.0,
                visibility: standing.1,
                reasons,
            }));
        }
        let standing = standing_of(&mut tx, &input).await?;
        tx.rollback()
            .await
            .map_err(|_| RestoreContentError::StorageFailed)?;
        return Ok(RestoreOutcome::Retained(RetainedContent {
            target_kind: input.target_kind,
            target_id: input.target_id,
            state: standing.0,
            visibility: standing.1,
            reasons,
        }));
    }
    let (prior_state, prior_visibility) = prior_live_standing(&mut tx, &input).await?;
    sqlx::query(&format!(
        "UPDATE {table} SET state = $2, visibility = $3, updated_at = now()
         WHERE id = $1"
    ))
    .bind(input.target_id)
    .bind(&prior_state)
    .bind(&prior_visibility)
    .execute(&mut *tx)
    .await
    .map_err(|_| RestoreContentError::StorageFailed)?;
    let kind = match input.target_kind.as_str() {
        "request" => "request.restored",
        _ => "offer.restored",
    };
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(moderator_id),
            resource_kind: match input.target_kind.as_str() {
                "request" => "request",
                _ => "offer",
            },
            resource_id: input.target_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind,
            policy: "mvp-free",
            source: "staff",
            payload: serde_json::json!({
                "reason": input.reason,
                "scope": input.target_id,
                "previous_state": "suspended",
                "restored_state": prior_state,
                "restored_visibility": prior_visibility,
                "policy_version": input.policy_version,
            }),
        },
    )
    .await
    .map_err(|_| RestoreContentError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| RestoreContentError::StorageFailed)?;
    Ok(RestoreOutcome::Restored(RestoredContent {
        target_kind: input.target_kind,
        target_id: input.target_id,
        state: prior_state,
        visibility: prior_visibility,
        restored: true,
    }))
}

async fn standing_of(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: &RestoreContentInput,
) -> Result<(String, String), RestoreContentError> {
    let table = match input.target_kind.as_str() {
        "request" => "requests",
        _ => "offers",
    };
    sqlx::query_as(&format!(
        "SELECT state, visibility FROM {table} WHERE id = $1"
    ))
    .bind(input.target_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| RestoreContentError::StorageFailed)
}

/// Prior live standing from the suspension fact, defaulting to the plain
/// live row when no fact survives. Never invents beyond live vocabulary.
async fn prior_live_standing(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: &RestoreContentInput,
) -> Result<(String, String), RestoreContentError> {
    let kind = match input.target_kind.as_str() {
        "request" => "request.suspended",
        _ => "offer.suspended",
    };
    let payload: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload FROM business_events
         WHERE resource_kind = $1 AND resource_id = $2 AND kind = $3
         ORDER BY id DESC LIMIT 1",
    )
    .bind(input.target_kind.as_str())
    .bind(input.target_id)
    .bind(kind)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| RestoreContentError::StorageFailed)?;
    let state = payload
        .as_ref()
        .and_then(|payload| payload.get("previous_state"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(match input.target_kind.as_str() {
            "request" => "active",
            _ => "sent",
        })
        .to_owned();
    let visibility = payload
        .as_ref()
        .and_then(|payload| payload.get("previous_visibility"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(match input.target_kind.as_str() {
            "request" => "public",
            _ => "visible",
        })
        .to_owned();
    Ok((state, visibility))
}

async fn account_standing(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: uuid::Uuid,
) -> Result<(String, bool), RestoreContentError> {
    let row: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| RestoreContentError::StorageFailed)?;
    Ok(match row {
        Some((state, deleted_at)) => (state, deleted_at.is_some()),
        None => ("missing".to_owned(), false),
    })
}

/// Request restrictions, oldest cause first. Returns whether the row must
/// settle into expiry (deadline elapsed).
async fn collect_request_restrictions(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request_id: uuid::Uuid,
    reasons: &mut Vec<String>,
) -> Result<bool, RestoreContentError> {
    let row: Option<(uuid::Uuid, i32, i32)> = sqlx::query_as(
        "SELECT author_id, current_cycle_number, current_revision_number
         FROM requests WHERE id = $1",
    )
    .bind(request_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| RestoreContentError::StorageFailed)?;
    let (author_id, cycle_number, revision_number) = match row {
        Some(row) => row,
        None => return Err(RestoreContentError::StorageFailed),
    };
    let (account_state, account_deleted) = account_standing(tx, author_id).await?;
    if account_deleted || account_state == "missing" {
        reasons.push("account_gone".to_owned());
        return Ok(false);
    }
    if account_state == "banned" {
        reasons.push("account_banned".to_owned());
        return Ok(false);
    }
    if account_state != "active" {
        reasons.push("account_suspended".to_owned());
        return Ok(false);
    }
    let category: Option<String> = sqlx::query_scalar(
        "SELECT category_code FROM request_revisions
         WHERE request_id = $1 AND revision_number = $2",
    )
    .bind(request_id)
    .bind(revision_number)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| RestoreContentError::StorageFailed)?;
    if let Some(code) = category {
        let standing = category_standing(&mut **tx, &code)
            .await
            .map_err(|_| RestoreContentError::StorageFailed)?;
        match standing.map(|entry| entry.status) {
            Some(CategoryStatus::Prohibited) => {
                reasons.push("prohibited_category".to_owned());
                return Ok(false);
            }
            Some(CategoryStatus::Retired) => {
                reasons.push("retired_category".to_owned());
                return Ok(false);
            }
            _ => {}
        }
    }
    let deadline: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT deadline FROM request_cycles
         WHERE request_id = $1 AND cycle_number = $2",
    )
    .bind(request_id)
    .bind(cycle_number)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| RestoreContentError::StorageFailed)?;
    match deadline {
        Some(deadline) if chrono::Utc::now() >= deadline => {
            reasons.push("deadline_elapsed".to_owned());
            Ok(true)
        }
        Some(_) => Ok(false),
        None => {
            reasons.push("deadline_missing".to_owned());
            Ok(false)
        }
    }
}

/// Offer restrictions, oldest cause first. Returns whether the parent row
/// must settle into expiry (shared with the request path).
async fn collect_offer_restrictions(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    offer_id: uuid::Uuid,
    reasons: &mut Vec<String>,
) -> Result<bool, RestoreContentError> {
    let row: Option<(uuid::Uuid, uuid::Uuid, i32, i32)> = sqlx::query_as(
        "SELECT seller_id, request_id, cycle_number, revision_number
         FROM offers WHERE id = $1",
    )
    .bind(offer_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| RestoreContentError::StorageFailed)?;
    let (seller_id, request_id, offer_cycle, offer_revision) = match row {
        Some(row) => row,
        None => return Err(RestoreContentError::StorageFailed),
    };
    let (seller_state, seller_deleted) = account_standing(tx, seller_id).await?;
    if seller_deleted || seller_state == "missing" {
        reasons.push("account_gone".to_owned());
        return Ok(false);
    }
    if seller_state == "banned" {
        reasons.push("account_banned".to_owned());
        return Ok(false);
    }
    if seller_state != "active" {
        reasons.push("account_suspended".to_owned());
        return Ok(false);
    }
    let parent: Option<(uuid::Uuid, String, String, i32, i32)> = sqlx::query_as(
        "SELECT author_id, state, visibility, current_cycle_number,
                current_revision_number
         FROM requests WHERE id = $1",
    )
    .bind(request_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| RestoreContentError::StorageFailed)?;
    let (author_id, parent_state, parent_visibility, parent_cycle, parent_revision) = match parent {
        Some(parent) => parent,
        None => {
            reasons.push("parent_missing".to_owned());
            return Ok(false);
        }
    };
    if parent_state != "active" || parent_visibility != "public" {
        reasons.push("parent_restricted".to_owned());
        return Ok(false);
    }
    let (author_state, author_deleted) = account_standing(tx, author_id).await?;
    if author_deleted || author_state != "active" {
        reasons.push("counterparty_restricted".to_owned());
        return Ok(false);
    }
    if blocked_either_direction(&mut **tx, author_id, seller_id)
        .await
        .map_err(|_| RestoreContentError::StorageFailed)?
    {
        reasons.push("pair_blocked".to_owned());
        return Ok(false);
    }
    if offer_cycle != parent_cycle || offer_revision != parent_revision {
        reasons.push("stale_terms".to_owned());
        return Ok(false);
    }
    let deadline: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT deadline FROM request_cycles
         WHERE request_id = $1 AND cycle_number = $2",
    )
    .bind(request_id)
    .bind(parent_cycle)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| RestoreContentError::StorageFailed)?;
    match deadline {
        Some(deadline) if chrono::Utc::now() >= deadline => {
            reasons.push("deadline_elapsed".to_owned());
            Ok(true)
        }
        Some(_) => Ok(false),
        None => {
            reasons.push("deadline_missing".to_owned());
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restoration_text_has_bounds() {
        assert!(check_bounded("cleared on re-review", RESTORE_REASON_MAX_CHARS).is_ok());
        assert_eq!(
            check_bounded("   ", RESTORE_REASON_MAX_CHARS),
            Err(RestoreContentError::InvalidField)
        );
        assert_eq!(
            check_bounded(&"x".repeat(1001), RESTORE_REASON_MAX_CHARS),
            Err(RestoreContentError::InvalidField)
        );
        assert!(!RESTORABLE_KINDS.contains(&"user"));
        assert!(!RESTORABLE_KINDS.contains(&"case"));
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            RestoreContentError::InvalidField,
            RestoreContentError::NotActive,
            RestoreContentError::NotFound,
            RestoreContentError::NotPermitted,
            RestoreContentError::InvalidState,
            RestoreContentError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
