//! Eligible local discovery reads: only live public demand, ranked.
//!
//! Canonical rules: discovery 9.1 (active, visible, unexpired, allowed
//! requests in the selected city; inclusive positive filter bounds;
//! preferred region first, then original publication newest-first with a
//! stable identifier tie-break; renewal and edits never boost rank; no
//! silent national fallback), INV-08 with INV-09 (only actionable demand
//! lists — terminal, removed, suspended, and restricted rows stay out),
//! INV-31 (projections carry business fields and declared display names
//! only — no phone, notes, reporter, token, or secret material exists
//! here), INV-35 (pair-blocked authors stay out of each other's listing),
//! and EC-35 (category codes keep identity; standing resolves live).
//!
//! City scoping is mandatory and exact: region codes qualify inside their
//! city only, so same-named regions elsewhere never leak in. Cursor pages
//! are keyset-addressed on (region rank, publication time, identifier):
//! bounded, deterministic, and free of silent fallbacks.

use serde::{Deserialize, Serialize};

/// Page bound (mirrors sibling bounded lists).
pub const DISCOVERY_LIMIT_MAX: i64 = 50;
/// Default page size.
pub const DISCOVERY_LIMIT_DEFAULT: i64 = 20;

/// One listed demand: business fields with the declared buyer name. No
/// phone, notes, reporter, token, or secret field exists here.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiscoveredRequest {
    /// Listed demand identifier.
    pub id: uuid::Uuid,
    /// Listed title.
    pub title: String,
    /// Current category code.
    pub category_code: String,
    /// Current budget in cents (bounds apply inclusively upstream).
    pub budget_cents: i64,
    /// Current condition.
    pub condition: String,
    /// Listed city code.
    pub city_code: String,
    /// Listed region code.
    pub region_code: String,
    /// Declared buyer display name.
    pub author_name: String,
    /// Original first-publication instant (rank key, never rewritten by
    /// renewal or edits).
    pub original_published_at: chrono::DateTime<chrono::Utc>,
}

/// Keyset position: region rank, publication time, identifier. Callers
/// treat the encoded token opaquely and never construct it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CursorPosition {
    rank: i32,
    published_at: chrono::DateTime<chrono::Utc>,
    id: uuid::Uuid,
}

/// Encode a keyset position as one dot-separated token.
fn encode_cursor(position: &CursorPosition) -> String {
    format!(
        "{}.{}.{}",
        position.rank,
        position.published_at.timestamp_micros(),
        position.id.as_simple()
    )
}

/// Decode a caller-supplied cursor with strict shape: exactly three
/// dot-separated parts holding a 0/1 rank, integer micros, and a simple
/// identifier.
fn decode_cursor(raw: &str) -> Option<CursorPosition> {
    let mut parts = raw.split('.');
    let (rank, micros, id) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let rank: i32 = rank.parse().ok()?;
    if rank != 0 && rank != 1 {
        return None;
    }
    let micros: i64 = micros.parse().ok()?;
    let published_at = chrono::DateTime::from_timestamp_micros(micros)?;
    let id = uuid::Uuid::parse_str(id).ok()?;
    Some(CursorPosition {
        rank,
        published_at,
        id,
    })
}

/// One discovery read: the viewer scope, the mandatory city, explicit
/// filters, and bounded keyset paging.
#[derive(Debug, Clone)]
pub struct DiscoveryQuery<'a> {
    /// Block-exclusion scope, if signed in.
    pub viewer_id: Option<uuid::Uuid>,
    /// Selected city scope (required, exact).
    pub city_code: &'a str,
    /// Category code filter, if narrowing.
    pub category_code: Option<&'a str>,
    /// Region code filter within the city, if narrowing.
    pub region_code: Option<&'a str>,
    /// Region preference for ranking without filtering, if any.
    pub preferred_region: Option<&'a str>,
    /// Inclusive minimum budget in cents, if bounding.
    pub min_budget_cents: Option<i64>,
    /// Inclusive maximum budget in cents, if bounding.
    pub max_budget_cents: Option<i64>,
    /// Condition filter, if narrowing.
    pub condition: Option<&'a str>,
    /// Page size (already bounded upstream).
    pub limit: i64,
    /// Keyset position from a previous page, if continuing.
    pub cursor: Option<&'a str>,
}

/// Typed discovery failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryError {
    /// Missing/unknown city, region, or category, inverted or
    /// non-positive bounds, unknown condition, bad paging or cursor.
    InvalidField,
    /// The lookup failed.
    StorageFailed,
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid discovery field"),
            Self::StorageFailed => f.write_str("discovery storage failed"),
        }
    }
}

impl std::error::Error for DiscoveryError {}

fn read_row(row: &sqlx::postgres::PgRow) -> Result<(DiscoveredRequest, i32), DiscoveryError> {
    use sqlx::Row;
    let item = DiscoveredRequest {
        id: row
            .try_get("id")
            .map_err(|_| DiscoveryError::StorageFailed)?,
        title: row
            .try_get("title")
            .map_err(|_| DiscoveryError::StorageFailed)?,
        category_code: row
            .try_get("category_code")
            .map_err(|_| DiscoveryError::StorageFailed)?,
        budget_cents: row
            .try_get("budget_cents")
            .map_err(|_| DiscoveryError::StorageFailed)?,
        condition: row
            .try_get("condition")
            .map_err(|_| DiscoveryError::StorageFailed)?,
        city_code: row
            .try_get("city_code")
            .map_err(|_| DiscoveryError::StorageFailed)?,
        region_code: row
            .try_get("region_code")
            .map_err(|_| DiscoveryError::StorageFailed)?,
        author_name: row
            .try_get("author_name")
            .map_err(|_| DiscoveryError::StorageFailed)?,
        original_published_at: row
            .try_get("original_published_at")
            .map_err(|_| DiscoveryError::StorageFailed)?,
    };
    let rank: i32 = row
        .try_get("region_rank")
        .map_err(|_| DiscoveryError::StorageFailed)?;
    Ok((item, rank))
}

/// List eligible demand in one city with explicit filters and keyset
/// paging: active public rows with a future deadline and an allowed
/// category, minus pair-blocked authors. `viewer_id` scopes the block
/// exclusion; `None` lists anonymously with no block filtering (kept for
/// the public cards). Preferred region ranks first without filtering;
/// inside each band, original publication runs newest-first with the
/// identifier breaking ties.
///
/// Runtime-checked SQL keeps `cargo build` offline-capable.
///
/// # Errors
///
/// Returns [`DiscoveryError::StorageFailed`] on database failure only;
/// filter shape is the application layer's job. Reasons are static.
pub async fn discover<'e, E>(
    executor: E,
    query: DiscoveryQuery<'_>,
) -> Result<(Vec<DiscoveredRequest>, Option<String>), DiscoveryError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let position = match query.cursor {
        Some(raw) => Some(decode_cursor(raw).ok_or(DiscoveryError::InvalidField)?),
        None => None,
    };
    let (has_cursor, cursor_rank, cursor_published, cursor_id) = match &position {
        Some(position) => (
            true,
            position.rank,
            Some(position.published_at),
            Some(position.id),
        ),
        None => (false, 0, None, None),
    };
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(
        "SELECT requests.id, requests.title, requests.category_code,
                requests.budget_cents, requests.condition, requests.city_code,
                requests.region_code, users.display_name AS author_name,
                requests.original_published_at,
                CASE WHEN $7::text IS NOT NULL
                      AND requests.region_code = $7 THEN 1 ELSE 0 END AS region_rank
         FROM requests
         JOIN users ON users.id = requests.author_id
         JOIN request_cycles AS cycle
           ON cycle.request_id = requests.id
          AND cycle.cycle_number = requests.current_cycle_number
         JOIN catalog_categories AS category
           ON category.code = requests.category_code
         WHERE requests.state = 'active'
           AND requests.visibility = 'public'
           AND requests.city_code = $1
           AND ($2::text IS NULL OR requests.category_code = $2)
           AND ($3::text IS NULL OR requests.region_code = $3)
           AND ($4::bigint IS NULL OR requests.budget_cents >= $4)
           AND ($5::bigint IS NULL OR requests.budget_cents <= $5)
           AND ($6::text IS NULL OR requests.condition = $6)
           AND cycle.deadline > now()
           AND category.status = 'allowed'
           AND ($8::uuid IS NULL
                OR NOT EXISTS (SELECT 1 FROM user_blocks
                               WHERE (blocker_id = $8 AND blocked_id = requests.author_id)
                                  OR (blocker_id = requests.author_id AND blocked_id = $8)))
           AND ((NOT $9::boolean)
                OR (CASE WHEN $7::text IS NOT NULL
                          AND requests.region_code = $7 THEN 1 ELSE 0 END < $10
                    OR (CASE WHEN $7::text IS NOT NULL
                              AND requests.region_code = $7 THEN 1 ELSE 0 END = $10
                        AND (requests.original_published_at < $11
                             OR (requests.original_published_at = $11
                                 AND requests.id > $12)))))
         ORDER BY region_rank DESC, requests.original_published_at DESC, requests.id ASC
         LIMIT $13",
    )
    .bind(query.city_code)
    .bind(query.category_code)
    .bind(query.region_code)
    .bind(query.min_budget_cents)
    .bind(query.max_budget_cents)
    .bind(query.condition)
    .bind(query.preferred_region)
    .bind(query.viewer_id)
    .bind(has_cursor)
    .bind(cursor_rank)
    .bind(cursor_published)
    .bind(cursor_id)
    .bind(query.limit + 1)
    .fetch_all(executor)
    .await
    .map_err(|_| DiscoveryError::StorageFailed)?;
    let mut items = Vec::new();
    let mut positions = Vec::new();
    for row in rows {
        let (item, rank) = read_row(&row)?;
        positions.push(CursorPosition {
            rank,
            published_at: item.original_published_at,
            id: item.id,
        });
        items.push(item);
    }
    let next = if items.len() as i64 > query.limit {
        items.truncate(query.limit as usize);
        positions.truncate(query.limit as usize);
        positions.last().map(encode_cursor)
    } else {
        None
    };
    Ok((items, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trip_is_strict() {
        let position = CursorPosition {
            rank: 1,
            published_at: chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z")
                .expect("fixed time parses")
                .with_timezone(&chrono::Utc),
            id: uuid::Uuid::now_v7(),
        };
        let encoded = encode_cursor(&position);
        assert_eq!(encoded.matches('.').count(), 2);
        assert_eq!(decode_cursor(&encoded), Some(position));
        assert_eq!(decode_cursor("not-a-cursor"), None);
        assert_eq!(decode_cursor(""), None);
        assert_eq!(
            decode_cursor("2.1767319845000000.0123456789abcdef0123456789abcdef"),
            None
        );
        assert_eq!(
            decode_cursor("1.not-a-number.0123456789abcdef0123456789abcdef"),
            None
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [DiscoveryError::InvalidField, DiscoveryError::StorageFailed] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
