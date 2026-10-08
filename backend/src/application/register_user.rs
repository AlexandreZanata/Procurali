//! Minimal registration and phone-proof verification flows.
//!
//! Canonical rules: INV-04 (new accounts start `pending` and cannot act until
//! a provider-confirmed code activates them), INV-07 (one current number per
//! non-deleted account — duplicates follow the generic recovery path, AC-03,
//! EC-22, without revealing another owner's history), AC-02 (minimum
//! registration: name, phone, city, approximate region, and policy acceptance
//! only). Mechanism follows `docs/engineering/authentication.md` and the
//! frozen route inventory (`contracts/openapi.yaml`).
//!
//! Endpoint split: registration creates the pending account (and its first
//! fact) without touching the provider; the challenge endpoint sends proof
//! requests for pending holders through the provider boundary and records a
//! local challenge anchor; confirmation activates exactly once on
//! provider-confirmed codes. Provider calls always happen outside database
//! transactions. Sessions and cookies arrive with the session lifecycle card;
//! abuse thresholds arrive with the abuse-controls card.

use crate::application::phone_verification::{
    CheckOutcome, StartOutcome, VerificationError, VerificationProvider,
};
use crate::persistence::events::{record as record_event, NewEvent};
use crate::persistence::users::{
    canonicalize_phone, create_challenge, create_user, find_user_by_phone, NewChallenge, NewUser,
    PhoneKeys, User, UserError,
};
use serde_json::json;

/// Challenge validity window (authentication protocol: 5 minutes).
pub const CHALLENGE_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);

/// Keyed phone-lookup SQL expression. The canonical definition lives in
/// `persistence::users` (unmodifiable from this card); this copy is identical
/// and covered by the same database behavior.
const PHONE_LOOKUP_SQL: &str =
    "encode(hmac(convert_to($1, 'UTF8'), convert_to($2, 'UTF8'), 'sha256'), 'hex')";

/// Typed registration/verification failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationError {
    /// A name, phone, locality, policy, or code field is missing or malformed.
    InvalidInput,
    /// The number already belongs to a non-deleted account (AC-03).
    DuplicatePhone,
    /// The provider could not be reached in time.
    ProviderUnavailable,
    /// The provider rate-limited the request.
    RateLimited,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput => f.write_str("invalid registration input"),
            Self::DuplicatePhone => f.write_str("phone number already registered"),
            Self::ProviderUnavailable => f.write_str("verification provider unavailable"),
            Self::RateLimited => f.write_str("verification rate limited"),
            Self::StorageFailed => f.write_str("registration storage failed"),
        }
    }
}

impl std::error::Error for RegistrationError {}

/// Outcome of requesting a challenge. Both variants answer the same generic
/// response: strangers cannot tell pending holders from anyone else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeDispatch {
    /// A challenge was sent and anchored for a pending holder.
    Sent,
    /// Nothing was sent (unknown number or ineligible holder); indistinguishable.
    NotSent,
}

/// Outcome of confirming a challenge. `Failed` covers wrong codes and unknown
/// numbers indistinguishably (anti-enumeration); `Expired` names only the
/// caller's own elapsed window; `AlreadyActive` replays a proven completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationOutcome {
    /// The account activated on this call.
    Activated,
    /// The account is active and the provider confirmed the retried code again.
    AlreadyActive,
    /// Wrong code, unknown number, or ineligible holder: never activates.
    Failed,
    /// The caller's own challenge window elapsed.
    Expired,
    /// The provider rate-limited the request.
    RateLimited,
}

fn account_event(user_id: uuid::Uuid, kind: &'static str, state: &'static str) -> NewEvent {
    NewEvent {
        actor_id: Some(user_id),
        resource_kind: "account",
        resource_id: user_id,
        cycle: None,
        revision: None,
        effective_at: chrono::Utc::now(),
        kind,
        policy: "mvp-free",
        source: "api",
        payload: json!({"state": state}),
    }
}

/// Register one minimal pending account.
///
/// Validates only the five registration fields, persists the pending account
/// with its first fact, and never calls the provider (challenge sending is the
/// challenges endpoint). Concurrent duplicates collapse to
/// [`RegistrationError::DuplicatePhone`] with exactly one account committed.
///
/// # Errors
///
/// Returns [`RegistrationError::InvalidInput`] for malformed fields,
/// [`RegistrationError::DuplicatePhone`] for held numbers, else
/// [`RegistrationError::StorageFailed`]. Reasons are static.
pub async fn register_account(
    pool: &sqlx::PgPool,
    input: NewUser,
    keys: PhoneKeys<'_>,
) -> Result<User, RegistrationError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| RegistrationError::StorageFailed)?;
    let user = create_user(&mut tx, input, keys)
        .await
        .map_err(|error| match error {
            UserError::InvalidPhone | UserError::InvalidProfile => RegistrationError::InvalidInput,
            UserError::DuplicatePhone => RegistrationError::DuplicatePhone,
            UserError::InvalidSecret | UserError::StorageFailed => RegistrationError::StorageFailed,
        })?;
    record_event(
        &mut *tx,
        account_event(user.id, "account.registered", "pending"),
    )
    .await
    .map_err(|_| RegistrationError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| RegistrationError::StorageFailed)?;
    Ok(user)
}

/// Marker digest input for one local challenge anchor: fresh randomness whose
/// digest proves nothing and matches no submitted code. Code secrecy stays
/// with the provider; the local row anchors window, single use, and rate data.
fn challenge_marker() -> String {
    let nonce: [u8; 16] = rand::random();
    nonce.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Request a verification challenge for one phone.
///
/// Pending holders get a provider challenge plus a local anchor; everyone else
/// (unknown numbers, active, restricted, or deleted holders) gets the same
/// generic outcome with zero side effects and no provider call.
///
/// # Errors
///
/// Returns [`RegistrationError::InvalidInput`] for malformed phones,
/// [`RegistrationError::ProviderUnavailable`] when the provider cannot be
/// reached, [`RegistrationError::RateLimited`] when limited, else
/// [`RegistrationError::StorageFailed`]. Reasons are static.
pub async fn start_challenge<S: VerificationProvider>(
    pool: &sqlx::PgPool,
    provider: &S,
    phone: &str,
    keys: PhoneKeys<'_>,
    window: std::time::Duration,
) -> Result<ChallengeDispatch, RegistrationError> {
    let canonical = canonicalize_phone(phone).map_err(|_| RegistrationError::InvalidInput)?;
    let holder = find_user_by_phone(pool, &canonical, keys.lookup_key)
        .await
        .map_err(|error| match error {
            UserError::InvalidPhone => RegistrationError::InvalidInput,
            _ => RegistrationError::StorageFailed,
        })?;
    let pending = holder.is_some_and(|user| user.state == "pending");
    if !pending {
        return Ok(ChallengeDispatch::NotSent);
    }
    match provider.start_verification(&canonical).await {
        Ok(StartOutcome::ChallengeSent) => {}
        Ok(StartOutcome::RateLimited) => return Err(RegistrationError::RateLimited),
        Err(VerificationError::Timeout | VerificationError::ConnectionFailed) => {
            return Err(RegistrationError::ProviderUnavailable);
        }
        Err(_) => return Err(RegistrationError::StorageFailed),
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| RegistrationError::StorageFailed)?;
    let expires_at = chrono::Utc::now()
        + chrono::Duration::from_std(window).map_err(|_| RegistrationError::StorageFailed)?;
    create_challenge(
        &mut tx,
        NewChallenge {
            phone: canonical,
            code: challenge_marker(),
            expires_at,
        },
        keys.lookup_key,
    )
    .await
    .map_err(|_| RegistrationError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| RegistrationError::StorageFailed)?;
    Ok(ChallengeDispatch::Sent)
}

/// Any live (unexpired, unconsumed) challenge for one lookup.
async fn live_challenge_exists(
    pool: &sqlx::PgPool,
    canonical: &str,
    lookup_key: &str,
) -> Result<bool, RegistrationError> {
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
    .map_err(|_| RegistrationError::StorageFailed)?;
    Ok(found.is_some())
}

/// Any elapsed, unconsumed challenge for one lookup (the caller's own window).
async fn expired_challenge_exists(
    pool: &sqlx::PgPool,
    canonical: &str,
    lookup_key: &str,
) -> Result<bool, RegistrationError> {
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
    .map_err(|_| RegistrationError::StorageFailed)?;
    Ok(found.is_some())
}

/// Consume the newest live challenge and activate one pending account.
///
/// Returns `true` when exactly one account activated; `false` when the window
/// closed underneath the provider confirmation (the retry then replays).
async fn activate_pending(
    pool: &sqlx::PgPool,
    user_id: uuid::Uuid,
    canonical: &str,
    lookup_key: &str,
) -> Result<bool, RegistrationError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| RegistrationError::StorageFailed)?;
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
    .bind(canonical)
    .bind(lookup_key)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| RegistrationError::StorageFailed)?;
    if consumed.is_none() {
        tx.rollback()
            .await
            .map_err(|_| RegistrationError::StorageFailed)?;
        return Ok(false);
    }
    let activated: u64 =
        sqlx::query("UPDATE users SET state = 'active' WHERE id = $1 AND state = 'pending'")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| RegistrationError::StorageFailed)?
            .rows_affected();
    if activated != 1 {
        tx.rollback()
            .await
            .map_err(|_| RegistrationError::StorageFailed)?;
        return Ok(false);
    }
    record_event(
        &mut *tx,
        account_event(user_id, "account.activated", "active"),
    )
    .await
    .map_err(|_| RegistrationError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| RegistrationError::StorageFailed)?;
    Ok(true)
}

/// Confirm phone control and activate the pending account.
///
/// Wrong codes, unknown numbers, and ineligible holders share one generic
/// refusal that never activates; an elapsed window reports expiry; a proven
/// retry of an active account replays success without duplicating facts. The
/// provider is consulted only while a live local challenge exists (pending
/// holders) or to settle retries of active ones.
///
/// # Errors
///
/// Returns [`RegistrationError::InvalidInput`] for malformed input,
/// [`RegistrationError::ProviderUnavailable`] when the provider cannot be
/// reached, [`RegistrationError::RateLimited`] when limited, else
/// [`RegistrationError::StorageFailed`]. Reasons are static.
pub async fn confirm_challenge<S: VerificationProvider>(
    pool: &sqlx::PgPool,
    provider: &S,
    phone: &str,
    code: &str,
    keys: PhoneKeys<'_>,
) -> Result<ConfirmationOutcome, RegistrationError> {
    if code.trim().is_empty() || code.chars().count() > 32 {
        return Err(RegistrationError::InvalidInput);
    }
    let canonical = canonicalize_phone(phone).map_err(|_| RegistrationError::InvalidInput)?;
    let holder = find_user_by_phone(pool, &canonical, keys.lookup_key)
        .await
        .map_err(|_| RegistrationError::StorageFailed)?;
    let user = match holder {
        Some(user) if user.state == "pending" => user,
        Some(user) if user.state == "active" => {
            // Idempotent completion stays provider-authoritative: the local
            // anchor holds no code, so only a fresh confirmation proves the
            // retry carries the proven code.
            match provider.check_verification(&canonical, code).await {
                Ok(CheckOutcome::Verified) => return Ok(ConfirmationOutcome::AlreadyActive),
                Ok(CheckOutcome::Incorrect) => return Ok(ConfirmationOutcome::Failed),
                Ok(CheckOutcome::RateLimited) => return Ok(ConfirmationOutcome::RateLimited),
                Err(VerificationError::Timeout | VerificationError::ConnectionFailed) => {
                    return Err(RegistrationError::ProviderUnavailable);
                }
                Err(_) => return Err(RegistrationError::StorageFailed),
            }
        }
        _ => return Ok(ConfirmationOutcome::Failed),
    };
    let live = live_challenge_exists(pool, &canonical, keys.lookup_key).await?;
    if !live {
        if expired_challenge_exists(pool, &canonical, keys.lookup_key).await? {
            return Ok(ConfirmationOutcome::Expired);
        }
        return Ok(ConfirmationOutcome::Failed);
    }
    match provider.check_verification(&canonical, code).await {
        Ok(CheckOutcome::Verified) => {}
        Ok(CheckOutcome::Incorrect) => return Ok(ConfirmationOutcome::Failed),
        Ok(CheckOutcome::RateLimited) => return Ok(ConfirmationOutcome::RateLimited),
        Err(VerificationError::Timeout | VerificationError::ConnectionFailed) => {
            return Err(RegistrationError::ProviderUnavailable);
        }
        Err(_) => return Err(RegistrationError::StorageFailed),
    }
    if activate_pending(pool, user.id, &canonical, keys.lookup_key).await? {
        Ok(ConfirmationOutcome::Activated)
    } else {
        // The window closed under the provider confirmation, or a concurrent
        // confirmation won first: re-reading decides between replay and expiry.
        let current = find_user_by_phone(pool, &canonical, keys.lookup_key)
            .await
            .map_err(|_| RegistrationError::StorageFailed)?;
        match current {
            Some(user) if user.state == "active" => Ok(ConfirmationOutcome::AlreadyActive),
            Some(user) if user.state == "pending" => Ok(ConfirmationOutcome::Expired),
            _ => Ok(ConfirmationOutcome::Failed),
        }
    }
}
