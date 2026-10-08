//! Request cycle and revision persistence: ownership, requirements, history.
//!
//! Canonical rules: INV-01 (exactly one author per request — `author_id` has
//! no UPDATE path here, so ownership never transfers), INV-08 (an active
//! request needs complete valid fields, an allowed category, an enabled city,
//! and a future deadline — completeness is stored here, policy evaluation
//! belongs to the publication writers of later cards), INV-10 (positive BRL
//! minor units, at most two decimals on the wire, never floats), INV-12
//! (renewal starts a new cycle without rewriting `original_published_at` —
//! [`start_cycle`] sets it once via `COALESCE`), INV-14 (material changes
//! preserve the prior revision — revision rows are insert-only and current
//! requirements move only through [`update_current_requirements`]), INV-31
//! (no phone material anywhere near these tables — there is no phone,
//! ciphertext, lookup, address, reporter, token, or secret column, and the
//! public projection carries an allowlist only).
//!
//! Lifecycle state and visibility are independent columns (state-transitions
//! 6.1): business state (`draft`, `active`, ...) coexists with visibility
//! (`private`, `public`, `hidden`). Removal hides; it never erases history.
//! Numbers increase: revision and cycle writers refuse a number that does not
//! advance the current counter. Foreign keys with `ON DELETE RESTRICT` make
//! orphan cycles or revisions unrepresentable and keep catalog identity stable.

use serde::Serialize;

/// A new request draft: one author plus the complete requirement set.
///
/// Drafts carry complete fields so later publication validates policy at the
/// effective time; saving a draft consumes no allowance (a later card's job).
#[derive(Debug, Clone)]
pub struct NewRequest {
    /// Owning account; must already exist. Never changes afterwards.
    pub author_id: uuid::Uuid,
    /// Item title, 1..=120 scalar values.
    pub title: String,
    /// Stable catalog category code (must be seeded).
    pub category_code: String,
    /// Maximum budget in integer minor units (cents). Strictly positive.
    pub budget_cents: i64,
    /// Accepted condition: `new` | `used` | `either`.
    pub condition: String,
    /// Stable catalog city code (must be seeded).
    pub city_code: String,
    /// Region code within the city (composite identity with the city).
    pub region_code: String,
    /// Optional notes, 0..=500 scalar values.
    pub notes: String,
}

/// An immutable requirement snapshot shared by revisions and current edits.
#[derive(Debug, Clone)]
pub struct RequirementSnapshot {
    /// Item title, 1..=120 scalar values.
    pub title: String,
    /// Stable catalog category code.
    pub category_code: String,
    /// Maximum budget in integer minor units. Strictly positive.
    pub budget_cents: i64,
    /// Accepted condition: `new` | `used` | `either`.
    pub condition: String,
    /// Stable catalog city code.
    pub city_code: String,
    /// Region code within the city.
    pub region_code: String,
    /// Optional notes, 0..=500 scalar values.
    pub notes: String,
}

/// One persisted request: ownership, current requirements, lifecycle, timing.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Owning account. Immutable after creation.
    pub author_id: uuid::Uuid,
    /// Current item title.
    pub title: String,
    /// Current category code.
    pub category_code: String,
    /// Current maximum budget in minor units.
    pub budget_cents: i64,
    /// Current accepted condition.
    pub condition: String,
    /// Current city code.
    pub city_code: String,
    /// Current region code.
    pub region_code: String,
    /// Current notes.
    pub notes: String,
    /// Lifecycle state (`draft`, `active`, `completed`, `expired`, ...).
    pub state: String,
    /// Visibility (`private`, `public`, `hidden`), independent of state.
    pub visibility: String,
    /// First-publication instant. Set once by the first cycle, never rewritten.
    pub original_published_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Highest cycle number started. Zero while no cycle exists.
    pub current_cycle_number: i32,
    /// Highest revision number recorded. Zero while no revision exists.
    pub current_revision_number: i32,
    /// Recording instant (database clock).
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last current-requirement change (database clock).
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// One immutable requirement revision: what sellers responded to.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestRevision {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Owning request.
    pub request_id: uuid::Uuid,
    /// Monotonic revision number within the request.
    pub revision_number: i32,
    /// Snapshot title.
    pub title: String,
    /// Snapshot category code.
    pub category_code: String,
    /// Snapshot budget in minor units.
    pub budget_cents: i64,
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

/// One activation cycle: a bounded seven-day window owned by later writers.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestCycle {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Owning request.
    pub request_id: uuid::Uuid,
    /// Monotonic cycle number within the request.
    pub cycle_number: i32,
    /// Cycle start (effective instant of publication or renewal).
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Exclusive deadline: at or after it the request is expired for actions.
    pub deadline: chrono::DateTime<chrono::Utc>,
    /// Cycle end, once superseded or closed.
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Public request projection: allowlist only, no author or phone material.
///
/// The phone destination lives only in `users` (ciphertext plus keyed
/// lookup); it can never derive into this shape because no such field exists
/// on any request table or on this struct.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PublicRequest {
    /// Request identifier.
    pub id: uuid::Uuid,
    /// Current item title.
    pub title: String,
    /// Current category code.
    pub category_code: String,
    /// Current maximum budget in minor units.
    pub budget_cents: i64,
    /// Exact decimal rendering (`"520.00"`), never binary floating point.
    pub budget: String,
    /// Current accepted condition.
    pub condition: String,
    /// Current city code.
    pub city_code: String,
    /// Current region code.
    pub region_code: String,
    /// Current notes.
    pub notes: String,
    /// Lifecycle state.
    pub state: String,
    /// Current cycle number.
    pub current_cycle_number: i32,
    /// First-publication instant, once published.
    pub original_published_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Render integer minor units as an exact decimal string (`"520.00"`).
#[must_use]
pub fn format_budget(budget_cents: i64) -> String {
    format!("{}.{:02}", budget_cents / 100, budget_cents % 100)
}

/// Project the owner-held row into its public allowlist (drops `author_id`).
#[must_use]
pub fn to_public(request: &Request) -> PublicRequest {
    PublicRequest {
        id: request.id,
        title: request.title.clone(),
        category_code: request.category_code.clone(),
        budget_cents: request.budget_cents,
        budget: format_budget(request.budget_cents),
        condition: request.condition.clone(),
        city_code: request.city_code.clone(),
        region_code: request.region_code.clone(),
        notes: request.notes.clone(),
        state: request.state.clone(),
        current_cycle_number: request.current_cycle_number,
        original_published_at: request.original_published_at,
    }
}

/// Typed request-storage failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestError {
    /// A requirement, code, number, or timestamp is missing or out of bounds.
    InvalidField,
    /// No such request, author, category, city, or region exists.
    Unknown,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid request field"),
            Self::Unknown => f.write_str("unknown request reference"),
            Self::StorageFailed => f.write_str("request storage failed"),
        }
    }
}

impl std::error::Error for RequestError {}

fn check_title(value: &str) -> Result<(), RequestError> {
    let count = value.chars().count();
    if !(1..=120).contains(&count) {
        return Err(RequestError::InvalidField);
    }
    Ok(())
}

fn check_notes(value: &str) -> Result<(), RequestError> {
    if value.chars().count() > 500 {
        return Err(RequestError::InvalidField);
    }
    Ok(())
}

fn check_budget(cents: i64) -> Result<(), RequestError> {
    if cents <= 0 {
        return Err(RequestError::InvalidField);
    }
    Ok(())
}

fn check_condition(value: &str) -> Result<(), RequestError> {
    if !matches!(value, "new" | "used" | "either") {
        return Err(RequestError::InvalidField);
    }
    Ok(())
}

fn check_code(value: &str) -> Result<(), RequestError> {
    if value.trim().is_empty()
        || value.chars().count() > 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(RequestError::InvalidField);
    }
    Ok(())
}

fn check_snapshot(snapshot: &RequirementSnapshot) -> Result<(), RequestError> {
    check_title(&snapshot.title)?;
    check_code(&snapshot.category_code)?;
    check_budget(snapshot.budget_cents)?;
    check_condition(&snapshot.condition)?;
    check_code(&snapshot.city_code)?;
    check_code(&snapshot.region_code)?;
    check_notes(&snapshot.notes)?;
    Ok(())
}

async fn exists_one(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    sql: &str,
    bind_one: &str,
) -> Result<bool, RequestError> {
    let found: Option<i32> = sqlx::query_scalar(sql)
        .bind(bind_one)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| RequestError::StorageFailed)?;
    Ok(found.is_some())
}

async fn check_catalog_refs(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    snapshot: &RequirementSnapshot,
) -> Result<(), RequestError> {
    if !exists_one(
        tx,
        "SELECT 1 FROM catalog_categories WHERE code = $1",
        &snapshot.category_code,
    )
    .await?
    {
        return Err(RequestError::Unknown);
    }
    if !exists_one(
        tx,
        "SELECT 1 FROM catalog_cities WHERE code = $1",
        &snapshot.city_code,
    )
    .await?
    {
        return Err(RequestError::Unknown);
    }
    let region: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM catalog_regions WHERE city_code = $1 AND code = $2")
            .bind(&snapshot.city_code)
            .bind(&snapshot.region_code)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| RequestError::StorageFailed)?;
    if region.is_none() {
        return Err(RequestError::Unknown);
    }
    Ok(())
}

fn read_request(row: &sqlx::postgres::PgRow) -> Result<Request, RequestError> {
    use sqlx::Row;
    Ok(Request {
        id: row.try_get("id").map_err(|_| RequestError::StorageFailed)?,
        author_id: row
            .try_get("author_id")
            .map_err(|_| RequestError::StorageFailed)?,
        title: row
            .try_get("title")
            .map_err(|_| RequestError::StorageFailed)?,
        category_code: row
            .try_get("category_code")
            .map_err(|_| RequestError::StorageFailed)?,
        budget_cents: row
            .try_get("budget_cents")
            .map_err(|_| RequestError::StorageFailed)?,
        condition: row
            .try_get("condition")
            .map_err(|_| RequestError::StorageFailed)?,
        city_code: row
            .try_get("city_code")
            .map_err(|_| RequestError::StorageFailed)?,
        region_code: row
            .try_get("region_code")
            .map_err(|_| RequestError::StorageFailed)?,
        notes: row
            .try_get("notes")
            .map_err(|_| RequestError::StorageFailed)?,
        state: row
            .try_get("state")
            .map_err(|_| RequestError::StorageFailed)?,
        visibility: row
            .try_get("visibility")
            .map_err(|_| RequestError::StorageFailed)?,
        original_published_at: row
            .try_get("original_published_at")
            .map_err(|_| RequestError::StorageFailed)?,
        current_cycle_number: row
            .try_get("current_cycle_number")
            .map_err(|_| RequestError::StorageFailed)?,
        current_revision_number: row
            .try_get("current_revision_number")
            .map_err(|_| RequestError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| RequestError::StorageFailed)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|_| RequestError::StorageFailed)?,
    })
}

fn read_revision(row: &sqlx::postgres::PgRow) -> Result<RequestRevision, RequestError> {
    use sqlx::Row;
    Ok(RequestRevision {
        id: row.try_get("id").map_err(|_| RequestError::StorageFailed)?,
        request_id: row
            .try_get("request_id")
            .map_err(|_| RequestError::StorageFailed)?,
        revision_number: row
            .try_get("revision_number")
            .map_err(|_| RequestError::StorageFailed)?,
        title: row
            .try_get("title")
            .map_err(|_| RequestError::StorageFailed)?,
        category_code: row
            .try_get("category_code")
            .map_err(|_| RequestError::StorageFailed)?,
        budget_cents: row
            .try_get("budget_cents")
            .map_err(|_| RequestError::StorageFailed)?,
        condition: row
            .try_get("condition")
            .map_err(|_| RequestError::StorageFailed)?,
        city_code: row
            .try_get("city_code")
            .map_err(|_| RequestError::StorageFailed)?,
        region_code: row
            .try_get("region_code")
            .map_err(|_| RequestError::StorageFailed)?,
        notes: row
            .try_get("notes")
            .map_err(|_| RequestError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| RequestError::StorageFailed)?,
    })
}

fn read_cycle(row: &sqlx::postgres::PgRow) -> Result<RequestCycle, RequestError> {
    use sqlx::Row;
    Ok(RequestCycle {
        id: row.try_get("id").map_err(|_| RequestError::StorageFailed)?,
        request_id: row
            .try_get("request_id")
            .map_err(|_| RequestError::StorageFailed)?,
        cycle_number: row
            .try_get("cycle_number")
            .map_err(|_| RequestError::StorageFailed)?,
        started_at: row
            .try_get("started_at")
            .map_err(|_| RequestError::StorageFailed)?,
        deadline: row
            .try_get("deadline")
            .map_err(|_| RequestError::StorageFailed)?,
        ended_at: row
            .try_get("ended_at")
            .map_err(|_| RequestError::StorageFailed)?,
    })
}

const REQUEST_COLUMNS: &str = "id, author_id, title, category_code, budget_cents, \"condition\", city_code, region_code, notes, state, visibility, original_published_at, current_cycle_number, current_revision_number, created_at, updated_at";

const REVISION_COLUMNS: &str = "id, request_id, revision_number, title, category_code, budget_cents, \"condition\", city_code, region_code, notes, created_at";

const CYCLE_COLUMNS: &str = "id, request_id, cycle_number, started_at, deadline, ended_at";

/// Create one private draft request for an existing author.
///
/// The draft starts in `draft` / `private` with no cycle, no revision, and no
/// publication time. Allowances, duplicate checks, and publication live with
/// later writers; this writer only stores a coherent requirement set.
///
/// Runtime-checked SQL keeps `cargo build` offline-capable.
///
/// # Errors
///
/// Returns [`RequestError::InvalidField`] for malformed requirements,
/// [`RequestError::Unknown`] for a missing author/category/city/region, else
/// [`RequestError::StorageFailed`]. Reasons are static.
pub async fn create_request(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: NewRequest,
) -> Result<Request, RequestError> {
    let snapshot = RequirementSnapshot {
        title: input.title,
        category_code: input.category_code,
        budget_cents: input.budget_cents,
        condition: input.condition,
        city_code: input.city_code,
        region_code: input.region_code,
        notes: input.notes,
    };
    check_snapshot(&snapshot)?;
    let author: Option<i32> = sqlx::query_scalar("SELECT 1 FROM users WHERE id = $1")
        .bind(input.author_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| RequestError::StorageFailed)?;
    if author.is_none() {
        return Err(RequestError::Unknown);
    }
    check_catalog_refs(tx, &snapshot).await?;
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "INSERT INTO requests
            (author_id, title, category_code, budget_cents, \"condition\",
             city_code, region_code, notes)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING {REQUEST_COLUMNS}"
    ))
    .bind(input.author_id)
    .bind(&snapshot.title)
    .bind(&snapshot.category_code)
    .bind(snapshot.budget_cents)
    .bind(&snapshot.condition)
    .bind(&snapshot.city_code)
    .bind(&snapshot.region_code)
    .bind(&snapshot.notes)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| RequestError::StorageFailed)?;
    read_request(&row)
}

/// Record one immutable requirement revision without touching current state.
///
/// Later material-edit writers compose this with
/// [`update_current_requirements`]; kept separate so history writes stay
/// append-only and current promotion stays explicit.
///
/// # Errors
///
/// Returns [`RequestError::InvalidField`] for malformed snapshots or a
/// non-positive number, [`RequestError::Unknown`] for a missing request or
/// catalog reference, else [`RequestError::StorageFailed`].
pub async fn insert_revision(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request_id: uuid::Uuid,
    revision_number: i32,
    snapshot: &RequirementSnapshot,
) -> Result<RequestRevision, RequestError> {
    if revision_number < 1 {
        return Err(RequestError::InvalidField);
    }
    check_snapshot(snapshot)?;
    let parent: Option<i32> = sqlx::query_scalar("SELECT 1 FROM requests WHERE id = $1")
        .bind(request_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| RequestError::StorageFailed)?;
    if parent.is_none() {
        return Err(RequestError::Unknown);
    }
    check_catalog_refs(tx, snapshot).await?;
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "INSERT INTO request_revisions
            (request_id, revision_number, title, category_code, budget_cents,
             \"condition\", city_code, region_code, notes)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         RETURNING {REVISION_COLUMNS}"
    ))
    .bind(request_id)
    .bind(revision_number)
    .bind(&snapshot.title)
    .bind(&snapshot.category_code)
    .bind(snapshot.budget_cents)
    .bind(&snapshot.condition)
    .bind(&snapshot.city_code)
    .bind(&snapshot.region_code)
    .bind(&snapshot.notes)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| RequestError::StorageFailed)?;
    read_revision(&row)
}

/// Promote a requirement snapshot to current without rewriting history.
///
/// The revision row itself is untouched (reload it afterwards: it must read
/// back unchanged). The number must advance the stored counter, so revisions
/// stay monotonic. `author_id`, state, visibility, cycles, and the original
/// publication time are never modified here.
///
/// # Errors
///
/// Returns [`RequestError::InvalidField`] for malformed snapshots or a
/// non-advancing number, [`RequestError::Unknown`] for a missing request or
/// catalog reference, else [`RequestError::StorageFailed`].
pub async fn update_current_requirements(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request_id: uuid::Uuid,
    revision_number: i32,
    snapshot: &RequirementSnapshot,
) -> Result<Request, RequestError> {
    if revision_number < 1 {
        return Err(RequestError::InvalidField);
    }
    check_snapshot(snapshot)?;
    check_catalog_refs(tx, snapshot).await?;
    let current: Option<i32> =
        sqlx::query_scalar("SELECT current_revision_number FROM requests WHERE id = $1")
            .bind(request_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| RequestError::StorageFailed)?;
    let current = current.ok_or(RequestError::Unknown)?;
    if revision_number <= current {
        return Err(RequestError::InvalidField);
    }
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "UPDATE requests SET title = $2, category_code = $3, budget_cents = $4,
             \"condition\" = $5, city_code = $6, region_code = $7, notes = $8,
             current_revision_number = $9, updated_at = now()
         WHERE id = $1 RETURNING {REQUEST_COLUMNS}"
    ))
    .bind(request_id)
    .bind(&snapshot.title)
    .bind(&snapshot.category_code)
    .bind(snapshot.budget_cents)
    .bind(&snapshot.condition)
    .bind(&snapshot.city_code)
    .bind(&snapshot.region_code)
    .bind(&snapshot.notes)
    .bind(revision_number)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| RequestError::StorageFailed)?;
    read_request(&row)
}

/// Start one activation cycle with an exclusive deadline.
///
/// The number must advance the stored counter. The request's
/// `original_published_at` is set from this cycle's start only when still
/// unset (INV-12: renewal never rewrites first-publication history).
/// Ending previous cycles and expiring their offers belongs to later writers.
///
/// # Errors
///
/// Returns [`RequestError::InvalidField`] for a non-advancing number or a
/// deadline at/before the start, [`RequestError::Unknown`] for a missing
/// request, else [`RequestError::StorageFailed`].
pub async fn start_cycle(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request_id: uuid::Uuid,
    cycle_number: i32,
    started_at: chrono::DateTime<chrono::Utc>,
    deadline: chrono::DateTime<chrono::Utc>,
) -> Result<RequestCycle, RequestError> {
    if cycle_number < 1 || deadline <= started_at {
        return Err(RequestError::InvalidField);
    }
    let current: Option<i32> =
        sqlx::query_scalar("SELECT current_cycle_number FROM requests WHERE id = $1")
            .bind(request_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| RequestError::StorageFailed)?;
    let current = current.ok_or(RequestError::Unknown)?;
    if cycle_number <= current {
        return Err(RequestError::InvalidField);
    }
    let row: sqlx::postgres::PgRow = sqlx::query(&format!(
        "INSERT INTO request_cycles (request_id, cycle_number, started_at, deadline)
         VALUES ($1, $2, $3, $4) RETURNING {CYCLE_COLUMNS}"
    ))
    .bind(request_id)
    .bind(cycle_number)
    .bind(started_at)
    .bind(deadline)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| RequestError::StorageFailed)?;
    sqlx::query(
        "UPDATE requests SET current_cycle_number = $2,
             original_published_at = COALESCE(original_published_at, $3),
             updated_at = now() WHERE id = $1",
    )
    .bind(request_id)
    .bind(cycle_number)
    .bind(started_at)
    .execute(&mut **tx)
    .await
    .map_err(|_| RequestError::StorageFailed)?;
    read_cycle(&row)
}

/// One request by id, if it exists.
///
/// # Errors
///
/// Returns [`RequestError::StorageFailed`] on database failure only.
pub async fn request<'e, E>(executor: E, id: uuid::Uuid) -> Result<Option<Request>, RequestError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {REQUEST_COLUMNS} FROM requests WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(executor)
    .await
    .map_err(|_| RequestError::StorageFailed)?;
    row.map(|row| read_request(&row)).transpose()
}

/// One owner's requests in creation order (private owner read).
///
/// # Errors
///
/// Returns [`RequestError::StorageFailed`] on database failure only.
pub async fn requests_for_author<'e, E>(
    executor: E,
    author_id: uuid::Uuid,
) -> Result<Vec<Request>, RequestError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {REQUEST_COLUMNS} FROM requests WHERE author_id = $1 ORDER BY created_at, id"
    ))
    .bind(author_id)
    .fetch_all(executor)
    .await
    .map_err(|_| RequestError::StorageFailed)?;
    rows.iter().map(read_request).collect()
}

/// One revision by its monotonic number, if it exists.
///
/// # Errors
///
/// Returns [`RequestError::StorageFailed`] on database failure only.
pub async fn revision<'e, E>(
    executor: E,
    request_id: uuid::Uuid,
    revision_number: i32,
) -> Result<Option<RequestRevision>, RequestError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {REVISION_COLUMNS} FROM request_revisions
         WHERE request_id = $1 AND revision_number = $2"
    ))
    .bind(request_id)
    .bind(revision_number)
    .fetch_optional(executor)
    .await
    .map_err(|_| RequestError::StorageFailed)?;
    row.map(|row| read_revision(&row)).transpose()
}

/// All revisions of one request in number order (oldest first).
///
/// # Errors
///
/// Returns [`RequestError::StorageFailed`] on database failure only.
pub async fn revisions_for_request<'e, E>(
    executor: E,
    request_id: uuid::Uuid,
) -> Result<Vec<RequestRevision>, RequestError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {REVISION_COLUMNS} FROM request_revisions
         WHERE request_id = $1 ORDER BY revision_number"
    ))
    .bind(request_id)
    .fetch_all(executor)
    .await
    .map_err(|_| RequestError::StorageFailed)?;
    rows.iter().map(read_revision).collect()
}

/// One cycle by its monotonic number, if it exists.
///
/// # Errors
///
/// Returns [`RequestError::StorageFailed`] on database failure only.
pub async fn cycle<'e, E>(
    executor: E,
    request_id: uuid::Uuid,
    cycle_number: i32,
) -> Result<Option<RequestCycle>, RequestError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {CYCLE_COLUMNS} FROM request_cycles
         WHERE request_id = $1 AND cycle_number = $2"
    ))
    .bind(request_id)
    .bind(cycle_number)
    .fetch_optional(executor)
    .await
    .map_err(|_| RequestError::StorageFailed)?;
    row.map(|row| read_cycle(&row)).transpose()
}

/// All cycles of one request in number order (oldest first).
///
/// # Errors
///
/// Returns [`RequestError::StorageFailed`] on database failure only.
pub async fn cycles_for_request<'e, E>(
    executor: E,
    request_id: uuid::Uuid,
) -> Result<Vec<RequestCycle>, RequestError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(&format!(
        "SELECT {CYCLE_COLUMNS} FROM request_cycles
         WHERE request_id = $1 ORDER BY cycle_number"
    ))
    .bind(request_id)
    .fetch_all(executor)
    .await
    .map_err(|_| RequestError::StorageFailed)?;
    rows.iter().map(read_cycle).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> RequirementSnapshot {
        RequirementSnapshot {
            title: "Refrigerator".to_owned(),
            category_code: "home_appliances".to_owned(),
            budget_cents: 52_000,
            condition: "either".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            notes: "Preferably frost-free.".to_owned(),
        }
    }

    #[test]
    fn budgets_render_exactly_without_floats() {
        assert_eq!(format_budget(52_000), "520.00");
        assert_eq!(format_budget(1), "0.01");
        assert_eq!(format_budget(105), "1.05");
    }

    #[test]
    fn validation_refuses_bad_requirements() {
        let mut bad = snapshot();
        bad.budget_cents = 0;
        assert!(check_snapshot(&bad).is_err());
        let mut bad = snapshot();
        bad.budget_cents = -100;
        assert!(check_snapshot(&bad).is_err());
        let mut bad = snapshot();
        bad.condition = "refurbished".to_owned();
        assert!(check_snapshot(&bad).is_err());
        let mut bad = snapshot();
        bad.title = String::new();
        assert!(check_snapshot(&bad).is_err());
        let mut bad = snapshot();
        bad.title = "x".repeat(121);
        assert!(check_snapshot(&bad).is_err());
        let mut bad = snapshot();
        bad.notes = "x".repeat(501);
        assert!(check_snapshot(&bad).is_err());
        let mut bad = snapshot();
        bad.category_code = "Motor Vehicles".to_owned();
        assert!(check_snapshot(&bad).is_err());
        assert!(check_snapshot(&snapshot()).is_ok());
    }

    #[test]
    fn public_projection_drops_author_and_has_no_phone_shape() {
        let request = Request {
            id: uuid::Uuid::now_v7(),
            author_id: uuid::Uuid::now_v7(),
            title: "Refrigerator".to_owned(),
            category_code: "home_appliances".to_owned(),
            budget_cents: 52_000,
            condition: "either".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            notes: "Preferably frost-free.".to_owned(),
            state: "draft".to_owned(),
            visibility: "private".to_owned(),
            original_published_at: None,
            current_cycle_number: 0,
            current_revision_number: 0,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let public = to_public(&request);
        assert_eq!(public.id, request.id);
        assert_eq!(public.budget, "520.00");
        let rendered = serde_json::to_string(&public).expect("public serializes");
        for absent in [
            "author",
            "phone",
            "ciphertext",
            "lookup",
            "destination",
            "address",
            "token",
            "session",
            "reporter",
            "secret",
            "visibility",
        ] {
            assert!(!rendered.contains(absent), "no {absent} in public output");
        }
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            RequestError::InvalidField,
            RequestError::Unknown,
            RequestError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
