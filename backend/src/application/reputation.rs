//! Modest factual reputation projection: labeled evidence only.
//!
//! Canonical rules: reputation 14.1 (factual account age, broad recent
//! activity, and buyer-reported resolutions when supported — each labeled
//! exactly as what it is), INV-31 (no phone, exact activity history, or
//! reporter identities exist in this projection), INV-39 (professional
//! declaration, paid access, phone control, and reputation stay separate
//! facts — classification never enters the projection), INV-42
//! (duplicates excluded by construction with traceable corrections — one
//! set per contact, current answers only, distinct reporters), and
//! INV-43 with AC-42 (no verified-transaction or complaint-badge claim
//! exists anywhere here: contact initiation alone shows nothing, and
//! negative unreviewed feedback publishes no badge).
//!
//! There is deliberately no trust score, verified-sales count, complaint
//! count, star rating, or legitimacy summary in this module — broader
//! summaries belong to later scope with their own minimums.

use serde::Serialize;

/// Recent-activity window in days ("active recently" means at least one
/// actor-attributed event inside it).
pub const RECENT_ACTIVITY_DAYS: i64 = 7;

/// One modest public projection: facts with exact labels, nothing more.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reputation {
    /// Projected account.
    pub user_id: uuid::Uuid,
    /// Full days since account creation.
    pub account_age_days: i64,
    /// At least one actor-attributed event in the recent window.
    pub active_recently: bool,
    /// Buyer-reported resolutions with the observation count, present
    /// only when at least one distinct reporting buyer currently answers
    /// yes about this seller. Never a verified-sales claim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolutions_reported: Option<i64>,
}

/// Typed reputation failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReputationError {
    /// No such account exists.
    NotFound,
    /// The lookup failed.
    StorageFailed,
}

impl std::fmt::Display for ReputationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("account not found"),
            Self::StorageFailed => f.write_str("reputation lookup failed"),
        }
    }
}

impl std::error::Error for ReputationError {}

/// Project one account's modest public evidence: age in days, a recent
/// activity flag, and the buyer-reported resolution count with its
/// observation basis only when supported. Raw contact totals stay
/// operational (never projected), negative answers publish no badge,
/// and professional classification stays out by construction.
///
/// # Errors
///
/// Returns [`ReputationError::NotFound`] for missing accounts, else
/// [`ReputationError::StorageFailed`]. Reasons are static.
pub async fn project_reputation(
    pool: &sqlx::PgPool,
    user_id: uuid::Uuid,
) -> Result<Reputation, ReputationError> {
    let created_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT created_at FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| ReputationError::StorageFailed)?;
    let created_at = match created_at {
        Some(created_at) => created_at,
        None => return Err(ReputationError::NotFound),
    };
    let now = chrono::Utc::now();
    let account_age_days = (now - created_at).num_days().max(0);
    let recent: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM business_events
         WHERE actor_id = $1 AND occurred_at > $2 LIMIT 1",
    )
    .bind(user_id)
    .bind(now - chrono::Duration::days(RECENT_ACTIVITY_DAYS))
    .fetch_optional(pool)
    .await
    .map_err(|_| ReputationError::StorageFailed)?;
    // Distinct reporting buyers currently answering yes about this
    // seller: one set per contact by construction, current answers only,
    // no complaint vocabulary anywhere near the count.
    let reporters: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT buyer_id) FROM contact_feedback
         WHERE seller_id = $1 AND answer = 'yes'",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(|_| ReputationError::StorageFailed)?;
    Ok(Reputation {
        user_id,
        account_age_days,
        active_recently: recent.is_some(),
        resolutions_reported: if reporters > 0 { Some(reporters) } else { None },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_marks_no_verified_claim() {
        let reputation = Reputation {
            user_id: uuid::Uuid::now_v7(),
            account_age_days: 3,
            active_recently: true,
            resolutions_reported: Some(2),
        };
        let rendered = serde_json::to_string(&reputation).expect("projection serializes");
        assert!(rendered.contains("resolutions_reported"));
        for absent in [
            "verified",
            "sales",
            "sale",
            "trust",
            "score",
            "rating",
            "stars",
            "badge",
            "complaint",
            "negative",
            "dispute",
            "phone",
            "reporter",
            "token",
            "secret",
        ] {
            assert!(!rendered.contains(absent), "no {absent} in projection");
        }
    }

    #[test]
    fn unsupported_label_omits_itself() {
        let reputation = Reputation {
            user_id: uuid::Uuid::now_v7(),
            account_age_days: 0,
            active_recently: false,
            resolutions_reported: None,
        };
        let rendered = serde_json::to_string(&reputation).expect("projection serializes");
        assert!(!rendered.contains("resolutions_reported"));
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [ReputationError::NotFound, ReputationError::StorageFailed] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
