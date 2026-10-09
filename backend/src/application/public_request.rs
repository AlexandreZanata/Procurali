//! Public request lifecycle projection: details or limited status.
//!
//! Canonical rules: sharing 13.3 with discovery 9.3 (active eligible rows
//! project safe item, budget, condition, category, region, display name,
//! and the offer action; expired, completed, or cancelled rows project a
//! limited status with no interaction; suspended, prohibited,
//! deleted-account, or owner-removed rows project one generic unavailable
//! result; blocked signed-in parties receive the same unavailable result
//! as anonymous missing rows), INV-09 (terminal rows offer no contact),
//! INV-31 (no phone, notes, received offers, reporter identities, tokens,
//! or secrets exist in any projection here), INV-35 (pair blocks hide
//! without disclosing themselves — state evaluates before relationships),
//! AC-01 with EC-37 (anonymous visitors understand eligible demand from
//! the live row, which stays authoritative over outside previews).
//!
//! Missing, draft, private, hidden, suspended, and prohibited rows share
//! one indistinguishable refusal: strangers learn nothing about which
//! restriction applies, and blocked viewers learn nothing about the block.

use serde::Serialize;

/// Full public projection: everything an anonymous visitor needs before
/// registration, and nothing they must not see.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FullProjection {
    /// Projected demand identifier.
    pub id: uuid::Uuid,
    /// Current title (the live row stays authoritative).
    pub title: String,
    /// Current category code.
    pub category_code: String,
    /// Current budget in cents.
    pub budget_cents: i64,
    /// Current condition.
    pub condition: String,
    /// Listed city code.
    pub city_code: String,
    /// Listed region code.
    pub region_code: String,
    /// Declared buyer display name.
    pub author_name: String,
    /// Always `available` on this shape.
    pub availability: String,
    /// Always true on this shape: the offer action is open.
    pub offer_action: bool,
}

/// Limited terminal projection: standing with no interaction and no
/// content beyond the identifier.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LimitedProjection {
    /// Projected demand identifier.
    pub id: uuid::Uuid,
    /// Terminal standing (`expired`, `completed`, or `cancelled`).
    pub status: String,
    /// Always `unavailable` on this shape.
    pub availability: String,
    /// Always false on this shape: no offer action.
    pub offer_action: bool,
}

/// Lifecycle projection: full details, limited status, or nothing at all.
#[derive(Debug, Clone, PartialEq)]
pub enum PublicProjection {
    /// Active eligible demand with the offer action.
    Full(FullProjection),
    /// Terminal demand with standing only.
    Limited(LimitedProjection),
    /// Anything else: draft, private, hidden, suspended, prohibited,
    /// missing, or blocked for this viewer.
    Unavailable,
}

#[derive(Debug, sqlx::FromRow)]
struct ProjectionRow {
    id: uuid::Uuid,
    author_id: uuid::Uuid,
    title: String,
    category_code: String,
    budget_cents: i64,
    condition: String,
    city_code: String,
    region_code: String,
    author_name: String,
    state: String,
    visibility: String,
    deadline: Option<chrono::DateTime<chrono::Utc>>,
}

/// Typed projection failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicRequestError {
    /// No public projection exists for this viewer and identifier.
    NotFound,
    /// The lookup failed.
    StorageFailed,
}

impl std::fmt::Display for PublicRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("request not found"),
            Self::StorageFailed => f.write_str("request lookup failed"),
        }
    }
}

impl std::error::Error for PublicRequestError {}

/// Project one request for an optional viewer: full details for active
/// eligible demand, limited status for terminal demand, and one generic
/// refusal for everything else — including blocked viewers, who learn
/// nothing about the block. Anonymous viewers (`None`) skip only the
/// block check, since anonymous browsing cannot enforce a personal block.
///
/// # Errors
///
/// Returns [`PublicRequestError::NotFound`] for missing, draft, private,
/// hidden, suspended, prohibited, or blocked rows, else
/// [`PublicRequestError::StorageFailed`]. Reasons are static.
pub async fn project_request(
    pool: &sqlx::PgPool,
    viewer_id: Option<uuid::Uuid>,
    request_id: uuid::Uuid,
) -> Result<PublicProjection, PublicRequestError> {
    let row: Option<ProjectionRow> = sqlx::query_as(
        "SELECT requests.id, requests.author_id, requests.title, requests.category_code,
                requests.budget_cents, requests.condition, requests.city_code,
                requests.region_code, users.display_name AS author_name, requests.state,
                requests.visibility, cycle.deadline
         FROM requests
         JOIN users ON users.id = requests.author_id
         LEFT JOIN request_cycles AS cycle
           ON cycle.request_id = requests.id
          AND cycle.cycle_number = requests.current_cycle_number
         JOIN catalog_categories AS category
           ON category.code = requests.category_code
         WHERE requests.id = $1",
    )
    .bind(request_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| PublicRequestError::StorageFailed)?;
    let row = match row {
        Some(row) => row,
        None => return Err(PublicRequestError::NotFound),
    };
    let ProjectionRow {
        id,
        author_id,
        title,
        category_code,
        budget_cents,
        condition,
        city_code,
        region_code,
        author_name,
        state,
        visibility,
        deadline,
    } = row;
    // Terminal demand projects limited status with no interaction —
    // visibility-independent, since the link outlived availability.
    if state == "expired" || state == "completed" || state == "cancelled" {
        return Ok(PublicProjection::Limited(LimitedProjection {
            id,
            status: state,
            availability: "unavailable".to_owned(),
            offer_action: false,
        }));
    }
    // Everything below refuses identically: drafts, non-public rows,
    // suspended rows, prohibited classes, elapsed deadlines, and blocked
    // viewers share one generic shape with missing rows.
    let eligible = state == "active" && visibility == "public";
    let category_query: Option<String> =
        sqlx::query_scalar("SELECT status FROM catalog_categories WHERE code = $1")
            .bind(&category_code)
            .fetch_optional(pool)
            .await
            .map_err(|_| PublicRequestError::StorageFailed)?;
    let category_allowed = category_query.as_deref() == Some("allowed");
    let deadline_ok = deadline.is_some_and(|deadline| deadline > chrono::Utc::now());
    let blocked = match viewer_id {
        Some(viewer) => {
            let pair: Option<i32> = sqlx::query_scalar(
                "SELECT 1 FROM user_blocks
                 WHERE (blocker_id = $1 AND blocked_id = $2)
                    OR (blocker_id = $2 AND blocked_id = $1)",
            )
            .bind(viewer)
            .bind(author_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| PublicRequestError::StorageFailed)?;
            pair.is_some()
        }
        None => false,
    };
    if !eligible || !category_allowed || !deadline_ok || blocked {
        return Err(PublicRequestError::NotFound);
    }
    Ok(PublicProjection::Full(FullProjection {
        id,
        title,
        category_code,
        budget_cents,
        condition,
        city_code,
        region_code,
        author_name,
        availability: "available".to_owned(),
        offer_action: true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_projection_marks_availability() {
        let full = FullProjection {
            id: uuid::Uuid::now_v7(),
            title: "Refrigerator".to_owned(),
            category_code: "home_appliances".to_owned(),
            budget_cents: 60_000,
            condition: "either".to_owned(),
            city_code: "campinas".to_owned(),
            region_code: "centro".to_owned(),
            author_name: "Buyer".to_owned(),
            availability: "available".to_owned(),
            offer_action: true,
        };
        let rendered = serde_json::to_string(&full).expect("projection serializes");
        assert!(rendered.contains("Refrigerator"));
        for absent in [
            "phone", "lookup", "cipher", "token", "notes", "offers", "reporter", "secret",
        ] {
            assert!(!rendered.contains(absent), "no {absent} in projection");
        }
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            PublicRequestError::NotFound,
            PublicRequestError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
