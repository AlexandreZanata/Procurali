//! Private offer comparison reads: buyer sets and seller self-views.
//!
//! Canonical rules: INV-05 (no role input — every read scopes by the
//! authenticated account identifier against ownership, never labels),
//! INV-25 with AC-19 and EC-13 (offers stay independent and private — a
//! seller inspects only their own terms, competing prices and buyer phone
//! data are inaccessible, and sorting never picks a winner), INV-30 with
//! INV-31 (projections carry safe factual fields only — declared names,
//! professional classification, and approximate locality labels; no phone,
//! precise address, or reporter material exists on any read path).
//!
//! The buyer receives every offer on their request partitioned into the
//! actionable comparison set and the collapsed historical context; each
//! seller receives only their own rows. Liveness derives from current
//! lifecycle, cycle, and time — never from preflight client state — and
//! contact additionally requires current terms, so stale rows visibly lose
//! contact without any job having run.

use crate::application::eligibility::{check_actor, CheckOutcome};
use crate::application::professional_profile::profile_for;
use crate::persistence::catalogs;
use crate::persistence::offers::{self};
use crate::persistence::requests;
use crate::persistence::requests::format_budget;

/// Comparison order for offer lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferSort {
    /// Newest submission first (default): original submission order, so term
    /// edits never refresh a row's position.
    Newest,
    /// Exact integer-cent price, lowest first, submission order breaking ties.
    PriceAsc,
    /// Exact integer-cent price, highest first, submission order breaking ties.
    PriceDesc,
}

impl OfferSort {
    /// Parse the `sort` query value.
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value {
            None | Some("newest") => Some(Self::Newest),
            Some("price_asc") => Some(Self::PriceAsc),
            Some("price_desc") => Some(Self::PriceDesc),
            Some(_) => None,
        }
    }
}

/// One offer as its buyer sees it: full terms plus the seller's declared
/// factual card. No buyer fields exist here — the caller already owns them.
#[derive(Debug, Clone, PartialEq)]
pub struct BuyerOfferView {
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
    /// Seller city code with its approximate label.
    pub city_code: String,
    /// Approximate city label (code fallback when unseeded).
    pub city_label: String,
    /// Seller region code with its approximate label.
    pub region_code: String,
    /// Approximate region label (code fallback when unseeded).
    pub region_label: String,
    /// Notes.
    pub notes: String,
    /// Seller display name as declared.
    pub seller_name: String,
    /// Professional business name, when the seller declared one.
    pub seller_business_name: Option<String>,
    /// Professional commercial type, when declared.
    pub seller_business_type: Option<String>,
    /// Lifecycle state.
    pub state: String,
    /// In the actionable comparison set.
    pub live: bool,
    /// Contact may start from these terms.
    pub contact_allowed: bool,
    /// Original submission instant (ordering key — edits never move it).
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// One offer as its seller sees it: own full terms plus derived flags. No
/// buyer field of any kind exists here — not even an identifier.
#[derive(Debug, Clone, PartialEq)]
pub struct SellerOfferView {
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
    /// Lifecycle state.
    pub state: String,
    /// In the actionable comparison set.
    pub live: bool,
    /// Contact may start from these terms.
    pub contact_allowed: bool,
    /// Original submission instant.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Buyer comparison set: actionable offers apart from history.
#[derive(Debug, Clone, PartialEq)]
pub struct BuyerOfferList {
    /// Actionable comparison set, in the requested order.
    pub live: Vec<BuyerOfferView>,
    /// Collapsed historical context, in the requested order.
    pub history: Vec<BuyerOfferView>,
    /// Total offers on the request.
    pub total: usize,
}

/// Typed comparison-read failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferReadError {
    /// The account is missing, deleted, or not `active`.
    NotActive,
    /// No such row for this caller (missing or non-owned —
    /// deliberately indistinguishable).
    NotFound,
    /// The lookup failed.
    StorageFailed,
}

impl std::fmt::Display for OfferReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotActive => f.write_str("account is not active"),
            Self::NotFound => f.write_str("offer not found"),
            Self::StorageFailed => f.write_str("offer lookup failed"),
        }
    }
}

impl std::error::Error for OfferReadError {}

async fn eligible_caller(
    pool: &sqlx::PgPool,
    account_id: uuid::Uuid,
) -> Result<(), OfferReadError> {
    match check_actor(pool, account_id)
        .await
        .map_err(|_| OfferReadError::StorageFailed)?
    {
        CheckOutcome::Permitted => Ok(()),
        CheckOutcome::Refused(_) => Err(OfferReadError::NotActive),
    }
}

async fn seller_display_name(
    pool: &sqlx::PgPool,
    seller_id: uuid::Uuid,
) -> Result<String, OfferReadError> {
    sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
        .bind(seller_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| OfferReadError::StorageFailed)?
        .ok_or(OfferReadError::StorageFailed)
}

async fn locality_labels(
    pool: &sqlx::PgPool,
    city_code: &str,
    region_code: &str,
) -> Result<(String, String), OfferReadError> {
    let cities = catalogs::cities(pool, false)
        .await
        .map_err(|_| OfferReadError::StorageFailed)?;
    let city_label = cities
        .iter()
        .find(|city| city.code == city_code)
        .map(|city| city.name.clone())
        .unwrap_or_else(|| city_code.to_owned());
    let region = catalogs::region(pool, city_code, region_code)
        .await
        .map_err(|_| OfferReadError::StorageFailed)?;
    let region_label = region
        .map(|entry| entry.name)
        .unwrap_or_else(|| region_code.to_owned());
    Ok((city_label, region_label))
}

/// Pure liveness derivation: engagement-capable state on the current cycle
/// before its exclusive deadline, on an active request.
fn is_live(
    state: &str,
    cycle_number: i32,
    current_cycle: i32,
    deadline: Option<chrono::DateTime<chrono::Utc>>,
    request_state: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    matches!(state, "sent" | "viewed" | "contacted")
        && cycle_number == current_cycle
        && request_state == "active"
        && deadline.is_some_and(|deadline| now < deadline)
}

fn sort_offers<T>(
    rows: &mut [T],
    sort: OfferSort,
    key: impl Fn(&T) -> (i64, chrono::DateTime<chrono::Utc>, uuid::Uuid),
) {
    match sort {
        OfferSort::Newest => rows.sort_by_key(|row| {
            let (_, created, id) = key(row);
            (std::cmp::Reverse(created), id)
        }),
        OfferSort::PriceAsc => rows.sort_by_key(key),
        OfferSort::PriceDesc => rows.sort_by_key(|row| {
            let (price, created, id) = key(row);
            (std::cmp::Reverse(price), created, id)
        }),
    }
}

async fn buyer_view(
    pool: &sqlx::PgPool,
    stored: offers::Offer,
    current_cycle: i32,
    current_revision: i32,
    deadline: Option<chrono::DateTime<chrono::Utc>>,
    request_state: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<BuyerOfferView, OfferReadError> {
    let live = is_live(
        &stored.state,
        stored.cycle_number,
        current_cycle,
        deadline,
        request_state,
        now,
    );
    let (city_label, region_label) =
        locality_labels(pool, &stored.city_code, &stored.region_code).await?;
    let profile = profile_for(pool, stored.seller_id)
        .await
        .map_err(|_| OfferReadError::StorageFailed)?;
    Ok(BuyerOfferView {
        id: stored.id,
        request_id: stored.request_id,
        cycle_number: stored.cycle_number,
        revision_number: stored.revision_number,
        description: stored.description.clone(),
        price_cents: stored.price_cents,
        price: format_budget(stored.price_cents),
        condition: stored.condition.clone(),
        city_code: stored.city_code.clone(),
        city_label,
        region_code: stored.region_code.clone(),
        region_label,
        notes: stored.notes.clone(),
        seller_name: seller_display_name(pool, stored.seller_id).await?,
        seller_business_name: profile.as_ref().map(|entry| entry.business_name.clone()),
        seller_business_type: profile.as_ref().map(|entry| entry.business_type.clone()),
        state: stored.state.clone(),
        live,
        contact_allowed: live && stored.revision_number == current_revision,
        created_at: stored.created_at,
    })
}

fn seller_view(
    stored: offers::Offer,
    current_cycle: i32,
    current_revision: i32,
    deadline: Option<chrono::DateTime<chrono::Utc>>,
    request_state: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> SellerOfferView {
    let live = is_live(
        &stored.state,
        stored.cycle_number,
        current_cycle,
        deadline,
        request_state,
        now,
    );
    SellerOfferView {
        id: stored.id,
        request_id: stored.request_id,
        cycle_number: stored.cycle_number,
        revision_number: stored.revision_number,
        description: stored.description.clone(),
        price_cents: stored.price_cents,
        price: format_budget(stored.price_cents),
        condition: stored.condition.clone(),
        city_code: stored.city_code.clone(),
        region_code: stored.region_code.clone(),
        notes: stored.notes.clone(),
        state: stored.state.clone(),
        live,
        contact_allowed: live && stored.revision_number == current_revision,
        created_at: stored.created_at,
    }
}

struct RequestFrame {
    author_id: uuid::Uuid,
    state: String,
    current_cycle_number: i32,
    current_revision_number: i32,
    deadline: Option<chrono::DateTime<chrono::Utc>>,
}

async fn request_frame(
    pool: &sqlx::PgPool,
    request_id: uuid::Uuid,
) -> Result<Option<RequestFrame>, OfferReadError> {
    let stored = requests::request(pool, request_id)
        .await
        .map_err(|_| OfferReadError::StorageFailed)?;
    let stored = match stored {
        Some(stored) => stored,
        None => return Ok(None),
    };
    let deadline = if stored.current_cycle_number >= 1 {
        requests::cycle(pool, request_id, stored.current_cycle_number)
            .await
            .map_err(|_| OfferReadError::StorageFailed)?
            .map(|cycle| cycle.deadline)
    } else {
        None
    };
    Ok(Some(RequestFrame {
        author_id: stored.author_id,
        state: stored.state,
        current_cycle_number: stored.current_cycle_number,
        current_revision_number: stored.current_revision_number,
        deadline,
    }))
}

/// All offers on one owned request, partitioned into the actionable set and
/// history, in the requested order within each group.
///
/// # Errors
///
/// Returns [`OfferReadError::NotActive`] for restricted callers,
/// [`OfferReadError::NotFound`] for missing or non-owned requests, else
/// [`OfferReadError::StorageFailed`].
pub async fn buyer_offers(
    pool: &sqlx::PgPool,
    author_id: uuid::Uuid,
    request_id: uuid::Uuid,
    sort: OfferSort,
) -> Result<BuyerOfferList, OfferReadError> {
    eligible_caller(pool, author_id).await?;
    let frame = request_frame(pool, request_id)
        .await?
        .filter(|frame| frame.author_id == author_id)
        .ok_or(OfferReadError::NotFound)?;
    let now = chrono::Utc::now();
    let mut rows = offers::offers_for_request(pool, request_id)
        .await
        .map_err(|_| OfferReadError::StorageFailed)?;
    sort_offers(&mut rows, sort, |row| {
        (row.price_cents, row.created_at, row.id)
    });
    let mut live = Vec::new();
    let mut history = Vec::new();
    for stored in rows {
        let view = buyer_view(
            pool,
            stored,
            frame.current_cycle_number,
            frame.current_revision_number,
            frame.deadline,
            &frame.state,
            now,
        )
        .await?;
        if view.live {
            live.push(view);
        } else {
            history.push(view);
        }
    }
    let total = live.len() + history.len();
    Ok(BuyerOfferList {
        live,
        history,
        total,
    })
}

/// One seller's own offers newest-first across requests.
///
/// # Errors
///
/// Returns [`OfferReadError::NotActive`] for restricted callers, else
/// [`OfferReadError::StorageFailed`].
pub async fn seller_offers(
    pool: &sqlx::PgPool,
    seller_id: uuid::Uuid,
    sort: OfferSort,
) -> Result<Vec<SellerOfferView>, OfferReadError> {
    eligible_caller(pool, seller_id).await?;
    let now = chrono::Utc::now();
    let mut rows = offers::offers_for_seller(pool, seller_id)
        .await
        .map_err(|_| OfferReadError::StorageFailed)?;
    sort_offers(&mut rows, sort, |row| {
        (row.price_cents, row.created_at, row.id)
    });
    let mut views = Vec::with_capacity(rows.len());
    for stored in rows {
        let frame = request_frame(pool, stored.request_id)
            .await?
            .ok_or(OfferReadError::StorageFailed)?;
        views.push(seller_view(
            stored,
            frame.current_cycle_number,
            frame.current_revision_number,
            frame.deadline,
            &frame.state,
            now,
        ));
    }
    Ok(views)
}

/// One seller's own offer, if it belongs to them.
///
/// # Errors
///
/// Returns [`OfferReadError::NotActive`] for restricted callers,
/// [`OfferReadError::NotFound`] for missing or foreign rows, else
/// [`OfferReadError::StorageFailed`].
pub async fn seller_offer(
    pool: &sqlx::PgPool,
    seller_id: uuid::Uuid,
    offer_id: uuid::Uuid,
) -> Result<SellerOfferView, OfferReadError> {
    eligible_caller(pool, seller_id).await?;
    let stored = offers::offer(pool, offer_id)
        .await
        .map_err(|_| OfferReadError::StorageFailed)?;
    let stored = match stored {
        Some(stored) if stored.seller_id == seller_id => stored,
        _ => return Err(OfferReadError::NotFound),
    };
    let frame = request_frame(pool, stored.request_id)
        .await?
        .ok_or(OfferReadError::StorageFailed)?;
    Ok(seller_view(
        stored,
        frame.current_cycle_number,
        frame.current_revision_number,
        frame.deadline,
        &frame.state,
        chrono::Utc::now(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liveness_needs_state_cycle_time_and_request() {
        let deadline =
            chrono::DateTime::from_timestamp(1_791_000_000, 0).expect("fixture instant builds");
        let before = deadline - chrono::Duration::seconds(1);
        assert!(is_live("sent", 1, 1, Some(deadline), "active", before));
        assert!(is_live("viewed", 1, 1, Some(deadline), "active", before));
        // Terminal, stale-cycle, elapsed, and non-active rows never compare.
        for (state, cycle, current, end, request) in [
            ("withdrawn", 1, 1, Some(deadline), "active"),
            ("rejected", 1, 1, Some(deadline), "active"),
            ("sent", 1, 2, Some(deadline), "active"),
            ("sent", 1, 1, Some(deadline), "completed"),
            ("sent", 1, 1, Some(deadline), "suspended"),
            ("sent", 1, 1, None, "active"),
        ] {
            assert!(!is_live(state, cycle, current, end, request, before));
        }
        assert!(!is_live("sent", 1, 1, Some(deadline), "active", deadline));
    }

    #[test]
    fn sorts_keep_ties_stable() {
        let epoch = chrono::DateTime::from_timestamp(0, 0).expect("epoch builds");
        let mut rows = vec![(52000_i64, "b"), (48000_i64, "a")];
        sort_offers(&mut rows, OfferSort::PriceAsc, |row| {
            (row.0, epoch, uuid::Uuid::nil())
        });
        assert_eq!(rows[0].1, "a");
        let mut rows = vec![(52000_i64, "b"), (48000_i64, "a")];
        sort_offers(&mut rows, OfferSort::PriceDesc, |row| {
            (row.0, epoch, uuid::Uuid::nil())
        });
        assert_eq!(rows[0].1, "b");
        // Equal prices keep submission order under every sort.
        let mut rows = vec![(52000_i64, "older"), (52000_i64, "newer")];
        sort_offers(&mut rows, OfferSort::PriceAsc, |row| {
            (row.0, epoch, uuid::Uuid::nil())
        });
        assert_eq!(rows[0].1, "older");
    }

    #[test]
    fn sort_parses_known_values_only() {
        assert_eq!(OfferSort::parse(None), Some(OfferSort::Newest));
        assert_eq!(OfferSort::parse(Some("newest")), Some(OfferSort::Newest));
        assert_eq!(
            OfferSort::parse(Some("price_asc")),
            Some(OfferSort::PriceAsc)
        );
        assert_eq!(
            OfferSort::parse(Some("price_desc")),
            Some(OfferSort::PriceDesc)
        );
        assert_eq!(OfferSort::parse(Some("cheapest")), None);
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            OfferReadError::NotActive,
            OfferReadError::NotFound,
            OfferReadError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
