//! Accountable metric corrections without rewriting history.
//!
//! Canonical rules: INV-42 (duplicates and identified abuse stay out of
//! aggregates through traceable corrections — rows stay, meaning is
//! corrected by reference), INV-37 with AC-39 (corrections record actor,
//! reason, scope, time, and policy version), INV-31 (no reporter, phone,
//! or secret material travels here), and AC-50 (new policy versions apply
//! prospectively; historical events keep the version they recorded).
//!
//! False or prohibited offers leave live supply by transitioning to
//! `invalidated` (the row and its terms survive, so historical response
//! evidence stays auditable while current health excludes them). Policy
//! versions are explicit rows; cohorts key on original publication time,
//! so a new version never rewrites an old cohort.

use serde::Serialize;

use crate::application::staff_permissions::{authorized_inspect, require_admin, require_moderator};
use crate::application::staff_permissions::{InspectInput, StaffError};
use crate::persistence::events::{record as record_event, NewEvent};

/// Correctable offer reasons: knowingly false or prohibited supply.
pub const CORRECTION_REASONS: [&str; 2] = ["false_information", "prohibited_item"];
/// Purpose bound (mirrors the staff audit backstop).
pub const PURPOSE_MAX_CHARS: usize = 500;
/// Policy-version bound (mirrors the version backstop).
pub const POLICY_VERSION_MAX_CHARS: usize = 32;
/// Description bound for prospective versions.
pub const DESCRIPTION_MAX_CHARS: usize = 1000;

/// One recorded correction reference.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Correction {
    /// Correction event identifier.
    pub id: uuid::Uuid,
    /// Corrected family (`offer` here).
    pub target_kind: String,
    /// Corrected identifier.
    pub target_id: uuid::Uuid,
    /// Correction reason.
    pub reason: String,
    /// Policy version the correction was recorded under.
    pub policy_version: String,
    /// Recording instant.
    pub corrected_at: chrono::DateTime<chrono::Utc>,
}

/// Typed correction failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorrectionError {
    /// Unknown reason, bad purpose/policy text, or past-dated version.
    InvalidField,
    /// The actor is missing or restricted.
    NotActive,
    /// No staff power for this action.
    NotPermitted,
    /// No such target or version.
    NotFound,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for CorrectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField => f.write_str("invalid correction field"),
            Self::NotActive => f.write_str("account is not active"),
            Self::NotPermitted => f.write_str("staff power required"),
            Self::NotFound => f.write_str("correction target not found"),
            Self::StorageFailed => f.write_str("correction storage failed"),
        }
    }
}

impl std::error::Error for CorrectionError {}

fn check_text(value: &str, max: usize) -> Result<(), CorrectionError> {
    if value.trim().is_empty() || value.chars().count() > max {
        return Err(CorrectionError::InvalidField);
    }
    Ok(())
}

async fn known_policy(pool: &sqlx::PgPool, version: &str) -> Result<bool, CorrectionError> {
    let found: Option<i32> = sqlx::query_scalar("SELECT 1 FROM policy_versions WHERE version = $1")
        .bind(version)
        .fetch_optional(pool)
        .await
        .map_err(|_| CorrectionError::StorageFailed)?;
    Ok(found.is_some())
}

fn staff_error(error: StaffError) -> CorrectionError {
    match error {
        StaffError::InvalidField => CorrectionError::InvalidField,
        StaffError::NotActive => CorrectionError::NotActive,
        StaffError::NotFound => CorrectionError::NotFound,
        StaffError::NotPermitted => CorrectionError::NotPermitted,
        _ => CorrectionError::StorageFailed,
    }
}

/// Invalidate one false or prohibited offer by staff reference: the row
/// transitions to `invalidated` (excluded from live supply and future
/// first-offer liveness), its terms history survives, and a
/// `metric.corrected` fact records actor, reason, and policy version.
///
/// # Errors
///
/// Returns [`CorrectionError::InvalidField`] for unknown reasons or bad
/// purpose/policy text, [`CorrectionError::NotActive`] for restricted
/// actors, [`CorrectionError::NotPermitted`] without a live moderator
/// grant, [`CorrectionError::NotFound`] for missing offers or versions,
/// else [`CorrectionError::StorageFailed`]. Reasons are static.
pub async fn apply_offer_correction(
    pool: &sqlx::PgPool,
    staff_id: uuid::Uuid,
    offer_id: uuid::Uuid,
    reason: &str,
    policy_version: &str,
    purpose: &str,
) -> Result<Correction, CorrectionError> {
    if !CORRECTION_REASONS.contains(&reason) {
        return Err(CorrectionError::InvalidField);
    }
    check_text(purpose, PURPOSE_MAX_CHARS)?;
    check_text(policy_version, POLICY_VERSION_MAX_CHARS)?;
    if !known_policy(pool, policy_version).await? {
        return Err(CorrectionError::NotFound);
    }
    require_moderator(pool, staff_id)
        .await
        .map_err(staff_error)?;
    authorized_inspect(
        pool,
        staff_id,
        InspectInput {
            target_kind: "offer".to_owned(),
            target_id: offer_id,
            purpose: purpose.to_owned(),
            policy_version: policy_version.to_owned(),
        },
    )
    .await
    .map_err(staff_error)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| CorrectionError::StorageFailed)?;
    let exists: Option<i32> = sqlx::query_scalar("SELECT 1 FROM offers WHERE id = $1")
        .bind(offer_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| CorrectionError::StorageFailed)?;
    if exists.is_none() {
        tx.rollback()
            .await
            .map_err(|_| CorrectionError::StorageFailed)?;
        return Err(CorrectionError::NotFound);
    }
    sqlx::query("UPDATE offers SET state = 'invalidated', updated_at = now() WHERE id = $1")
        .bind(offer_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| CorrectionError::StorageFailed)?;
    let event_id = record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(staff_id),
            resource_kind: "offer",
            resource_id: offer_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "metric.corrected",
            policy: "mvp-free",
            source: "staff",
            payload: serde_json::json!({"reason": reason, "policy_version": policy_version}),
        },
    )
    .await
    .map_err(|_| CorrectionError::StorageFailed)?
    .id;
    tx.commit()
        .await
        .map_err(|_| CorrectionError::StorageFailed)?;
    let corrected_at: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT occurred_at FROM business_events WHERE id = $1")
            .bind(event_id)
            .fetch_one(pool)
            .await
            .map_err(|_| CorrectionError::StorageFailed)?;
    Ok(Correction {
        id: event_id,
        target_kind: "offer".to_owned(),
        target_id: offer_id,
        reason: reason.to_owned(),
        policy_version: policy_version.to_owned(),
        corrected_at,
    })
}

/// Effective policy version at one instant: the latest version whose
/// effective date has passed. Historical facts keep the version they
/// recorded; this resolver only names the current rule.
pub async fn effective_policy_version(
    pool: &sqlx::PgPool,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<String, CorrectionError> {
    let version: Option<String> = sqlx::query_scalar(
        "SELECT version FROM policy_versions
         WHERE effective_from <= $1 ORDER BY effective_from DESC, version DESC LIMIT 1",
    )
    .bind(at)
    .fetch_optional(pool)
    .await
    .map_err(|_| CorrectionError::StorageFailed)?;
    version.ok_or(CorrectionError::NotFound)
}

/// Create one prospective policy version by administrator authority: the
/// effective date must lie in the future, so current rules stay untouched
/// until it arrives. Old cohorts never rewrite; new facts name the new
/// version once effective.
pub async fn create_prospective_version(
    pool: &sqlx::PgPool,
    admin_id: uuid::Uuid,
    version: &str,
    effective_from: chrono::DateTime<chrono::Utc>,
    description: &str,
) -> Result<String, CorrectionError> {
    check_text(version, POLICY_VERSION_MAX_CHARS)?;
    check_text(description, DESCRIPTION_MAX_CHARS)?;
    if effective_from <= chrono::Utc::now() {
        return Err(CorrectionError::InvalidField);
    }
    require_admin(pool, admin_id).await.map_err(staff_error)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| CorrectionError::StorageFailed)?;
    let exists: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM policy_versions WHERE version = $1")
            .bind(version)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| CorrectionError::StorageFailed)?;
    if exists.is_some() {
        tx.rollback()
            .await
            .map_err(|_| CorrectionError::StorageFailed)?;
        return Err(CorrectionError::InvalidField);
    }
    sqlx::query(
        "INSERT INTO policy_versions (version, effective_from, description, created_by)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(version)
    .bind(effective_from)
    .bind(description)
    .bind(admin_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| CorrectionError::StorageFailed)?;
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(admin_id),
            resource_kind: "policy",
            resource_id: uuid::Uuid::now_v7(),
            cycle: None,
            revision: None,
            effective_at: effective_from,
            kind: "business_policy_changed",
            policy: "mvp-free",
            source: "staff",
            payload: serde_json::json!({"version": version}),
        },
    )
    .await
    .map_err(|_| CorrectionError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| CorrectionError::StorageFailed)?;
    Ok(version.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correction_reasons_are_closed() {
        assert!(CORRECTION_REASONS.contains(&"false_information"));
        assert!(CORRECTION_REASONS.contains(&"prohibited_item"));
        assert!(!CORRECTION_REASONS.contains(&"spam"));
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            CorrectionError::InvalidField,
            CorrectionError::NotActive,
            CorrectionError::NotPermitted,
            CorrectionError::NotFound,
            CorrectionError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
        }
    }
}
