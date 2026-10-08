//! Account recovery and recycled-number review boundary.
//!
//! Canonical rules: INV-07 (one current number per non-deleted account —
//! recovery never duplicates or transfers a held number, AC-03 generic
//! refusal without another owner's history) and EC-22 (recycled numbers need
//! reviewed handling: a new subscriber gets a distinct account with no access
//! to a previous owner's records, and presenting a recycled number records a
//! review fact instead of transferring anything).
//!
//! Two paths: `request_recovery` answers every number with the same generic
//! outcome, sending provider challenges only for live active holders and for
//! numbers whose only history is deleted (anyone else gets zero side effects
//! and no provider call). `confirm_recovery` restores access for a verified
//! active holder with a fresh session, and records a review fact on the
//! deleted account's stream for verified recycled numbers — never a session,
//! never a merge, never history access. Hourly and resend bounds mirror the
//! challenge guards; provider failures delete unused anchors, consuming no
//! allowance.

use crate::application::auth_limits::AbuseLimits;
use crate::application::phone_verification::{
    CheckOutcome, StartOutcome, VerificationError, VerificationProvider,
};
use crate::application::sessions::{login_after_proof, LoginOutcome};
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::users::{canonicalize_phone, create_challenge, NewChallenge, PhoneKeys};
use serde_json::json;

/// Keyed phone-lookup SQL expression. The canonical definition lives in
/// `persistence::users` (unmodifiable from this card); this copy is identical.
const PHONE_LOOKUP_SQL: &str =
    "encode(hmac(convert_to($1, 'UTF8'), convert_to($2, 'UTF8'), 'sha256'), 'hex')";

/// Challenge validity window for recovery (authentication protocol: 5 minutes).
pub const RECOVERY_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);

/// Outcome of requesting recovery. `Requested` answers sent and silent cases
/// identically; only provider-side rate limits surface separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryRequestOutcome {
    /// The generic response: a challenge may or may not have gone out.
    Requested,
    /// The provider rate-limited the request.
    ProviderLimited,
}

/// Failure of a recovery request. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryRequestError {
    /// The phone is missing or malformed.
    InvalidInput,
    /// The provider could not be reached in time.
    ProviderFailed,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for RecoveryRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput => f.write_str("invalid recovery request"),
            Self::ProviderFailed => f.write_str("verification provider unavailable"),
            Self::StorageFailed => f.write_str("recovery storage failed"),
        }
    }
}

impl std::error::Error for RecoveryRequestError {}

/// Outcome of confirming recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryOutcome {
    /// Access restored: fresh session token plus the active account id.
    /// The token is returned exactly once, for the login cookie.
    Recovered {
        /// Opaque session token (plaintext, cookie-bound, never stored).
        token: String,
        /// The recovered active account.
        user_id: uuid::Uuid,
    },
    /// Verified recycled number: recorded for staff review, no session, no
    /// access, no history transfer.
    UnderReview,
    /// Wrong code, unknown number, or ineligible holder: nothing restored.
    Failed,
    /// The caller's own challenge window elapsed.
    Expired,
    /// The provider rate-limited the request.
    RateLimited,
}

/// Failure of a recovery confirmation. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryError {
    /// The phone or code is missing or malformed.
    InvalidInput,
    /// The provider could not be reached in time.
    ProviderFailed,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput => f.write_str("invalid recovery confirmation"),
            Self::ProviderFailed => f.write_str("verification provider unavailable"),
            Self::StorageFailed => f.write_str("recovery storage failed"),
        }
    }
}

impl std::error::Error for RecoveryError {}

/// Marker digest input for one recovery anchor: fresh randomness whose digest
/// proves nothing and matches no submitted code.
fn challenge_marker() -> String {
    let nonce: [u8; 16] = rand::random();
    nonce.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Request recovery for one phone: generic unless the provider objects.
///
/// Live active holders and deleted-history numbers get a provider challenge
/// inside hourly/resend bounds and answer the same generic outcome as
/// everyone else (zero side effects, no provider call otherwise). Provider
/// rate limits surface so callers back off; provider outages surface so
/// callers retry; unused anchors are always deleted, consuming no allowance.
///
/// # Errors
///
/// Returns [`RecoveryRequestError::InvalidInput`] for malformed phones,
/// [`RecoveryRequestError::ProviderFailed`] when the provider cannot be
/// reached, else [`RecoveryRequestError::StorageFailed`]. Reasons are static.
pub async fn request_recovery<S: VerificationProvider>(
    pool: &sqlx::PgPool,
    provider: &S,
    phone: &str,
    keys: PhoneKeys<'_>,
    window: std::time::Duration,
    limits: AbuseLimits,
) -> Result<RecoveryRequestOutcome, RecoveryRequestError> {
    let canonical = canonicalize_phone(phone).map_err(|_| RecoveryRequestError::InvalidInput)?;
    if keys.lookup_key.is_empty() {
        return Err(RecoveryRequestError::StorageFailed);
    }
    if !recovery_eligible(pool, &canonical, keys.lookup_key).await? {
        return Ok(RecoveryRequestOutcome::Requested);
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| RecoveryRequestError::StorageFailed)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(&canonical)
        .execute(&mut *tx)
        .await
        .map_err(|_| RecoveryRequestError::StorageFailed)?;
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
    .map_err(|_| RecoveryRequestError::StorageFailed)?;
    if starts_last_hour >= i64::from(limits.max_starts_per_hour) {
        tx.rollback()
            .await
            .map_err(|_| RecoveryRequestError::StorageFailed)?;
        return Ok(RecoveryRequestOutcome::Requested);
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
    .map_err(|_| RecoveryRequestError::StorageFailed)?;
    if recent.is_some() {
        tx.rollback()
            .await
            .map_err(|_| RecoveryRequestError::StorageFailed)?;
        return Ok(RecoveryRequestOutcome::Requested);
    }
    let expires_at = chrono::Utc::now()
        + chrono::Duration::from_std(window).map_err(|_| RecoveryRequestError::StorageFailed)?;
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
    .map_err(|_| RecoveryRequestError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| RecoveryRequestError::StorageFailed)?;
    match provider.start_verification(&canonical).await {
        Ok(StartOutcome::ChallengeSent) => Ok(RecoveryRequestOutcome::Requested),
        Ok(StartOutcome::RateLimited) => {
            delete_anchor(pool, anchor).await?;
            Ok(RecoveryRequestOutcome::ProviderLimited)
        }
        Err(VerificationError::Timeout | VerificationError::ConnectionFailed) => {
            delete_anchor(pool, anchor).await?;
            Err(RecoveryRequestError::ProviderFailed)
        }
        Err(_) => {
            delete_anchor(pool, anchor).await?;
            Err(RecoveryRequestError::StorageFailed)
        }
    }
}

/// True for live active holders and deleted-history numbers; false otherwise.
/// Pending holders use registration challenges; restricted holders and
/// never-seen numbers get nothing.
async fn recovery_eligible(
    pool: &sqlx::PgPool,
    canonical: &str,
    lookup_key: &str,
) -> Result<bool, RecoveryRequestError> {
    let live_active: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM users WHERE phone_lookup = encode(hmac(convert_to($1, 'UTF8'),
         convert_to($2, 'UTF8'), 'sha256'), 'hex') AND deleted_at IS NULL AND state = 'active' LIMIT 1",
    )
    .bind(canonical)
    .bind(lookup_key)
    .fetch_optional(pool)
    .await
    .map_err(|_| RecoveryRequestError::StorageFailed)?;
    if live_active.is_some() {
        return Ok(true);
    }
    let any_history: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM users WHERE phone_lookup = encode(hmac(convert_to($1, 'UTF8'),
         convert_to($2, 'UTF8'), 'sha256'), 'hex') LIMIT 1",
    )
    .bind(canonical)
    .bind(lookup_key)
    .fetch_optional(pool)
    .await
    .map_err(|_| RecoveryRequestError::StorageFailed)?;
    Ok(any_history.is_some())
}

/// Delete one unused anchor after a provider failure or limit.
async fn delete_anchor(
    pool: &sqlx::PgPool,
    anchor: uuid::Uuid,
) -> Result<(), RecoveryRequestError> {
    sqlx::query("DELETE FROM phone_challenges WHERE id = $1")
        .bind(anchor)
        .execute(pool)
        .await
        .map_err(|_| RecoveryRequestError::StorageFailed)?;
    Ok(())
}

/// Confirm recovery: restore access or record review.
///
/// A verified active holder gets a fresh session; a verified recycled number
/// records a review fact on the deleted account's stream with no session and
/// no history access; everything else shares one generic refusal.
///
/// # Errors
///
/// Returns [`RecoveryError::InvalidInput`] for malformed input,
/// [`RecoveryError::ProviderFailed`] when the provider cannot be reached,
/// else [`RecoveryError::StorageFailed`]. Reasons are static.
pub async fn confirm_recovery<S: VerificationProvider>(
    pool: &sqlx::PgPool,
    provider: &S,
    phone: &str,
    code: &str,
    keys: PhoneKeys<'_>,
) -> Result<RecoveryOutcome, RecoveryError> {
    if code.trim().is_empty() || code.chars().count() > 32 {
        return Err(RecoveryError::InvalidInput);
    }
    let canonical = canonicalize_phone(phone).map_err(|_| RecoveryError::InvalidInput)?;
    if keys.lookup_key.is_empty() {
        return Err(RecoveryError::StorageFailed);
    }
    let live: Option<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT id, state FROM users WHERE phone_lookup = encode(hmac(convert_to($1, 'UTF8'),
         convert_to($2, 'UTF8'), 'sha256'), 'hex') AND deleted_at IS NULL",
    )
    .bind(&canonical)
    .bind(keys.lookup_key)
    .fetch_optional(pool)
    .await
    .map_err(|_| RecoveryError::StorageFailed)?;
    if let Some((_, state)) = live {
        if state != "active" {
            return Ok(RecoveryOutcome::Failed);
        }
        return match login_after_proof(pool, provider, &canonical, code, keys).await {
            Ok(LoginOutcome::Authenticated { token, user }) => Ok(RecoveryOutcome::Recovered {
                token,
                user_id: user.id,
            }),
            Ok(LoginOutcome::Failed) => Ok(RecoveryOutcome::Failed),
            Ok(LoginOutcome::Expired) => Ok(RecoveryOutcome::Expired),
            Ok(LoginOutcome::RateLimited) => Ok(RecoveryOutcome::RateLimited),
            Err(_) => Err(RecoveryError::StorageFailed),
        };
    }
    let deleted: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM users WHERE phone_lookup = encode(hmac(convert_to($1, 'UTF8'),
         convert_to($2, 'UTF8'), 'sha256'), 'hex') AND deleted_at IS NOT NULL
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&canonical)
    .bind(keys.lookup_key)
    .fetch_optional(pool)
    .await
    .map_err(|_| RecoveryError::StorageFailed)?;
    let deleted_id = match deleted {
        Some(id) => id,
        None => return Ok(RecoveryOutcome::Failed),
    };
    if !live_anchor_exists(pool, &canonical, keys.lookup_key).await? {
        if expired_anchor_exists(pool, &canonical, keys.lookup_key).await? {
            return Ok(RecoveryOutcome::Expired);
        }
        return Ok(RecoveryOutcome::Failed);
    }
    match provider.check_verification(&canonical, code).await {
        Ok(CheckOutcome::Verified) => {}
        Ok(CheckOutcome::Incorrect) => return Ok(RecoveryOutcome::Failed),
        Ok(CheckOutcome::RateLimited) => return Ok(RecoveryOutcome::RateLimited),
        Err(VerificationError::Timeout | VerificationError::ConnectionFailed) => {
            return Err(RecoveryError::ProviderFailed);
        }
        Err(_) => return Err(RecoveryError::StorageFailed),
    }
    let digest = lookup_digest(pool, &canonical, keys.lookup_key).await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| RecoveryError::StorageFailed)?;
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
    .map_err(|_| RecoveryError::StorageFailed)?;
    if consumed.is_none() {
        tx.rollback()
            .await
            .map_err(|_| RecoveryError::StorageFailed)?;
        return Ok(RecoveryOutcome::Expired);
    }
    record_event(
        &mut *tx,
        NewEvent {
            actor_id: None,
            resource_kind: "account",
            resource_id: deleted_id,
            cycle: None,
            revision: None,
            effective_at: chrono::Utc::now(),
            kind: "account.recycled_review",
            policy: "mvp-free",
            source: "api",
            payload: json!({"lookup": digest}),
        },
    )
    .await
    .map_err(|_| RecoveryError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| RecoveryError::StorageFailed)?;
    Ok(RecoveryOutcome::UnderReview)
}

/// Keyed lookup digest for payloads: opaque without the server-side key, so
/// review facts identify the number under review without storing it.
async fn lookup_digest(
    pool: &sqlx::PgPool,
    canonical: &str,
    lookup_key: &str,
) -> Result<String, RecoveryError> {
    sqlx::query_scalar(
        "SELECT encode(hmac(convert_to($1, 'UTF8'), convert_to($2, 'UTF8'), 'sha256'), 'hex')",
    )
    .bind(canonical)
    .bind(lookup_key)
    .fetch_one(pool)
    .await
    .map_err(|_| RecoveryError::StorageFailed)
}

/// Any live anchor for one lookup.
async fn live_anchor_exists(
    pool: &sqlx::PgPool,
    canonical: &str,
    lookup_key: &str,
) -> Result<bool, RecoveryError> {
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
    .map_err(|_| RecoveryError::StorageFailed)?;
    Ok(found.is_some())
}

/// Any elapsed, unconsumed anchor for one lookup.
async fn expired_anchor_exists(
    pool: &sqlx::PgPool,
    canonical: &str,
    lookup_key: &str,
) -> Result<bool, RecoveryError> {
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
    .map_err(|_| RecoveryError::StorageFailed)?;
    Ok(found.is_some())
}
