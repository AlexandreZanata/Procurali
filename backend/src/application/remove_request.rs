//! Owner removal: business concealment with immutable history.
//!
//! Canonical rules: INV-09 (removed rows take no offers or contact —
//! removal cancels unresolved demand and hides it, and the counters and
//! checks already exclude cancelled and hidden rows), INV-13 (removal is
//! its own distinct `request.removed` fact carrying the prior state),
//! INV-15 with EC-32 (removal frees an open slot without refunding rolling
//! activations — facts are append-only, so nothing here can reset them),
//! INV-44 with EC-14 (removal hides content without deleting required
//! safety context — rows, revisions, cycles, and facts stay queryable for
//! authorized review, never publicly reused), EC-03 (closing already
//! stopped handoffs; removal additionally conceals, without selecting or
//! crediting anyone), EC-27 (terminal rows stay terminal — repeats change
//! nothing, and no restore or reopen endpoint exists in the MVP).
//!
//! Unresolved rows (drafts and active, expired, or suspended demand) become
//! `cancelled` and hidden with the owner-removal reason; already completed
//! or cancelled rows keep their outcome and only gain concealment. Either
//! way the live cycle ends, exactly one removal fact records the prior
//! state on first removal, and repeats return the same hidden row with no
//! new fact.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::requests;
use serde_json::json;

/// Owner-removal reason recorded on unresolved rows.
pub const OWNER_REMOVAL_REASON: &str = "owner_removal";

/// One removed request: concealed with its outcome preserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedRequest {
    /// Request identifier.
    pub id: uuid::Uuid,
    /// Lifecycle state afterwards (`cancelled`, or the kept terminal one).
    pub state: String,
    /// Visibility afterwards (always `hidden`).
    pub visibility: String,
    /// True when this call concealed the row; false on idempotent repeats.
    pub transitioned: bool,
}

/// Typed removal failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveError {
    /// The account is missing, deleted, or not `active`.
    NotActive,
    /// No such request for this owner (missing or non-owned —
    /// deliberately indistinguishable).
    NotFound,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for RemoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("request not found"),
            Self::StorageFailed => f.write_str("removal storage failed"),
        }
    }
}

impl std::error::Error for RemoveError {}

/// Remove one owned request: conceal it, cancel it when unresolved, and
/// record exactly one `request.removed` fact carrying the prior state.
///
/// Completed and cancelled rows keep their outcome — removal never converts
/// completion into abandonment — and only gain concealment. History
/// (revisions, cycles, facts) is never deleted, so safety review keeps its
/// evidence while discovery, sharing, and new interaction lose the row.
/// Repeats return the same hidden row with no new fact, and no restore or
/// reopen path exists.
///
/// # Errors
///
/// Returns [`RemoveError::NotActive`] for restricted accounts,
/// [`RemoveError::NotFound`] for missing or non-owned rows, else
/// [`RemoveError::StorageFailed`].
pub async fn remove_request(
    pool: &sqlx::PgPool,
    author_id: uuid::Uuid,
    request_id: uuid::Uuid,
) -> Result<RemovedRequest, RemoveError> {
    match check_actor(pool, author_id)
        .await
        .map_err(|_| RemoveError::StorageFailed)?
    {
        CheckOutcome::Permitted => {}
        CheckOutcome::Refused(_) => return Err(RemoveError::NotActive),
    }
    let stored = requests::request(pool, request_id)
        .await
        .map_err(|_| RemoveError::StorageFailed)?;
    let stored = match stored {
        Some(stored) if stored.author_id == author_id => stored,
        _ => return Err(RemoveError::NotFound),
    };
    // Idempotent repeats: an already concealed terminal row stands exactly
    // as it was, with no new fact.
    if (stored.state == "completed" || stored.state == "cancelled") && stored.visibility == "hidden"
    {
        return Ok(RemovedRequest {
            id: stored.id,
            state: stored.state,
            visibility: stored.visibility,
            transitioned: false,
        });
    }

    let mut tx = pool.begin().await.map_err(|_| RemoveError::StorageFailed)?;
    // Lock the row first: concurrent removals serialize here, and every
    // loser replays the winner's hidden row instead of duplicating facts.
    let locked: Option<(String, String, i32, i32)> = sqlx::query_as(
        "SELECT state, visibility, current_cycle_number, current_revision_number
         FROM requests WHERE id = $1 FOR UPDATE",
    )
    .bind(request_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| RemoveError::StorageFailed)?;
    let (state, visibility, cycle_number, revision_number) =
        locked.ok_or(RemoveError::StorageFailed)?;
    if (state == "completed" || state == "cancelled") && visibility == "hidden" {
        tx.rollback()
            .await
            .map_err(|_| RemoveError::StorageFailed)?;
        return Ok(RemovedRequest {
            id: request_id,
            state,
            visibility,
            transitioned: false,
        });
    }
    // Unresolved demand cancels with the owner reason; terminal outcomes
    // stay exactly as they were. Either way the row hides.
    let next_state = if state == "completed" || state == "cancelled" {
        state.clone()
    } else {
        "cancelled".to_owned()
    };
    sqlx::query(
        "UPDATE request_cycles SET ended_at = $3
         WHERE request_id = $1 AND cycle_number = $2 AND ended_at IS NULL",
    )
    .bind(request_id)
    .bind(cycle_number)
    .bind(chrono::Utc::now())
    .execute(&mut *tx)
    .await
    .map_err(|_| RemoveError::StorageFailed)?;
    sqlx::query(
        "UPDATE requests SET state = $2, visibility = 'hidden', updated_at = now()
         WHERE id = $1",
    )
    .bind(request_id)
    .bind(&next_state)
    .execute(&mut *tx)
    .await
    .map_err(|_| RemoveError::StorageFailed)?;
    let revision_id: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM request_revisions WHERE request_id = $1 AND revision_number = $2",
    )
    .bind(request_id)
    .bind(revision_number)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| RemoveError::StorageFailed)?;
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(author_id),
            resource_kind: "request",
            resource_id: request_id,
            cycle: Some(cycle_number),
            revision: revision_id,
            effective_at: chrono::Utc::now(),
            kind: "request.removed",
            policy: "mvp-free",
            source: "api",
            payload: json!({"prior_state": state, "reason": OWNER_REMOVAL_REASON}),
        },
    )
    .await
    .map_err(|_| RemoveError::StorageFailed)?;
    tx.commit().await.map_err(|_| RemoveError::StorageFailed)?;
    Ok(RemovedRequest {
        id: request_id,
        state: next_state,
        visibility: "hidden".to_owned(),
        transitioned: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removal_reason_is_stable() {
        assert_eq!(OWNER_REMOVAL_REASON, "owner_removal");
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            RemoveError::NotActive,
            RemoveError::NotFound,
            RemoveError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
