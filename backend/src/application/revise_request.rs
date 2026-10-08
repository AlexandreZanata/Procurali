//! Request revisions: material classification with preserved history.
//!
//! Canonical rules: INV-14 (a material change preserves the prior revision
//! — revision rows are insert-only and current promotion never rewrites
//! them; live offers retire against the preserved terms, a mechanism the
//! offer writers consume later), INV-15 (nothing resets rolling counts —
//! the material-revision allowance derives from immutable facts),
//! workflows/requests 8.3 (drafts edit freely elsewhere; active, expired,
//! and suspended rows edit here subject to validity and the revision
//! allowance; expired edits never republish; suspended corrections stay
//! private; completed and cancelled rows are read-only), AC-08 (a
//! spelling-only correction keeps offers, deadlines, cycles, and history
//! compatible), AC-09 with EC-09 (budget up and down are both material;
//! neither resurrects old offers nor rewrites past prices — resubmission is
//! always explicit), EC-28 (a suspended correction stores a private
//! revision and keeps the restriction; expiry stays expired).
//!
//! Classification is mechanical: any change to budget, category, condition,
//! city, or region is material, as is any suitability-changing title or
//! note — detected as a normalized difference via the domain duplicate
//! vocabulary (trim, collapse, lowercase). Spelling, punctuation, or
//! clearer wording that normalizes identically is nonmaterial: it updates
//! the current wording in place with no new revision, no new cycle, no
//! deadline movement, and no fact. Material revisions advance the revision
//! counter, preserve the prior row, record one `request.revised` fact, and
//! never touch cycles, deadlines, visibility, or publication history.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::application::update_profile::contains_phone_shaped_digits;
use crate::domain::condition::RequestCondition;
use crate::domain::money::{Money, MoneyError};
use crate::domain::text::{normalize_for_comparison, TextKind};
use crate::persistence::catalogs::{self, CategoryStatus};
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::requests::{
    self, format_budget, insert_revision, update_current_requirements, RequirementSnapshot,
};
use serde_json::json;

/// Successful material revisions per rolling 24 hours per account.
pub const MAX_MATERIAL_REVISIONS_PER_DAY: u32 = 3;

/// Material-revision window length in hours: elapsed time, not calendar days.
pub const REVISION_WINDOW_HOURS: i64 = 24;

/// New requirements as supplied: every key required, notes included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviseInput {
    /// Item title.
    pub title: String,
    /// Stable catalog category code.
    pub category_code: String,
    /// Maximum budget as an exact decimal string.
    pub budget: String,
    /// Accepted condition (`new` | `used` | `either`).
    pub condition: String,
    /// Stable catalog city code.
    pub city_code: String,
    /// Region code within the city.
    pub region_code: String,
    /// Notes.
    pub notes: String,
}

/// The request after revision: current terms plus what the edit did.
#[derive(Debug, Clone, PartialEq)]
pub struct RevisedRequest {
    /// Request identifier.
    pub id: uuid::Uuid,
    /// Current revision number (advanced only when material).
    pub revision_number: i32,
    /// True when the edit changed requirements identity.
    pub material: bool,
    /// Current title.
    pub title: String,
    /// Current category code.
    pub category_code: String,
    /// Current budget in minor units.
    pub budget_cents: i64,
    /// Exact decimal rendering.
    pub budget: String,
    /// Current condition.
    pub condition: String,
    /// Current city code.
    pub city_code: String,
    /// Current region code.
    pub region_code: String,
    /// Current notes.
    pub notes: String,
    /// Lifecycle state (never moved by revision).
    pub state: String,
}

/// Typed revision failure. Static reasons and static field names only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviseError {
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
    /// The category exists but is retired: existing cycles may finish, new
    /// requirement sets may not adopt it.
    RetiredCategory,
    /// The category exists but is prohibited: never usable.
    ProhibitedCategory,
    /// The city exists but is not enabled for new requirement sets.
    CityNotEnabled,
    /// The account is missing, deleted, or not `active`.
    NotActive,
    /// No such request for this owner (missing or non-owned —
    /// deliberately indistinguishable).
    NotFound,
    /// The row is a draft (edited elsewhere) or terminal (read-only).
    ForbiddenState,
    /// Three successful material revisions already fall inside the rolling
    /// day.
    RevisionQuotaExhausted,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for ReviseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField(_) => f.write_str("invalid revision field"),
            Self::InvalidAmount(error) => std::fmt::Display::fmt(error, f),
            Self::UnknownCategory => f.write_str("unknown category"),
            Self::UnknownCity => f.write_str("unknown city"),
            Self::UnknownRegion => f.write_str("unknown region"),
            Self::RetiredCategory => f.write_str("retired category"),
            Self::ProhibitedCategory => f.write_str("prohibited category"),
            Self::CityNotEnabled => f.write_str("city is not enabled"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("request not found"),
            Self::ForbiddenState => f.write_str("request cannot be revised"),
            Self::RevisionQuotaExhausted => f.write_str("revision quota exhausted"),
            Self::StorageFailed => f.write_str("revision storage failed"),
        }
    }
}

impl std::error::Error for ReviseError {}

/// Strict catalog-code grammar, mirroring the persistence backstop.
fn check_code_shape(value: &str) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn validate(input: &ReviseInput) -> Result<RequirementSnapshot, ReviseError> {
    if input.title.trim().is_empty() {
        return Err(ReviseError::InvalidField("title"));
    }
    TextKind::RequestTitle
        .check_length(&input.title)
        .map_err(|_| ReviseError::InvalidField("title"))?;
    if contains_phone_shaped_digits(&input.title) {
        return Err(ReviseError::InvalidField("title"));
    }
    if !check_code_shape(&input.category_code) {
        return Err(ReviseError::InvalidField("category_code"));
    }
    let budget = Money::parse(&input.budget).map_err(ReviseError::InvalidAmount)?;
    RequestCondition::parse(&input.condition)
        .map_err(|_| ReviseError::InvalidField("condition"))?;
    if !check_code_shape(&input.city_code) {
        return Err(ReviseError::InvalidField("city_code"));
    }
    if !check_code_shape(&input.region_code) {
        return Err(ReviseError::InvalidField("region_code"));
    }
    TextKind::Note
        .check_length(&input.notes)
        .map_err(|_| ReviseError::InvalidField("notes"))?;
    if contains_phone_shaped_digits(&input.notes) {
        return Err(ReviseError::InvalidField("notes"));
    }
    Ok(RequirementSnapshot {
        title: input.title.clone(),
        category_code: input.category_code.clone(),
        budget_cents: budget.cents(),
        condition: input.condition.clone(),
        city_code: input.city_code.clone(),
        region_code: input.region_code.clone(),
        notes: input.notes.clone(),
    })
}

/// True when the new requirements change suitability: any budget, category,
/// condition, or locality change, or a normalized title/note difference.
/// Spelling-only wording normalizes identically and stays nonmaterial.
fn is_material(current: &requests::Request, next: &RequirementSnapshot) -> bool {
    next.budget_cents != current.budget_cents
        || next.category_code != current.category_code
        || next.condition != current.condition
        || next.city_code != current.city_code
        || next.region_code != current.region_code
        || normalize_for_comparison(&next.title) != normalize_for_comparison(&current.title)
        || normalize_for_comparison(&next.notes) != normalize_for_comparison(&current.notes)
}

/// Enforce the allowed-use policy for a new requirement set inside the
/// guarded transaction. Material revisions may not adopt retired or
/// prohibited categories or disabled cities.
async fn check_revision_policy(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    snapshot: &RequirementSnapshot,
) -> Result<(), ReviseError> {
    let category = catalogs::category(&mut **tx, &snapshot.category_code)
        .await
        .map_err(|_| ReviseError::StorageFailed)?;
    match category.map(|entry| entry.status) {
        None => return Err(ReviseError::UnknownCategory),
        Some(CategoryStatus::Retired) => return Err(ReviseError::RetiredCategory),
        Some(CategoryStatus::Prohibited) => return Err(ReviseError::ProhibitedCategory),
        Some(CategoryStatus::Allowed) => {}
    }
    let cities = catalogs::cities(&mut **tx, false)
        .await
        .map_err(|_| ReviseError::StorageFailed)?;
    let city = cities.iter().find(|entry| entry.code == snapshot.city_code);
    match city {
        None => return Err(ReviseError::UnknownCity),
        Some(entry) if !entry.enabled => return Err(ReviseError::CityNotEnabled),
        Some(_) => {}
    }
    let region = catalogs::region(&mut **tx, &snapshot.city_code, &snapshot.region_code)
        .await
        .map_err(|_| ReviseError::StorageFailed)?;
    if region.is_none() {
        return Err(ReviseError::UnknownRegion);
    }
    Ok(())
}

async fn material_count(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    author_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<u32, ReviseError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM business_events
         WHERE actor_id = $1
           AND resource_kind = 'request'
           AND kind = 'request.revised'
           AND effective_at > $2",
    )
    .bind(author_id)
    .bind(now - chrono::Duration::hours(REVISION_WINDOW_HOURS))
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| ReviseError::StorageFailed)?;
    u32::try_from(count).map_err(|_| ReviseError::StorageFailed)
}

/// Revise one owned active, expired, or suspended request.
///
/// Material edits preserve the prior revision row, promote the new terms,
/// record one `request.revised` fact, and leave cycles, deadlines,
/// visibility, and publication history untouched — so expired rows stay
/// expired, suspended rows stay restricted, and no past offer can silently
/// resurrect. Nonmaterial wording updates the current text in place with no
/// new revision, no cycle movement, and no fact. Drafts revise through
/// draft editing; terminal rows are read-only.
///
/// # Errors
///
/// Returns field-naming refusals for invalid input, catalog and policy
/// refusals for disallowed requirement sets, [`ReviseError::NotActive`] for
/// restricted accounts, [`ReviseError::NotFound`] for missing or non-owned
/// rows, [`ReviseError::ForbiddenState`] past revision scope,
/// [`ReviseError::RevisionQuotaExhausted`] past three successful material
/// revisions per rolling day, else [`ReviseError::StorageFailed`].
pub async fn revise_request(
    pool: &sqlx::PgPool,
    author_id: uuid::Uuid,
    request_id: uuid::Uuid,
    input: ReviseInput,
) -> Result<RevisedRequest, ReviseError> {
    match check_actor(pool, author_id)
        .await
        .map_err(|_| ReviseError::StorageFailed)?
    {
        CheckOutcome::Permitted => {}
        CheckOutcome::Refused(_) => return Err(ReviseError::NotActive),
    }
    let stored = requests::request(pool, request_id)
        .await
        .map_err(|_| ReviseError::StorageFailed)?;
    let stored = match stored {
        Some(stored) if stored.author_id == author_id => stored,
        _ => return Err(ReviseError::NotFound),
    };
    if stored.state == "draft" || stored.state == "completed" || stored.state == "cancelled" {
        return Err(ReviseError::ForbiddenState);
    }
    let snapshot = validate(&input)?;
    let material = is_material(&stored, &snapshot);

    let mut tx = pool.begin().await.map_err(|_| ReviseError::StorageFailed)?;
    let locked: Option<(String, i32)> = sqlx::query_as(
        "SELECT state, current_revision_number FROM requests WHERE id = $1 FOR UPDATE",
    )
    .bind(request_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ReviseError::StorageFailed)?;
    let (state, current_revision) = locked.ok_or(ReviseError::StorageFailed)?;
    if state == "draft" || state == "completed" || state == "cancelled" {
        tx.rollback()
            .await
            .map_err(|_| ReviseError::StorageFailed)?;
        return Err(ReviseError::ForbiddenState);
    }
    if material {
        if material_count(&mut tx, author_id, chrono::Utc::now()).await?
            >= MAX_MATERIAL_REVISIONS_PER_DAY
        {
            tx.rollback()
                .await
                .map_err(|_| ReviseError::StorageFailed)?;
            return Err(ReviseError::RevisionQuotaExhausted);
        }
        if let Err(error) = check_revision_policy(&mut tx, &snapshot).await {
            tx.rollback()
                .await
                .map_err(|_| ReviseError::StorageFailed)?;
            return Err(error);
        }
        let revision = insert_revision(&mut tx, request_id, current_revision + 1, &snapshot)
            .await
            .map_err(|_| ReviseError::StorageFailed)?;
        update_current_requirements(&mut tx, request_id, current_revision + 1, &snapshot)
            .await
            .map_err(|_| ReviseError::StorageFailed)?;
        record_event(
            &mut *tx,
            NewEvent {
                actor_id: Some(author_id),
                resource_kind: "request",
                resource_id: request_id,
                cycle: Some(stored.current_cycle_number),
                revision: Some(revision.id),
                effective_at: chrono::Utc::now(),
                kind: "request.revised",
                policy: "mvp-free",
                source: "api",
                payload: json!({"revision_number": current_revision + 1}),
            },
        )
        .await
        .map_err(|_| ReviseError::StorageFailed)?;
    } else {
        sqlx::query("UPDATE requests SET title = $2, notes = $3, updated_at = now() WHERE id = $1")
            .bind(request_id)
            .bind(&snapshot.title)
            .bind(&snapshot.notes)
            .execute(&mut *tx)
            .await
            .map_err(|_| ReviseError::StorageFailed)?;
    }
    tx.commit().await.map_err(|_| ReviseError::StorageFailed)?;
    let updated = requests::request(pool, request_id)
        .await
        .map_err(|_| ReviseError::StorageFailed)?
        .ok_or(ReviseError::StorageFailed)?;
    let budget = format_budget(updated.budget_cents);
    Ok(RevisedRequest {
        id: updated.id,
        revision_number: updated.current_revision_number,
        material,
        title: updated.title,
        category_code: updated.category_code,
        budget_cents: updated.budget_cents,
        budget,
        condition: updated.condition,
        city_code: updated.city_code,
        region_code: updated.region_code,
        notes: updated.notes,
        state: updated.state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(title: &str, notes: &str) -> requests::Request {
        requests::Request {
            id: uuid::Uuid::now_v7(),
            author_id: uuid::Uuid::now_v7(),
            title: title.to_owned(),
            category_code: "home_appliances".to_owned(),
            budget_cents: 52_000,
            condition: "either".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            notes: notes.to_owned(),
            state: "active".to_owned(),
            visibility: "public".to_owned(),
            original_published_at: None,
            current_cycle_number: 1,
            current_revision_number: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn snapshot(title: &str, notes: &str) -> RequirementSnapshot {
        RequirementSnapshot {
            title: title.to_owned(),
            category_code: "home_appliances".to_owned(),
            budget_cents: 52_000,
            condition: "either".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            notes: notes.to_owned(),
        }
    }

    #[test]
    fn spelling_only_is_nonmaterial() {
        let current = stored("Refrigerator", "Preferably frost-free.");
        assert!(!is_material(
            &current,
            &snapshot("  REFRIGERATOR ", "preferably   FROST-free.")
        ));
        assert!(!is_material(
            &current,
            &snapshot("Refrigerator", "Preferably frost-free.")
        ));
    }

    #[test]
    fn every_identity_field_is_material() {
        let current = stored("Refrigerator", "Preferably frost-free.");
        let mut varied = snapshot("Refrigerator", "Preferably frost-free.");
        varied.budget_cents = 52_001;
        assert!(is_material(&current, &varied));
        for varied in [
            RequirementSnapshot {
                title: "Double-door refrigerator".to_owned(),
                ..snapshot("Refrigerator", "Preferably frost-free.")
            },
            RequirementSnapshot {
                category_code: "furniture".to_owned(),
                ..snapshot("Refrigerator", "Preferably frost-free.")
            },
            RequirementSnapshot {
                budget_cents: 51_999,
                ..snapshot("Refrigerator", "Preferably frost-free.")
            },
            RequirementSnapshot {
                condition: "new".to_owned(),
                ..snapshot("Refrigerator", "Preferably frost-free.")
            },
            RequirementSnapshot {
                city_code: "valinhos".to_owned(),
                ..snapshot("Refrigerator", "Preferably frost-free.")
            },
            RequirementSnapshot {
                notes: "Must include a water dispenser.".to_owned(),
                ..snapshot("Refrigerator", "Preferably frost-free.")
            },
        ] {
            assert!(is_material(&current, &varied));
        }
    }

    #[test]
    fn quotas_match_the_policy_defaults() {
        assert_eq!(MAX_MATERIAL_REVISIONS_PER_DAY, 3);
        assert_eq!(REVISION_WINDOW_HOURS, 24);
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            ReviseError::InvalidField("title"),
            ReviseError::InvalidAmount(crate::domain::money::MoneyError::Overprecision),
            ReviseError::UnknownCategory,
            ReviseError::UnknownCity,
            ReviseError::UnknownRegion,
            ReviseError::RetiredCategory,
            ReviseError::ProhibitedCategory,
            ReviseError::CityNotEnabled,
            ReviseError::NotActive,
            ReviseError::NotFound,
            ReviseError::ForbiddenState,
            ReviseError::RevisionQuotaExhausted,
            ReviseError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
            assert!(!rendered.contains("+55"));
        }
    }
}
