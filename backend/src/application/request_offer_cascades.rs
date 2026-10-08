//! Request-driven offer cascades: demand moves, live offers follow.
//!
//! Canonical rules: INV-09 (closed, expired, or removed demand takes no
//! offers — cascades move every live offer off actionability the moment its
//! demand moves), INV-14 with AC-09 and EC-09 (a material change preserves
//! the requirement revision and invalidates prior live offers, which
//! sellers explicitly resubmit against the latest revision inside their
//! existing slots), INV-18 (slots survive every cascade — invalidation
//! never deletes rows, so resubmission reuses them), INV-26 with AC-11 and
//! EC-08 (renewal never revives previous-cycle offers — the ended cycle's
//! rows go `expired` and stay there while fresh responses open new slots),
//! EC-03 (closure and removal invalidate with the relevant reason, never
//! selecting or crediting anyone), EC-33 (resubmission consumes daily
//! submission like any success), EC-34 (stale observed terms refuse).
//!
//! One entry point moves every live engagement row (`sent`, `viewed`,
//! `contacted`, `suspended`) for a cause: material revisions and closures
//! invalidate with their specific recoverable reason, ended cycles expire.
//! Already terminal rows keep their original reason unconditionally —
//! nothing here revives, rewrites, or reactivates, whatever the new budget
//! or cycle claims. Every write runs in the caller's transaction under the
//! DEC-0003 [`AttemptError`] contract, so future writers compose cascades
//! into their own atomic transitions.

use crate::persistence::transaction::AttemptError;

/// Demand-side cause moving live offers, with its terminal vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CascadeCause {
    /// A material requirement revision minted a new current revision.
    MaterialRevision,
    /// A cycle ended through expiry or renewal.
    CycleEnded,
    /// The request closed (completed or cancelled).
    RequestClosed,
    /// The request was owner-removed.
    RequestRemoved,
}

impl CascadeCause {
    /// Resulting offer state: ended cycles expire, everything else
    /// invalidates (both retire actionability; only the reason differs).
    #[must_use]
    pub const fn offer_state(self) -> &'static str {
        match self {
            Self::CycleEnded => "expired",
            Self::MaterialRevision | Self::RequestClosed | Self::RequestRemoved => "invalidated",
        }
    }

    /// Specific recoverable terminal reason recorded per offer.
    #[must_use]
    pub const fn terminal_reason(self) -> &'static str {
        match self {
            Self::MaterialRevision => "material_revision",
            Self::CycleEnded => "cycle_ended",
            Self::RequestClosed => "request_closed",
            Self::RequestRemoved => "request_removed",
        }
    }
}

/// Typed cascade failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CascadeError {
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for CascadeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StorageFailed => f.write_str("offer cascade failed"),
        }
    }
}

impl std::error::Error for CascadeError {}

/// Move every live offer on one request for `cause`, returning the moved
/// count. Live means `sent`, `viewed`, `contacted`, or `suspended`;
/// withdrawn, rejected, expired, and invalidated rows keep their original
/// reason untouched. Unknown requests move nothing. Resubmission stays
/// explicit: slots persist, so sellers re-enter through the normal
/// submission path against current requirements.
///
/// # Errors
///
/// Returns [`AttemptError::Db`] on database failure (retryable only for
/// genuine conflicts).
pub async fn cascade_request_offers(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request_id: uuid::Uuid,
    cause: CascadeCause,
) -> Result<u64, AttemptError<CascadeError>> {
    let moved = sqlx::query(
        "UPDATE offers SET state = $2, terminal_reason = $3, updated_at = now()
         WHERE request_id = $1
           AND state IN ('sent', 'viewed', 'contacted', 'suspended')",
    )
    .bind(request_id)
    .bind(cause.offer_state())
    .bind(cause.terminal_reason())
    .execute(&mut **tx)
    .await
    .map_err(AttemptError::Db)?;
    Ok(moved.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn causes_carry_distinct_recoverable_reasons() {
        assert_eq!(CascadeCause::MaterialRevision.offer_state(), "invalidated");
        assert_eq!(
            CascadeCause::MaterialRevision.terminal_reason(),
            "material_revision"
        );
        assert_eq!(CascadeCause::CycleEnded.offer_state(), "expired");
        assert_eq!(CascadeCause::CycleEnded.terminal_reason(), "cycle_ended");
        assert_eq!(CascadeCause::RequestClosed.offer_state(), "invalidated");
        assert_eq!(
            CascadeCause::RequestClosed.terminal_reason(),
            "request_closed"
        );
        assert_eq!(CascadeCause::RequestRemoved.offer_state(), "invalidated");
        assert_eq!(
            CascadeCause::RequestRemoved.terminal_reason(),
            "request_removed"
        );
    }

    #[test]
    fn errors_carry_no_values() {
        let rendered = format!(
            "{:?} {}",
            CascadeError::StorageFailed,
            CascadeError::StorageFailed
        );
        assert!(!rendered.contains("canary"));
    }
}
