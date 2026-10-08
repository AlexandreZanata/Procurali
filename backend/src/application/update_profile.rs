//! Owner profile editing: display name and default locality.
//!
//! Canonical rules: INV-16 (a default-locality change never silently moves a
//! published request — this update touches the user row only; request/offer
//! snapshots stay exactly as declared until an explicit resource revision),
//! INV-31 (no phone in public surfaces — phone-shaped digit runs are refused
//! in every public text field, closing the name-as-destination workaround),
//! AC-41/EC-17 (locality updates change account defaults only).
//!
//! Only `active` accounts edit; the verification flow owns everything before
//! that. Policy acceptance history (`policy_version`, `policy_accepted_at`)
//! is never rewritten here, and every update appends one `account.updated`
//! fact in the same transaction instead of rewriting history.

use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::users::{CITY_MAX_CHARS, DISPLAY_NAME_MAX_CHARS, REGION_MAX_CHARS};
use serde_json::json;

/// Minimum consecutive-digit run refused as phone-shaped (E.164 minimum).
pub const PHONE_SHAPED_DIGITS: usize = 8;

/// Partial profile update: every field optional, at least one required.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileUpdate {
    /// New display name, if changing.
    pub display_name: Option<String>,
    /// New default city label, if changing.
    pub city: Option<String>,
    /// New default region label, if changing.
    pub region: Option<String>,
}

/// The updated account profile: public fields only, never phone material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatedProfile {
    /// Account identifier.
    pub id: uuid::Uuid,
    /// Current display name.
    pub display_name: String,
    /// Current default city label.
    pub city: String,
    /// Current default region label.
    pub region: String,
    /// Lifecycle state (unchanged by this update).
    pub state: String,
}

/// Typed profile-update failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileError {
    /// A field is missing, empty, out of bounds, or phone-shaped.
    InvalidField,
    /// The account is not active or no longer exists.
    NotActive,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid profile field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::StorageFailed => f.write_str("profile storage failed"),
        }
    }
}

impl std::error::Error for ProfileError {}

/// True when `value` carries a phone-shaped digit run: eight or more digits
/// allowing common visual separators between them. Names, cities, and regions
/// are public text (INV-31); digit runs are never legitimate there.
#[must_use]
pub fn contains_phone_shaped_digits(value: &str) -> bool {
    let mut run = 0usize;
    for char in value.chars() {
        if char.is_ascii_digit() {
            run += 1;
            if run >= PHONE_SHAPED_DIGITS {
                return true;
            }
        } else if matches!(char, ' ' | '-' | '(' | ')' | '.' | '+') {
            continue;
        } else {
            run = 0;
        }
    }
    false
}

fn check_field(value: &str, max_chars: usize) -> Result<(), ProfileError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || value.chars().count() > max_chars {
        return Err(ProfileError::InvalidField);
    }
    if contains_phone_shaped_digits(value) {
        return Err(ProfileError::InvalidField);
    }
    Ok(())
}

/// Update one active account's profile fields atomically with its fact.
///
/// Only the supplied fields change; policy history, state, phone material,
/// and timestamps stay exactly as recorded. Resource locality snapshots live
/// in other tables and are never touched here (INV-16, AC-41, EC-17).
///
/// # Errors
///
/// Returns [`ProfileError::InvalidField`] for missing, empty, oversized, or
/// phone-shaped input, [`ProfileError::NotActive`] for non-active or missing
/// accounts, else [`ProfileError::StorageFailed`]. Reasons are static.
pub async fn update_profile(
    pool: &sqlx::PgPool,
    user_id: uuid::Uuid,
    update: ProfileUpdate,
) -> Result<UpdatedProfile, ProfileError> {
    if update.display_name.is_none() && update.city.is_none() && update.region.is_none() {
        return Err(ProfileError::InvalidField);
    }
    if let Some(ref display_name) = update.display_name {
        check_field(display_name, DISPLAY_NAME_MAX_CHARS)?;
    }
    if let Some(ref city) = update.city {
        check_field(city, CITY_MAX_CHARS)?;
    }
    if let Some(ref region) = update.region {
        check_field(region, REGION_MAX_CHARS)?;
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ProfileError::StorageFailed)?;
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(
        "UPDATE users SET display_name = COALESCE($2, display_name),
                          city = COALESCE($3, city),
                          region = COALESCE($4, region)
          WHERE id = $1 AND state = 'active' AND deleted_at IS NULL
          RETURNING id, display_name, city, region, state",
    )
    .bind(user_id)
    .bind(update.display_name.as_deref())
    .bind(update.city.as_deref())
    .bind(update.region.as_deref())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ProfileError::StorageFailed)?;
    let row = match row {
        Some(row) => row,
        None => {
            tx.rollback()
                .await
                .map_err(|_| ProfileError::StorageFailed)?;
            return Err(ProfileError::NotActive);
        }
    };
    use sqlx::Row;
    let updated = UpdatedProfile {
        id: row.try_get("id").map_err(|_| ProfileError::StorageFailed)?,
        display_name: row
            .try_get("display_name")
            .map_err(|_| ProfileError::StorageFailed)?,
        city: row
            .try_get("city")
            .map_err(|_| ProfileError::StorageFailed)?,
        region: row
            .try_get("region")
            .map_err(|_| ProfileError::StorageFailed)?,
        state: row
            .try_get("state")
            .map_err(|_| ProfileError::StorageFailed)?,
    };
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(user_id),
            resource_kind: "account",
            resource_id: user_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "account.updated",
            policy: "mvp-free",
            source: "api",
            payload: json!({"state": "active"}),
        },
    )
    .await
    .map_err(|_| ProfileError::StorageFailed)?;
    tx.commit().await.map_err(|_| ProfileError::StorageFailed)?;
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digit_runs_classify_phone_shaped_text() {
        assert!(contains_phone_shaped_digits("+5511987654321"));
        assert!(contains_phone_shaped_digits("call +55 11 98765-4321 now"));
        assert!(contains_phone_shaped_digits("12345678"));
        assert!(!contains_phone_shaped_digits("User 123"));
        assert!(!contains_phone_shaped_digits("Apartment 12345, Block B"));
        assert!(!contains_phone_shaped_digits("Campinas"));
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            ProfileError::InvalidField,
            ProfileError::NotActive,
            ProfileError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
            assert!(!rendered.contains("+55"));
        }
    }
}
