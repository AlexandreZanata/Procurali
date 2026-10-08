//! Owner request reads: bounded lists and details with truthful actions.
//!
//! Canonical rules: INV-05 (no role input — every read scopes by the
//! authenticated account identifier and nothing else), INV-09 (no new offers
//! or contact past completion, cancellation, expiry, suspension, removal, or
//! restriction — offer/contact actions derive from current lifecycle, time,
//! visibility, and category standing, never from preflight client state),
//! AC-10 (a deadline reached refuses submissions and contact and drops the
//! row from active discovery even before any expiry job runs — reads derive
//! expiry from the recorded deadline against the current time, so a
//! nominally `active` elapsed row already carries no offer/contact action),
//! INV-31 (owner and public projections stay separate — summaries here are
//! owner-private; the public projection lives in persistence).
//!
//! The action vocabulary names only capabilities that exist: drafts offer
//! editing and publication; live unexpired public cycles on non-prohibited
//! categories offer receiving and starting contact. Lifecycle writers
//! (renew, complete, cancel, remove) extend this vocabulary in their owning
//! cards; until then no phantom action is advertised.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::domain::money::Money;
use crate::persistence::catalogs::{self, CategoryStatus};
use crate::persistence::requests::{self, format_budget};

/// Default page size for owner lists.
pub const DEFAULT_PAGE_LIMIT: u32 = 20;

/// Hard ceiling for one owner-list page.
pub const MAX_PAGE_LIMIT: u32 = 50;

/// One currently permitted owner capability. Only existing routes appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestAction {
    /// Replace the draft's requirements (`PUT` draft route).
    Edit,
    /// Publish the draft into its first cycle (publication route).
    Publish,
    /// Accept new offers on the live cycle (offer writers, next phase).
    ReceiveOffers,
    /// Start WhatsApp contact from a live offer (contact writers, next phase).
    StartContact,
}

impl RequestAction {
    /// Stable wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Publish => "publish",
            Self::ReceiveOffers => "receive_offers",
            Self::StartContact => "start_contact",
        }
    }
}

/// Category standing for action derivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CategoryStanding {
    Allowed,
    Retired,
    Prohibited,
    Unknown,
}

/// Pure action derivation: lifecycle, time, visibility, and policy only.
/// Quotas and duplicates are writer-enforced at attempt time, never
/// precomputed here.
fn derive_actions(
    state: &str,
    visibility: &str,
    expired: bool,
    category: CategoryStanding,
    city_enabled: bool,
) -> Vec<RequestAction> {
    if state == "draft" {
        return match category {
            CategoryStanding::Allowed if city_enabled => {
                vec![RequestAction::Edit, RequestAction::Publish]
            }
            _ => vec![RequestAction::Edit],
        };
    }
    if state == "active"
        && !expired
        && visibility == "public"
        && category != CategoryStanding::Prohibited
    {
        return vec![RequestAction::ReceiveOffers, RequestAction::StartContact];
    }
    Vec::new()
}

/// One owner-visible request: requirements plus derived timing and actions.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestSummary {
    /// Request identifier.
    pub id: uuid::Uuid,
    /// Item title.
    pub title: String,
    /// Category code.
    pub category_code: String,
    /// Maximum budget in minor units.
    pub budget_cents: i64,
    /// Exact decimal rendering.
    pub budget: String,
    /// Accepted condition.
    pub condition: String,
    /// City code.
    pub city_code: String,
    /// Region code.
    pub region_code: String,
    /// Notes.
    pub notes: String,
    /// Lifecycle state as recorded (jobs may lag behind time).
    pub state: String,
    /// Current cycle number (0 while never published).
    pub cycle_number: i32,
    /// Current revision number (0 while never published).
    pub revision_number: i32,
    /// First-publication instant, once published.
    pub original_published_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Current cycle deadline, once published.
    pub deadline: Option<chrono::DateTime<chrono::Utc>>,
    /// True once the deadline passes, regardless of recorded state.
    pub expired: bool,
    /// Currently permitted capabilities (existing routes only).
    pub actions: Vec<RequestAction>,
}

/// One activation cycle for owner history.
#[derive(Debug, Clone, PartialEq)]
pub struct CycleView {
    /// Monotonic cycle number.
    pub cycle_number: i32,
    /// Cycle start.
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Exclusive deadline.
    pub deadline: chrono::DateTime<chrono::Utc>,
}

/// One immutable requirement revision for owner history.
#[derive(Debug, Clone, PartialEq)]
pub struct RevisionView {
    /// Monotonic revision number.
    pub revision_number: i32,
    /// Snapshot title.
    pub title: String,
    /// Snapshot category code.
    pub category_code: String,
    /// Snapshot budget rendering.
    pub budget: String,
    /// Snapshot condition.
    pub condition: String,
    /// Snapshot city code.
    pub city_code: String,
    /// Snapshot region code.
    pub region_code: String,
    /// Snapshot notes.
    pub notes: String,
}

/// One owner-visible request with its cycles and revisions.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestDetail {
    /// The request itself with derived timing and actions.
    pub summary: RequestSummary,
    /// Activation cycles, oldest first.
    pub cycles: Vec<CycleView>,
    /// Requirement revisions, oldest first.
    pub revisions: Vec<RevisionView>,
}

/// Typed read failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadError {
    /// The account is missing, deleted, or not `active`.
    NotActive,
    /// No such request for this owner (missing or non-owned —
    /// deliberately indistinguishable).
    NotFound,
    /// Pagination is malformed or the lookup failed.
    InvalidPage,
    /// The lookup failed.
    StorageFailed,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("request not found"),
            Self::InvalidPage => f.write_str("invalid pagination"),
            Self::StorageFailed => f.write_str("request lookup failed"),
        }
    }
}

impl std::error::Error for ReadError {}

async fn eligible_owner(pool: &sqlx::PgPool, author_id: uuid::Uuid) -> Result<(), ReadError> {
    match check_actor(pool, author_id)
        .await
        .map_err(|_| ReadError::StorageFailed)?
    {
        CheckOutcome::Permitted => Ok(()),
        CheckOutcome::Refused(_) => Err(ReadError::NotActive),
    }
}

async fn policy_standing(
    pool: &sqlx::PgPool,
    category_code: &str,
    city_code: &str,
) -> Result<(CategoryStanding, bool), ReadError> {
    let category = catalogs::category(pool, category_code)
        .await
        .map_err(|_| ReadError::StorageFailed)?;
    let standing = match category.map(|entry| entry.status) {
        None => CategoryStanding::Unknown,
        Some(CategoryStatus::Allowed) => CategoryStanding::Allowed,
        Some(CategoryStatus::Retired) => CategoryStanding::Retired,
        Some(CategoryStatus::Prohibited) => CategoryStanding::Prohibited,
    };
    let cities = catalogs::cities(pool, false)
        .await
        .map_err(|_| ReadError::StorageFailed)?;
    let enabled = cities
        .iter()
        .any(|city| city.code == city_code && city.enabled);
    Ok((standing, enabled))
}

async fn summarize(
    pool: &sqlx::PgPool,
    stored: requests::Request,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<RequestSummary, ReadError> {
    let cycle = if stored.current_cycle_number >= 1 {
        requests::cycle(pool, stored.id, stored.current_cycle_number)
            .await
            .map_err(|_| ReadError::StorageFailed)?
    } else {
        None
    };
    let deadline = cycle.map(|entry| entry.deadline);
    let expired = deadline.is_some_and(|deadline| now >= deadline);
    let (category, city_enabled) =
        policy_standing(pool, &stored.category_code, &stored.city_code).await?;
    let budget = Money::parse(&format_budget(stored.budget_cents))
        .map(|money| money.format())
        .unwrap_or_else(|_| format_budget(stored.budget_cents));
    Ok(RequestSummary {
        id: stored.id,
        title: stored.title.clone(),
        category_code: stored.category_code.clone(),
        budget_cents: stored.budget_cents,
        budget,
        condition: stored.condition.clone(),
        city_code: stored.city_code.clone(),
        region_code: stored.region_code.clone(),
        notes: stored.notes.clone(),
        actions: derive_actions(
            &stored.state,
            &stored.visibility,
            expired,
            category,
            city_enabled,
        ),
        state: stored.state,
        cycle_number: stored.current_cycle_number,
        revision_number: stored.current_revision_number,
        original_published_at: stored.original_published_at,
        deadline,
        expired,
    })
}

/// Bounded owner request list, newest first, with the total owned count.
///
/// # Errors
///
/// Returns [`ReadError::NotActive`] for restricted accounts,
/// [`ReadError::InvalidPage`] for malformed pagination, else
/// [`ReadError::StorageFailed`].
pub async fn list_requests(
    pool: &sqlx::PgPool,
    author_id: uuid::Uuid,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<(Vec<RequestSummary>, i64), ReadError> {
    eligible_owner(pool, author_id).await?;
    let limit = limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    let offset = offset.unwrap_or(0);
    if !(1..=MAX_PAGE_LIMIT).contains(&limit) {
        return Err(ReadError::InvalidPage);
    }
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM requests WHERE author_id = $1")
        .bind(author_id)
        .fetch_one(pool)
        .await
        .map_err(|_| ReadError::StorageFailed)?;
    let ids: Vec<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM requests WHERE author_id = $1
         ORDER BY created_at DESC, id DESC LIMIT $2 OFFSET $3",
    )
    .bind(author_id)
    .bind(i64::from(limit))
    .bind(i64::from(offset))
    .fetch_all(pool)
    .await
    .map_err(|_| ReadError::StorageFailed)?;
    let now = chrono::Utc::now();
    let mut summaries = Vec::with_capacity(ids.len());
    for id in ids {
        let stored = requests::request(pool, id)
            .await
            .map_err(|_| ReadError::StorageFailed)?
            .ok_or(ReadError::StorageFailed)?;
        if stored.author_id != author_id {
            return Err(ReadError::StorageFailed);
        }
        summaries.push(summarize(pool, stored, now).await?);
    }
    Ok((summaries, total))
}

/// One owned request with its cycles and revisions.
///
/// # Errors
///
/// Returns [`ReadError::NotActive`] for restricted accounts,
/// [`ReadError::NotFound`] for missing or non-owned rows, else
/// [`ReadError::StorageFailed`].
pub async fn get_request(
    pool: &sqlx::PgPool,
    author_id: uuid::Uuid,
    request_id: uuid::Uuid,
) -> Result<RequestDetail, ReadError> {
    eligible_owner(pool, author_id).await?;
    let stored = requests::request(pool, request_id)
        .await
        .map_err(|_| ReadError::StorageFailed)?;
    let stored = match stored {
        Some(stored) if stored.author_id == author_id => stored,
        _ => return Err(ReadError::NotFound),
    };
    let now = chrono::Utc::now();
    let summary = summarize(pool, stored, now).await?;
    let cycles = requests::cycles_for_request(pool, request_id)
        .await
        .map_err(|_| ReadError::StorageFailed)?
        .into_iter()
        .map(|cycle| CycleView {
            cycle_number: cycle.cycle_number,
            started_at: cycle.started_at,
            deadline: cycle.deadline,
        })
        .collect();
    let revisions = requests::revisions_for_request(pool, request_id)
        .await
        .map_err(|_| ReadError::StorageFailed)?
        .into_iter()
        .map(|revision| RevisionView {
            revision_number: revision.revision_number,
            title: revision.title,
            category_code: revision.category_code,
            budget: format_budget(revision.budget_cents),
            condition: revision.condition,
            city_code: revision.city_code,
            region_code: revision.region_code,
            notes: revision.notes,
        })
        .collect();
    Ok(RequestDetail {
        summary,
        cycles,
        revisions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drafts_offer_edit_and_policy_gated_publish() {
        assert_eq!(
            derive_actions("draft", "private", false, CategoryStanding::Allowed, true),
            [RequestAction::Edit, RequestAction::Publish]
        );
        // A disallowed category or city leaves editing but removes publish.
        for (category, city) in [
            (CategoryStanding::Retired, true),
            (CategoryStanding::Prohibited, true),
            (CategoryStanding::Unknown, true),
            (CategoryStanding::Allowed, false),
        ] {
            assert_eq!(
                derive_actions("draft", "private", false, category, city),
                [RequestAction::Edit]
            );
        }
    }

    #[test]
    fn live_cycles_offer_contact_until_time_or_policy_stops_them() {
        assert_eq!(
            derive_actions("active", "public", false, CategoryStanding::Allowed, true),
            [RequestAction::ReceiveOffers, RequestAction::StartContact]
        );
        assert_eq!(
            derive_actions("active", "public", false, CategoryStanding::Retired, true),
            [RequestAction::ReceiveOffers, RequestAction::StartContact]
        );
        // Elapsed, terminal, hidden, and prohibited rows carry no action.
        for (state, visibility, expired, category) in [
            ("active", "public", true, CategoryStanding::Allowed),
            ("completed", "public", false, CategoryStanding::Allowed),
            ("cancelled", "hidden", false, CategoryStanding::Allowed),
            ("suspended", "public", false, CategoryStanding::Allowed),
            ("active", "public", false, CategoryStanding::Prohibited),
            ("active", "hidden", false, CategoryStanding::Allowed),
        ] {
            assert!(
                derive_actions(state, visibility, expired, category, true).is_empty(),
                "{state}/{visibility} carries no action"
            );
        }
    }

    #[test]
    fn actions_have_stable_strings() {
        assert_eq!(RequestAction::Edit.as_str(), "edit");
        assert_eq!(RequestAction::Publish.as_str(), "publish");
        assert_eq!(RequestAction::ReceiveOffers.as_str(), "receive_offers");
        assert_eq!(RequestAction::StartContact.as_str(), "start_contact");
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            ReadError::NotActive,
            ReadError::NotFound,
            ReadError::InvalidPage,
            ReadError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
