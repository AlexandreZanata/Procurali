//! Idempotent contact replay: the same action returns its result, renewed.
//!
//! Canonical rules: INV-28 with AC-23 and EC-25 (a retried handoff returns
//! its existing business result — no duplicate unique contact, and a later
//! eligible handoff stays a repeat event rather than another conversion),
//! INV-29 with EC-21 (the replay re-decrypts the current verified
//! destination instead of returning anything cached, while the frozen
//! snapshot keeps the number valid at initiation — even across a verified
//! phone change), INV-09 with AC-25 and EC-23 (a restriction landing after
//! initiation — withdrawal, rejection, expiry, blocks, suspension, or
//! prohibition — refuses the replay with no new disclosure).
//!
//! Replays are side-effect free: they read, re-verify, and decrypt, but
//! record nothing. Currency still applies — moved terms or demand refuse as
//! stale instead of silently accepting — so a replay never hands out a
//! destination for context the buyer has not freshly chosen.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::application::request_eligibility::{check_request_actionable, RequestEligibilityError};
use crate::persistence::catalogs::{self, CategoryStatus};
use crate::persistence::contacts;
use crate::persistence::eligibility::blocked_either_direction;
use crate::persistence::transaction::AttemptError;
use crate::persistence::users::{reveal_phone, PhoneKeys};

/// One replayed handoff: the standing identifiers plus the currently
/// verified destination. The destination travels only here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayResult {
    /// Contact identifier.
    pub contact_id: uuid::Uuid,
    /// Target request identifier.
    pub request_id: uuid::Uuid,
    /// Target offer identifier.
    pub offer_id: uuid::Uuid,
    /// Bound cycle number.
    pub cycle_number: i32,
    /// Current verified seller destination, plaintext, buyer-only.
    pub destination: String,
}

/// Typed replay failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayError {
    /// The caller account is missing, deleted, or not `active`.
    NotActive,
    /// No such handoff for this buyer (missing or foreign —
    /// deliberately indistinguishable).
    NotFound,
    /// The demand or offer side cannot take contact anymore.
    ForbiddenState,
    /// The exclusive deadline is reached or passed.
    ExpiredRequest,
    /// Frozen terms no longer match current terms or demand.
    StaleTerms,
    /// The seller account is missing, deleted, or not `active`.
    SellerNotActive,
    /// An either-direction block relates buyer and seller.
    Blocked,
    /// The category is prohibited: contact disables immediately.
    ProhibitedCategory,
    /// The lookup or decrypt failed.
    StorageFailed,
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("handoff not found"),
            Self::ForbiddenState => f.write_str("offer cannot take contact"),
            Self::ExpiredRequest => f.write_str("request deadline passed"),
            Self::StaleTerms => f.write_str("contact terms moved"),
            Self::SellerNotActive => f.write_str("seller is not active"),
            Self::Blocked => f.write_str("relationship is blocked"),
            Self::ProhibitedCategory => f.write_str("prohibited category"),
            Self::StorageFailed => f.write_str("contact replay failed"),
        }
    }
}

impl std::error::Error for ReplayError {}

/// Replay one recorded handoff for its buyer: re-verify everything live,
/// then return the standing contact with the currently verified
/// destination. Writes nothing — retries converge without inflating rows,
/// facts, or metrics.
///
/// # Errors
///
/// Returns the named business refusals without ever disclosing a
/// destination on any refusal path. Reasons are static.
pub async fn replay_contact(
    pool: &sqlx::PgPool,
    keys: &PhoneKeys<'_>,
    buyer_id: uuid::Uuid,
    handoff_id: uuid::Uuid,
) -> Result<ReplayResult, ReplayError> {
    match check_actor(pool, buyer_id)
        .await
        .map_err(|_| ReplayError::StorageFailed)?
    {
        CheckOutcome::Permitted => {}
        CheckOutcome::Refused(_) => return Err(ReplayError::NotActive),
    }
    let mut tx = pool.begin().await.map_err(|_| ReplayError::StorageFailed)?;
    // Scope by handoff identity and buyer together: strangers learn
    // nothing, whatever they guess.
    let contact_id = contact_id_of(&mut tx, handoff_id, buyer_id).await?;
    let contact = contacts::contact(&mut *tx, contact_id)
        .await
        .map_err(|_| ReplayError::StorageFailed)?
        .ok_or(ReplayError::NotFound)?;
    // Eligibility, liveness, currency, and policy mirror initiation: a
    // restriction landing after initiation refuses instead of disclosing.
    let standing = account_standing(&mut tx, contact.seller_id).await?;
    if !standing {
        tx.rollback()
            .await
            .map_err(|_| ReplayError::StorageFailed)?;
        return Err(ReplayError::SellerNotActive);
    }
    if blocked_either_direction(&mut *tx, buyer_id, contact.seller_id)
        .await
        .map_err(|_| ReplayError::StorageFailed)?
    {
        tx.rollback()
            .await
            .map_err(|_| ReplayError::StorageFailed)?;
        return Err(ReplayError::Blocked);
    }
    let actionable = check_request_actionable(&mut tx, contact.request_id, chrono::Utc::now())
        .await
        .map_err(|error| match error {
            AttemptError::Abort(RequestEligibilityError::Expired) => ReplayError::ExpiredRequest,
            AttemptError::Abort(RequestEligibilityError::NotFound) => ReplayError::NotFound,
            AttemptError::Abort(_) => ReplayError::ForbiddenState,
            AttemptError::Db(_) => ReplayError::StorageFailed,
        })?;
    let offer: Option<(String, i32, i32, i32)> = sqlx::query_as(
        "SELECT state, cycle_number, revision_number, current_terms_number
         FROM offers WHERE id = $1",
    )
    .bind(contact.offer_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ReplayError::StorageFailed)?;
    let (offer_state, offer_cycle, offer_revision, offer_terms) =
        offer.ok_or(ReplayError::NotFound)?;
    if !matches!(offer_state.as_str(), "sent" | "viewed" | "contacted")
        || offer_cycle != actionable.cycle_number
        || offer_revision != actionable.revision_number
    {
        tx.rollback()
            .await
            .map_err(|_| ReplayError::StorageFailed)?;
        return Err(ReplayError::ForbiddenState);
    }
    if offer_terms != contact.offer_terms_number
        || offer_revision != contact.request_revision_number
        || offer_cycle != contact.cycle_number
    {
        tx.rollback()
            .await
            .map_err(|_| ReplayError::StorageFailed)?;
        return Err(ReplayError::StaleTerms);
    }
    let category: Option<String> =
        sqlx::query_scalar("SELECT category_code FROM requests WHERE id = $1")
            .bind(contact.request_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| ReplayError::StorageFailed)?;
    let category = category.ok_or(ReplayError::StorageFailed)?;
    let standing = catalogs::category(&mut *tx, &category)
        .await
        .map_err(|_| ReplayError::StorageFailed)?;
    if matches!(
        standing.map(|entry| entry.status),
        Some(CategoryStatus::Prohibited)
    ) {
        tx.rollback()
            .await
            .map_err(|_| ReplayError::StorageFailed)?;
        return Err(ReplayError::ProhibitedCategory);
    }
    // Authorization complete: the current verified destination decrypts
    // fresh — never the frozen snapshot, never a cached URL.
    let destination = reveal_phone(&mut *tx, contact.seller_id, keys.encryption_key)
        .await
        .map_err(|_| ReplayError::StorageFailed)?;
    tx.rollback()
        .await
        .map_err(|_| ReplayError::StorageFailed)?;
    Ok(ReplayResult {
        contact_id: contact.id,
        request_id: contact.request_id,
        offer_id: contact.offer_id,
        cycle_number: contact.cycle_number,
        destination,
    })
}

async fn contact_id_of(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    handoff_id: uuid::Uuid,
    buyer_id: uuid::Uuid,
) -> Result<uuid::Uuid, ReplayError> {
    sqlx::query_scalar("SELECT id FROM contacts WHERE handoff_id = $1 AND buyer_id = $2")
        .bind(handoff_id)
        .bind(buyer_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| ReplayError::StorageFailed)?
        .ok_or(ReplayError::NotFound)
}

async fn account_standing(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account_id: uuid::Uuid,
) -> Result<bool, ReplayError> {
    let standing: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1")
            .bind(account_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| ReplayError::StorageFailed)?;
    Ok(matches!(standing, Some((state, None)) if state == "active"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_no_values() {
        for error in [
            ReplayError::NotActive,
            ReplayError::NotFound,
            ReplayError::ForbiddenState,
            ReplayError::ExpiredRequest,
            ReplayError::StaleTerms,
            ReplayError::SellerNotActive,
            ReplayError::Blocked,
            ReplayError::ProhibitedCategory,
            ReplayError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
            assert!(!rendered.contains("+55"));
        }
    }
}
