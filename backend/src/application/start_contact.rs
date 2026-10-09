//! Guarded contact initiation: the author's handoff on live matching terms.
//!
//! Canonical rules: INV-09 with INV-28 and AC-25 (contact needs a live
//! public unexpired cycle, an actionable offer, both parties eligible, no
//! block, and no restriction — suspended, banned, or deleted parties
//! refuse with no new disclosure), INV-27 with AC-22 (only the current
//! request author initiates — unrelated buyers, sellers acting as buyers,
//! and staff-like third parties share one refusal), INV-29 (no
//! unrestricted user-phone lookup — the destination decrypts only for the
//! authorized offer's seller, after every check), INV-30 (nothing here
//! leaks the buyer's phone anywhere), AC-20 with EC-10 (changed terms need
//! a fresh buyer choice — the acknowledged terms number must still be
//! current), AC-21 (one contextual handoff with one initiation record and
//! no purchase or payment semantics).
//!
//! The caller supplies the handoff identity: retries reuse it and converge
//! on the existing contact, while genuinely later handoffs mint a fresh
//! one. The plaintext destination decrypts only after authorization and
//! travels solely in the buyer's response — storage keeps the frozen
//! ciphertext the initiation snapshotted.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::application::request_eligibility::{check_request_actionable, RequestEligibilityError};
use crate::persistence::catalogs::{self, CategoryStatus};
use crate::persistence::contacts::{initiate_contact, NewContact};
use crate::persistence::eligibility::blocked_either_direction;
use crate::persistence::transaction::AttemptError;
use crate::persistence::users::{reveal_phone, PhoneKeys};

/// Contact intent as supplied: handoff identity, acknowledged terms, and
/// entry source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContactInput {
    /// Caller-supplied handoff identity for idempotent retries.
    pub handoff_id: Option<String>,
    /// Offer terms number the buyer acknowledged.
    pub expected_offer_terms: Option<i32>,
    /// Where the buyer chose contact.
    pub entry_source: Option<String>,
}

/// Validated contact intent: storage- and policy-ready.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidContact {
    handoff_id: uuid::Uuid,
    expected_terms: i32,
    entry_source: String,
}

/// One contextual handoff for the initiating buyer: identifiers plus the
/// current verified destination. The destination travels only here — never
/// storage, never any other response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
    /// Contact identifier (new or pre-existing).
    pub contact_id: uuid::Uuid,
    /// Target request identifier.
    pub request_id: uuid::Uuid,
    /// Target offer identifier.
    pub offer_id: uuid::Uuid,
    /// Bound cycle number.
    pub cycle_number: i32,
    /// Current verified seller destination, plaintext, buyer-only.
    pub destination: String,
    /// True when this call repeated an established combination.
    pub repeat: bool,
}

/// Typed handoff failure. Static reasons and static field names only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactError {
    /// A required key is absent; carries the static field name.
    MissingField(&'static str),
    /// A supplied value fails shape or vocabulary rules; carries the static
    /// field name.
    InvalidField(&'static str),
    /// The request side cannot take contact (buyer restricted, or wrong
    /// lifecycle beyond expiry).
    ForbiddenState,
    /// The exclusive deadline is reached or passed.
    ExpiredRequest,
    /// The acknowledged terms moved.
    StaleTerms,
    /// The seller account is missing, deleted, or not `active`.
    SellerNotActive,
    /// An either-direction block relates buyer and seller.
    Blocked,
    /// The category is prohibited: contact disables immediately.
    ProhibitedCategory,
    /// The caller account is missing, deleted, or not `active`.
    NotActive,
    /// No such request or offer for this buyer (missing, foreign, or
    /// cross-scoped — deliberately indistinguishable).
    NotFound,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for ContactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField(_) => f.write_str("missing contact field"),
            Self::InvalidField(_) => f.write_str("invalid contact field"),
            Self::ForbiddenState => f.write_str("request cannot take contact"),
            Self::ExpiredRequest => f.write_str("request deadline passed"),
            Self::StaleTerms => f.write_str("offer terms moved"),
            Self::SellerNotActive => f.write_str("seller is not active"),
            Self::Blocked => f.write_str("relationship is blocked"),
            Self::ProhibitedCategory => f.write_str("prohibited category"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("offer not found"),
            Self::StorageFailed => f.write_str("contact storage failed"),
        }
    }
}

impl std::error::Error for ContactError {}

fn validate(input: &ContactInput) -> Result<ValidContact, ContactError> {
    let handoff_raw = input
        .handoff_id
        .as_deref()
        .ok_or(ContactError::MissingField("handoff_id"))?;
    let handoff_id = handoff_raw
        .parse::<uuid::Uuid>()
        .map_err(|_| ContactError::InvalidField("handoff_id"))?;
    let expected_terms = match input.expected_offer_terms {
        Some(number) if number >= 1 => number,
        Some(_) => return Err(ContactError::InvalidField("expected_offer_terms")),
        None => return Err(ContactError::MissingField("expected_offer_terms")),
    };
    let entry_source = input
        .entry_source
        .as_deref()
        .ok_or(ContactError::MissingField("entry_source"))?;
    if entry_source.trim().is_empty() || entry_source.chars().count() > 64 {
        return Err(ContactError::InvalidField("entry_source"));
    }
    Ok(ValidContact {
        handoff_id,
        expected_terms,
        entry_source: entry_source.to_owned(),
    })
}

/// Start one guarded contact for the owning buyer.
///
/// Every check runs inside one transaction against live rows: caller and
/// seller eligibility, pair blocks, request actionability, offer liveness
/// on the current cycle and revision, acknowledged-terms currency, and the
/// category gate. Only then does the seller destination decrypt — for the
/// authorized offer alone — and the initiation record with its frozen
/// snapshot.
///
/// # Errors
///
/// Returns field-naming refusals for missing/invalid input and the named
/// business refusals above, without ever disclosing a destination on any
/// refusal path. Reasons and field names are static.
pub async fn start_contact(
    pool: &sqlx::PgPool,
    keys: &PhoneKeys<'_>,
    buyer_id: uuid::Uuid,
    request_id: uuid::Uuid,
    offer_id: uuid::Uuid,
    input: ContactInput,
) -> Result<Handoff, ContactError> {
    match check_actor(pool, buyer_id)
        .await
        .map_err(|_| ContactError::StorageFailed)?
    {
        CheckOutcome::Permitted => {}
        CheckOutcome::Refused(_) => return Err(ContactError::NotActive),
    }
    let valid = validate(&input)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ContactError::StorageFailed)?;
    let author: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT author_id FROM requests WHERE id = $1")
            .bind(request_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| ContactError::StorageFailed)?;
    if author != Some(buyer_id) {
        tx.rollback()
            .await
            .map_err(|_| ContactError::StorageFailed)?;
        return Err(ContactError::NotFound);
    }
    let offer: Option<(uuid::Uuid, uuid::Uuid, String, i32, i32, i32)> = sqlx::query_as(
        "SELECT request_id, seller_id, state, cycle_number, revision_number,
                current_terms_number
         FROM offers WHERE id = $1",
    )
    .bind(offer_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ContactError::StorageFailed)?;
    let (offer_request, seller_id, offer_state, offer_cycle, offer_revision, offer_terms) =
        offer.ok_or(ContactError::NotFound)?;
    if offer_request != request_id {
        tx.rollback()
            .await
            .map_err(|_| ContactError::StorageFailed)?;
        return Err(ContactError::NotFound);
    }
    if !matches!(offer_state.as_str(), "sent" | "viewed" | "contacted") {
        tx.rollback()
            .await
            .map_err(|_| ContactError::StorageFailed)?;
        return Err(ContactError::ForbiddenState);
    }
    let parties: Vec<(uuid::Uuid, String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT id, state, deleted_at FROM users WHERE id = ANY($1)")
            .bind(vec![buyer_id, seller_id])
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| ContactError::StorageFailed)?;
    let standing = |id: uuid::Uuid| {
        parties
            .iter()
            .find(|(row, _, _)| *row == id)
            .map(|(_, state, deleted)| state == "active" && deleted.is_none())
            .unwrap_or(false)
    };
    if !standing(buyer_id) {
        tx.rollback()
            .await
            .map_err(|_| ContactError::StorageFailed)?;
        return Err(ContactError::NotActive);
    }
    if !standing(seller_id) {
        tx.rollback()
            .await
            .map_err(|_| ContactError::StorageFailed)?;
        return Err(ContactError::ForbiddenState);
    }
    if blocked_either_direction(&mut *tx, buyer_id, seller_id)
        .await
        .map_err(|_| ContactError::StorageFailed)?
    {
        tx.rollback()
            .await
            .map_err(|_| ContactError::StorageFailed)?;
        return Err(ContactError::Blocked);
    }
    let actionable = check_request_actionable(&mut tx, request_id, chrono::Utc::now())
        .await
        .map_err(|error| match error {
            AttemptError::Abort(RequestEligibilityError::Expired) => ContactError::ExpiredRequest,
            AttemptError::Abort(RequestEligibilityError::NotFound) => ContactError::NotFound,
            AttemptError::Abort(_) => ContactError::ForbiddenState,
            AttemptError::Db(_) => ContactError::StorageFailed,
        })?;
    // Currency mismatches on either side refuse as stale: a revised demand
    // strands the offer exactly like moved offer terms do — the buyer
    // re-acknowledges current terms (resubmission scope) instead of
    // silently accepting hidden ones.
    if actionable.cycle_number != offer_cycle || actionable.revision_number != offer_revision {
        tx.rollback()
            .await
            .map_err(|_| ContactError::StorageFailed)?;
        return Err(ContactError::StaleTerms);
    }
    // Changed terms require a fresh buyer choice: the acknowledged number
    // must still be current, and the offer must sit on current demand.
    if offer_terms != valid.expected_terms || offer_cycle != actionable.cycle_number {
        tx.rollback()
            .await
            .map_err(|_| ContactError::StorageFailed)?;
        return Err(ContactError::StaleTerms);
    }
    let category: Option<String> =
        sqlx::query_scalar("SELECT category_code FROM requests WHERE id = $1")
            .bind(request_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| ContactError::StorageFailed)?;
    let category = category.ok_or(ContactError::StorageFailed)?;
    let standing = catalogs::category(&mut *tx, &category)
        .await
        .map_err(|_| ContactError::StorageFailed)?;
    if matches!(
        standing.map(|entry| entry.status),
        Some(CategoryStatus::Prohibited)
    ) {
        tx.rollback()
            .await
            .map_err(|_| ContactError::StorageFailed)?;
        return Err(ContactError::ProhibitedCategory);
    }
    // Authorization complete: decrypt the authorized seller destination.
    // No other number is ever read here, whatever the caller supplied.
    let destination = reveal_phone(&mut *tx, seller_id, keys.encryption_key)
        .await
        .map_err(|_| ContactError::StorageFailed)?;
    let initiated = initiate_contact(
        &mut tx,
        NewContact {
            handoff_id: valid.handoff_id,
            buyer_id,
            seller_id,
            request_id,
            cycle_number: actionable.cycle_number,
            offer_id,
            entry_source: valid.entry_source,
        },
    )
    .await
    .map_err(|_| ContactError::StorageFailed)?;
    tx.commit().await.map_err(|_| ContactError::StorageFailed)?;
    Ok(Handoff {
        contact_id: initiated.contact.id,
        request_id,
        offer_id,
        cycle_number: actionable.cycle_number,
        destination,
        repeat: initiated.repeat,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ContactInput {
        ContactInput {
            handoff_id: Some(uuid::Uuid::now_v7().to_string()),
            expected_offer_terms: Some(1),
            entry_source: Some("offer_detail".to_owned()),
        }
    }

    #[test]
    fn valid_input_parses_handoff_identity() {
        let valid = validate(&input()).expect("valid input validates");
        assert_eq!(valid.expected_terms, 1);
        assert_eq!(valid.entry_source, "offer_detail");
    }

    #[test]
    fn missing_keys_name_their_field() {
        assert_eq!(
            validate(&ContactInput {
                handoff_id: None,
                ..input()
            }),
            Err(ContactError::MissingField("handoff_id"))
        );
        assert_eq!(
            validate(&ContactInput {
                expected_offer_terms: None,
                ..input()
            }),
            Err(ContactError::MissingField("expected_offer_terms"))
        );
        assert_eq!(
            validate(&ContactInput {
                entry_source: None,
                ..input()
            }),
            Err(ContactError::MissingField("entry_source"))
        );
    }

    #[test]
    fn malformed_identities_are_refused() {
        assert_eq!(
            validate(&ContactInput {
                handoff_id: Some("not-a-uuid".to_owned()),
                ..input()
            }),
            Err(ContactError::InvalidField("handoff_id"))
        );
        assert_eq!(
            validate(&ContactInput {
                expected_offer_terms: Some(0),
                ..input()
            }),
            Err(ContactError::InvalidField("expected_offer_terms"))
        );
        assert_eq!(
            validate(&ContactInput {
                entry_source: Some(String::new()),
                ..input()
            }),
            Err(ContactError::InvalidField("entry_source"))
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            ContactError::MissingField("handoff_id"),
            ContactError::InvalidField("entry_source"),
            ContactError::ForbiddenState,
            ContactError::ExpiredRequest,
            ContactError::StaleTerms,
            ContactError::SellerNotActive,
            ContactError::Blocked,
            ContactError::ProhibitedCategory,
            ContactError::NotActive,
            ContactError::NotFound,
            ContactError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
            assert!(!rendered.contains("+55"));
        }
    }
}
