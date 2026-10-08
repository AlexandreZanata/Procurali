//! Declared professional classification: free, factual, and separate.
//!
//! Canonical rules: INV-06 (payment cannot grant powers — this module has no
//! payment input and the table has no subscription column), INV-39
//! (declaration, paid access, phone control, and reputation are separate
//! facts — declaring changes none of the others), AC-44 (declared
//! professionals without a plan keep ordinary free capabilities),
//! EC-18 (ending payment never removes the declaration; withdrawal keeps
//! ownership and history).
//!
//! Owners declare, update, and withdraw a business name, a type from the
//! fixed commercial vocabulary, and an approximate locality. Only `active`
//! accounts act. Public text refuses phone-shaped digit runs (INV-31), the
//! same rule as profile editing. Transitions append business facts;
//! withdrawal stamps instead of deleting, so history survives.

use crate::application::update_profile::contains_phone_shaped_digits;
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::users::{CITY_MAX_CHARS, DISPLAY_NAME_MAX_CHARS, REGION_MAX_CHARS};
use serde_json::json;

/// Fixed commercial vocabulary (professional-sellers 17.1).
pub const BUSINESS_TYPES: [&str; 5] = ["shop", "merchant", "reseller", "business", "recurring"];

/// A declared professional classification: factual public fields only. No
/// badge, verification, staff, subscription, or volume field exists, so none
/// can leak or authorize by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfessionalProfile {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Owning account.
    pub user_id: uuid::Uuid,
    /// Business display name.
    pub business_name: String,
    /// Commercial type from the fixed vocabulary.
    pub business_type: String,
    /// Approximate city label.
    pub city: String,
    /// Approximate region label.
    pub region: String,
}

/// New or updated declaration input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProfessional {
    /// Business display name, 1..=80 scalar values, no phone-shaped runs.
    pub business_name: String,
    /// Commercial type from [`BUSINESS_TYPES`].
    pub business_type: String,
    /// Approximate city label, 1..=120 scalar values, no phone-shaped runs.
    pub city: String,
    /// Approximate region label, 1..=40 scalar values, no phone-shaped runs.
    pub region: String,
}

/// Typed professional-profile failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfessionalError {
    /// A field is missing, out of bounds, off-vocabulary, or phone-shaped.
    InvalidField,
    /// The account is not active or no longer exists.
    NotActive,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for ProfessionalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid professional field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::StorageFailed => f.write_str("professional storage failed"),
        }
    }
}

impl std::error::Error for ProfessionalError {}

fn check_input(input: &NewProfessional) -> Result<(), ProfessionalError> {
    let bounded = |value: &str, max: usize| {
        !value.trim().is_empty()
            && value.chars().count() <= max
            && !contains_phone_shaped_digits(value)
    };
    if !bounded(&input.business_name, DISPLAY_NAME_MAX_CHARS)
        || !bounded(&input.city, CITY_MAX_CHARS)
        || !bounded(&input.region, REGION_MAX_CHARS)
        || !BUSINESS_TYPES.contains(&input.business_type.as_str())
    {
        return Err(ProfessionalError::InvalidField);
    }
    Ok(())
}

fn read_profile(row: &sqlx::postgres::PgRow) -> Result<ProfessionalProfile, ProfessionalError> {
    use sqlx::Row;
    Ok(ProfessionalProfile {
        id: row
            .try_get("id")
            .map_err(|_| ProfessionalError::StorageFailed)?,
        user_id: row
            .try_get("user_id")
            .map_err(|_| ProfessionalError::StorageFailed)?,
        business_name: row
            .try_get("business_name")
            .map_err(|_| ProfessionalError::StorageFailed)?,
        business_type: row
            .try_get("business_type")
            .map_err(|_| ProfessionalError::StorageFailed)?,
        city: row
            .try_get("city")
            .map_err(|_| ProfessionalError::StorageFailed)?,
        region: row
            .try_get("region")
            .map_err(|_| ProfessionalError::StorageFailed)?,
    })
}

async fn active_account_exists(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: uuid::Uuid,
) -> Result<bool, ProfessionalError> {
    let found: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM users WHERE id = $1 AND state = 'active' AND deleted_at IS NULL LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| ProfessionalError::StorageFailed)?;
    Ok(found.is_some())
}

/// Declare or update one active account's professional classification.
///
/// Free of charge: no payment input exists and no payment column is written.
/// First declarations append `account.professional_declared`; value changes
/// append `account.professional_updated`; revivals clear the withdrawal.
///
/// # Errors
///
/// Returns [`ProfessionalError::InvalidField`] for malformed input,
/// [`ProfessionalError::NotActive`] for non-active or missing accounts, else
/// [`ProfessionalError::StorageFailed`]. Reasons are static.
pub async fn declare_profile(
    pool: &sqlx::PgPool,
    user_id: uuid::Uuid,
    input: NewProfessional,
) -> Result<ProfessionalProfile, ProfessionalError> {
    check_input(&input)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ProfessionalError::StorageFailed)?;
    if !active_account_exists(&mut tx, user_id).await? {
        tx.rollback()
            .await
            .map_err(|_| ProfessionalError::StorageFailed)?;
        return Err(ProfessionalError::NotActive);
    }
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(
        r#"INSERT INTO professional_profiles
            (user_id, business_name, business_type, city, region)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (user_id) DO UPDATE SET
             business_name = EXCLUDED.business_name,
             business_type = EXCLUDED.business_type,
             city = EXCLUDED.city,
             region = EXCLUDED.region,
             withdrawn_at = NULL
           RETURNING id, user_id, business_name, business_type, city, region,
                     (xmax = 0) AS inserted"#,
    )
    .bind(user_id)
    .bind(&input.business_name)
    .bind(&input.business_type)
    .bind(&input.city)
    .bind(&input.region)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ProfessionalError::StorageFailed)?;
    let row = row.ok_or(ProfessionalError::StorageFailed)?;
    use sqlx::Row;
    let inserted: bool = row
        .try_get("inserted")
        .map_err(|_| ProfessionalError::StorageFailed)?;
    let profile = read_profile(&row)?;
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(user_id),
            resource_kind: "account",
            resource_id: user_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: if inserted {
                "account.professional_declared"
            } else {
                "account.professional_updated"
            },
            policy: "mvp-free",
            source: "api",
            payload: json!({"business_type": profile.business_type}),
        },
    )
    .await
    .map_err(|_| ProfessionalError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| ProfessionalError::StorageFailed)?;
    Ok(profile)
}

/// Withdraw one active account's declaration, keeping row and history.
///
/// Idempotent: withdrawing an absent or already-withdrawn declaration still
/// succeeds with `false`.
///
/// # Errors
///
/// Returns [`ProfessionalError::StorageFailed`] on database failure only.
pub async fn withdraw_profile(
    pool: &sqlx::PgPool,
    user_id: uuid::Uuid,
) -> Result<bool, ProfessionalError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ProfessionalError::StorageFailed)?;
    if !active_account_exists(&mut tx, user_id).await? {
        tx.rollback()
            .await
            .map_err(|_| ProfessionalError::StorageFailed)?;
        return Ok(false);
    }
    let transitioned: bool = sqlx::query(
        "UPDATE professional_profiles SET withdrawn_at = now()
          WHERE user_id = $1 AND withdrawn_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| ProfessionalError::StorageFailed)?
    .rows_affected()
        == 1;
    if !transitioned {
        tx.rollback()
            .await
            .map_err(|_| ProfessionalError::StorageFailed)?;
        return Ok(false);
    }
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(user_id),
            resource_kind: "account",
            resource_id: user_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "account.professional_withdrawn",
            policy: "mvp-free",
            source: "api",
            payload: json!({}),
        },
    )
    .await
    .map_err(|_| ProfessionalError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| ProfessionalError::StorageFailed)?;
    Ok(true)
}

/// Read one account's current declaration, if declared and not withdrawn.
///
/// Runtime-checked SQL keeps `cargo build` offline-capable.
///
/// # Errors
///
/// Returns [`ProfessionalError::StorageFailed`] on database failure only.
pub async fn profile_for<'e, E>(
    executor: E,
    user_id: uuid::Uuid,
) -> Result<Option<ProfessionalProfile>, ProfessionalError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(
        "SELECT id, user_id, business_name, business_type, city, region
         FROM professional_profiles WHERE user_id = $1 AND withdrawn_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(executor)
    .await
    .map_err(|_| ProfessionalError::StorageFailed)?;
    row.map(|row| read_profile(&row)).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vocabulary_pins_commercial_types() {
        assert_eq!(
            BUSINESS_TYPES,
            ["shop", "merchant", "reseller", "business", "recurring"]
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            ProfessionalError::InvalidField,
            ProfessionalError::NotActive,
            ProfessionalError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
            assert!(!rendered.contains("+55"));
        }
    }
}
