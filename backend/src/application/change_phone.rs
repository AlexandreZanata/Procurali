//! Proof-gated phone reassignment: verified new destination, atomic commit.
//!
//! Canonical rules: INV-07 (the new number must be free across non-deleted
//! accounts — enforced by the application and the partial unique index, AC-03
//! refusal without another owner's history), INV-24 (history appends; past
//! snapshots stay intact), INV-31 (no destination in public or history
//! surfaces — only keyed lookups travel outside the ciphertext), EC-21
//! (future handoffs use the new verified destination; earlier contacts stay
//! historical), EC-22 (recycled numbers need review, never automatic transfer
//! through edit — a held number is refused here, full stop).
//!
//! Two phases, mirroring registration: `request_number_change` verifies the
//! account is active, the new number is free and different, then guards hourly
//! and resend bounds before sending a provider challenge and anchoring it;
//! `confirm_number_change` requires a live anchor plus a provider-confirmed
//! code, then reassigns atomically — anchor consumption, stale-anchor voiding,
//! ciphertext/lookup swap, history fact, and owner notice commit as one unit,
//! so the old destination serves until the commit lands. Sessions stay bound
//! to the unchanged account and are revalidated, never revoked, by a number
//! change; stale challenge anchors die in the same transaction.
//!
//! The guarded mechanics mirror `application::auth_limits` for a different
//! eligibility predicate (active holders instead of pending ones); unifying
//! both behind one helper lands with the card that can touch both files.

use crate::application::auth_limits::AbuseLimits;
use crate::application::phone_verification::{
    CheckOutcome, StartOutcome, VerificationError, VerificationProvider,
};
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::notices::{record as record_notice, NewNotice};
use crate::persistence::users::{
    canonicalize_phone, create_challenge, NewChallenge, PhoneKeys, User,
};
use serde_json::json;

/// Keyed phone-lookup SQL expression. The canonical definition lives in
/// `persistence::users` (unmodifiable from this card); this copy is identical.
const PHONE_LOOKUP_SQL: &str =
    "encode(hmac(convert_to($1, 'UTF8'), convert_to($2, 'UTF8'), 'sha256'), 'hex')";

/// Outcome of requesting a number change. Every non-sent outcome answers
/// generically with zero observable difference between reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeRequestOutcome {
    /// A challenge was sent to the new number and anchored.
    Sent,
    /// The hourly or resend bound refused this request; no provider call ran.
    RateLimited,
    /// Nothing was sent and nothing was recorded.
    NotEligible,
}

/// Failure of a number-change request. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeRequestError {
    /// The new phone is missing or malformed.
    InvalidInput,
    /// The provider could not be reached in time.
    ProviderFailed,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for ChangeRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput => f.write_str("invalid new phone number"),
            Self::ProviderFailed => f.write_str("verification provider unavailable"),
            Self::StorageFailed => f.write_str("phone change storage failed"),
        }
    }
}

impl std::error::Error for ChangeRequestError {}

/// Outcome of confirming a number change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeConfirmOutcome {
    /// The destination reassigned atomically.
    Changed,
    /// Wrong code, unknown number, held number, or ineligible holder.
    Failed,
    /// The caller's own challenge window elapsed.
    Expired,
    /// The provider rate-limited the request.
    RateLimited,
}

/// Failure of a number-change confirmation. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeConfirmError {
    /// The new phone or code is missing or malformed.
    InvalidInput,
    /// The provider could not be reached in time.
    ProviderFailed,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for ChangeConfirmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput => f.write_str("invalid confirmation"),
            Self::ProviderFailed => f.write_str("verification provider unavailable"),
            Self::StorageFailed => f.write_str("phone change storage failed"),
        }
    }
}

impl std::error::Error for ChangeConfirmError {}

/// Marker digest input for one change anchor: fresh randomness whose digest
/// proves nothing and matches no submitted code. Code secrecy stays with the
/// provider; the local row anchors window, single use, and rate data.
fn challenge_marker() -> String {
    let nonce: [u8; 16] = rand::random();
    nonce.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The live, active holder behind `user_id`, if eligible to change numbers.
async fn active_holder(
    pool: &sqlx::PgPool,
    user_id: uuid::Uuid,
) -> Result<Option<User>, ChangeRequestError> {
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(
        "SELECT id, display_name, city, region, state, policy_version,
                policy_accepted_at, deleted_at, created_at
         FROM users WHERE id = $1 AND state = 'active' AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| ChangeRequestError::StorageFailed)?;
    row.map(|row| {
        use sqlx::Row;
        Ok(User {
            id: row
                .try_get("id")
                .map_err(|_| ChangeRequestError::StorageFailed)?,
            display_name: row
                .try_get("display_name")
                .map_err(|_| ChangeRequestError::StorageFailed)?,
            city: row
                .try_get("city")
                .map_err(|_| ChangeRequestError::StorageFailed)?,
            region: row
                .try_get("region")
                .map_err(|_| ChangeRequestError::StorageFailed)?,
            state: row
                .try_get("state")
                .map_err(|_| ChangeRequestError::StorageFailed)?,
            policy_version: row
                .try_get("policy_version")
                .map_err(|_| ChangeRequestError::StorageFailed)?,
            policy_accepted_at: row
                .try_get("policy_accepted_at")
                .map_err(|_| ChangeRequestError::StorageFailed)?,
            deleted_at: row
                .try_get("deleted_at")
                .map_err(|_| ChangeRequestError::StorageFailed)?,
            created_at: row
                .try_get("created_at")
                .map_err(|_| ChangeRequestError::StorageFailed)?,
        })
    })
    .transpose()
}

/// Request a destination change for one active account.
///
/// The account must be active, the new number must differ and be free across
/// non-deleted accounts, and hourly/resend bounds must allow it; only then a
/// provider challenge goes out and anchors. Provider failures delete the
/// unused anchor, consuming no allowance.
///
/// # Errors
///
/// Returns [`ChangeRequestError::InvalidInput`] for malformed phones,
/// [`ChangeRequestError::ProviderFailed`] when the provider cannot be
/// reached, else [`ChangeRequestError::StorageFailed`]. Reasons are static.
pub async fn request_number_change<S: VerificationProvider>(
    pool: &sqlx::PgPool,
    provider: &S,
    user_id: uuid::Uuid,
    new_phone: &str,
    keys: PhoneKeys<'_>,
    window: std::time::Duration,
    limits: AbuseLimits,
) -> Result<ChangeRequestOutcome, ChangeRequestError> {
    let canonical = canonicalize_phone(new_phone).map_err(|_| ChangeRequestError::InvalidInput)?;
    if lookup_key_empty(keys) {
        return Err(ChangeRequestError::StorageFailed);
    }
    let active = active_holder(pool, user_id).await?.is_some();
    if !active {
        return Ok(ChangeRequestOutcome::NotEligible);
    }
    let new_lookup: String = sqlx::query_scalar(
        "SELECT encode(hmac(convert_to($1, 'UTF8'), convert_to($2, 'UTF8'), 'sha256'), 'hex')",
    )
    .bind(&canonical)
    .bind(keys.lookup_key)
    .fetch_one(pool)
    .await
    .map_err(|_| ChangeRequestError::StorageFailed)?;
    let current: Option<String> =
        sqlx::query_scalar("SELECT phone_lookup FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| ChangeRequestError::StorageFailed)?;
    if Some(&new_lookup) == current.as_ref() {
        return Ok(ChangeRequestOutcome::NotEligible);
    }
    if number_held(pool, &canonical, keys.lookup_key).await? {
        return Ok(ChangeRequestOutcome::NotEligible);
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ChangeRequestError::StorageFailed)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(&canonical)
        .execute(&mut *tx)
        .await
        .map_err(|_| ChangeRequestError::StorageFailed)?;
    let starts_last_hour: i64 = sqlx::query_scalar(
        &[
            r#"SELECT count(*) FROM phone_challenges WHERE phone_lookup = "#,
            PHONE_LOOKUP_SQL,
            " AND created_at > now() - interval '1 hour'",
        ]
        .concat(),
    )
    .bind(&canonical)
    .bind(keys.lookup_key)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| ChangeRequestError::StorageFailed)?;
    if starts_last_hour >= i64::from(limits.max_starts_per_hour) {
        tx.rollback()
            .await
            .map_err(|_| ChangeRequestError::StorageFailed)?;
        return Ok(ChangeRequestOutcome::RateLimited);
    }
    let recent: Option<i32> = sqlx::query_scalar(
        &[
            r#"SELECT 1 FROM phone_challenges WHERE phone_lookup = "#,
            PHONE_LOOKUP_SQL,
            " AND created_at > now() - make_interval(secs => $3) LIMIT 1",
        ]
        .concat(),
    )
    .bind(&canonical)
    .bind(keys.lookup_key)
    .bind(limits.resend_minimum_secs as f64)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ChangeRequestError::StorageFailed)?;
    if recent.is_some() {
        tx.rollback()
            .await
            .map_err(|_| ChangeRequestError::StorageFailed)?;
        return Ok(ChangeRequestOutcome::RateLimited);
    }
    let expires_at = chrono::Utc::now()
        + chrono::Duration::from_std(window).map_err(|_| ChangeRequestError::StorageFailed)?;
    let anchor: uuid::Uuid = create_challenge(
        &mut tx,
        NewChallenge {
            phone: canonical.clone(),
            code: challenge_marker(),
            expires_at,
        },
        keys.lookup_key,
    )
    .await
    .map_err(|_| ChangeRequestError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| ChangeRequestError::StorageFailed)?;
    match provider.start_verification(&canonical).await {
        Ok(StartOutcome::ChallengeSent) => Ok(ChangeRequestOutcome::Sent),
        Ok(StartOutcome::RateLimited) => {
            delete_anchor(pool, anchor).await?;
            Ok(ChangeRequestOutcome::RateLimited)
        }
        Err(VerificationError::Timeout | VerificationError::ConnectionFailed) => {
            delete_anchor(pool, anchor).await?;
            Err(ChangeRequestError::ProviderFailed)
        }
        Err(_) => {
            delete_anchor(pool, anchor).await?;
            Err(ChangeRequestError::StorageFailed)
        }
    }
}

fn lookup_key_empty(keys: PhoneKeys<'_>) -> bool {
    keys.lookup_key.is_empty() || keys.encryption_key.is_empty()
}

/// True when a live account already holds `canonical`.
async fn number_held(
    pool: &sqlx::PgPool,
    canonical: &str,
    lookup_key: &str,
) -> Result<bool, ChangeRequestError> {
    let held: Option<i32> = sqlx::query_scalar(
        &[
            r#"SELECT 1 FROM users WHERE phone_lookup = "#,
            PHONE_LOOKUP_SQL,
            " AND deleted_at IS NULL LIMIT 1",
        ]
        .concat(),
    )
    .bind(canonical)
    .bind(lookup_key)
    .fetch_optional(pool)
    .await
    .map_err(|_| ChangeRequestError::StorageFailed)?;
    Ok(held.is_some())
}

/// Delete one unused anchor after a provider failure or limit.
async fn delete_anchor(pool: &sqlx::PgPool, anchor: uuid::Uuid) -> Result<(), ChangeRequestError> {
    sqlx::query("DELETE FROM phone_challenges WHERE id = $1")
        .bind(anchor)
        .execute(pool)
        .await
        .map_err(|_| ChangeRequestError::StorageFailed)?;
    Ok(())
}

/// Confirm a destination change and reassign atomically.
///
/// Requires a live anchor for the new number plus a provider-confirmed code.
/// Anchor consumption, stale-anchor voiding, the ciphertext/lookup swap, the
/// history fact, and the owner notice commit as one unit; the old destination
/// serves until that commit lands. Sessions stay bound to the unchanged
/// account and are implicitly revalidated by its still-`active` state.
///
/// # Errors
///
/// Returns [`ChangeConfirmError::InvalidInput`] for malformed input,
/// [`ChangeConfirmError::ProviderFailed`] when the provider cannot be
/// reached, else [`ChangeConfirmError::StorageFailed`]. Reasons are static.
pub async fn confirm_number_change<S: VerificationProvider>(
    pool: &sqlx::PgPool,
    provider: &S,
    user_id: uuid::Uuid,
    new_phone: &str,
    new_code: &str,
    keys: PhoneKeys<'_>,
) -> Result<ChangeConfirmOutcome, ChangeConfirmError> {
    if new_code.trim().is_empty() || new_code.chars().count() > 32 {
        return Err(ChangeConfirmError::InvalidInput);
    }
    let canonical = canonicalize_phone(new_phone).map_err(|_| ChangeConfirmError::InvalidInput)?;
    if lookup_key_empty(keys) {
        return Err(ChangeConfirmError::StorageFailed);
    }
    let current: Option<(String, String)> = sqlx::query_as(
        "SELECT phone_lookup, state FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| ChangeConfirmError::StorageFailed)?;
    let current_lookup = match current {
        Some((lookup, state)) if state == "active" => lookup,
        _ => return Ok(ChangeConfirmOutcome::Failed),
    };
    let new_lookup: String = sqlx::query_scalar(
        "SELECT encode(hmac(convert_to($1, 'UTF8'), convert_to($2, 'UTF8'), 'sha256'), 'hex')",
    )
    .bind(&canonical)
    .bind(keys.lookup_key)
    .fetch_one(pool)
    .await
    .map_err(|_| ChangeConfirmError::StorageFailed)?;
    if new_lookup == current_lookup {
        return Ok(ChangeConfirmOutcome::Failed);
    }
    if number_held_confirm(pool, &canonical, keys.lookup_key).await? {
        return Ok(ChangeConfirmOutcome::Failed);
    }
    if !live_anchor_exists(pool, &canonical, keys.lookup_key).await? {
        if expired_anchor_exists(pool, &canonical, keys.lookup_key).await? {
            return Ok(ChangeConfirmOutcome::Expired);
        }
        return Ok(ChangeConfirmOutcome::Failed);
    }
    match provider.check_verification(&canonical, new_code).await {
        Ok(CheckOutcome::Verified) => {}
        Ok(CheckOutcome::Incorrect) => return Ok(ChangeConfirmOutcome::Failed),
        Ok(CheckOutcome::RateLimited) => return Ok(ChangeConfirmOutcome::RateLimited),
        Err(VerificationError::Timeout | VerificationError::ConnectionFailed) => {
            return Err(ChangeConfirmError::ProviderFailed);
        }
        Err(_) => return Err(ChangeConfirmError::StorageFailed),
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ChangeConfirmError::StorageFailed)?;
    let consumed: Option<uuid::Uuid> = sqlx::query_scalar(
        &[
            r#"UPDATE phone_challenges SET consumed_at = now() WHERE id = (
                 SELECT id FROM phone_challenges WHERE phone_lookup = "#,
            PHONE_LOOKUP_SQL,
            " AND consumed_at IS NULL AND expires_at > now()
                 ORDER BY created_at DESC LIMIT 1)
               RETURNING id",
        ]
        .concat(),
    )
    .bind(&canonical)
    .bind(keys.lookup_key)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ChangeConfirmError::StorageFailed)?;
    if consumed.is_none() {
        tx.rollback()
            .await
            .map_err(|_| ChangeConfirmError::StorageFailed)?;
        return Ok(ChangeConfirmOutcome::Expired);
    }
    sqlx::query(
        "UPDATE phone_challenges SET consumed_at = now()
          WHERE consumed_at IS NULL AND (phone_lookup = $1 OR phone_lookup = $2)",
    )
    .bind(&current_lookup)
    .bind(&new_lookup)
    .execute(&mut *tx)
    .await
    .map_err(|_| ChangeConfirmError::StorageFailed)?;
    let swapped = match sqlx::query(
        "UPDATE users SET phone_ciphertext = pgp_sym_encrypt($2, $3), phone_lookup = $4
          WHERE id = $1 AND state = 'active' AND deleted_at IS NULL",
    )
    .bind(user_id)
    .bind(&canonical)
    .bind(keys.encryption_key)
    .bind(&new_lookup)
    .execute(&mut *tx)
    .await
    {
        Ok(done) => done,
        Err(error)
            if error
                .as_database_error()
                .is_some_and(|database| database.code().as_deref() == Some("23505")) =>
        {
            // A concurrent reassignment won the number after our check: roll
            // back, leaving our destination untouched, and report the generic
            // refusal.
            tx.rollback()
                .await
                .map_err(|_| ChangeConfirmError::StorageFailed)?;
            return Ok(ChangeConfirmOutcome::Failed);
        }
        Err(_) => return Err(ChangeConfirmError::StorageFailed),
    };
    if swapped.rows_affected() != 1 {
        tx.rollback()
            .await
            .map_err(|_| ChangeConfirmError::StorageFailed)?;
        return Ok(ChangeConfirmOutcome::Failed);
    }
    let event = record_event(
        &mut *tx,
        NewEvent {
            actor_id: Some(user_id),
            resource_kind: "account",
            resource_id: user_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "account.phone_changed",
            policy: "mvp-free",
            source: "api",
            payload: json!({"old_lookup": current_lookup, "new_lookup": new_lookup}),
        },
    )
    .await
    .map_err(|_| ChangeConfirmError::StorageFailed)?;
    record_notice(
        &mut *tx,
        NewNotice {
            account_id: user_id,
            kind: "account.phone_changed",
            resource_kind: "account",
            resource_id: user_id,
            event_id: event.id,
            body: "Your contact destination changed.".to_owned(),
        },
    )
    .await
    .map_err(|_| ChangeConfirmError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| ChangeConfirmError::StorageFailed)?;
    Ok(ChangeConfirmOutcome::Changed)
}

/// True when a live account already holds `canonical` (confirmation path).
async fn number_held_confirm(
    pool: &sqlx::PgPool,
    canonical: &str,
    lookup_key: &str,
) -> Result<bool, ChangeConfirmError> {
    let held: Option<i32> = sqlx::query_scalar(
        &[
            r#"SELECT 1 FROM users WHERE phone_lookup = "#,
            PHONE_LOOKUP_SQL,
            " AND deleted_at IS NULL LIMIT 1",
        ]
        .concat(),
    )
    .bind(canonical)
    .bind(lookup_key)
    .fetch_optional(pool)
    .await
    .map_err(|_| ChangeConfirmError::StorageFailed)?;
    Ok(held.is_some())
}

/// Any live anchor for one lookup.
async fn live_anchor_exists(
    pool: &sqlx::PgPool,
    canonical: &str,
    lookup_key: &str,
) -> Result<bool, ChangeConfirmError> {
    let found: Option<i32> = sqlx::query_scalar(
        &[
            r#"SELECT 1 FROM phone_challenges WHERE phone_lookup = "#,
            PHONE_LOOKUP_SQL,
            " AND consumed_at IS NULL AND expires_at > now() LIMIT 1",
        ]
        .concat(),
    )
    .bind(canonical)
    .bind(lookup_key)
    .fetch_optional(pool)
    .await
    .map_err(|_| ChangeConfirmError::StorageFailed)?;
    Ok(found.is_some())
}

/// Any elapsed, unconsumed anchor for one lookup.
async fn expired_anchor_exists(
    pool: &sqlx::PgPool,
    canonical: &str,
    lookup_key: &str,
) -> Result<bool, ChangeConfirmError> {
    let found: Option<i32> = sqlx::query_scalar(
        &[
            r#"SELECT 1 FROM phone_challenges WHERE phone_lookup = "#,
            PHONE_LOOKUP_SQL,
            " AND consumed_at IS NULL AND expires_at <= now() LIMIT 1",
        ]
        .concat(),
    )
    .bind(canonical)
    .bind(lookup_key)
    .fetch_optional(pool)
    .await
    .map_err(|_| ChangeConfirmError::StorageFailed)?;
    Ok(found.is_some())
}
