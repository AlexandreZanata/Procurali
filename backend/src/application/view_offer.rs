//! First buyer view: sent-to-viewed engagement with one conversion fact.
//!
//! Canonical rules: AC-18 (opening a sent offer's details views it and
//! records exactly one first-view fact — repeat openings never inflate
//! conversion), INV-34 (repeats duplicate nothing), state-transitions 6.2
//! (only the sent-to-viewed edge moves here; viewing never unsees, erases
//! contacts, or touches terms).
//!
//! Only the owning buyer views, on any lifecycle state: history rows read
//! back unchanged with no transition and no fact. Viewing is engagement
//! truth, never authorization — contact and comparison rules read the same
//! stored state elsewhere.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::application::offer_reads::{buyer_offers, BuyerOfferView, OfferSort};
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::offers;
use serde_json::json;

/// Typed first-view failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewError {
    /// The account is missing, deleted, or not `active`.
    NotActive,
    /// No such offer for this buyer on this request (missing, foreign, or
    /// cross-request — deliberately indistinguishable).
    NotFound,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("offer not found"),
            Self::StorageFailed => f.write_str("view storage failed"),
        }
    }
}

impl std::error::Error for ViewError {}

/// Open one owned offer's details for its buyer.
///
/// A `sent` row becomes `viewed` with exactly one `offer.viewed` fact,
/// atomically; every other state reads back as-is with no transition and
/// no fact, so repeats converge without inflating conversion.
///
/// # Errors
///
/// Returns [`ViewError::NotActive`] for restricted callers,
/// [`ViewError::NotFound`] for missing or foreign rows, else
/// [`ViewError::StorageFailed`].
pub async fn view_offer(
    pool: &sqlx::PgPool,
    author_id: uuid::Uuid,
    request_id: uuid::Uuid,
    offer_id: uuid::Uuid,
) -> Result<BuyerOfferView, ViewError> {
    match check_actor(pool, author_id)
        .await
        .map_err(|_| ViewError::StorageFailed)?
    {
        CheckOutcome::Permitted => {}
        CheckOutcome::Refused(_) => return Err(ViewError::NotActive),
    }
    let mut tx = pool.begin().await.map_err(|_| ViewError::StorageFailed)?;
    let stored = offers::offer(&mut *tx, offer_id)
        .await
        .map_err(|_| ViewError::StorageFailed)?;
    let stored = match stored {
        Some(stored) if stored.request_id == request_id => stored,
        _ => {
            tx.rollback().await.map_err(|_| ViewError::StorageFailed)?;
            return Err(ViewError::NotFound);
        }
    };
    let owner: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT author_id FROM requests WHERE id = $1")
            .bind(request_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| ViewError::StorageFailed)?;
    if owner != Some(author_id) {
        tx.rollback().await.map_err(|_| ViewError::StorageFailed)?;
        return Err(ViewError::NotFound);
    }
    if stored.state == "sent" {
        sqlx::query("UPDATE offers SET state = 'viewed', updated_at = now() WHERE id = $1")
            .bind(offer_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ViewError::StorageFailed)?;
        let terms_id: Option<uuid::Uuid> = sqlx::query_scalar(
            "SELECT id FROM offer_terms WHERE offer_id = $1 AND terms_number = $2",
        )
        .bind(offer_id)
        .bind(stored.current_terms_number)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ViewError::StorageFailed)?;
        record_event(
            &mut *tx,
            NewEvent {
                actor_id: Some(author_id),
                resource_kind: "offer",
                resource_id: offer_id,
                cycle: Some(stored.cycle_number),
                revision: terms_id,
                effective_at: chrono::Utc::now(),
                kind: "offer.viewed",
                policy: "mvp-free",
                source: "api",
                payload: json!({"state": "viewed"}),
            },
        )
        .await
        .map_err(|_| ViewError::StorageFailed)?;
    }
    tx.commit().await.map_err(|_| ViewError::StorageFailed)?;
    // Reuse the buyer comparison projection (see tests): it re-scopes by
    // ownership, so the returned view can never leak a foreign row.
    let list = buyer_offers(pool, author_id, request_id, OfferSort::Newest)
        .await
        .map_err(|_| ViewError::StorageFailed)?;
    list.live
        .into_iter()
        .chain(list.history)
        .find(|view| view.id == offer_id)
        .ok_or(ViewError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_no_values() {
        for error in [
            ViewError::NotActive,
            ViewError::NotFound,
            ViewError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
