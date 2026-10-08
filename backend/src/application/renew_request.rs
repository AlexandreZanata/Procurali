//! Explicit renewal: a fresh seven-day cycle from eligible demand only.
//!
//! Canonical rules: INV-12 (renewal starts a new cycle and never rewrites
//! first-publication history — `original_published_at` is never touched),
//! INV-13 (renewal is its own distinct `request.renewed` fact, never a new
//! publication metric), INV-15 (renewal neither resets rolling counts nor
//! counts as newly acquired demand — it consumes one activation and joins
//! the same rolling window publications draw from), INV-26 with AC-11 and
//! EC-08 (previous-cycle offers stay expired and never go live again — the
//! ended cycle is stamped, the new cycle number scopes all fresh responses,
//! and original age and history are retained), AC-12 (anything short of an
//! explicit confirmed renewal writes nothing: no cycle, no fact, no
//! activation).
//!
//! Eligibility: expired visible rows, or active visible rows inside their
//! last 24 hours. Drafts revise elsewhere; terminal, suspended, and hidden
//! rows are refused — restoration and removal have their own writers. The
//! buyer confirms the requirements are current by resupplying them: any
//! material difference refuses (revise first, then renew). Open slots and
//! the combined publication-plus-renewal activation window are guarded in
//! the same locked transaction; the prior cycle ends and the new one opens
//! atomically with its fact.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::application::request_limits::{
    activation_count, open_request_count, MAX_ACTIVATIONS_PER_WINDOW, MAX_OPEN_REQUESTS,
};
use crate::domain::money::{Money, MoneyError};
use crate::domain::text::normalize_for_comparison;
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::requests;
use crate::persistence::transaction::AttemptError;
use serde_json::json;

/// Renewal window: active rows may renew inside their final hours.
pub const RENEWAL_WINDOW_HOURS: i64 = 24;

/// Buyer-confirmed current requirements: resupplied in full, every key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenewalConfirmation {
    /// Item title, as currently displayed.
    pub title: String,
    /// Category code, as currently displayed.
    pub category_code: String,
    /// Maximum budget as an exact decimal string, as currently displayed.
    pub budget: String,
    /// Accepted condition, as currently displayed.
    pub condition: String,
    /// City code, as currently displayed.
    pub city_code: String,
    /// Region code, as currently displayed.
    pub region_code: String,
    /// Notes, as currently displayed.
    pub notes: String,
}

/// One renewed request: the fresh cycle with retained identity.
#[derive(Debug, Clone, PartialEq)]
pub struct RenewedRequest {
    /// Request identifier (unchanged by renewal).
    pub id: uuid::Uuid,
    /// New current cycle number.
    pub cycle_number: i32,
    /// Current revision number (untouched by renewal).
    pub revision_number: i32,
    /// New cycle start (effective instant).
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// New exclusive deadline, seven days out.
    pub deadline: chrono::DateTime<chrono::Utc>,
    /// First-publication instant (never rewritten).
    pub original_published_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Lifecycle state after renewal (`active`).
    pub state: String,
}

/// Renewal window verdict for one row at the effective time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenewWindow {
    /// Expired, or active inside its final hours.
    Eligible,
    /// Active with more than a day left.
    TooEarly,
    /// Draft, terminal, suspended, hidden, or cycless.
    Ineligible,
}

/// Pure window derivation: lifecycle, visibility, and time only.
fn renewal_window(
    state: &str,
    visibility: &str,
    deadline: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> RenewWindow {
    if state == "draft"
        || state == "completed"
        || state == "cancelled"
        || state == "suspended"
        || visibility == "hidden"
    {
        return RenewWindow::Ineligible;
    }
    if state == "expired" {
        return RenewWindow::Eligible;
    }
    if state != "active" {
        return RenewWindow::Ineligible;
    }
    match deadline {
        None => RenewWindow::Ineligible,
        Some(deadline) if now >= deadline => RenewWindow::Eligible,
        Some(deadline) if deadline - now <= chrono::Duration::hours(RENEWAL_WINDOW_HOURS) => {
            RenewWindow::Eligible
        }
        Some(_) => RenewWindow::TooEarly,
    }
}

/// Typed renewal failure. Static reasons and static field names only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenewError {
    /// The confirmation budget fails the exact-decimal grammar.
    InvalidAmount(MoneyError),
    /// The confirmation differs materially: revise first, then renew.
    StaleRequirements,
    /// The active cycle has more than a day left.
    EarlyRenewal,
    /// Three genuinely open requests already exist.
    OpenSlotsExhausted,
    /// Six successful activations already fall inside the rolling window.
    ActivationQuotaExhausted,
    /// The account is missing, deleted, or not `active`.
    NotActive,
    /// No such request for this owner (missing or non-owned —
    /// deliberately indistinguishable).
    NotFound,
    /// The row is a draft, terminal, suspended, or hidden row.
    ForbiddenState,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for RenewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAmount(error) => std::fmt::Display::fmt(error, f),
            Self::StaleRequirements => f.write_str("requirements changed; revise first"),
            Self::EarlyRenewal => f.write_str("renewal opens in the final day"),
            Self::OpenSlotsExhausted => f.write_str("open request slots exhausted"),
            Self::ActivationQuotaExhausted => f.write_str("activation quota exhausted"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("request not found"),
            Self::ForbiddenState => f.write_str("request cannot be renewed"),
            Self::StorageFailed => f.write_str("renewal storage failed"),
        }
    }
}

impl std::error::Error for RenewError {}

/// Successful `request.renewed` facts for one account inside the rolling
/// window ending at `now`. Joins the publication count so renewals consume
/// the same allowance without resetting it (EC-08, INV-15); a future
/// unification owns the single predicate.
async fn renewal_count(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    author_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<u32, RenewError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM business_events
         WHERE actor_id = $1
           AND resource_kind = 'request'
           AND kind = 'request.renewed'
           AND effective_at > $2",
    )
    .bind(author_id)
    .bind(now - chrono::Duration::hours(24))
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| RenewError::StorageFailed)?;
    u32::try_from(count).map_err(|_| RenewError::StorageFailed)
}

/// Renew one owned eligible request: confirm currency, guard quotas, end
/// the prior cycle, and open a fresh seven-day cycle with exactly one
/// `request.renewed` fact. Identity, revision, and first-publication time
/// are retained; previous-cycle offers stay expired under the stamped cycle.
///
/// # Errors
///
/// Returns [`RenewError::StaleRequirements`] for changed requirements,
/// [`RenewError::EarlyRenewal`] outside the final day,
/// [`RenewError::OpenSlotsExhausted`]/
/// [`RenewError::ActivationQuotaExhausted`] past allowances,
/// [`RenewError::NotActive`] for restricted accounts,
/// [`RenewError::NotFound`] for missing or non-owned rows,
/// [`RenewError::ForbiddenState`] past renewal scope, else
/// [`RenewError::StorageFailed`].
pub async fn renew_request(
    pool: &sqlx::PgPool,
    author_id: uuid::Uuid,
    request_id: uuid::Uuid,
    confirmation: RenewalConfirmation,
) -> Result<RenewedRequest, RenewError> {
    match check_actor(pool, author_id)
        .await
        .map_err(|_| RenewError::StorageFailed)?
    {
        CheckOutcome::Permitted => {}
        CheckOutcome::Refused(_) => return Err(RenewError::NotActive),
    }
    let stored = requests::request(pool, request_id)
        .await
        .map_err(|_| RenewError::StorageFailed)?;
    let stored = match stored {
        Some(stored) if stored.author_id == author_id => stored,
        _ => return Err(RenewError::NotFound),
    };
    // Input shape first; currency of the requirements second. Lifecycle
    // scope outranks confirmation content: a terminal row is refused as
    // terminal whatever the caller supplied.
    let budget = Money::parse(&confirmation.budget).map_err(RenewError::InvalidAmount)?;

    let mut tx = pool.begin().await.map_err(|_| RenewError::StorageFailed)?;
    // Lock the row first: concurrent renewals of the same request serialize
    // here, and the loser observes the winner's fresh cycle.
    let locked: Option<(String, String, i32, i32)> = sqlx::query_as(
        "SELECT state, visibility, current_cycle_number, current_revision_number
         FROM requests WHERE id = $1 FOR UPDATE",
    )
    .bind(request_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| RenewError::StorageFailed)?;
    let (state, visibility, cycle_number, revision_number) =
        locked.ok_or(RenewError::StorageFailed)?;
    let deadline: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT deadline FROM request_cycles WHERE request_id = $1 AND cycle_number = $2",
    )
    .bind(request_id)
    .bind(cycle_number)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| RenewError::StorageFailed)?;
    let now = chrono::Utc::now();
    match renewal_window(&state, &visibility, deadline, now) {
        RenewWindow::Eligible => {}
        RenewWindow::TooEarly => {
            tx.rollback().await.map_err(|_| RenewError::StorageFailed)?;
            return Err(RenewError::EarlyRenewal);
        }
        RenewWindow::Ineligible => {
            tx.rollback().await.map_err(|_| RenewError::StorageFailed)?;
            return Err(RenewError::ForbiddenState);
        }
    }
    let confirmed_current = normalize_for_comparison(&confirmation.title)
        == normalize_for_comparison(&stored.title)
        && confirmation.category_code == stored.category_code
        && budget.cents() == stored.budget_cents
        && confirmation.condition == stored.condition
        && confirmation.city_code == stored.city_code
        && confirmation.region_code == stored.region_code
        && normalize_for_comparison(&confirmation.notes) == normalize_for_comparison(&stored.notes);
    if !confirmed_current {
        tx.rollback().await.map_err(|_| RenewError::StorageFailed)?;
        return Err(RenewError::StaleRequirements);
    }
    let map_limit =
        |error: AttemptError<crate::application::request_limits::LimitError>| match error {
            AttemptError::Abort(limit) => {
                use crate::application::request_limits::LimitError as Limit;
                match limit {
                    Limit::OpenSlotsExhausted => RenewError::OpenSlotsExhausted,
                    Limit::ActivationQuotaExhausted => RenewError::ActivationQuotaExhausted,
                    Limit::NotActive => RenewError::NotActive,
                    Limit::StorageFailed => RenewError::StorageFailed,
                }
            }
            AttemptError::Db(_) => RenewError::StorageFailed,
        };
    if open_request_count(&mut tx, author_id, now)
        .await
        .map_err(map_limit)?
        >= MAX_OPEN_REQUESTS
    {
        tx.rollback().await.map_err(|_| RenewError::StorageFailed)?;
        return Err(RenewError::OpenSlotsExhausted);
    }
    let published = activation_count(&mut tx, author_id, now)
        .await
        .map_err(map_limit)?;
    let renewed = renewal_count(&mut tx, author_id, now).await?;
    if published.saturating_add(renewed) >= MAX_ACTIVATIONS_PER_WINDOW {
        tx.rollback().await.map_err(|_| RenewError::StorageFailed)?;
        return Err(RenewError::ActivationQuotaExhausted);
    }
    sqlx::query(
        "UPDATE request_cycles SET ended_at = $3
         WHERE request_id = $1 AND cycle_number = $2 AND ended_at IS NULL",
    )
    .bind(request_id)
    .bind(cycle_number)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|_| RenewError::StorageFailed)?;
    let started_at = now;
    let deadline = started_at + chrono::Duration::days(7);
    let cycle: Option<i32> = sqlx::query_scalar(
        "INSERT INTO request_cycles (request_id, cycle_number, started_at, deadline)
         VALUES ($1, $2, $3, $4) RETURNING cycle_number",
    )
    .bind(request_id)
    .bind(cycle_number + 1)
    .bind(started_at)
    .bind(deadline)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| RenewError::StorageFailed)?;
    if cycle.is_none() {
        tx.rollback().await.map_err(|_| RenewError::StorageFailed)?;
        return Err(RenewError::StorageFailed);
    }
    sqlx::query(
        "UPDATE requests SET state = 'active', visibility = 'public',
                current_cycle_number = $2, updated_at = now()
         WHERE id = $1",
    )
    .bind(request_id)
    .bind(cycle_number + 1)
    .execute(&mut *tx)
    .await
    .map_err(|_| RenewError::StorageFailed)?;
    let revision_id: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM request_revisions WHERE request_id = $1 AND revision_number = $2",
    )
    .bind(request_id)
    .bind(revision_number)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| RenewError::StorageFailed)?;
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(author_id),
            resource_kind: "request",
            resource_id: request_id,
            cycle: Some(cycle_number + 1),
            revision: revision_id,
            effective_at: started_at,
            kind: "request.renewed",
            policy: "mvp-free",
            source: "api",
            payload: json!({"state": "active", "cycle_number": cycle_number + 1}),
        },
    )
    .await
    .map_err(|_| RenewError::StorageFailed)?;
    tx.commit().await.map_err(|_| RenewError::StorageFailed)?;
    let stored = requests::request(pool, request_id)
        .await
        .map_err(|_| RenewError::StorageFailed)?
        .ok_or(RenewError::StorageFailed)?;
    Ok(RenewedRequest {
        id: stored.id,
        cycle_number: stored.current_cycle_number,
        revision_number: stored.current_revision_number,
        started_at,
        deadline,
        original_published_at: stored.original_published_at,
        state: stored.state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instant(seconds: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(seconds, 0).expect("fixture instant builds")
    }

    #[test]
    fn window_matches_lifecycle_time_and_visibility() {
        let now = instant(1_791_000_000);
        let far = now + chrono::Duration::days(7);
        let near = now + chrono::Duration::hours(12);
        let past = now - chrono::Duration::hours(1);
        // Expired rows renew regardless of how long ago; active rows only in
        // the final day (boundary inclusive).
        assert_eq!(
            renewal_window("expired", "public", Some(past), now),
            RenewWindow::Eligible
        );
        assert_eq!(
            renewal_window("active", "public", Some(near), now),
            RenewWindow::Eligible
        );
        assert_eq!(
            renewal_window(
                "active",
                "public",
                Some(now + chrono::Duration::hours(24)),
                now
            ),
            RenewWindow::Eligible
        );
        assert_eq!(
            renewal_window("active", "public", Some(far), now),
            RenewWindow::TooEarly
        );
        // Drafts, terminal rows, suspended rows, and hidden rows never renew.
        for (state, visibility) in [
            ("draft", "private"),
            ("completed", "public"),
            ("cancelled", "hidden"),
            ("suspended", "public"),
            ("active", "hidden"),
            ("expired", "hidden"),
        ] {
            assert_eq!(
                renewal_window(state, visibility, Some(past), now),
                RenewWindow::Ineligible,
                "{state}/{visibility} never renews"
            );
        }
        assert_eq!(
            renewal_window("active", "public", None, now),
            RenewWindow::Ineligible
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            RenewError::InvalidAmount(crate::domain::money::MoneyError::Overprecision),
            RenewError::StaleRequirements,
            RenewError::EarlyRenewal,
            RenewError::OpenSlotsExhausted,
            RenewError::ActivationQuotaExhausted,
            RenewError::NotActive,
            RenewError::NotFound,
            RenewError::ForbiddenState,
            RenewError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
            assert!(!rendered.contains("+55"));
        }
    }
}
