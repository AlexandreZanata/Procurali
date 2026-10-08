//! Private request draft creation and editing: owner-only, exact validation.
//!
//! Canonical rules: INV-01 (the author is always the authenticated account —
//! no author input exists anywhere here, so ownership cannot be transferred
//! or forged), INV-05 (no role input; ownership and lifecycle state are
//! checked server-side on every read and write), INV-11 (one need per
//! request — the schema holds exactly one category code), INV-31 (phone-
//! shaped digit runs are refused in public text; responses carry no phone
//! material), AC-05 (each missing or invalid requirement is refused naming
//! its field).
//!
//! Drafts stay `draft` / `private` with no cycle, no revision, no publication
//! time, and no business event: saving a draft consumes no activation
//! allowance and creates no publication fact (allowance and publication
//! writers arrive in later cards). Draft edits move current requirements only
//! while no history exists; publication and material-revision writers own
//! everything afterwards.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::domain::condition::RequestCondition;
use crate::domain::location::{CategoryId, CityId, RegionId};
use crate::domain::money::{Money, MoneyError};
use crate::domain::text::TextKind;
use crate::persistence::catalogs;
use crate::persistence::requests::{self, format_budget};

/// Draft requirements as supplied: every requirement key must be present.
/// `notes` alone is optional and defaults to empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DraftInput {
    /// Item title.
    pub title: Option<String>,
    /// Stable catalog category code.
    pub category_code: Option<String>,
    /// Maximum budget as an exact decimal string (`"520.00"`).
    pub budget: Option<String>,
    /// Accepted condition (`new` | `used` | `either`).
    pub condition: Option<String>,
    /// Stable catalog city code.
    pub city_code: Option<String>,
    /// Region code within the city.
    pub region_code: Option<String>,
    /// Optional notes.
    pub notes: Option<String>,
}

/// Validated draft requirements: ownership-explicit, storage-ready.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidDraft {
    title: String,
    category_code: String,
    budget_cents: i64,
    condition: String,
    city_code: String,
    region_code: String,
    notes: String,
}

/// One saved draft as returned to its owner: requirements plus lifecycle.
/// The author is the caller by construction and is never echoed back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// Draft identifier.
    pub id: uuid::Uuid,
    /// Item title.
    pub title: String,
    /// Category code.
    pub category_code: String,
    /// Maximum budget in minor units.
    pub budget_cents: i64,
    /// Exact decimal rendering (`"520.00"`).
    pub budget: String,
    /// Accepted condition.
    pub condition: String,
    /// City code.
    pub city_code: String,
    /// Region code.
    pub region_code: String,
    /// Notes.
    pub notes: String,
    /// Lifecycle state (`draft` while this writer owns it).
    pub state: String,
}

/// Typed draft failure. Static reasons and static field names only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftError {
    /// A required key is absent; carries the static field name.
    MissingField(&'static str),
    /// A supplied value fails shape, vocabulary, bounds, or phone-shape
    /// rules; carries the static field name.
    InvalidField(&'static str),
    /// The budget fails the exact-decimal grammar.
    InvalidAmount(MoneyError),
    /// The category code is well-formed but not seeded.
    UnknownCategory,
    /// The city code is well-formed but not seeded.
    UnknownCity,
    /// The region code is well-formed but not seeded under the city.
    UnknownRegion,
    /// The account is missing, deleted, or not `active`.
    NotActive,
    /// No such draft for this owner (missing, non-owned, or no longer a
    /// history-free draft — deliberately indistinguishable).
    NotFound,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for DraftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField(_) => f.write_str("missing draft field"),
            Self::InvalidField(_) => f.write_str("invalid draft field"),
            Self::InvalidAmount(error) => std::fmt::Display::fmt(error, f),
            Self::UnknownCategory => f.write_str("unknown category"),
            Self::UnknownCity => f.write_str("unknown city"),
            Self::UnknownRegion => f.write_str("unknown region"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("draft not found"),
            Self::StorageFailed => f.write_str("draft storage failed"),
        }
    }
}

impl std::error::Error for DraftError {}

/// Strict catalog-code grammar, mirroring the persistence backstop
/// (`backend/src/persistence/requests.rs`): lowercase, digits, underscores,
/// 1..=32. Domain codes trim and bound only; this check keeps the wire
/// refusal precise instead of leaking a storage-shaped error.
fn check_code_shape(value: &str) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn require_field<'a>(
    value: &'a Option<String>,
    field: &'static str,
) -> Result<&'a str, DraftError> {
    value.as_deref().ok_or(DraftError::MissingField(field))
}

fn validate(input: &DraftInput) -> Result<ValidDraft, DraftError> {
    let title = require_field(&input.title, "title")?;
    if title.trim().is_empty() {
        return Err(DraftError::InvalidField("title"));
    }
    TextKind::RequestTitle
        .check_length(title)
        .map_err(|_| DraftError::InvalidField("title"))?;
    if crate::application::update_profile::contains_phone_shaped_digits(title) {
        return Err(DraftError::InvalidField("title"));
    }
    let category_raw = require_field(&input.category_code, "category_code")?;
    let category =
        CategoryId::parse(category_raw).map_err(|_| DraftError::InvalidField("category_code"))?;
    if !check_code_shape(category.as_str()) {
        return Err(DraftError::InvalidField("category_code"));
    }
    let budget_raw = require_field(&input.budget, "budget")?;
    let budget = Money::parse(budget_raw).map_err(DraftError::InvalidAmount)?;
    let condition_raw = require_field(&input.condition, "condition")?;
    RequestCondition::parse(condition_raw).map_err(|_| DraftError::InvalidField("condition"))?;
    let city_raw = require_field(&input.city_code, "city_code")?;
    let city = CityId::parse(city_raw).map_err(|_| DraftError::InvalidField("city_code"))?;
    if !check_code_shape(city.as_str()) {
        return Err(DraftError::InvalidField("city_code"));
    }
    let region_raw = require_field(&input.region_code, "region_code")?;
    let region =
        RegionId::parse(region_raw).map_err(|_| DraftError::InvalidField("region_code"))?;
    if !check_code_shape(region.as_str()) {
        return Err(DraftError::InvalidField("region_code"));
    }
    let notes = input.notes.as_deref().unwrap_or("");
    TextKind::Note
        .check_length(notes)
        .map_err(|_| DraftError::InvalidField("notes"))?;
    if crate::application::update_profile::contains_phone_shaped_digits(notes) {
        return Err(DraftError::InvalidField("notes"));
    }
    Ok(ValidDraft {
        title: title.to_owned(),
        category_code: category.as_str().to_owned(),
        budget_cents: budget.cents(),
        condition: condition_raw.to_owned(),
        city_code: city.as_str().to_owned(),
        region_code: region.as_str().to_owned(),
        notes: notes.to_owned(),
    })
}

fn draft_from(request: requests::Request) -> Draft {
    let budget = format_budget(request.budget_cents);
    Draft {
        id: request.id,
        title: request.title,
        category_code: request.category_code,
        budget_cents: request.budget_cents,
        budget,
        condition: request.condition,
        city_code: request.city_code,
        region_code: request.region_code,
        notes: request.notes,
        state: request.state,
    }
}

/// Classify a persistence unknown-reference into its precise refusal.
/// Best-effort post-hoc read: concurrent catalog changes can only move one
/// 4xx refusal to another, never to success.
async fn classify_unknown(
    pool: &sqlx::PgPool,
    valid: &ValidDraft,
) -> Result<DraftError, DraftError> {
    let category = catalogs::category(pool, &valid.category_code)
        .await
        .map_err(|_| DraftError::StorageFailed)?;
    if category.is_none() {
        return Ok(DraftError::UnknownCategory);
    }
    let cities = catalogs::cities(pool, false)
        .await
        .map_err(|_| DraftError::StorageFailed)?;
    if !cities.iter().any(|city| city.code == valid.city_code) {
        return Ok(DraftError::UnknownCity);
    }
    let region = catalogs::region(pool, &valid.city_code, &valid.region_code)
        .await
        .map_err(|_| DraftError::StorageFailed)?;
    if region.is_none() {
        return Ok(DraftError::UnknownRegion);
    }
    Ok(DraftError::StorageFailed)
}

async fn eligible_owner(pool: &sqlx::PgPool, author_id: uuid::Uuid) -> Result<(), DraftError> {
    match check_actor(pool, author_id)
        .await
        .map_err(|_| DraftError::StorageFailed)?
    {
        CheckOutcome::Permitted => Ok(()),
        CheckOutcome::Refused(_) => Err(DraftError::NotActive),
    }
}

/// Save one private draft for the authenticated owner.
///
/// No allowance is consumed and no business event is recorded: the only write
/// is the draft row itself, starting `draft` / `private` with no cycle, no
/// revision, and no publication time.
///
/// # Errors
///
/// Returns field-naming refusals for missing/invalid input, exact-grammar
/// refusals for money, catalog refusals for unseeded references,
/// [`DraftError::NotActive`] for restricted accounts, else
/// [`DraftError::StorageFailed`]. Reasons and field names are static.
pub async fn create_draft(
    pool: &sqlx::PgPool,
    author_id: uuid::Uuid,
    input: DraftInput,
) -> Result<Draft, DraftError> {
    let valid = validate(&input)?;
    eligible_owner(pool, author_id).await?;
    let mut tx = pool.begin().await.map_err(|_| DraftError::StorageFailed)?;
    let stored = requests::create_request(
        &mut tx,
        requests::NewRequest {
            author_id,
            title: valid.title.clone(),
            category_code: valid.category_code.clone(),
            budget_cents: valid.budget_cents,
            condition: valid.condition.clone(),
            city_code: valid.city_code.clone(),
            region_code: valid.region_code.clone(),
            notes: valid.notes.clone(),
        },
    )
    .await;
    let stored = match stored {
        Ok(stored) => stored,
        Err(requests::RequestError::Unknown) => {
            tx.rollback().await.map_err(|_| DraftError::StorageFailed)?;
            return Err(classify_unknown(pool, &valid).await?);
        }
        Err(requests::RequestError::InvalidField) => {
            tx.rollback().await.map_err(|_| DraftError::StorageFailed)?;
            return Err(DraftError::StorageFailed);
        }
        Err(requests::RequestError::StorageFailed) => {
            tx.rollback().await.map_err(|_| DraftError::StorageFailed)?;
            return Err(DraftError::StorageFailed);
        }
    };
    tx.commit().await.map_err(|_| DraftError::StorageFailed)?;
    Ok(draft_from(stored))
}

/// Read one history-free draft belonging to the authenticated owner.
///
/// Missing, non-owned, and already-advanced rows share one refusal: no
/// existence oracle for strangers.
///
/// # Errors
///
/// Returns [`DraftError::NotActive`] for restricted accounts,
/// [`DraftError::NotFound`] for anything not readable by this owner, else
/// [`DraftError::StorageFailed`].
pub async fn get_draft(
    pool: &sqlx::PgPool,
    author_id: uuid::Uuid,
    draft_id: uuid::Uuid,
) -> Result<Draft, DraftError> {
    eligible_owner(pool, author_id).await?;
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(
        "SELECT id, author_id, title, category_code, budget_cents, \"condition\",
                city_code, region_code, notes, state, visibility,
                original_published_at, current_cycle_number,
                current_revision_number, created_at, updated_at
         FROM requests WHERE id = $1 AND author_id = $2 AND state = 'draft'",
    )
    .bind(draft_id)
    .bind(author_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| DraftError::StorageFailed)?;
    if row.is_none() {
        return Err(DraftError::NotFound);
    }
    let stored = requests::request(pool, draft_id)
        .await
        .map_err(|_| DraftError::StorageFailed)?
        .ok_or(DraftError::NotFound)?;
    // Re-check after the read: ownership never moves, but the lifecycle can
    // advance concurrently; a published row is not this endpoint's draft.
    if stored.author_id != author_id || stored.state != "draft" {
        return Err(DraftError::NotFound);
    }
    Ok(draft_from(stored))
}

/// Replace one history-free draft's requirements for its owning author.
///
/// The row must still be the owner's untouched draft (no cycles, no
/// revisions); anything else shares the [`DraftError::NotFound`] refusal.
/// History rows are never created here — publication writers own those.
///
/// # Errors
///
/// Same refusals as [`create_draft`], plus [`DraftError::NotFound`] when the
/// draft is missing, non-owned, or already advanced past drafting.
pub async fn edit_draft(
    pool: &sqlx::PgPool,
    author_id: uuid::Uuid,
    draft_id: uuid::Uuid,
    input: DraftInput,
) -> Result<Draft, DraftError> {
    let valid = validate(&input)?;
    eligible_owner(pool, author_id).await?;
    let mut tx = pool.begin().await.map_err(|_| DraftError::StorageFailed)?;
    // Only foreign-key failures reach the error path after validation (every
    // CHECK is pre-validated above); classify the missing reference post-hoc
    // after rolling back. A dead connection surfaces as storage failure from
    // the classification reads themselves.
    let updated: Option<sqlx::postgres::PgRow> = match sqlx::query(
        "UPDATE requests SET title = $3, category_code = $4, budget_cents = $5,
                \"condition\" = $6, city_code = $7, region_code = $8, notes = $9,
                updated_at = now()
         WHERE id = $1 AND author_id = $2 AND state = 'draft'
           AND current_cycle_number = 0 AND current_revision_number = 0
         RETURNING id",
    )
    .bind(draft_id)
    .bind(author_id)
    .bind(&valid.title)
    .bind(&valid.category_code)
    .bind(valid.budget_cents)
    .bind(&valid.condition)
    .bind(&valid.city_code)
    .bind(&valid.region_code)
    .bind(&valid.notes)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(updated) => updated,
        Err(_) => {
            tx.rollback().await.map_err(|_| DraftError::StorageFailed)?;
            return Err(classify_unknown(pool, &valid).await?);
        }
    };
    if updated.is_none() {
        // Distinguish a failed ownership/state guard (no oracle) from a
        // vanished catalog reference (precise refusal).
        let guard: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM requests WHERE id = $1 AND author_id = $2
              AND state = 'draft'
              AND current_cycle_number = 0 AND current_revision_number = 0",
        )
        .bind(draft_id)
        .bind(author_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DraftError::StorageFailed)?;
        tx.rollback().await.map_err(|_| DraftError::StorageFailed)?;
        if guard.is_none() {
            return Err(DraftError::NotFound);
        }
        return Err(classify_unknown(pool, &valid).await?);
    }
    tx.commit().await.map_err(|_| DraftError::StorageFailed)?;
    let stored = requests::request(pool, draft_id)
        .await
        .map_err(|_| DraftError::StorageFailed)?
        .ok_or(DraftError::StorageFailed)?;
    Ok(draft_from(stored))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> DraftInput {
        DraftInput {
            title: Some("Refrigerator".to_owned()),
            category_code: Some("home_appliances".to_owned()),
            budget: Some("520.00".to_owned()),
            condition: Some("either".to_owned()),
            city_code: Some("campinas".to_owned()),
            region_code: Some("centro".to_owned()),
            notes: Some("Preferably frost-free.".to_owned()),
        }
    }

    #[test]
    fn valid_input_parses_money_exactly() {
        let valid = validate(&input()).expect("valid input validates");
        assert_eq!(valid.budget_cents, 52_000);
        assert_eq!(valid.condition, "either");
    }

    #[test]
    fn missing_keys_name_their_field() {
        assert_eq!(
            validate(&DraftInput {
                title: None,
                ..input()
            }),
            Err(DraftError::MissingField("title"))
        );
        assert_eq!(
            validate(&DraftInput {
                budget: None,
                ..input()
            }),
            Err(DraftError::MissingField("budget"))
        );
    }

    #[test]
    fn money_grammar_keeps_exact_refusals() {
        for budget in ["0", "-5.00", "10.123", "abc", ""] {
            let error = validate(&DraftInput {
                budget: Some(budget.to_owned()),
                ..input()
            });
            assert!(
                matches!(error, Err(DraftError::InvalidAmount(_))),
                "{budget}"
            );
        }
    }

    #[test]
    fn phone_shaped_text_and_bad_shapes_are_refused() {
        assert_eq!(
            validate(&DraftInput {
                title: Some("Call +55 11 98765-4321 now".to_owned()),
                ..input()
            }),
            Err(DraftError::InvalidField("title"))
        );
        assert_eq!(
            validate(&DraftInput {
                condition: Some("refurbished".to_owned()),
                ..input()
            }),
            Err(DraftError::InvalidField("condition"))
        );
        assert_eq!(
            validate(&DraftInput {
                title: Some("x".repeat(121)),
                ..input()
            }),
            Err(DraftError::InvalidField("title"))
        );
        assert_eq!(
            validate(&DraftInput {
                category_code: Some("Motor Vehicles".to_owned()),
                ..input()
            }),
            Err(DraftError::InvalidField("category_code"))
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            DraftError::MissingField("title"),
            DraftError::InvalidField("notes"),
            DraftError::InvalidAmount(MoneyError::Overprecision),
            DraftError::UnknownCategory,
            DraftError::UnknownCity,
            DraftError::UnknownRegion,
            DraftError::NotActive,
            DraftError::NotFound,
            DraftError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
            assert!(!rendered.contains("+55"));
        }
    }
}
