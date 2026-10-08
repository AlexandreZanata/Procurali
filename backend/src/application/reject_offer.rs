//! Buyer decline: rejection as a comparison choice, never misconduct.
//!
//! Canonical rules: state-transitions 6.2 (sent, viewed, or contacted rows
//! decline; withdrawn, rejected, expired, and invalidated rows never
//! reactivate in-cycle), INV-18 with AC-17 (the slot stays taken through
//! decline — resubmission in the same cycle refuses), INV-25 (terminal
//! offers take no new contact), INV-34 (repeats duplicate nothing —
//! declining twice records once), EC-13 (decline is one buyer choice among
//! many; nothing here scores sellers, and no automatic negative reputation
//! exists anywhere in this path).
//!
//! Decline needs no reason: an optional buyer note rides the fact payload
//! (bounded, phone-free) while the row keeps the static `declined` terminal
//! reason. View and contact history survive — decline touches state and the
//! single fact only.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::application::offer_reads::{buyer_offers, BuyerOfferView, OfferSort};
use crate::application::update_profile::contains_phone_shaped_digits;
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::offers;
use serde_json::json;

/// Optional decline note bound in scalar values.
pub const DECLINE_NOTE_MAX_CHARS: usize = 500;

/// Typed decline failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectError {
    /// The note exceeds its bound or carries phone-shaped runs.
    InvalidNote,
    /// The account is missing, deleted, or not `active`.
    NotActive,
    /// No such offer for this buyer on this request (missing, foreign, or
    /// cross-request — deliberately indistinguishable).
    NotFound,
    /// The row already left the declinable states.
    ForbiddenState,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for RejectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidNote => f.write_str("invalid decline note"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("offer not found"),
            Self::ForbiddenState => f.write_str("offer cannot be declined"),
            Self::StorageFailed => f.write_str("decline storage failed"),
        }
    }
}

impl std::error::Error for RejectError {}

/// Decline one owned offer with an optional buyer note.
///
/// Sent, viewed, and contacted rows become `rejected` with exactly one
/// `offer.rejected` fact, atomically; every other state refuses with no
/// write, so repeats converge without duplicating. The seller slot stays
/// taken, prior view and contact history stands, and nobody is scored.
///
/// # Errors
///
/// Returns [`RejectError::InvalidNote`] for an overlong or phone-shaped
/// note, [`RejectError::NotActive`] for restricted callers,
/// [`RejectError::NotFound`] for missing or foreign rows,
/// [`RejectError::ForbiddenState`] past the declinable states, else
/// [`RejectError::StorageFailed`].
pub async fn reject_offer(
    pool: &sqlx::PgPool,
    author_id: uuid::Uuid,
    request_id: uuid::Uuid,
    offer_id: uuid::Uuid,
    note: Option<String>,
) -> Result<BuyerOfferView, RejectError> {
    if let Some(note) = note.as_deref() {
        if note.chars().count() > DECLINE_NOTE_MAX_CHARS || contains_phone_shaped_digits(note) {
            return Err(RejectError::InvalidNote);
        }
    }
    match check_actor(pool, author_id)
        .await
        .map_err(|_| RejectError::StorageFailed)?
    {
        CheckOutcome::Permitted => {}
        CheckOutcome::Refused(_) => return Err(RejectError::NotActive),
    }
    let mut tx = pool.begin().await.map_err(|_| RejectError::StorageFailed)?;
    let stored = offers::offer(&mut *tx, offer_id)
        .await
        .map_err(|_| RejectError::StorageFailed)?;
    let stored = match stored {
        Some(stored) if stored.request_id == request_id => stored,
        _ => {
            tx.rollback()
                .await
                .map_err(|_| RejectError::StorageFailed)?;
            return Err(RejectError::NotFound);
        }
    };
    let owner: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT author_id FROM requests WHERE id = $1")
            .bind(request_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| RejectError::StorageFailed)?;
    if owner != Some(author_id) {
        tx.rollback()
            .await
            .map_err(|_| RejectError::StorageFailed)?;
        return Err(RejectError::NotFound);
    }
    if !matches!(stored.state.as_str(), "sent" | "viewed" | "contacted") {
        tx.rollback()
            .await
            .map_err(|_| RejectError::StorageFailed)?;
        return Err(RejectError::ForbiddenState);
    }
    sqlx::query(
        "UPDATE offers SET state = 'rejected', terminal_reason = 'declined', updated_at = now()
         WHERE id = $1",
    )
    .bind(offer_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| RejectError::StorageFailed)?;
    let terms_id: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM offer_terms WHERE offer_id = $1 AND terms_number = $2")
            .bind(offer_id)
            .bind(stored.current_terms_number)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| RejectError::StorageFailed)?;
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(author_id),
            resource_kind: "offer",
            resource_id: offer_id,
            cycle: Some(stored.cycle_number),
            revision: terms_id,
            effective_at: chrono::Utc::now(),
            kind: "offer.rejected",
            policy: "mvp-free",
            source: "api",
            payload: json!({"state": "rejected", "note": note}),
        },
    )
    .await
    .map_err(|_| RejectError::StorageFailed)?;
    tx.commit().await.map_err(|_| RejectError::StorageFailed)?;
    let list = buyer_offers(pool, author_id, request_id, OfferSort::Newest)
        .await
        .map_err(|_| RejectError::StorageFailed)?;
    list.live
        .into_iter()
        .chain(list.history)
        .find(|view| view.id == offer_id)
        .ok_or(RejectError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_bound_matches_policy() {
        assert_eq!(DECLINE_NOTE_MAX_CHARS, 500);
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            RejectError::InvalidNote,
            RejectError::NotActive,
            RejectError::NotFound,
            RejectError::ForbiddenState,
            RejectError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
            assert!(!rendered.contains("+55"));
        }
    }
}
