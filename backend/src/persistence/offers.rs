//! Offer slots and immutable terms persistence: seller, terms, history.
//!
//! Canonical rules: INV-02 (every offer names one seller plus one existing
//! request, cycle, and revision — composite foreign keys make orphans
//! unrepresentable), INV-03 (a user never offers on their own request —
//! checked here against the request author, since a CHECK cannot compare
//! across tables), INV-18 (one slot per seller and cycle, never freed by
//! withdrawal or rejection — the UNIQUE key plus no-delete history makes a
//! second slot unrepresentable), INV-19 (positive minor units; the
//! within-budget rule is submission policy for the offer writers, not this
//! schema), INV-24 (terms changes append history — terms rows are
//! insert-only and current promotion never rewrites them).
//!
//! Lifecycle state and visibility are independent columns
//! (state-transitions 6.2): new offers start `sent` / `visible`, and later
//! writers move engagement and terminal states without touching terms.

use serde::Serialize;

/// One offer submission: seller, target, and opening terms.
#[derive(Debug, Clone)]
pub struct NewOffer {
    /// Target request; must already exist.
    pub request_id: uuid::Uuid,
    /// Target cycle number; must already exist on the request.
    pub cycle_number: i32,
    /// Target requirement revision; must already exist on the request.
    pub revision_number: i32,
    /// Offering account; must already exist and differ from the author.
    pub seller_id: uuid::Uuid,
    /// Short item/model description, 1..=120 scalar values.
    pub description: String,
    /// Item price in integer minor units (cents). Strictly positive.
    pub price_cents: i64,
    /// Item condition: `new` | `used`.
    pub condition: String,
    /// Seller locality snapshot: city code at submission.
    pub city_code: String,
    /// Seller locality snapshot: region code at submission.
    pub region_code: String,
    /// Optional notes, 0..=500 scalar values.
    pub notes: String,
}

/// An immutable terms snapshot shared by submissions and term updates.
#[derive(Debug, Clone)]
pub struct TermsSnapshot {
    /// Item description, 1..=120 scalar values.
    pub description: String,
    /// Price in minor units. Strictly positive.
    pub price_cents: i64,
    /// Condition: `new` | `used`.
    pub condition: String,
    /// Locality snapshot city code.
    pub city_code: String,
    /// Locality snapshot region code.
    pub region_code: String,
    /// Optional notes, 0..=500 scalar values.
    pub notes: String,
}

/// One persisted offer: slot, current terms, lifecycle, timing.
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Target request.
    pub request_id: uuid::Uuid,
    /// Target cycle number.
    pub cycle_number: i32,
    /// Target requirement revision.
    pub revision_number: i32,
    /// Offering account. Immutable after creation.
    pub seller_id: uuid::Uuid,
    /// Current item description.
    pub description: String,
    /// Current price in minor units.
    pub price_cents: i64,
    /// Current condition.
    pub condition: String,
    /// Locality snapshot city code.
    pub city_code: String,
    /// Locality snapshot region code.
    pub region_code: String,
    /// Current notes.
    pub notes: String,
    /// Lifecycle state (`sent`, `viewed`, ...).
    pub state: String,
    /// Visibility (`visible`, `hidden`), independent of state.
    pub visibility: String,
    /// Terminal reason, once terminal.
    pub terminal_reason: Option<String>,
    /// Highest terms number recorded. One after submission.
    pub current_terms_number: i32,
    /// Recording instant (database clock).
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last current-terms change (database clock).
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// One immutable offer-terms revision: what the buyer responded to.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OfferTerms {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Owning offer.
    pub offer_id: uuid::Uuid,
    /// Monotonic terms number within the offer.
    pub terms_number: i32,
    /// Snapshot description.
    pub description: String,
    /// Snapshot price in minor units.
    pub price_cents: i64,
    /// Snapshot condition.
    pub condition: String,
    /// Snapshot city code.
    pub city_code: String,
    /// Snapshot region code.
    pub region_code: String,
    /// Snapshot notes.
    pub notes: String,
    /// Recording instant (database clock).
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Typed offer-storage failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferError {
    /// A term, code, number, or reference is missing or out of bounds.
    InvalidField,
    /// The seller offers on their own request.
    SelfOffer,
    /// No such seller, request, cycle, or revision exists.
    Unknown,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for OfferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid offer field"),
            Self::SelfOffer => f.write_str("seller owns the request"),
            Self::Unknown => f.write_str("unknown offer reference"),
            Self::StorageFailed => f.write_str("offer storage failed"),
        }
    }
}

impl std::error::Error for OfferError {}

fn check_description(value: &str) -> Result<(), OfferError> {
    let count = value.chars().count();
    if !(1..=120).contains(&count) {
        return Err(OfferError::InvalidField);
    }
    Ok(())
}

fn check_notes(value: &str) -> Result<(), OfferError> {
    if value.chars().count() > 500 {
        return Err(OfferError::InvalidField);
    }
    Ok(())
}

fn check_price(cents: i64) -> Result<(), OfferError> {
    if cents <= 0 {
        return Err(OfferError::InvalidField);
    }
    Ok(())
}

fn check_condition(value: &str) -> Result<(), OfferError> {
    if !matches!(value, "new" | "used") {
        return Err(OfferError::InvalidField);
    }
    Ok(())
}

fn check_code(value: &str) -> Result<(), OfferError> {
    if value.trim().is_empty()
        || value.chars().count() > 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(OfferError::InvalidField);
    }
    Ok(())
}

fn check_snapshot(snapshot: &TermsSnapshot) -> Result<(), OfferError> {
    check_description(&snapshot.description)?;
    check_price(snapshot.price_cents)?;
    check_condition(&snapshot.condition)?;
    check_code(&snapshot.city_code)?;
    check_code(&snapshot.region_code)?;
    check_notes(&snapshot.notes)?;
    Ok(())
}

fn read_offer(row: &sqlx::postgres::PgRow) -> Result<Offer, OfferError> {
    use sqlx::Row;
    Ok(Offer {
        id: row.try_get("id").map_err(|_| OfferError::StorageFailed)?,
        request_id: row
            .try_get("request_id")
            .map_err(|_| OfferError::StorageFailed)?,
        cycle_number: row
            .try_get("cycle_number")
            .map_err(|_| OfferError::StorageFailed)?,
        revision_number: row
            .try_get("revision_number")
            .map_err(|_| OfferError::StorageFailed)?,
        seller_id: row
            .try_get("seller_id")
            .map_err(|_| OfferError::StorageFailed)?,
        description: row
            .try_get("description")
            .map_err(|_| OfferError::StorageFailed)?,
        price_cents: row
            .try_get("price_cents")
            .map_err(|_| OfferError::StorageFailed)?,
        condition: row
            .try_get("condition")
            .map_err(|_| OfferError::StorageFailed)?,
        city_code: row
            .try_get("city_code")
            .map_err(|_| OfferError::StorageFailed)?,
        region_code: row
            .try_get("region_code")
            .map_err(|_| OfferError::StorageFailed)?,
        notes: row
            .try_get("notes")
            .map_err(|_| OfferError::StorageFailed)?,
        state: row
            .try_get("state")
            .map_err(|_| OfferError::StorageFailed)?,
        visibility: row
            .try_get("visibility")
            .map_err(|_| OfferError::StorageFailed)?,
        terminal_reason: row
            .try_get("terminal_reason")
            .map_err(|_| OfferError::StorageFailed)?,
        current_terms_number: row
            .try_get("current_terms_number")
            .map_err(|_| OfferError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| OfferError::StorageFailed)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|_| OfferError::StorageFailed)?,
    })
}

fn read_terms(row: &sqlx::postgres::PgRow) -> Result<OfferTerms, OfferError> {
    use sqlx::Row;
    Ok(OfferTerms {
        id: row.try_get("id").map_err(|_| OfferError::StorageFailed)?,
        offer_id: row
            .try_get("offer_id")
            .map_err(|_| OfferError::StorageFailed)?,
        terms_number: row
            .try_get("terms_number")
            .map_err(|_| OfferError::StorageFailed)?,
        description: row
            .try_get("description")
            .map_err(|_| OfferError::StorageFailed)?,
        price_cents: row
            .try_get("price_cents")
            .map_err(|_| OfferError::StorageFailed)?,
        condition: row
            .try_get("condition")
            .map_err(|_| OfferError::StorageFailed)?,
        city_code: row
            .try_get("city_code")
            .map_err(|_| OfferError::StorageFailed)?,
        region_code: row
            .try_get("region_code")
            .map_err(|_| OfferError::StorageFailed)?,
        notes: row
            .try_get("notes")
            .map_err(|_| OfferError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| OfferError::StorageFailed)?,
    })
}

const OFFER_COLUMNS: &str = "id, request_id, cycle_number, revision_number, seller_id, description, price_cents, \"condition\", city_code, region_code, notes, state, visibility, terminal_reason, current_terms_number, created_at, updated_at";

const TERMS_COLUMNS: &str = "id, offer_id, terms_number, description, price_cents, \"condition\", city_code, region_code, notes, created_at";

/// Submit one offer into its seller/cycle slot with its first terms.
///
/// The seller must exist and differ from the request author, and the
/// request, cycle, and revision must already exist; the slot must be free.
/// Remaining submission policy (budget fit, condition and locality match,
/// eligibility) belongs to the offer writers.
///
/// Runtime-checked SQL keeps `cargo build` offline-capable.
///
/// # Errors
///
/// Returns [`OfferError::InvalidField`] for malformed terms,
/// [`OfferError::SelfOffer`] when the seller owns the request,
/// [`OfferError::Unknown`] for a missing seller/request/cycle/revision,
/// else [`OfferError::StorageFailed`]. Reasons are static.
pub async fn create_offer(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: NewOffer,
) -> Result<Offer, OfferError> {
    let snapshot = TermsSnapshot {
        description: input.description,
        price_cents: input.price_cents,
        condition: input.condition,
        city_code: input.city_code,
        region_code: input.region_code,
        notes: input.notes,
    };
    check_snapshot(&snapshot)?;
    if input.cycle_number < 1 || input.revision_number < 1 {
        return Err(OfferError::InvalidField);
    }
    let seller: Option<i32> = sqlx::query_scalar("SELECT 1 FROM users WHERE id = $1")
        .bind(input.seller_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| OfferError::StorageFailed)?;
    if seller.is_none() {
        return Err(OfferError::Unknown);
    }
    let author: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT author_id FROM requests WHERE id = $1")
            .bind(input.request_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| OfferError::StorageFailed)?;
    let author = author.ok_or(OfferError::Unknown)?;
    if author == input.seller_id {
        return Err(OfferError::SelfOffer);
    }
    let cycle: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM request_cycles WHERE request_id = $1 AND cycle_number = $2",
    )
    .bind(input.request_id)
    .bind(input.cycle_number)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| OfferError::StorageFailed)?;
    if cycle.is_none() {
        return Err(OfferError::Unknown);
    }
    let revision: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM request_revisions WHERE request_id = $1 AND revision_number = $2",
    )
    .bind(input.request_id)
    .bind(input.revision_number)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| OfferError::StorageFailed)?;
    if revision.is_none() {
        return Err(OfferError::Unknown);
    }
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "INSERT INTO offers
            (request_id, cycle_number, revision_number, seller_id, description,
             price_cents, \"condition\", city_code, region_code, notes,
             current_terms_number)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 1)
         RETURNING {OFFER_COLUMNS}"
    ))
    .bind(input.request_id)
    .bind(input.cycle_number)
    .bind(input.revision_number)
    .bind(input.seller_id)
    .bind(&snapshot.description)
    .bind(snapshot.price_cents)
    .bind(&snapshot.condition)
    .bind(&snapshot.city_code)
    .bind(&snapshot.region_code)
    .bind(&snapshot.notes)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| OfferError::StorageFailed)?;
    let stored = read_offer(&row)?;
    let terms_row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "INSERT INTO offer_terms
            (offer_id, terms_number, description, price_cents, \"condition\",
             city_code, region_code, notes)
         VALUES ($1, 1, $2, $3, $4, $5, $6, $7)
         RETURNING {TERMS_COLUMNS}"
    ))
    .bind(stored.id)
    .bind(&snapshot.description)
    .bind(snapshot.price_cents)
    .bind(&snapshot.condition)
    .bind(&snapshot.city_code)
    .bind(&snapshot.region_code)
    .bind(&snapshot.notes)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| OfferError::StorageFailed)?;
    read_terms(&terms_row)?;
    Ok(stored)
}

/// Record one immutable offer-terms revision without touching current state.
///
/// Later term-edit writers compose this with [`update_current_terms`];
/// kept separate so history writes stay append-only and current promotion
/// stays explicit.
///
/// # Errors
///
/// Returns [`OfferError::InvalidField`] for malformed snapshots or a
/// non-positive number, [`OfferError::Unknown`] for a missing offer, else
/// [`OfferError::StorageFailed`].
pub async fn insert_terms(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    offer_id: uuid::Uuid,
    terms_number: i32,
    snapshot: &TermsSnapshot,
) -> Result<OfferTerms, OfferError> {
    if terms_number < 1 {
        return Err(OfferError::InvalidField);
    }
    check_snapshot(snapshot)?;
    let parent: Option<i32> = sqlx::query_scalar("SELECT 1 FROM offers WHERE id = $1")
        .bind(offer_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| OfferError::StorageFailed)?;
    if parent.is_none() {
        return Err(OfferError::Unknown);
    }
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "INSERT INTO offer_terms
            (offer_id, terms_number, description, price_cents, \"condition\",
             city_code, region_code, notes)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING {TERMS_COLUMNS}"
    ))
    .bind(offer_id)
    .bind(terms_number)
    .bind(&snapshot.description)
    .bind(snapshot.price_cents)
    .bind(&snapshot.condition)
    .bind(&snapshot.city_code)
    .bind(&snapshot.region_code)
    .bind(&snapshot.notes)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| OfferError::StorageFailed)?;
    read_terms(&row)
}

/// Promote a terms snapshot to current without rewriting history.
///
/// The terms row itself is untouched (reload it afterwards: it must read
/// back unchanged). The number must advance the stored counter, so terms
/// stay monotonic.
///
/// # Errors
///
/// Returns [`OfferError::InvalidField`] for malformed snapshots or a
/// non-advancing number, [`OfferError::Unknown`] for a missing offer, else
/// [`OfferError::StorageFailed`].
pub async fn update_current_terms(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    offer_id: uuid::Uuid,
    terms_number: i32,
    snapshot: &TermsSnapshot,
) -> Result<Offer, OfferError> {
    if terms_number < 1 {
        return Err(OfferError::InvalidField);
    }
    check_snapshot(snapshot)?;
    let current: Option<i32> =
        sqlx::query_scalar("SELECT current_terms_number FROM offers WHERE id = $1")
            .bind(offer_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| OfferError::StorageFailed)?;
    let current = current.ok_or(OfferError::Unknown)?;
    if terms_number <= current {
        return Err(OfferError::InvalidField);
    }
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "UPDATE offers SET description = $2, price_cents = $3,
             \"condition\" = $4, city_code = $5, region_code = $6, notes = $7,
             current_terms_number = $8, updated_at = now()
         WHERE id = $1 RETURNING {OFFER_COLUMNS}"
    ))
    .bind(offer_id)
    .bind(&snapshot.description)
    .bind(snapshot.price_cents)
    .bind(&snapshot.condition)
    .bind(&snapshot.city_code)
    .bind(&snapshot.region_code)
    .bind(&snapshot.notes)
    .bind(terms_number)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| OfferError::StorageFailed)?;
    read_offer(&row)
}

/// One offer by id, if it exists.
///
/// # Errors
///
/// Returns [`OfferError::StorageFailed`] on database failure only.
pub async fn offer<'e, E>(executor: E, id: uuid::Uuid) -> Result<Option<Offer>, OfferError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<sqlx::postgres::PgRow> =
        sqlx::query(&format!("SELECT {OFFER_COLUMNS} FROM offers WHERE id = $1"))
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(|_| OfferError::StorageFailed)?;
    row.map(|row| read_offer(&row)).transpose()
}

/// One request's offers in creation order.
///
/// # Errors
///
/// Returns [`OfferError::StorageFailed`] on database failure only.
pub async fn offers_for_request<'e, E>(
    executor: E,
    request_id: uuid::Uuid,
) -> Result<Vec<Offer>, OfferError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {OFFER_COLUMNS} FROM offers WHERE request_id = $1 ORDER BY created_at, id"
    ))
    .bind(request_id)
    .fetch_all(executor)
    .await
    .map_err(|_| OfferError::StorageFailed)?;
    rows.iter().map(read_offer).collect()
}

/// One seller's offers in creation order.
///
/// # Errors
///
/// Returns [`OfferError::StorageFailed`] on database failure only.
pub async fn offers_for_seller<'e, E>(
    executor: E,
    seller_id: uuid::Uuid,
) -> Result<Vec<Offer>, OfferError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {OFFER_COLUMNS} FROM offers WHERE seller_id = $1 ORDER BY created_at, id"
    ))
    .bind(seller_id)
    .fetch_all(executor)
    .await
    .map_err(|_| OfferError::StorageFailed)?;
    rows.iter().map(read_offer).collect()
}

/// One terms revision by its monotonic number, if it exists.
///
/// # Errors
///
/// Returns [`OfferError::StorageFailed`] on database failure only.
pub async fn terms<'e, E>(
    executor: E,
    offer_id: uuid::Uuid,
    terms_number: i32,
) -> Result<Option<OfferTerms>, OfferError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {TERMS_COLUMNS} FROM offer_terms
         WHERE offer_id = $1 AND terms_number = $2"
    ))
    .bind(offer_id)
    .bind(terms_number)
    .fetch_optional(executor)
    .await
    .map_err(|_| OfferError::StorageFailed)?;
    row.map(|row| read_terms(&row)).transpose()
}

/// All terms of one offer in number order (oldest first).
///
/// # Errors
///
/// Returns [`OfferError::StorageFailed`] on database failure only.
pub async fn terms_for_offer<'e, E>(
    executor: E,
    offer_id: uuid::Uuid,
) -> Result<Vec<OfferTerms>, OfferError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {TERMS_COLUMNS} FROM offer_terms
         WHERE offer_id = $1 ORDER BY terms_number"
    ))
    .bind(offer_id)
    .fetch_all(executor)
    .await
    .map_err(|_| OfferError::StorageFailed)?;
    rows.iter().map(read_terms).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::requests::format_budget;

    fn snapshot() -> TermsSnapshot {
        TermsSnapshot {
            description: "Frost-free 300L".to_owned(),
            price_cents: 45_000,
            condition: "used".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            notes: "Pickup only.".to_owned(),
        }
    }

    #[test]
    fn prices_render_exactly_without_floats() {
        assert_eq!(format_budget(45_000), "450.00");
        assert_eq!(format_budget(1), "0.01");
    }

    #[test]
    fn validation_refuses_bad_terms() {
        let mut bad = snapshot();
        bad.price_cents = 0;
        assert!(check_snapshot(&bad).is_err());
        let mut bad = snapshot();
        bad.price_cents = -100;
        assert!(check_snapshot(&bad).is_err());
        let mut bad = snapshot();
        bad.condition = "either".to_owned();
        assert!(check_snapshot(&bad).is_err());
        let mut bad = snapshot();
        bad.condition = "refurbished".to_owned();
        assert!(check_snapshot(&bad).is_err());
        let mut bad = snapshot();
        bad.description = String::new();
        assert!(check_snapshot(&bad).is_err());
        let mut bad = snapshot();
        bad.description = "x".repeat(121);
        assert!(check_snapshot(&bad).is_err());
        let mut bad = snapshot();
        bad.notes = "x".repeat(501);
        assert!(check_snapshot(&bad).is_err());
        let mut bad = snapshot();
        bad.city_code = "Campinas".to_owned();
        assert!(check_snapshot(&bad).is_err());
        assert!(check_snapshot(&snapshot()).is_ok());
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            OfferError::InvalidField,
            OfferError::SelfOffer,
            OfferError::Unknown,
            OfferError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
