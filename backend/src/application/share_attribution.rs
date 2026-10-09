//! Landing attribution and continuity context: visits without tracking.
//!
//! Canonical rules: sharing 13.4 (record share intent, observable
//! direct-link landing, and attributable registration or offer submission
//! separately with a seven-day acquisition window; missing or ambiguous
//! identity stays unknown rather than guessed; never collect recipient
//! lists, group names, delivery, or conversation contents), AC-32 with
//! EC-34 and EC-37 (returning visitors meet the live request with current
//! requirements rechecked at submission — no stale acceptance, no frozen
//! authority), INV-09 (closed demand takes no new offers, landing or
//! not), and INV-31 (identifiers and instants only — no visitor identity
//! is ever stored or linked server-side).
//!
//! Attribution resolves through a client-held landing marker: the newest
//! identifiable landing the visitor's own context carries. The server
//! never joins visits to accounts, so stranger visits can never
//! misattribute. A marker is attributable only when it names an existing
//! landing on an identifiable demand, visited no later than registration
//! and no earlier than seven days before it.

use serde::Serialize;

/// Acquisition window in days after a landing.
pub const ATTRIBUTION_WINDOW_DAYS: i64 = 7;

/// One recorded landing: an eligible source with its time, and nothing
/// else.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Landing {
    /// Server-generated version-7 identifier (the client-held marker).
    pub id: uuid::Uuid,
    /// Visited demand, if identifiable (`None` counts a direct visit).
    pub request_id: Option<uuid::Uuid>,
    /// Visit instant.
    pub landed_at: chrono::DateTime<chrono::Utc>,
}

/// Registration attribution: a known source or an honest unknown.
#[derive(Debug, Clone, PartialEq)]
pub enum Attribution {
    /// The marker names a fresh identifiable landing before registration.
    Attributed {
        /// Attributed demand.
        request_id: uuid::Uuid,
        /// Landing instant behind the attribution.
        landed_at: chrono::DateTime<chrono::Utc>,
    },
    /// Missing, direct, expired, or post-registration markers — and
    /// missing accounts — never guess a conversion.
    Unknown,
}

/// Typed attribution failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributionError {
    /// No such visited demand exists.
    NotFound,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for AttributionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("landing target not found"),
            Self::StorageFailed => f.write_str("attribution storage failed"),
        }
    }
}

impl std::error::Error for AttributionError {}

/// Pure attribution rule: a marker attributes only when it is
/// identifiable, visited no later than registration, and inside the
/// seven-day window before it.
fn attribute(
    landed_at: chrono::DateTime<chrono::Utc>,
    registered_at: chrono::DateTime<chrono::Utc>,
    request_id: Option<uuid::Uuid>,
) -> Attribution {
    match request_id {
        Some(request_id)
            if landed_at <= registered_at
                && registered_at <= landed_at + chrono::Duration::days(ATTRIBUTION_WINDOW_DAYS) =>
        {
            Attribution::Attributed {
                request_id,
                landed_at,
            }
        }
        _ => Attribution::Unknown,
    }
}

/// Record one landing: the visited demand when identifiable, or a direct
/// visit carrying nothing. Referenced demands must exist; landings never
/// expire by themselves.
///
/// # Errors
///
/// Returns [`AttributionError::NotFound`] for missing demands, else
/// [`AttributionError::StorageFailed`]. Reasons are static.
pub async fn record_landing(
    pool: &sqlx::PgPool,
    request_id: Option<uuid::Uuid>,
    landed_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<Landing, AttributionError> {
    if let Some(request_id) = request_id {
        let found: Option<i32> = sqlx::query_scalar("SELECT 1 FROM requests WHERE id = $1")
            .bind(request_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| AttributionError::StorageFailed)?;
        if found.is_none() {
            return Err(AttributionError::NotFound);
        }
    }
    let row: (
        uuid::Uuid,
        Option<uuid::Uuid>,
        chrono::DateTime<chrono::Utc>,
    ) = sqlx::query_as(
        "INSERT INTO share_landings (request_id, landed_at)
         VALUES ($1, COALESCE($2, now()))
         RETURNING id, request_id, landed_at",
    )
    .bind(request_id)
    .bind(landed_at)
    .fetch_one(pool)
    .await
    .map_err(|_| AttributionError::StorageFailed)?;
    Ok(Landing {
        id: row.0,
        request_id: row.1,
        landed_at: row.2,
    })
}

/// Attribute one registration through its client-held landing marker:
/// fresh identifiable landings before registration attribute; missing
/// markers, direct visits, expired windows, post-registration visits,
/// and missing accounts all answer unknown — never a guessed conversion.
///
/// # Errors
///
/// Returns [`AttributionError::StorageFailed`] on database failure only.
/// Reasons are static.
pub async fn attribute_registration(
    pool: &sqlx::PgPool,
    user_id: uuid::Uuid,
    landing_id: Option<uuid::Uuid>,
) -> Result<Attribution, AttributionError> {
    let landing_id = match landing_id {
        Some(landing_id) => landing_id,
        None => return Ok(Attribution::Unknown),
    };
    let landing: Option<(Option<uuid::Uuid>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as("SELECT request_id, landed_at FROM share_landings WHERE id = $1")
            .bind(landing_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| AttributionError::StorageFailed)?;
    let (request_id, landed_at) = match landing {
        Some(landing) => landing,
        None => return Ok(Attribution::Unknown),
    };
    let registered_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT created_at FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| AttributionError::StorageFailed)?;
    match registered_at {
        Some(registered_at) => Ok(attribute(landed_at, registered_at, request_id)),
        None => Ok(Attribution::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribution_window_is_exact() {
        let landed = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("fixed time parses")
            .with_timezone(&chrono::Utc);
        let target = uuid::Uuid::now_v7();
        assert!(matches!(
            attribute(landed, landed + chrono::Duration::days(6), Some(target)),
            Attribution::Attributed { .. }
        ));
        assert!(matches!(
            attribute(landed, landed + chrono::Duration::days(7), Some(target)),
            Attribution::Attributed { .. }
        ));
        assert_eq!(
            attribute(
                landed,
                landed + chrono::Duration::days(7) + chrono::Duration::seconds(1),
                Some(target)
            ),
            Attribution::Unknown
        );
        assert_eq!(
            attribute(landed, landed - chrono::Duration::seconds(1), Some(target)),
            Attribution::Unknown
        );
        assert_eq!(attribute(landed, landed, None), Attribution::Unknown);
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [AttributionError::NotFound, AttributionError::StorageFailed] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
