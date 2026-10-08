//! Bounded verification abuse controls: guarded challenge flows.
//!
//! Canonical rules: INV-07 (one current number per non-deleted account —
//! limits never create a second holder), INV-36 (raw report volume alone
//! cannot ban — likewise, request volume alone never bans an account; limits
//! only refuse single attempts), AC-03 (duplicate or abusive requests get a
//! generic refusal plus a recovery offer, never another owner's history).
//! Thresholds follow `docs/engineering/authentication.md` and
//! `contracts/auth-cases.json`: 5 challenge starts per phone per hour,
//! 60-second resend minimum, 5 confirmation attempts per challenge. These are
//! technical SMS/code-guessing bounds, disjoint from marketplace quotas.
//!
//! The guarded flows below make the decision and the reservation atomically
//! in PostgreSQL with no schema change: challenge starts serialize per phone
//! on a transaction advisory lock, check the hourly count and the resend
//! window, and insert the challenge anchor inside the same transaction — so
//! the last allowed concurrent attempt has only permitted winners. Provider
//! calls always happen after the reservation commits; a provider failure or
//! limit deletes the unused anchor, so failed starts consume no allowance and
//! leave zero rows (explicit accounting). Confirmation attempts increment
//! atomically (`UPDATE ... RETURNING`), so over-attempts are refused without
//! any extra provider call.
//!
//! Client-IP resolution follows the trusted-proxy contract: only a trusted
//! deployment reads `X-Forwarded-For` (leftmost entry); otherwise the peer
//! address is authoritative. Per-IP counting needs its own ledger and lands
//! with the card that owns the next migration.

use std::time::Duration;

use crate::application::phone_verification::{
    CheckOutcome, StartOutcome, VerificationError, VerificationProvider,
};
use crate::persistence::users::canonicalize_phone;

/// Technical abuse thresholds. Defaults mirror the frozen contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbuseLimits {
    /// Challenge starts allowed per phone per rolling hour.
    pub max_starts_per_hour: u32,
    /// Minimum seconds between two challenge sends to one phone.
    pub resend_minimum_secs: u64,
    /// Confirmation attempts allowed per challenge; the next is refused.
    pub max_attempts_per_challenge: u32,
}

impl AbuseLimits {
    /// Contract thresholds: 5 starts/hour, 60s resend gap, 5 attempts.
    pub const DEFAULT: Self = Self {
        max_starts_per_hour: 5,
        resend_minimum_secs: 60,
        max_attempts_per_challenge: 5,
    };
}

/// Keyed phone-lookup SQL expression. The canonical definition lives in
/// `persistence::users` (unmodifiable from this card); this copy is identical.
const PHONE_LOOKUP_SQL: &str =
    "encode(hmac(convert_to($1, 'UTF8'), convert_to($2, 'UTF8'), 'sha256'), 'hex')";

/// Outcome of a guarded challenge request. `NotEligible` answers unknown
/// numbers and ineligible holders indistinguishably with zero side effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardedStart {
    /// A challenge was sent and anchored.
    Sent,
    /// The hourly or resend bound refused this request; no provider call ran.
    RateLimited,
    /// Nothing was sent and nothing was recorded.
    NotEligible,
}

/// Failure of a guarded challenge request. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartGuardError {
    /// The phone is missing or malformed.
    InvalidInput,
    /// The provider could not be reached in time.
    ProviderFailed,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for StartGuardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput => f.write_str("invalid challenge request"),
            Self::ProviderFailed => f.write_str("verification provider unavailable"),
            Self::StorageFailed => f.write_str("challenge storage failed"),
        }
    }
}

impl std::error::Error for StartGuardError {}

/// Outcome of a guarded code check. `NoChallenge` covers unknown numbers and
/// unconsumed-but-gone anchors indistinguishably; `Expired` names only the
/// caller's own elapsed window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardedCheck {
    /// The provider confirmed the code.
    Verified,
    /// The code did not match.
    Incorrect,
    /// The attempt bound refused this check; no provider call ran.
    RateLimited,
    /// The caller's own window elapsed.
    Expired,
    /// No live challenge exists for this phone.
    NoChallenge,
}

/// Failure of a guarded code check. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckGuardError {
    /// The phone or code is missing or malformed.
    InvalidInput,
    /// The provider could not be reached in time.
    ProviderFailed,
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for CheckGuardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput => f.write_str("invalid confirmation"),
            Self::ProviderFailed => f.write_str("verification provider unavailable"),
            Self::StorageFailed => f.write_str("confirmation storage failed"),
        }
    }
}

impl std::error::Error for CheckGuardError {}

/// Marker digest input for one guarded anchor: fresh randomness whose digest
/// proves nothing and matches no submitted code. See `register_user` for the
/// same convention.
fn challenge_marker() -> String {
    let nonce: [u8; 16] = rand::random();
    nonce.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Resolve the client IP under the trusted-proxy contract: the leftmost
/// `X-Forwarded-For` entry only when the proxy is trusted, else the peer
/// address. Pure and total; per-IP counting is follow-through work.
#[must_use]
pub fn client_ip(forwarded_for: Option<&str>, peer_address: &str, trust_proxy: bool) -> String {
    if trust_proxy {
        if let Some(header) = forwarded_for {
            if let Some(first) = header.split(',').next() {
                let trimmed = first.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_owned();
                }
            }
        }
    }
    peer_address.to_owned()
}

/// Request one challenge behind hourly, resend, and eligibility guards.
///
/// Pending holders inside the bounds get a provider challenge plus a local
/// anchor; over-bound requests are refused before any provider call; anyone
/// else gets the indistinguishable outcome with zero side effects. Provider
/// failures delete the unused anchor, consuming no allowance.
///
/// # Errors
///
/// Returns [`StartGuardError::InvalidInput`] for malformed phones,
/// [`StartGuardError::ProviderFailed`] when the provider cannot be reached,
/// else [`StartGuardError::StorageFailed`]. Reasons are static.
pub async fn request_challenge<S: VerificationProvider>(
    pool: &sqlx::PgPool,
    inner: &S,
    phone: &str,
    lookup_key: &str,
    window: Duration,
    limits: AbuseLimits,
) -> Result<GuardedStart, StartGuardError> {
    let canonical = canonicalize_phone(phone).map_err(|_| StartGuardError::InvalidInput)?;
    if lookup_key.is_empty() {
        return Err(StartGuardError::StorageFailed);
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| StartGuardError::StorageFailed)?;
    // Serialize per-phone decisions: the count, the resend check, and the
    // anchor insert below commit as one unit.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(&canonical)
        .execute(&mut *tx)
        .await
        .map_err(|_| StartGuardError::StorageFailed)?;
    let pending: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM users WHERE phone_lookup = encode(hmac(convert_to($1, 'UTF8'),
         convert_to($2, 'UTF8'), 'sha256'), 'hex') AND deleted_at IS NULL AND state = 'pending'",
    )
    .bind(&canonical)
    .bind(lookup_key)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| StartGuardError::StorageFailed)?;
    if pending.is_none() {
        tx.rollback()
            .await
            .map_err(|_| StartGuardError::StorageFailed)?;
        return Ok(GuardedStart::NotEligible);
    }
    let starts_last_hour: i64 = sqlx::query_scalar(
        &[
            r#"SELECT count(*) FROM phone_challenges WHERE phone_lookup = "#,
            PHONE_LOOKUP_SQL,
            " AND created_at > now() - interval '1 hour'",
        ]
        .concat(),
    )
    .bind(&canonical)
    .bind(lookup_key)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| StartGuardError::StorageFailed)?;
    if starts_last_hour >= i64::from(limits.max_starts_per_hour) {
        tx.rollback()
            .await
            .map_err(|_| StartGuardError::StorageFailed)?;
        return Ok(GuardedStart::RateLimited);
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
    .bind(lookup_key)
    .bind(limits.resend_minimum_secs as f64)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| StartGuardError::StorageFailed)?;
    if recent.is_some() {
        tx.rollback()
            .await
            .map_err(|_| StartGuardError::StorageFailed)?;
        return Ok(GuardedStart::RateLimited);
    }
    let anchor: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO phone_challenges (phone_lookup, challenge_digest, expires_at)
         VALUES (encode(hmac(convert_to($1, 'UTF8'), convert_to($2, 'UTF8'), 'sha256'), 'hex'),
                 encode(digest(convert_to($3, 'UTF8'), 'sha256'), 'hex'), $4)
         RETURNING id",
    )
    .bind(&canonical)
    .bind(lookup_key)
    .bind(challenge_marker())
    .bind(
        chrono::Utc::now()
            + chrono::Duration::from_std(window).map_err(|_| StartGuardError::StorageFailed)?,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| StartGuardError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| StartGuardError::StorageFailed)?;
    match inner.start_verification(&canonical).await {
        Ok(StartOutcome::ChallengeSent) => Ok(GuardedStart::Sent),
        Ok(StartOutcome::RateLimited) | Err(VerificationError::ProviderRejected) => {
            delete_anchor(pool, anchor).await?;
            Ok(GuardedStart::RateLimited)
        }
        Err(VerificationError::Timeout | VerificationError::ConnectionFailed) => {
            delete_anchor(pool, anchor).await?;
            Err(StartGuardError::ProviderFailed)
        }
        Err(_) => {
            delete_anchor(pool, anchor).await?;
            Err(StartGuardError::StorageFailed)
        }
    }
}

/// Delete one unused anchor after a provider failure or limit. Best effort
/// with a loud failure: callers must know accounting did not complete.
async fn delete_anchor(pool: &sqlx::PgPool, anchor: uuid::Uuid) -> Result<(), StartGuardError> {
    sqlx::query("DELETE FROM phone_challenges WHERE id = $1")
        .bind(anchor)
        .execute(pool)
        .await
        .map_err(|_| StartGuardError::StorageFailed)?;
    Ok(())
}

/// Check one code behind the per-challenge attempt bound.
///
/// The attempt counter increments atomically before consulting the provider:
/// the first `max_attempts_per_challenge` attempts delegate, later ones are
/// refused with no provider call. Unknown or anchor-less phones answer
/// without any provider call; an elapsed caller window reports expiry.
///
/// # Errors
///
/// Returns [`CheckGuardError::InvalidInput`] for malformed input,
/// [`CheckGuardError::ProviderFailed`] when the provider cannot be reached,
/// else [`CheckGuardError::StorageFailed`]. Reasons are static.
pub async fn check_code<S: VerificationProvider>(
    pool: &sqlx::PgPool,
    inner: &S,
    phone: &str,
    code: &str,
    lookup_key: &str,
    limits: AbuseLimits,
) -> Result<GuardedCheck, CheckGuardError> {
    if code.trim().is_empty() || code.chars().count() > 32 {
        return Err(CheckGuardError::InvalidInput);
    }
    let canonical = canonicalize_phone(phone).map_err(|_| CheckGuardError::InvalidInput)?;
    if lookup_key.is_empty() {
        return Err(CheckGuardError::StorageFailed);
    }
    // Atomic increment-then-decide on the newest live challenge: concurrent
    // attempts serialize here, so only the first N delegate to the provider.
    let attempt: Option<i32> = sqlx::query_scalar(
        &[
            r#"UPDATE phone_challenges SET attempts = attempts + 1 WHERE id = (
                 SELECT id FROM phone_challenges WHERE phone_lookup = "#,
            PHONE_LOOKUP_SQL,
            " AND consumed_at IS NULL AND expires_at > now()
                 ORDER BY created_at DESC LIMIT 1)
               RETURNING attempts",
        ]
        .concat(),
    )
    .bind(&canonical)
    .bind(lookup_key)
    .fetch_optional(pool)
    .await
    .map_err(|_| CheckGuardError::StorageFailed)?;
    let attempt = match attempt {
        Some(attempt) => attempt,
        None => {
            let expired: Option<i32> = sqlx::query_scalar(
                &[
                    r#"SELECT 1 FROM phone_challenges WHERE phone_lookup = "#,
                    PHONE_LOOKUP_SQL,
                    " AND consumed_at IS NULL AND expires_at <= now() LIMIT 1",
                ]
                .concat(),
            )
            .bind(&canonical)
            .bind(lookup_key)
            .fetch_optional(pool)
            .await
            .map_err(|_| CheckGuardError::StorageFailed)?;
            if expired.is_some() {
                return Ok(GuardedCheck::Expired);
            }
            return Ok(GuardedCheck::NoChallenge);
        }
    };
    if attempt > limits.max_attempts_per_challenge as i32 {
        return Ok(GuardedCheck::RateLimited);
    }
    match inner.check_verification(&canonical, code).await {
        Ok(CheckOutcome::Verified) => Ok(GuardedCheck::Verified),
        Ok(CheckOutcome::Incorrect) => Ok(GuardedCheck::Incorrect),
        Ok(CheckOutcome::RateLimited) => Ok(GuardedCheck::RateLimited),
        Err(VerificationError::Timeout | VerificationError::ConnectionFailed) => {
            Err(CheckGuardError::ProviderFailed)
        }
        Err(_) => Err(CheckGuardError::StorageFailed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_thresholds_match_the_frozen_contract() {
        assert_eq!(AbuseLimits::DEFAULT.max_starts_per_hour, 5);
        assert_eq!(AbuseLimits::DEFAULT.resend_minimum_secs, 60);
        assert_eq!(AbuseLimits::DEFAULT.max_attempts_per_challenge, 5);
    }

    #[test]
    fn client_ip_trusts_only_configured_proxies() {
        assert_eq!(
            client_ip(Some("203.0.113.7, 10.0.0.1"), "10.0.0.9", true),
            "203.0.113.7"
        );
        assert_eq!(client_ip(None, "10.0.0.9", true), "10.0.0.9");
        assert_eq!(
            client_ip(Some("203.0.113.7"), "10.0.0.9", false),
            "10.0.0.9"
        );
        assert_eq!(client_ip(Some(""), "10.0.0.9", true), "10.0.0.9");
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            StartGuardError::InvalidInput,
            StartGuardError::ProviderFailed,
            StartGuardError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
            assert!(!rendered.contains("+55"));
        }
        for error in [
            CheckGuardError::InvalidInput,
            CheckGuardError::ProviderFailed,
            CheckGuardError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
            assert!(!rendered.contains("+55"));
        }
    }
}
