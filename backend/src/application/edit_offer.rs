//! Live terms editing: new revisions with preserved engagement.
//!
//! Canonical rules: INV-19 with EC-10 (within-budget exact-decimal prices
//! against current requirements — over the ceiling refuses), INV-24 with
//! AC-20 (every change appends terms history — the row promotes while prior
//! rows reload byte-stable), INV-25 (terminal offers take no new contact —
//! only live engagement states edit), state-transitions 6.2 (editing
//! preserves engagement state and creates a new offer revision without
//! unseeing, erasing contacts, or touching the slot).
//!
//! Only the owning seller edits, and only live offers on current demand:
//! sent, viewed, or contacted rows bound to the live cycle and current
//! requirement revision. The seller names the terms they composed against;
//! a moved counter refuses as stale instead of adopting hidden terms.
//! Viewed and contacted rows additionally notify the buyer with the new
//! terms fact — unviewed rows change silently, since nobody saw them yet.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::application::offer_reads::{seller_offer, SellerOfferView};
use crate::application::request_eligibility::{check_request_actionable, RequestEligibilityError};
use crate::application::update_profile::contains_phone_shaped_digits;
use crate::domain::money::{Money, MoneyError};
use crate::domain::text::TextKind;
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::notices::{record as record_notice, NewNotice};
use crate::persistence::offers::{self, TermsSnapshot};
use crate::persistence::transaction::AttemptError;
use serde_json::json;

/// New terms as supplied: every key present, plus the composed-against
/// terms number.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditTermsInput {
    /// Terms number observed while composing.
    pub expected_terms_number: Option<i32>,
    /// Item description.
    pub description: Option<String>,
    /// Item price as an exact decimal string.
    pub price: Option<String>,
    /// Item condition (`new` | `used`).
    pub condition: Option<String>,
    /// Seller locality city code.
    pub city_code: Option<String>,
    /// Seller locality region code.
    pub region_code: Option<String>,
    /// Optional notes.
    pub notes: Option<String>,
}

/// Validated new terms: storage-ready.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidTerms {
    description: String,
    price_cents: i64,
    condition: String,
    city_code: String,
    region_code: String,
    notes: String,
}

/// Typed terms-edit failure. Static reasons and static field names only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditError {
    /// A required key is absent; carries the static field name.
    MissingField(&'static str),
    /// A supplied value fails shape, vocabulary, bounds, compatibility, or
    /// declaration rules; carries the static field name.
    InvalidField(&'static str),
    /// The price fails the exact-decimal grammar.
    InvalidAmount(MoneyError),
    /// The price exceeds the current maximum budget.
    OverBudget,
    /// The observed terms number moved.
    StaleTerms,
    /// The seller account is missing, deleted, or not `active`.
    NotActive,
    /// The request side cannot take edits (buyer restricted, or wrong
    /// lifecycle beyond expiry).
    ForbiddenState,
    /// The exclusive deadline is reached or passed.
    ExpiredRequest,
    /// No such offer for this seller (missing or foreign —
    /// deliberately indistinguishable).
    NotFound,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField(_) => f.write_str("missing offer field"),
            Self::InvalidField(_) => f.write_str("invalid offer field"),
            Self::InvalidAmount(error) => std::fmt::Display::fmt(error, f),
            Self::OverBudget => f.write_str("price exceeds the budget"),
            Self::StaleTerms => f.write_str("offer terms moved"),
            Self::NotActive => f.write_str("seller is not active"),
            Self::ForbiddenState => f.write_str("offer cannot be edited"),
            Self::ExpiredRequest => f.write_str("request deadline passed"),
            Self::NotFound => f.write_str("offer not found"),
            Self::StorageFailed => f.write_str("offer edit failed"),
        }
    }
}

impl std::error::Error for EditError {}

fn require_field<'a>(value: &'a Option<String>, field: &'static str) -> Result<&'a str, EditError> {
    value.as_deref().ok_or(EditError::MissingField(field))
}

fn check_code(value: &str, field: &'static str) -> Result<(), EditError> {
    if value.trim().is_empty()
        || value.chars().count() > 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(EditError::InvalidField(field));
    }
    Ok(())
}

fn validate(input: &EditTermsInput) -> Result<(i32, ValidTerms), EditError> {
    let expected = match input.expected_terms_number {
        Some(number) if number >= 1 => number,
        Some(_) => return Err(EditError::InvalidField("expected_terms_number")),
        None => return Err(EditError::MissingField("expected_terms_number")),
    };
    let description = require_field(&input.description, "description")?;
    if description.trim().is_empty() || description.chars().count() > 120 {
        return Err(EditError::InvalidField("description"));
    }
    if contains_phone_shaped_digits(description) {
        return Err(EditError::InvalidField("description"));
    }
    let price_raw = require_field(&input.price, "price")?;
    let price = Money::parse(price_raw).map_err(EditError::InvalidAmount)?;
    let condition = require_field(&input.condition, "condition")?;
    if !matches!(condition, "new" | "used") {
        return Err(EditError::InvalidField("condition"));
    }
    let city_code = require_field(&input.city_code, "city_code")?;
    check_code(city_code, "city_code")?;
    let region_code = require_field(&input.region_code, "region_code")?;
    check_code(region_code, "region_code")?;
    let notes = input.notes.as_deref().unwrap_or("");
    TextKind::Note
        .check_length(notes)
        .map_err(|_| EditError::InvalidField("notes"))?;
    if contains_phone_shaped_digits(notes) {
        return Err(EditError::InvalidField("notes"));
    }
    Ok((
        expected,
        ValidTerms {
            description: description.to_owned(),
            price_cents: price.cents(),
            condition: condition.to_owned(),
            city_code: city_code.to_owned(),
            region_code: region_code.to_owned(),
            notes: notes.to_owned(),
        },
    ))
}

/// Edit one owned live offer's terms: new revision, preserved engagement.
///
/// The offer must sit on the live cycle and current requirements with an
/// unexpired deadline; the observed terms number must still be current.
/// Terms append, engagement state stands, and previously viewed or
/// contacted rows additionally notify the buyer — unviewed rows change
/// silently. Budget, condition, and locality re-validate against current
/// demand.
///
/// # Errors
///
/// Returns field-naming refusals for missing/invalid input, exact-grammar
/// refusals for money, and the named business refusals above. Reasons and
/// field names are static.
pub async fn edit_offer(
    pool: &sqlx::PgPool,
    seller_id: uuid::Uuid,
    offer_id: uuid::Uuid,
    input: EditTermsInput,
) -> Result<SellerOfferView, EditError> {
    match check_actor(pool, seller_id)
        .await
        .map_err(|_| EditError::StorageFailed)?
    {
        CheckOutcome::Permitted => {}
        CheckOutcome::Refused(_) => return Err(EditError::NotActive),
    }
    let (expected, valid) = validate(&input)?;

    let mut tx = pool.begin().await.map_err(|_| EditError::StorageFailed)?;
    let stored = offers::offer(&mut *tx, offer_id)
        .await
        .map_err(|_| EditError::StorageFailed)?;
    let stored = match stored {
        Some(stored) if stored.seller_id == seller_id => stored,
        _ => {
            tx.rollback().await.map_err(|_| EditError::StorageFailed)?;
            return Err(EditError::NotFound);
        }
    };
    if !matches!(stored.state.as_str(), "sent" | "viewed" | "contacted") {
        tx.rollback().await.map_err(|_| EditError::StorageFailed)?;
        return Err(EditError::ForbiddenState);
    }
    if expected != stored.current_terms_number {
        tx.rollback().await.map_err(|_| EditError::StorageFailed)?;
        return Err(EditError::StaleTerms);
    }
    let actionable = check_request_actionable(&mut tx, stored.request_id, chrono::Utc::now())
        .await
        .map_err(|error| match error {
            AttemptError::Abort(RequestEligibilityError::Expired) => EditError::ExpiredRequest,
            AttemptError::Abort(RequestEligibilityError::NotFound) => EditError::NotFound,
            AttemptError::Abort(_) => EditError::ForbiddenState,
            AttemptError::Db(_) => EditError::StorageFailed,
        })?;
    if actionable.cycle_number != stored.cycle_number
        || actionable.revision_number != stored.revision_number
    {
        tx.rollback().await.map_err(|_| EditError::StorageFailed)?;
        return Err(EditError::ForbiddenState);
    }
    let demand: Option<(i64, String, String)> =
        sqlx::query_as("SELECT budget_cents, \"condition\", city_code FROM requests WHERE id = $1")
            .bind(stored.request_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| EditError::StorageFailed)?;
    let (budget_cents, request_condition, request_city) = demand.ok_or(EditError::StorageFailed)?;
    if request_condition != "either" && valid.condition != request_condition {
        tx.rollback().await.map_err(|_| EditError::StorageFailed)?;
        return Err(EditError::InvalidField("condition"));
    }
    if valid.city_code != request_city {
        tx.rollback().await.map_err(|_| EditError::StorageFailed)?;
        return Err(EditError::InvalidField("city_code"));
    }
    if valid.price_cents > budget_cents {
        tx.rollback().await.map_err(|_| EditError::StorageFailed)?;
        return Err(EditError::OverBudget);
    }
    let snapshot = TermsSnapshot {
        description: valid.description.clone(),
        price_cents: valid.price_cents,
        condition: valid.condition.clone(),
        city_code: valid.city_code.clone(),
        region_code: valid.region_code.clone(),
        notes: valid.notes.clone(),
    };
    let terms = offers::insert_terms(
        &mut tx,
        offer_id,
        stored.current_terms_number + 1,
        &snapshot,
    )
    .await
    .map_err(|_| EditError::StorageFailed)?;
    offers::update_current_terms(
        &mut tx,
        offer_id,
        stored.current_terms_number + 1,
        &snapshot,
    )
    .await
    .map_err(|_| EditError::StorageFailed)?;
    let event = record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(seller_id),
            resource_kind: "offer",
            resource_id: offer_id,
            cycle: Some(stored.cycle_number),
            revision: Some(terms.id),
            effective_at: chrono::Utc::now(),
            kind: "offer.terms_updated",
            policy: "mvp-free",
            source: "api",
            payload: json!({"terms_number": stored.current_terms_number + 1}),
        },
    )
    .await
    .map_err(|_| EditError::StorageFailed)?;
    if matches!(stored.state.as_str(), "viewed" | "contacted") {
        let author: Option<uuid::Uuid> =
            sqlx::query_scalar("SELECT author_id FROM requests WHERE id = $1")
                .bind(stored.request_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| EditError::StorageFailed)?;
        let author = author.ok_or(EditError::StorageFailed)?;
        record_notice_for(&mut tx, author, offer_id, event.id).await?;
    }
    tx.commit().await.map_err(|_| EditError::StorageFailed)?;
    seller_offer(pool, seller_id, offer_id)
        .await
        .map_err(|_| EditError::StorageFailed)
}

async fn record_notice_for(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    author_id: uuid::Uuid,
    offer_id: uuid::Uuid,
    event_id: uuid::Uuid,
) -> Result<(), EditError> {
    record_notice(
        &mut **tx,
        NewNotice {
            account_id: author_id,
            kind: "offer.updated",
            resource_kind: "offer",
            resource_id: offer_id,
            event_id,
            body: "An offer you viewed has new terms. Review the current terms before responding."
                .to_owned(),
        },
    )
    .await
    .map_err(|_| EditError::StorageFailed)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> EditTermsInput {
        EditTermsInput {
            expected_terms_number: Some(1),
            description: Some("Frost-free 350L".to_owned()),
            price: Some("550.00".to_owned()),
            condition: Some("used".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: None,
        }
    }

    #[test]
    fn valid_input_parses_money_exactly() {
        let (expected, valid) = validate(&input()).expect("valid input validates");
        assert_eq!(expected, 1);
        assert_eq!(valid.price_cents, 55_000);
    }

    #[test]
    fn missing_keys_name_their_field() {
        assert_eq!(
            validate(&EditTermsInput {
                expected_terms_number: None,
                ..input()
            }),
            Err(EditError::MissingField("expected_terms_number"))
        );
        assert_eq!(
            validate(&EditTermsInput {
                price: None,
                ..input()
            }),
            Err(EditError::MissingField("price"))
        );
    }

    #[test]
    fn money_grammar_keeps_exact_refusals() {
        for price in ["0", "-5.00", "10.123", "many"] {
            let error = validate(&EditTermsInput {
                price: Some(price.to_owned()),
                ..input()
            });
            assert!(matches!(error, Err(EditError::InvalidAmount(_))), "{price}");
        }
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            EditError::MissingField("price"),
            EditError::InvalidField("condition"),
            EditError::InvalidAmount(MoneyError::Overprecision),
            EditError::OverBudget,
            EditError::StaleTerms,
            EditError::NotActive,
            EditError::ForbiddenState,
            EditError::ExpiredRequest,
            EditError::NotFound,
            EditError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
            assert!(!rendered.contains("+55"));
        }
    }
}
