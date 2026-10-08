//! Complete publication validation: one valid draft becomes a seven-day
//! active first cycle.
//!
//! Canonical rules: INV-08 (an active request needs complete valid fields,
//! an allowed category for its cycle, an enabled city, and a future deadline
//! — all re-checked here at the effective time, since drafts may reference a
//! category or city whose policy moved since drafting), INV-10 (positive BRL
//! minor units, exact decimals, never floats), INV-11 (exactly one category
//! per need), INV-12 (the first cycle sets the original publication time;
//! renewal writers later add cycles without rewriting it), INV-34 (repeating
//! the same publication returns the existing result — no second request, no
//! second event), AC-04 (eligible buyer plus complete allowed request becomes
//! active for seven days with one publication fact), AC-05 (each missing or
//! invalid requirement is refused naming its field, with no successful
//! event).
//!
//! The whole promotion — first revision snapshot, first cycle with its
//! exclusive deadline, lifecycle/visibility flip, and the `request.published`
//! fact — commits atomically from one transaction. The draft row is locked
//! (`SELECT ... FOR UPDATE`) before mutating, so concurrent publications of
//! the same draft serialize: the loser sees the winner's active row and
//! returns the same existing receipt instead of duplicating anything.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::application::update_profile::contains_phone_shaped_digits;
use crate::domain::condition::RequestCondition;
use crate::domain::money::{Money, MoneyError};
use crate::domain::text::TextKind;
use crate::persistence::catalogs::{self, CategoryStatus};
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::requests::{
    self, format_budget, insert_revision, start_cycle, RequirementSnapshot,
};
use serde_json::json;

/// Publication window: seven consecutive 24-hour periods (principles 2.2).
pub const PUBLICATION_DAYS: i64 = 7;

/// Exclusive deadline exactly seven days after the effective start.
#[must_use]
pub fn seven_day_deadline(
    started_at: chrono::DateTime<chrono::Utc>,
) -> chrono::DateTime<chrono::Utc> {
    started_at + chrono::Duration::days(PUBLICATION_DAYS)
}

/// One published request: public-safe facts plus its first-cycle timing.
/// The author is the caller by construction and is never echoed back.
#[derive(Debug, Clone, PartialEq)]
pub struct PublishedRequest {
    /// Request identifier (unchanged by publication).
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
    /// Lifecycle state (`active` after first publication).
    pub state: String,
    /// Current cycle number (1 after first publication).
    pub cycle_number: i32,
    /// Current revision number (1 after first publication).
    pub revision_number: i32,
    /// First-publication instant (never rewritten afterwards).
    pub original_published_at: chrono::DateTime<chrono::Utc>,
    /// Exclusive deadline: at or after it the request is expired for actions.
    pub deadline: chrono::DateTime<chrono::Utc>,
}

/// Typed publication failure. Static reasons and static field names only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishError {
    /// A stored requirement fails shape, vocabulary, bounds, or phone-shape
    /// rules; carries the static field name.
    InvalidField(&'static str),
    /// The stored budget fails the exact-decimal grammar.
    InvalidAmount(MoneyError),
    /// The category code is well-formed but not seeded.
    UnknownCategory,
    /// The city code is well-formed but not seeded.
    UnknownCity,
    /// The region code is well-formed but not seeded under the city.
    UnknownRegion,
    /// The category exists but is retired: existing cycles may finish, new
    /// use is refused.
    RetiredCategory,
    /// The category exists but is prohibited: never usable.
    ProhibitedCategory,
    /// The city exists but is not enabled for new publication.
    CityNotEnabled,
    /// The account is missing, deleted, or not `active`.
    NotActive,
    /// No such draft for this owner (missing or non-owned —
    /// deliberately indistinguishable).
    NotFound,
    /// The row is no longer a draft and not an active first cycle
    /// (terminal, suspended, or otherwise advanced elsewhere).
    ForbiddenState,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for PublishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField(_) => f.write_str("invalid publication field"),
            Self::InvalidAmount(error) => std::fmt::Display::fmt(error, f),
            Self::UnknownCategory => f.write_str("unknown category"),
            Self::UnknownCity => f.write_str("unknown city"),
            Self::UnknownRegion => f.write_str("unknown region"),
            Self::RetiredCategory => f.write_str("retired category"),
            Self::ProhibitedCategory => f.write_str("prohibited category"),
            Self::CityNotEnabled => f.write_str("city is not enabled"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("draft not found"),
            Self::ForbiddenState => f.write_str("request cannot be published"),
            Self::StorageFailed => f.write_str("publication storage failed"),
        }
    }
}

impl std::error::Error for PublishError {}

fn validate_stored(request: &requests::Request) -> Result<RequirementSnapshot, PublishError> {
    if request.title.trim().is_empty() {
        return Err(PublishError::InvalidField("title"));
    }
    TextKind::RequestTitle
        .check_length(&request.title)
        .map_err(|_| PublishError::InvalidField("title"))?;
    if contains_phone_shaped_digits(&request.title) {
        return Err(PublishError::InvalidField("title"));
    }
    RequestCondition::parse(&request.condition)
        .map_err(|_| PublishError::InvalidField("condition"))?;
    let budget =
        Money::parse(&format_budget(request.budget_cents)).map_err(PublishError::InvalidAmount)?;
    TextKind::Note
        .check_length(&request.notes)
        .map_err(|_| PublishError::InvalidField("notes"))?;
    if contains_phone_shaped_digits(&request.notes) {
        return Err(PublishError::InvalidField("notes"));
    }
    Ok(RequirementSnapshot {
        title: request.title.clone(),
        category_code: request.category_code.clone(),
        budget_cents: budget.cents(),
        condition: request.condition.clone(),
        city_code: request.city_code.clone(),
        region_code: request.region_code.clone(),
        notes: request.notes.clone(),
    })
}

/// Enforce the allowed-use policy at the effective time, inside the guarded
/// transaction: the category must allow new use and the city must be enabled.
async fn check_publication_policy(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    snapshot: &RequirementSnapshot,
) -> Result<(), PublishError> {
    let category = catalogs::category(&mut **tx, &snapshot.category_code)
        .await
        .map_err(|_| PublishError::StorageFailed)?;
    match category.map(|entry| entry.status) {
        None => return Err(PublishError::UnknownCategory),
        Some(CategoryStatus::Retired) => return Err(PublishError::RetiredCategory),
        Some(CategoryStatus::Prohibited) => return Err(PublishError::ProhibitedCategory),
        Some(CategoryStatus::Allowed) => {}
    }
    let cities = catalogs::cities(&mut **tx, false)
        .await
        .map_err(|_| PublishError::StorageFailed)?;
    let city = cities.iter().find(|entry| entry.code == snapshot.city_code);
    match city {
        None => return Err(PublishError::UnknownCity),
        Some(entry) if !entry.enabled => return Err(PublishError::CityNotEnabled),
        Some(_) => {}
    }
    let region = catalogs::region(&mut **tx, &snapshot.city_code, &snapshot.region_code)
        .await
        .map_err(|_| PublishError::StorageFailed)?;
    if region.is_none() {
        return Err(PublishError::UnknownRegion);
    }
    Ok(())
}

fn receipt(request: requests::Request, cycle: requests::RequestCycle) -> PublishedRequest {
    let budget = format_budget(request.budget_cents);
    let original_published_at = request.original_published_at.unwrap_or(cycle.started_at);
    PublishedRequest {
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
        cycle_number: cycle.cycle_number,
        revision_number: request.current_revision_number,
        original_published_at,
        deadline: cycle.deadline,
    }
}

async fn existing_receipt(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
) -> Result<PublishedRequest, PublishError> {
    let stored = requests::request(pool, request_id)
        .await
        .map_err(|_| PublishError::StorageFailed)?
        .ok_or(PublishError::StorageFailed)?;
    let first = requests::cycle(pool, request_id, 1)
        .await
        .map_err(|_| PublishError::StorageFailed)?
        .ok_or(PublishError::StorageFailed)?;
    Ok(receipt(stored, first))
}

/// Publish one owned draft: validate at the effective time, then atomically
/// record its first revision, first seven-day cycle, activation, and exactly
/// one `request.published` fact.
///
/// Repeating the call for an already-published draft returns the existing
/// receipt with no new row and no new event (INV-34).
///
/// # Errors
///
/// Returns field-naming refusals for invalid requirements, catalog and
/// policy refusals for disallowed references, [`PublishError::NotActive`]
/// for restricted accounts, [`PublishError::NotFound`] for missing or
/// non-owned drafts, [`PublishError::ForbiddenState`] past drafting, else
/// [`PublishError::StorageFailed`]. Reasons and field names are static.
pub async fn publish_request(
    pool: &sqlx::PgPool,
    author_id: uuid::Uuid,
    draft_id: uuid::Uuid,
) -> Result<PublishedRequest, PublishError> {
    match check_actor(pool, author_id)
        .await
        .map_err(|_| PublishError::StorageFailed)?
    {
        CheckOutcome::Permitted => {}
        CheckOutcome::Refused(_) => return Err(PublishError::NotActive),
    }
    let stored = requests::request(pool, draft_id)
        .await
        .map_err(|_| PublishError::StorageFailed)?;
    let stored = match stored {
        Some(stored) if stored.author_id == author_id => stored,
        _ => return Err(PublishError::NotFound),
    };
    // Idempotent replay: the active first cycle already exists, so return it
    // without writing anything.
    if stored.state == "active" {
        return existing_receipt(pool, draft_id).await;
    }
    if stored.state != "draft" {
        return Err(PublishError::ForbiddenState);
    }
    let snapshot = validate_stored(&stored)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|_| PublishError::StorageFailed)?;
    // Lock the draft before mutating: concurrent publications serialize here,
    // and the loser observes the winner's row below.
    let locked: Option<(String, i32, i32)> = sqlx::query_as(
        "SELECT state, current_cycle_number, current_revision_number
         FROM requests WHERE id = $1 FOR UPDATE",
    )
    .bind(draft_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| PublishError::StorageFailed)?;
    let (state, cycles, revisions) = locked.ok_or(PublishError::StorageFailed)?;
    if state == "active" && cycles >= 1 && revisions >= 1 {
        tx.rollback()
            .await
            .map_err(|_| PublishError::StorageFailed)?;
        return existing_receipt(pool, draft_id).await;
    }
    if state != "draft" || cycles != 0 || revisions != 0 {
        tx.rollback()
            .await
            .map_err(|_| PublishError::StorageFailed)?;
        return Err(PublishError::ForbiddenState);
    }
    // Roll back before reporting a policy refusal: no partial fact.
    if let Err(error) = check_publication_policy(&mut tx, &snapshot).await {
        tx.rollback()
            .await
            .map_err(|_| PublishError::StorageFailed)?;
        return Err(error);
    }
    let started_at = chrono::Utc::now();
    let deadline = seven_day_deadline(started_at);
    let revision = insert_revision(&mut tx, draft_id, 1, &snapshot)
        .await
        .map_err(|_| PublishError::StorageFailed)?;
    start_cycle(&mut tx, draft_id, 1, started_at, deadline)
        .await
        .map_err(|_| PublishError::StorageFailed)?;
    // The first revision is current from birth: promotion and activation are
    // one atomic flip, so current requirements and history never disagree.
    let activated: Option<i32> = sqlx::query_scalar(
        "UPDATE requests SET state = 'active', visibility = 'public',
                current_revision_number = 1, updated_at = now()
         WHERE id = $1 AND state = 'draft' RETURNING 1",
    )
    .bind(draft_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| PublishError::StorageFailed)?;
    if activated.is_none() {
        tx.rollback()
            .await
            .map_err(|_| PublishError::StorageFailed)?;
        return Err(PublishError::ForbiddenState);
    }
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(author_id),
            resource_kind: "request",
            resource_id: draft_id,
            cycle: Some(1),
            revision: Some(revision.id),
            effective_at: started_at,
            kind: "request.published",
            policy: "mvp-free",
            source: "api",
            payload: json!({"state": "active", "cycle_number": 1}),
        },
    )
    .await
    .map_err(|_| PublishError::StorageFailed)?;
    tx.commit().await.map_err(|_| PublishError::StorageFailed)?;
    existing_receipt(pool, draft_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_is_exactly_seven_days() {
        let start =
            chrono::DateTime::from_timestamp(1_791_000_000, 0).expect("fixture instant builds");
        let deadline = seven_day_deadline(start);
        assert_eq!(
            deadline.signed_duration_since(start),
            chrono::Duration::days(7)
        );
        assert_eq!(PUBLICATION_DAYS, 7);
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            PublishError::InvalidField("title"),
            PublishError::InvalidAmount(crate::domain::money::MoneyError::Overprecision),
            PublishError::UnknownCategory,
            PublishError::UnknownCity,
            PublishError::UnknownRegion,
            PublishError::RetiredCategory,
            PublishError::ProhibitedCategory,
            PublishError::CityNotEnabled,
            PublishError::NotActive,
            PublishError::NotFound,
            PublishError::ForbiddenState,
            PublishError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
            assert!(!rendered.contains("+55"));
        }
    }
}
