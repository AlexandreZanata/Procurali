//! Session lifecycle: login after proof, current-state authentication.
//!
//! Canonical rules: INV-04 (only `active` accounts act — every authentication
//! re-reads current state, so suspension, bans, and deletion take effect
//! immediately) and INV-05 (roles never authorize — callers supply no identity
//! or role; the session token resolves the account server-side).
//! Mechanism follows `docs/engineering/authentication.md`: opaque 256-bit
//! tokens persisted only as Argon2 digests, absolute maximum 7 days, idle
//! maximum 24 hours. Idle expiry slides `expires_at` forward on each
//! authenticated request without ever passing the absolute bound; both bounds
//! derive from the existing session columns, so no schema change is needed.
//! Login reuses the registration confirmation flow instead of duplicating it.

use crate::application::phone_verification::VerificationProvider;
use crate::application::register_user::{
    confirm_challenge, ConfirmationOutcome, RegistrationError,
};
use crate::persistence::users::{
    create_session, find_session_by_token, find_user_by_phone, revoke_session, PhoneKeys, User,
};

/// Idle session window: 24 hours of inactivity ends the session.
pub const SESSION_IDLE_WINDOW: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Absolute session bound: 7 days from creation, never extended.
pub const SESSION_ABSOLUTE_MAX: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

/// Outcome of logging in after proof. `Failed` stays generic (wrong code,
/// unknown number, ineligible holder); only confirmed control authenticates.
#[derive(Debug, Clone, PartialEq)]
pub enum LoginOutcome {
    /// Authenticated: single-use plaintext token plus the active account.
    /// The token is returned exactly once, for the login cookie.
    Authenticated {
        /// Opaque session token (plaintext, cookie-bound, never stored).
        token: String,
        /// The now-active account.
        user: User,
    },
    /// Proof failed: nothing was issued.
    Failed,
    /// The caller's own challenge window elapsed.
    Expired,
    /// The provider rate-limited the request.
    RateLimited,
}

/// A currently authenticated account: resolved server-side from the session
/// token, with a live (unexpired, unrevoked, within-bounds) session owned by
/// an `active` account. Carries no role and no caller-supplied identity.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthenticatedAccount {
    /// The active account behind the session.
    pub user: User,
    /// The session proving it.
    pub session_id: uuid::Uuid,
}

/// Fresh opaque token: 256 bits of randomness as lowercase hex.
fn mint_token() -> String {
    let bytes: [u8; 32] = rand::random();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Log in after phone proof: confirm control, then issue one session.
///
/// Successful confirmation (activation or idempotent replay) mints a session
/// whose idle expiry starts now; failures issue nothing.
///
/// # Errors
///
/// Returns [`RegistrationError::InvalidInput`] for malformed input,
/// [`RegistrationError::ProviderUnavailable`] when the provider cannot be
/// reached, else [`RegistrationError::StorageFailed`]. Reasons are static.
pub async fn login_after_proof<S: VerificationProvider>(
    pool: &sqlx::PgPool,
    provider: &S,
    phone: &str,
    code: &str,
    keys: PhoneKeys<'_>,
) -> Result<LoginOutcome, RegistrationError> {
    let confirmed = confirm_challenge(pool, provider, phone, code, keys).await?;
    match confirmed {
        ConfirmationOutcome::Activated | ConfirmationOutcome::AlreadyActive => {}
        ConfirmationOutcome::Failed => return Ok(LoginOutcome::Failed),
        ConfirmationOutcome::Expired => return Ok(LoginOutcome::Expired),
        ConfirmationOutcome::RateLimited => return Ok(LoginOutcome::RateLimited),
    }
    let canonical = crate::persistence::users::canonicalize_phone(phone)
        .map_err(|_| RegistrationError::InvalidInput)?;
    let user = find_user_by_phone(pool, &canonical, keys.lookup_key)
        .await
        .map_err(|_| RegistrationError::StorageFailed)?
        .filter(|user| user.state == "active")
        .ok_or(RegistrationError::StorageFailed)?;
    let token = mint_token();
    let expires_at = chrono::Utc::now()
        + chrono::Duration::from_std(SESSION_IDLE_WINDOW)
            .map_err(|_| RegistrationError::StorageFailed)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| RegistrationError::StorageFailed)?;
    create_session(&mut tx, user.id, &token, expires_at)
        .await
        .map_err(|_| RegistrationError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| RegistrationError::StorageFailed)?;
    Ok(LoginOutcome::Authenticated { token, user })
}

/// Authenticate one session token against current server state.
///
/// Resolves the token through stored digests, then re-reads the account:
/// revoked, expired, idle-timed-out, absolutely-aged-out, missing, or
/// non-`active` sessions all refuse with `None` indistinguishably. A live
/// session slides its idle expiry forward, capped at the absolute bound.
///
/// # Errors
///
/// Returns [`RegistrationError::StorageFailed`] on database failure only.
pub async fn authenticate(
    pool: &sqlx::PgPool,
    token: &str,
) -> Result<Option<AuthenticatedAccount>, RegistrationError> {
    let session = match find_session_by_token(pool, token)
        .await
        .map_err(|_| RegistrationError::StorageFailed)?
    {
        Some(session) => session,
        None => return Ok(None),
    };
    let user: Option<User> = {
        let row: Option<sqlx::postgres::PgRow> = sqlx::query(
            "SELECT id, display_name, city, region, state, policy_version,
                    policy_accepted_at, deleted_at, created_at
             FROM users WHERE id = $1",
        )
        .bind(session.user_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| RegistrationError::StorageFailed)?;
        row.map(|row| read_user(&row))
            .transpose()
            .map_err(|_| RegistrationError::StorageFailed)?
    }
    .filter(|user| user.state == "active");
    let user = match user {
        Some(user) => user,
        None => return Ok(None),
    };
    let now = chrono::Utc::now();
    let absolute_end = user.created_at
        + chrono::Duration::from_std(SESSION_ABSOLUTE_MAX)
            .map_err(|_| RegistrationError::StorageFailed)?;
    if now >= absolute_end {
        return Ok(None);
    }
    let idle_end = now
        + chrono::Duration::from_std(SESSION_IDLE_WINDOW)
            .map_err(|_| RegistrationError::StorageFailed)?;
    let slid_at = std::cmp::min(idle_end, absolute_end);
    sqlx::query("UPDATE sessions SET expires_at = $1 WHERE id = $2")
        .bind(slid_at)
        .bind(session.id)
        .execute(pool)
        .await
        .map_err(|_| RegistrationError::StorageFailed)?;
    Ok(Some(AuthenticatedAccount {
        user,
        session_id: session.id,
    }))
}

fn read_user(row: &sqlx::postgres::PgRow) -> Result<User, sqlx::Error> {
    use sqlx::Row;
    Ok(User {
        id: row.try_get("id")?,
        display_name: row.try_get("display_name")?,
        city: row.try_get("city")?,
        region: row.try_get("region")?,
        state: row.try_get("state")?,
        policy_version: row.try_get("policy_version")?,
        policy_accepted_at: row.try_get("policy_accepted_at")?,
        deleted_at: row.try_get("deleted_at")?,
        created_at: row.try_get("created_at")?,
    })
}

/// Revoke one session by id. Thin alias keeping revocation behind this module.
///
/// # Errors
///
/// Returns [`RegistrationError::StorageFailed`] on database failure only.
pub async fn revoke(
    pool: &sqlx::PgPool,
    session_id: uuid::Uuid,
) -> Result<bool, RegistrationError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| RegistrationError::StorageFailed)?;
    let revoked = revoke_session(&mut tx, session_id)
        .await
        .map_err(|_| RegistrationError::StorageFailed)?;
    tx.commit()
        .await
        .map_err(|_| RegistrationError::StorageFailed)?;
    Ok(revoked)
}
