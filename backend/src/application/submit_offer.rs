//! Guarded offer submission: matching availability on eligible demand.
//!
//! Canonical rules: INV-03 with AC-15 (no self-offer in any context — the
//! seller is always the authenticated account and never the author), INV-09
//! with INV-17 and AC-14 (offers land only on live public unexpired cycles
//! with both parties eligible and no either-direction block), INV-19 with
//! AC-16 (positive exact-decimal prices within the current budget —
//! `600.01` against `600.00` refuses), INV-20 with AC-16 (new or used,
//! matching the request unless it accepts either), INV-21 (a truthful
//! specific description plus explicit availability declarations),
//! INV-22 (the offer locality satisfies the request's city scope),
//! INV-30 with AC-14 (submission alone never reveals the buyer's phone —
//! receipts and notices carry no author, phone, or contact material),
//! EC-07 (effects land strictly before the exclusive deadline) and EC-34
//! (an observed revision or cycle that moved since composing refuses as
//! stale — the seller reconfirms against current terms, never silently).
//!
//! The seller names the cycle and revision they composed against; the
//! operation binds exactly those when current and refuses otherwise. One
//! atomic transaction holds the slot insert, the `offer.submitted` fact,
//! and the buyer notice — the slot UNIQUE is the final backstop, so a lost
//! race refuses instead of duplicating.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::application::request_eligibility::{check_request_actionable, RequestEligibilityError};
use crate::application::update_profile::contains_phone_shaped_digits;
use crate::domain::money::{Money, MoneyError};
use crate::domain::text::TextKind;
use crate::persistence::catalogs::{self, CategoryStatus};
use crate::persistence::eligibility::blocked_either_direction;
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::notices::{record as record_notice, NewNotice};
use crate::persistence::offers::{self, NewOffer};
use crate::persistence::requests::format_budget;
use crate::persistence::transaction::AttemptError;
use serde_json::json;

/// Offer terms as supplied: every key present, observed cycle included.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OfferInput {
    /// Requirement revision observed while composing.
    pub revision_number: Option<i32>,
    /// Cycle observed while composing.
    pub cycle_number: Option<i32>,
    /// Short item/model description.
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
    /// The item is currently available.
    pub available: Option<bool>,
    /// The item can be made available in the request's city.
    pub available_in_city: Option<bool>,
}

/// Validated submission terms: storage-ready and policy-checked.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidOffer {
    revision_number: i32,
    cycle_number: i32,
    description: String,
    price_cents: i64,
    condition: String,
    city_code: String,
    region_code: String,
    notes: String,
}

/// One submitted offer as returned to its seller: terms plus slot facts.
/// The buyer appears only as the request identifier — never an account,
/// phone, or contact reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmittedOffer {
    /// Offer identifier.
    pub id: uuid::Uuid,
    /// Target request identifier.
    pub request_id: uuid::Uuid,
    /// Bound cycle number.
    pub cycle_number: i32,
    /// Bound requirement revision.
    pub revision_number: i32,
    /// Item description.
    pub description: String,
    /// Item price in minor units.
    pub price_cents: i64,
    /// Exact decimal rendering.
    pub price: String,
    /// Item condition.
    pub condition: String,
    /// Seller city code.
    pub city_code: String,
    /// Seller region code.
    pub region_code: String,
    /// Notes.
    pub notes: String,
    /// Lifecycle state (`sent` after submission).
    pub state: String,
}

/// Typed submission failure. Static reasons and static field names only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    /// A required key is absent; carries the static field name.
    MissingField(&'static str),
    /// A supplied value fails shape, vocabulary, bounds, compatibility, or
    /// declaration rules; carries the static field name.
    InvalidField(&'static str),
    /// The price fails the exact-decimal grammar.
    InvalidAmount(MoneyError),
    /// The price exceeds the current maximum budget.
    OverBudget,
    /// The seller owns the request.
    SelfOffer,
    /// An either-direction block relates seller and buyer.
    Blocked,
    /// The seller account is missing, deleted, or not `active`.
    NotActive,
    /// The request side cannot take offers (buyer restricted, or wrong
    /// lifecycle beyond expiry).
    ForbiddenState,
    /// The exclusive deadline is reached or passed.
    ExpiredRequest,
    /// The observed requirement revision moved.
    StaleRevision,
    /// The observed cycle moved.
    StaleCycle,
    /// The category exists but is retired: existing cycles may finish, new
    /// offers may not start.
    RetiredCategory,
    /// The category exists but is prohibited: never usable.
    ProhibitedCategory,
    /// The city exists but is not enabled for new offers.
    CityNotEnabled,
    /// No such request exists.
    NotFound,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField(_) => f.write_str("missing offer field"),
            Self::InvalidField(_) => f.write_str("invalid offer field"),
            Self::InvalidAmount(error) => std::fmt::Display::fmt(error, f),
            Self::OverBudget => f.write_str("price exceeds the budget"),
            Self::SelfOffer => f.write_str("seller owns the request"),
            Self::Blocked => f.write_str("relationship is blocked"),
            Self::NotActive => f.write_str("seller is not active"),
            Self::ForbiddenState => f.write_str("request cannot take offers"),
            Self::ExpiredRequest => f.write_str("request deadline passed"),
            Self::StaleRevision => f.write_str("requirement revision moved"),
            Self::StaleCycle => f.write_str("request cycle moved"),
            Self::RetiredCategory => f.write_str("retired category"),
            Self::ProhibitedCategory => f.write_str("prohibited category"),
            Self::CityNotEnabled => f.write_str("city is not enabled"),
            Self::NotFound => f.write_str("request not found"),
            Self::StorageFailed => f.write_str("offer storage failed"),
        }
    }
}

impl std::error::Error for SubmitError {}

fn require_field<'a>(
    value: &'a Option<String>,
    field: &'static str,
) -> Result<&'a str, SubmitError> {
    value.as_deref().ok_or(SubmitError::MissingField(field))
}

fn require_number(value: Option<i32>, field: &'static str) -> Result<i32, SubmitError> {
    match value {
        Some(number) if number >= 1 => Ok(number),
        Some(_) => Err(SubmitError::InvalidField(field)),
        None => Err(SubmitError::MissingField(field)),
    }
}

fn require_declaration(value: Option<bool>, field: &'static str) -> Result<(), SubmitError> {
    match value {
        Some(true) => Ok(()),
        Some(false) => Err(SubmitError::InvalidField(field)),
        None => Err(SubmitError::MissingField(field)),
    }
}

fn check_code(value: &str, field: &'static str) -> Result<(), SubmitError> {
    if value.trim().is_empty()
        || value.chars().count() > 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(SubmitError::InvalidField(field));
    }
    Ok(())
}

fn validate(input: &OfferInput) -> Result<ValidOffer, SubmitError> {
    let revision_number = require_number(input.revision_number, "revision_number")?;
    let cycle_number = require_number(input.cycle_number, "cycle_number")?;
    let description = require_field(&input.description, "description")?;
    if description.trim().is_empty() || description.chars().count() > 120 {
        return Err(SubmitError::InvalidField("description"));
    }
    if contains_phone_shaped_digits(description) {
        return Err(SubmitError::InvalidField("description"));
    }
    let price_raw = require_field(&input.price, "price")?;
    let price = Money::parse(price_raw).map_err(SubmitError::InvalidAmount)?;
    let condition = require_field(&input.condition, "condition")?;
    if !matches!(condition, "new" | "used") {
        return Err(SubmitError::InvalidField("condition"));
    }
    let city_code = require_field(&input.city_code, "city_code")?;
    check_code(city_code, "city_code")?;
    let region_code = require_field(&input.region_code, "region_code")?;
    check_code(region_code, "region_code")?;
    let notes = input.notes.as_deref().unwrap_or("");
    TextKind::Note
        .check_length(notes)
        .map_err(|_| SubmitError::InvalidField("notes"))?;
    if contains_phone_shaped_digits(notes) {
        return Err(SubmitError::InvalidField("notes"));
    }
    require_declaration(input.available, "available")?;
    require_declaration(input.available_in_city, "available_in_city")?;
    Ok(ValidOffer {
        revision_number,
        cycle_number,
        description: description.to_owned(),
        price_cents: price.cents(),
        condition: condition.to_owned(),
        city_code: city_code.to_owned(),
        region_code: region_code.to_owned(),
        notes: notes.to_owned(),
    })
}

/// Submit one offer for the authenticated seller.
///
/// The observed cycle and revision must still be current; the row, the
/// `offer.submitted` fact, and the buyer notice commit atomically.
///
/// # Errors
///
/// Returns field-naming refusals for missing/invalid input, exact-grammar
/// refusals for money, and the named business refusals above. Reasons and
/// field names are static.
pub async fn submit_offer(
    pool: &sqlx::PgPool,
    seller_id: uuid::Uuid,
    request_id: uuid::Uuid,
    input: OfferInput,
) -> Result<SubmittedOffer, SubmitError> {
    match check_actor(pool, seller_id)
        .await
        .map_err(|_| SubmitError::StorageFailed)?
    {
        CheckOutcome::Permitted => {}
        CheckOutcome::Refused(_) => return Err(SubmitError::NotActive),
    }
    let valid = validate(&input)?;

    let mut tx = pool.begin().await.map_err(|_| SubmitError::StorageFailed)?;
    /// Demand row needed for submission policy. Lifecycle and timing stay
    /// with `check_request_actionable`, which owns those refusals.
    #[derive(Debug, sqlx::FromRow)]
    struct DemandRow {
        author_id: uuid::Uuid,
        budget_cents: i64,
        condition: String,
        category_code: String,
    }
    let stored: Option<DemandRow> = sqlx::query_as(
        "SELECT author_id, budget_cents, \"condition\", category_code
         FROM requests WHERE id = $1",
    )
    .bind(request_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| SubmitError::StorageFailed)?;
    let stored = stored.ok_or(SubmitError::NotFound)?;
    let author_id = stored.author_id;
    let budget_cents = stored.budget_cents;
    let request_condition = stored.condition;
    let category_code = stored.category_code;
    if author_id == seller_id {
        tx.rollback()
            .await
            .map_err(|_| SubmitError::StorageFailed)?;
        return Err(SubmitError::SelfOffer);
    }
    let author_active: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT state, deleted_at FROM users WHERE id = $1")
            .bind(author_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| SubmitError::StorageFailed)?;
    match author_active {
        Some((state, None)) if state == "active" => {}
        _ => {
            tx.rollback()
                .await
                .map_err(|_| SubmitError::StorageFailed)?;
            return Err(SubmitError::ForbiddenState);
        }
    }
    if blocked_either_direction(&mut *tx, seller_id, author_id)
        .await
        .map_err(|_| SubmitError::StorageFailed)?
    {
        tx.rollback()
            .await
            .map_err(|_| SubmitError::StorageFailed)?;
        return Err(SubmitError::Blocked);
    }
    let actionable = check_request_actionable(&mut tx, request_id, chrono::Utc::now())
        .await
        .map_err(|error| match error {
            AttemptError::Abort(RequestEligibilityError::Expired) => SubmitError::ExpiredRequest,
            AttemptError::Abort(RequestEligibilityError::NotFound) => SubmitError::NotFound,
            AttemptError::Abort(_) => SubmitError::ForbiddenState,
            AttemptError::Db(_) => SubmitError::StorageFailed,
        })?;
    if valid.cycle_number != actionable.cycle_number {
        tx.rollback()
            .await
            .map_err(|_| SubmitError::StorageFailed)?;
        return Err(SubmitError::StaleCycle);
    }
    if valid.revision_number != actionable.revision_number {
        tx.rollback()
            .await
            .map_err(|_| SubmitError::StorageFailed)?;
        return Err(SubmitError::StaleRevision);
    }
    let category = catalogs::category(&mut *tx, &category_code)
        .await
        .map_err(|_| SubmitError::StorageFailed)?;
    match category.map(|entry| entry.status) {
        Some(CategoryStatus::Allowed) => {}
        Some(CategoryStatus::Retired) => {
            tx.rollback()
                .await
                .map_err(|_| SubmitError::StorageFailed)?;
            return Err(SubmitError::RetiredCategory);
        }
        _ => {
            tx.rollback()
                .await
                .map_err(|_| SubmitError::StorageFailed)?;
            return Err(SubmitError::ProhibitedCategory);
        }
    }
    let cities = catalogs::cities(&mut *tx, false)
        .await
        .map_err(|_| SubmitError::StorageFailed)?;
    let city = cities.iter().find(|entry| entry.code == valid.city_code);
    match city {
        Some(entry) if entry.enabled => {}
        _ => {
            tx.rollback()
                .await
                .map_err(|_| SubmitError::StorageFailed)?;
            return Err(SubmitError::CityNotEnabled);
        }
    }
    // Local availability means the request's own city scope.
    let request_city: String = sqlx::query_scalar("SELECT city_code FROM requests WHERE id = $1")
        .bind(request_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| SubmitError::StorageFailed)?;
    if valid.city_code != request_city {
        tx.rollback()
            .await
            .map_err(|_| SubmitError::StorageFailed)?;
        return Err(SubmitError::InvalidField("city_code"));
    }
    let region = catalogs::region(&mut *tx, &valid.city_code, &valid.region_code)
        .await
        .map_err(|_| SubmitError::StorageFailed)?;
    if region.is_none() {
        tx.rollback()
            .await
            .map_err(|_| SubmitError::StorageFailed)?;
        return Err(SubmitError::InvalidField("region_code"));
    }
    if request_condition != "either" && valid.condition != request_condition {
        tx.rollback()
            .await
            .map_err(|_| SubmitError::StorageFailed)?;
        return Err(SubmitError::InvalidField("condition"));
    }
    if valid.price_cents > budget_cents {
        tx.rollback()
            .await
            .map_err(|_| SubmitError::StorageFailed)?;
        return Err(SubmitError::OverBudget);
    }
    let stored_offer = offers::create_offer(
        &mut tx,
        NewOffer {
            request_id,
            cycle_number: valid.cycle_number,
            revision_number: valid.revision_number,
            seller_id,
            description: valid.description.clone(),
            price_cents: valid.price_cents,
            condition: valid.condition.clone(),
            city_code: valid.city_code.clone(),
            region_code: valid.region_code.clone(),
            notes: valid.notes.clone(),
        },
    )
    .await
    .map_err(|error| match error {
        offers::OfferError::SelfOffer => SubmitError::SelfOffer,
        _ => SubmitError::StorageFailed,
    })?;
    let revision_id: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM request_revisions WHERE request_id = $1 AND revision_number = $2",
    )
    .bind(request_id)
    .bind(valid.revision_number)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| SubmitError::StorageFailed)?;
    let event = record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(seller_id),
            resource_kind: "offer",
            resource_id: stored_offer.id,
            cycle: Some(valid.cycle_number),
            revision: revision_id,
            effective_at: chrono::Utc::now(),
            kind: "offer.submitted",
            policy: "mvp-free",
            source: "api",
            payload: json!({"state": "sent", "cycle_number": valid.cycle_number}),
        },
    )
    .await
    .map_err(|_| SubmitError::StorageFailed)?;
    record_notice(
        &mut *tx,
        NewNotice {
            account_id: author_id,
            kind: "offer.received",
            resource_kind: "offer",
            resource_id: stored_offer.id,
            event_id: event.id,
            body: "A new offer arrived on your request. Review it before starting contact."
                .to_owned(),
        },
    )
    .await
    .map_err(|_| SubmitError::StorageFailed)?;
    tx.commit().await.map_err(|_| SubmitError::StorageFailed)?;
    Ok(SubmittedOffer {
        id: stored_offer.id,
        request_id,
        cycle_number: valid.cycle_number,
        revision_number: valid.revision_number,
        description: stored_offer.description,
        price_cents: stored_offer.price_cents,
        price: format_budget(stored_offer.price_cents),
        condition: stored_offer.condition,
        city_code: stored_offer.city_code,
        region_code: stored_offer.region_code,
        notes: stored_offer.notes,
        state: stored_offer.state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> OfferInput {
        OfferInput {
            revision_number: Some(1),
            cycle_number: Some(1),
            description: Some("Frost-free 300L".to_owned()),
            price: Some("520.00".to_owned()),
            condition: Some("used".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: Some("Pickup only.".to_owned()),
            available: Some(true),
            available_in_city: Some(true),
        }
    }

    #[test]
    fn valid_input_parses_money_exactly() {
        let valid = validate(&input()).expect("valid input validates");
        assert_eq!(valid.price_cents, 52_000);
        assert_eq!(valid.revision_number, 1);
    }

    #[test]
    fn missing_keys_name_their_field() {
        assert_eq!(
            validate(&OfferInput {
                price: None,
                ..input()
            }),
            Err(SubmitError::MissingField("price"))
        );
        assert_eq!(
            validate(&OfferInput {
                revision_number: None,
                ..input()
            }),
            Err(SubmitError::MissingField("revision_number"))
        );
        assert_eq!(
            validate(&OfferInput {
                available: None,
                ..input()
            }),
            Err(SubmitError::MissingField("available"))
        );
    }

    #[test]
    fn declarations_must_be_affirmative() {
        assert_eq!(
            validate(&OfferInput {
                available: Some(false),
                ..input()
            }),
            Err(SubmitError::InvalidField("available"))
        );
        assert_eq!(
            validate(&OfferInput {
                available_in_city: Some(false),
                ..input()
            }),
            Err(SubmitError::InvalidField("available_in_city"))
        );
    }

    #[test]
    fn money_grammar_keeps_exact_refusals() {
        for price in ["0", "-5.00", "10.123", "many", ""] {
            let error = validate(&OfferInput {
                price: Some(price.to_owned()),
                ..input()
            });
            assert!(
                matches!(error, Err(SubmitError::InvalidAmount(_))),
                "{price}"
            );
        }
    }

    #[test]
    fn phone_shaped_text_and_bad_shapes_are_refused() {
        assert_eq!(
            validate(&OfferInput {
                description: Some("Call +55 11 98765-4321".to_owned()),
                ..input()
            }),
            Err(SubmitError::InvalidField("description"))
        );
        assert_eq!(
            validate(&OfferInput {
                condition: Some("either".to_owned()),
                ..input()
            }),
            Err(SubmitError::InvalidField("condition"))
        );
        assert_eq!(
            validate(&OfferInput {
                revision_number: Some(0),
                ..input()
            }),
            Err(SubmitError::InvalidField("revision_number"))
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            SubmitError::MissingField("price"),
            SubmitError::InvalidField("condition"),
            SubmitError::InvalidAmount(MoneyError::Overprecision),
            SubmitError::OverBudget,
            SubmitError::SelfOffer,
            SubmitError::Blocked,
            SubmitError::NotActive,
            SubmitError::ForbiddenState,
            SubmitError::ExpiredRequest,
            SubmitError::StaleRevision,
            SubmitError::StaleCycle,
            SubmitError::RetiredCategory,
            SubmitError::ProhibitedCategory,
            SubmitError::CityNotEnabled,
            SubmitError::NotFound,
            SubmitError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
            assert!(!rendered.contains("+55"));
        }
    }
}
