//! Buyer closure: completion and cancellation as declared outcomes.
//!
//! Canonical rules: INV-09 (closure stops offers and contact — the current
//! cycle ends and the terminal state derives no actions), INV-13
//! (completion, cancellation, and expiry are distinct facts, never one
//! silent flip), INV-34 (repeats duplicate nothing — terminal rows refuse
//! further closure instead of recording again), INV-43 with AC-28 and EC-26
//! (completion is a declared buyer report, never proof of a sale — the
//! source label is stored exactly as declared, unattributed completions
//! credit no seller and create no fictional contact), AC-30 (cancellation
//! records buyer abandonment, never expiry or resolution), AC-31 with EC-27
//! (no outcome fabricates success or cancellation; terminal rows stay
//! terminal — only a new need, subject to limits, moves on), EC-03 (closing
//! stops handoffs and retires live offers against the ended cycle without
//! selecting or crediting any seller), EC-20 (a suspended buyer completes
//! their own request while moderation visibility stays exactly as it was).
//!
//! Only the owning author closes, and only unresolved rows
//! (`active`, `expired`, `suspended`): drafts close elsewhere, terminal
//! rows are read-only. Visibility is never touched — suspended and hidden
//! rows keep their restrictions. The ended cycle plus the cycle-scoped fact
//! is the live-offer invalidation hook the offer writers consume; no offer
//! rows are touched here because none exist yet.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::requests;
use serde_json::json;

/// Declared completion source: the buyer's own label, never verified here.
/// Platform attribution without a linked contacted offer credits nobody.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeSource {
    /// Resolved through a platform offer (declared; linkage lands later).
    Platform,
    /// Resolved elsewhere.
    Elsewhere,
    /// The buyer prefers not to say.
    Undisclosed,
}

impl OutcomeSource {
    /// Stable fact string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Platform => "platform",
            Self::Elsewhere => "elsewhere",
            Self::Undisclosed => "undisclosed",
        }
    }
}

/// The buyer's answer: found, no longer needed, or not yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuyerOutcome {
    /// "Yes, I found it" with the declared source.
    Found(OutcomeSource),
    /// "I no longer need it."
    NotNeeded,
    /// "Not yet": still unresolved, nothing extended.
    NotYet,
}

/// The recorded outcome after closure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredOutcome {
    /// Completed with the declared source.
    Completed(OutcomeSource),
    /// Cancelled for buyer abandonment.
    Cancelled,
    /// Still unresolved: no mutation, no fact.
    Unresolved,
}

/// One closed (or confirmed unresolved) request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedRequest {
    /// Request identifier.
    pub id: uuid::Uuid,
    /// Lifecycle state afterwards.
    pub state: String,
    /// Recorded outcome.
    pub outcome: DeclaredOutcome,
}

/// Typed closure failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseError {
    /// The account is missing, deleted, or not `active`.
    NotActive,
    /// No such request for this owner (missing or non-owned —
    /// deliberately indistinguishable).
    NotFound,
    /// The row is a draft or already terminal.
    ForbiddenState,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for CloseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("request not found"),
            Self::ForbiddenState => f.write_str("request cannot be closed"),
            Self::StorageFailed => f.write_str("closure storage failed"),
        }
    }
}

impl std::error::Error for CloseError {}

/// Close one owned unresolved request with the buyer's declared outcome.
///
/// `Found` completes with the declared source and `NotNeeded` cancels for
/// buyer abandonment: both end the live cycle, flip the lifecycle, and
/// record exactly one distinct fact atomically, without touching
/// visibility, timing, or publication history. `NotYet` writes nothing and
/// records no fact — the row stays exactly as it was, deadline included —
/// so unknown never fabricates success or cancellation.
///
/// # Errors
///
/// Returns [`CloseError::NotActive`] for restricted accounts,
/// [`CloseError::NotFound`] for missing or non-owned rows,
/// [`CloseError::ForbiddenState`] for drafts and terminal rows, else
/// [`CloseError::StorageFailed`].
pub async fn close_request(
    pool: &sqlx::PgPool,
    author_id: uuid::Uuid,
    request_id: uuid::Uuid,
    outcome: BuyerOutcome,
) -> Result<ClosedRequest, CloseError> {
    match check_actor(pool, author_id)
        .await
        .map_err(|_| CloseError::StorageFailed)?
    {
        CheckOutcome::Permitted => {}
        CheckOutcome::Refused(_) => return Err(CloseError::NotActive),
    }
    let stored = requests::request(pool, request_id)
        .await
        .map_err(|_| CloseError::StorageFailed)?;
    let stored = match stored {
        Some(stored) if stored.author_id == author_id => stored,
        _ => return Err(CloseError::NotFound),
    };
    // Only unresolved demand answers: drafts close elsewhere and terminal
    // rows stay terminal (a repeat close records nothing further).
    if stored.state == "draft" || stored.state == "completed" || stored.state == "cancelled" {
        return Err(CloseError::ForbiddenState);
    }
    if outcome == BuyerOutcome::NotYet {
        return Ok(ClosedRequest {
            id: stored.id,
            state: stored.state,
            outcome: DeclaredOutcome::Unresolved,
        });
    }

    let mut tx = pool.begin().await.map_err(|_| CloseError::StorageFailed)?;
    // Lock the row first: concurrent closures serialize here, and the loser
    // observes the winner's terminal row.
    let locked: Option<(String, i32, i32)> = sqlx::query_as(
        "SELECT state, current_cycle_number, current_revision_number
         FROM requests WHERE id = $1 FOR UPDATE",
    )
    .bind(request_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| CloseError::StorageFailed)?;
    let (state, cycle_number, revision_number) = locked.ok_or(CloseError::StorageFailed)?;
    if state == "draft" || state == "completed" || state == "cancelled" {
        tx.rollback().await.map_err(|_| CloseError::StorageFailed)?;
        return Err(CloseError::ForbiddenState);
    }
    let (next_state, kind, payload) = match outcome {
        BuyerOutcome::Found(source) => (
            "completed",
            "request.completed",
            json!({"outcome": "completed", "source": source.as_str()}),
        ),
        BuyerOutcome::NotNeeded => (
            "cancelled",
            "request.cancelled",
            json!({"outcome": "cancelled", "reason": "buyer_abandonment"}),
        ),
        BuyerOutcome::NotYet => {
            tx.rollback().await.map_err(|_| CloseError::StorageFailed)?;
            return Ok(ClosedRequest {
                id: request_id,
                state,
                outcome: DeclaredOutcome::Unresolved,
            });
        }
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
    .map_err(|_| CloseError::StorageFailed)?;
    sqlx::query("UPDATE requests SET state = $2, updated_at = now() WHERE id = $1")
        .bind(request_id)
        .bind(next_state)
        .execute(&mut *tx)
        .await
        .map_err(|_| CloseError::StorageFailed)?;
    let revision_id: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM request_revisions WHERE request_id = $1 AND revision_number = $2",
    )
    .bind(request_id)
    .bind(revision_number)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| CloseError::StorageFailed)?;
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(author_id),
            resource_kind: "request",
            resource_id: request_id,
            cycle: Some(cycle_number),
            revision: revision_id,
            effective_at: chrono::Utc::now(),
            kind,
            policy: "mvp-free",
            source: "api",
            payload,
        },
    )
    .await
    .map_err(|_| CloseError::StorageFailed)?;
    tx.commit().await.map_err(|_| CloseError::StorageFailed)?;
    Ok(ClosedRequest {
        id: request_id,
        state: next_state.to_owned(),
        outcome: match outcome {
            BuyerOutcome::Found(source) => DeclaredOutcome::Completed(source),
            BuyerOutcome::NotNeeded => DeclaredOutcome::Cancelled,
            BuyerOutcome::NotYet => DeclaredOutcome::Unresolved,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_have_stable_strings() {
        assert_eq!(OutcomeSource::Platform.as_str(), "platform");
        assert_eq!(OutcomeSource::Elsewhere.as_str(), "elsewhere");
        assert_eq!(OutcomeSource::Undisclosed.as_str(), "undisclosed");
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            CloseError::NotActive,
            CloseError::NotFound,
            CloseError::ForbiddenState,
            CloseError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
