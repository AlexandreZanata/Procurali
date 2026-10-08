//! Seller withdrawal: terminal honesty without reservation inference.
//!
//! Canonical rules: state-transitions 6.2 (sent, viewed, or contacted rows
//! withdraw; withdrawn rows never reactivate in-cycle), INV-18 (the slot
//! stays taken — withdrawal never frees resubmission), INV-23 with EC-12
//! (an offer is never inventory exclusivity — withdrawing here touches
//! exactly this row and its fact, inferring no winner and no cross-request
//! effect), INV-25 with AC-26 and EC-04 (withdrawal after contact disables
//! future handoffs while historical contact and terms stand, with no claim
//! that anything shared is recalled), EC-11 (marking unavailable is
//! equivalent to withdrawal — same transition, distinct terminal reason).
//!
//! Only the owning seller withdraws. The terminal reason distinguishes a
//! plain withdrawal from unavailability; both record exactly one
//! `offer.withdrawn` fact and notify the buyer atomically. Daily counts and
//! slots are never refunded because nothing is deleted and no fact is
//! rewritten.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::application::offer_reads::{seller_offer, SellerOfferView};
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::notices::{record as record_notice, NewNotice};
use crate::persistence::offers;
use serde_json::json;

/// Terminal reason for a withdrawn offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawReason {
    /// The seller withdrew the offer.
    Withdrawn,
    /// The item is no longer available.
    Unavailable,
}

impl WithdrawReason {
    /// Stable fact and row string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Withdrawn => "withdrawn",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Typed withdrawal failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawError {
    /// The seller account is missing, deleted, or not `active`.
    NotActive,
    /// No such offer for this seller (missing or foreign —
    /// deliberately indistinguishable).
    NotFound,
    /// The row already left the withdrawable states.
    ForbiddenState,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for WithdrawError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotActive => f.write_str("seller is not active"),
            Self::NotFound => f.write_str("offer not found"),
            Self::ForbiddenState => f.write_str("offer cannot be withdrawn"),
            Self::StorageFailed => f.write_str("withdrawal storage failed"),
        }
    }
}

impl std::error::Error for WithdrawError {}

/// Withdraw one owned live offer with its terminal reason.
///
/// Sent, viewed, and contacted rows become `withdrawn` with exactly one
/// `offer.withdrawn` fact and one buyer notice, atomically; every other
/// state refuses with no write, so repeats converge without duplicating.
/// History stands exactly as it was — terms, prior facts, sibling offers,
/// and other demands are untouched — and nothing here reserves inventory,
/// names a winner, or refunds counts or slots.
///
/// # Errors
///
/// Returns [`WithdrawError::NotActive`] for restricted sellers,
/// [`WithdrawError::NotFound`] for missing or foreign rows,
/// [`WithdrawError::ForbiddenState`] past the withdrawable states, else
/// [`WithdrawError::StorageFailed`].
pub async fn withdraw_offer(
    pool: &sqlx::PgPool,
    seller_id: uuid::Uuid,
    offer_id: uuid::Uuid,
    reason: WithdrawReason,
) -> Result<SellerOfferView, WithdrawError> {
    match check_actor(pool, seller_id)
        .await
        .map_err(|_| WithdrawError::StorageFailed)?
    {
        CheckOutcome::Permitted => {}
        CheckOutcome::Refused(_) => return Err(WithdrawError::NotActive),
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| WithdrawError::StorageFailed)?;
    let stored = offers::offer(&mut *tx, offer_id)
        .await
        .map_err(|_| WithdrawError::StorageFailed)?;
    let stored = match stored {
        Some(stored) if stored.seller_id == seller_id => stored,
        _ => {
            tx.rollback()
                .await
                .map_err(|_| WithdrawError::StorageFailed)?;
            return Err(WithdrawError::NotFound);
        }
    };
    if !matches!(stored.state.as_str(), "sent" | "viewed" | "contacted") {
        tx.rollback()
            .await
            .map_err(|_| WithdrawError::StorageFailed)?;
        return Err(WithdrawError::ForbiddenState);
    }
    sqlx::query(
        "UPDATE offers SET state = 'withdrawn', terminal_reason = $2, updated_at = now()
         WHERE id = $1",
    )
    .bind(offer_id)
    .bind(reason.as_str())
    .execute(&mut *tx)
    .await
    .map_err(|_| WithdrawError::StorageFailed)?;
    let terms_id: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM offer_terms WHERE offer_id = $1 AND terms_number = $2")
            .bind(offer_id)
            .bind(stored.current_terms_number)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| WithdrawError::StorageFailed)?;
    let event = record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(seller_id),
            resource_kind: "offer",
            resource_id: offer_id,
            cycle: Some(stored.cycle_number),
            revision: terms_id,
            effective_at: chrono::Utc::now(),
            kind: "offer.withdrawn",
            policy: "mvp-free",
            source: "api",
            payload: json!({"state": "withdrawn", "reason": reason.as_str()}),
        },
    )
    .await
    .map_err(|_| WithdrawError::StorageFailed)?;
    let author: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT author_id FROM requests WHERE id = $1")
            .bind(stored.request_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| WithdrawError::StorageFailed)?;
    let author = author.ok_or(WithdrawError::StorageFailed)?;
    record_notice(
        &mut *tx,
        NewNotice {
            account_id: author,
            kind: "offer.withdrawn",
            resource_kind: "offer",
            resource_id: offer_id,
            event_id: event.id,
            body: "A seller withdrew their offer. It stays in your history for reference."
                .to_owned(),
        },
    )
    .await
    .map_err(|_| WithdrawError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| WithdrawError::StorageFailed)?;
    seller_offer(pool, seller_id, offer_id)
        .await
        .map_err(|_| WithdrawError::StorageFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_have_stable_strings() {
        assert_eq!(WithdrawReason::Withdrawn.as_str(), "withdrawn");
        assert_eq!(WithdrawReason::Unavailable.as_str(), "unavailable");
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            WithdrawError::NotActive,
            WithdrawError::NotFound,
            WithdrawError::ForbiddenState,
            WithdrawError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
