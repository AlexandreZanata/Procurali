//! Account identity persistence: users, credential digests, challenges, blocks.
//!
//! Canonical rules: INV-04 (restricted accounts cannot act — new accounts start
//! `pending`), INV-07 (one current number per non-deleted account, AC-03/EC-22
//! refusal without revealing another owner's history), INV-31 (no phone in
//! public surfaces — phones persist only as ciphertext plus a keyed lookup).
//! Mechanism follows `docs/engineering/authentication.md`: opaque random
//! session tokens and challenge codes persist only as Argon2 digests; the
//! canonical phone persists as authenticated ciphertext with a keyed
//! HMAC-SHA256 lookup (pgcrypto, vetted primitives). Keys travel per call and
//! are never stored.
//!
//! All writes belong in the caller's transaction: [`create_user`] checks for a
//! live holder and inserts with `ON CONFLICT DO NOTHING` so a plain failed
//! INSERT never aborts the transaction (25P02); the loser re-reads and reports
//! [`UserError::DuplicatePhone`]. Deletion stamps `deleted_at`, keeping the
//! identifier distinct from a later new account while releasing the number for
//! reviewed recycling. [`Session`] and challenge types carry no plaintext:
//! tokens and codes exist only in memory and in the caller's arguments.
//!
//! Phone shaping here is minimal mechanical canonicalization (visual
//! separators removed, international format required). Full parsing with a
//! maintained validator lands with the registration flow in its owning card.

use argon2::{password_hash::phc::PasswordHash, Argon2, PasswordHasher, PasswordVerifier};

/// Display-name bound in scalar values (mirrors the database backstop).
pub const DISPLAY_NAME_MAX_CHARS: usize = 80;

/// City bound in scalar values (mirrors the database backstop).
pub const CITY_MAX_CHARS: usize = 120;

/// Region bound in scalar values (mirrors the database backstop).
pub const REGION_MAX_CHARS: usize = 40;

/// Policy version bound in scalar values (mirrors the database backstop).
pub const POLICY_VERSION_MAX_CHARS: usize = 32;

/// Token/code bound in scalar values: non-empty and short by construction.
pub const SECRET_MAX_CHARS: usize = 512;

/// Per-call phone keys. Both stay outside the database; neither is logged.
#[derive(Debug, Clone, Copy)]
pub struct PhoneKeys<'a> {
    /// Key for the deterministic HMAC-SHA256 phone lookup.
    pub lookup_key: &'a str,
    /// Key for the authenticated phone ciphertext.
    pub encryption_key: &'a str,
}

/// A new account: profile, locality, policy acceptance, and a phone in any
/// common formatting. The account starts `pending` (INV-04); activation
/// arrives in the verification flow.
#[derive(Debug, Clone)]
pub struct NewUser {
    /// Display name, 1..=80 scalar values.
    pub display_name: String,
    /// City label, 1..=120 scalar values.
    pub city: String,
    /// Region label, 1..=40 scalar values.
    pub region: String,
    /// Accepted rules version, 1..=32 scalar values.
    pub policy_version: String,
    /// Instant the rules were accepted.
    pub policy_accepted_at: chrono::DateTime<chrono::Utc>,
    /// Phone in any common formatting; canonicalized before storage.
    pub phone: String,
}

/// A persisted account. Phone material is deliberately absent: ciphertext and
/// lookup are reachable only through dedicated functions with explicit keys.
#[derive(Debug, Clone, PartialEq)]
pub struct User {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Display name, exactly as supplied.
    pub display_name: String,
    /// City label.
    pub city: String,
    /// Region label.
    pub region: String,
    /// Lifecycle state (`pending`, `active`, `suspended`, `banned`, `deleted`).
    pub state: String,
    /// Accepted rules version.
    pub policy_version: String,
    /// Rules acceptance instant.
    pub policy_accepted_at: chrono::DateTime<chrono::Utc>,
    /// Deletion instant, if the account was deleted.
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Recording instant (database clock).
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// A persisted session. The digest is deliberately absent: only the
/// plaintext token holder can be verified, never read back.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// Server-generated version-7 identifier.
    pub id: uuid::Uuid,
    /// Owning account.
    pub user_id: uuid::Uuid,
    /// Expiry instant (absolute bound).
    pub expires_at: chrono::DateTime<chrono::Utc>,
    /// Revocation instant, if revoked.
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Recording instant (database clock).
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Typed identity-storage failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserError {
    /// The phone is missing, malformed, or outside E.164 shape.
    InvalidPhone,
    /// A profile, locality, or policy field is missing or out of bounds.
    InvalidProfile,
    /// The number already belongs to a non-deleted account (INV-07, AC-03).
    DuplicatePhone,
    /// The token or code is missing or out of bounds.
    InvalidSecret,
    /// The lookup or write failed (missing row, connection, transaction state).
    StorageFailed,
}

impl std::fmt::Display for UserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPhone => f.write_str("invalid phone number"),
            Self::InvalidProfile => f.write_str("invalid profile field"),
            Self::DuplicatePhone => {
                f.write_str("phone number already registered, recovery is available")
            }
            Self::InvalidSecret => f.write_str("invalid credential"),
            Self::StorageFailed => f.write_str("identity storage failed"),
        }
    }
}

impl std::error::Error for UserError {}

/// Shape one phone input into canonical E.164 form.
///
/// Removes common visual separators (spaces, dashes, parentheses, dots),
/// requires a leading `+` with 8..=15 digits after it, and returns the
/// canonical value. Everything else is refused without further detail.
///
/// # Errors
///
/// Returns [`UserError::InvalidPhone`] for missing or malformed input.
pub fn canonicalize_phone(input: &str) -> Result<String, UserError> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 32 {
        return Err(UserError::InvalidPhone);
    }
    let stripped: String = trimmed
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '(' | ')' | '.'))
        .collect();
    let digits = stripped.strip_prefix('+').ok_or(UserError::InvalidPhone)?;
    if !(8..=15).contains(&digits.len()) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(UserError::InvalidPhone);
    }
    Ok(format!("+{digits}"))
}

fn check_profile(user: &NewUser) -> Result<(), UserError> {
    let bounded =
        |value: &str, max: usize| !value.trim().is_empty() && value.chars().count() <= max;
    if !bounded(&user.display_name, DISPLAY_NAME_MAX_CHARS)
        || !bounded(&user.city, CITY_MAX_CHARS)
        || !bounded(&user.region, REGION_MAX_CHARS)
        || !bounded(&user.policy_version, POLICY_VERSION_MAX_CHARS)
    {
        return Err(UserError::InvalidProfile);
    }
    Ok(())
}

fn check_secret(secret: &str) -> Result<(), UserError> {
    if secret.is_empty() || secret.chars().count() > SECRET_MAX_CHARS {
        return Err(UserError::InvalidSecret);
    }
    Ok(())
}

/// Argon2 digest of one token or code with a fresh random salt.
///
/// # Errors
///
/// Returns [`UserError::StorageFailed`] when the digest cannot be computed.
/// The plaintext never appears in the error.
fn hash_secret(secret: &str) -> Result<String, UserError> {
    let salt_bytes: [u8; 16] = rand::random();
    Argon2::default()
        .hash_password_with_salt(secret.as_bytes(), &salt_bytes)
        .map(|hash| hash.to_string())
        .map_err(|_| UserError::StorageFailed)
}

/// True when `secret` verifies against the stored Argon2 `digest`.
fn verify_secret(secret: &str, digest: &str) -> bool {
    let parsed = match PasswordHash::new(digest) {
        Ok(parsed) => parsed,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(secret.as_bytes(), &parsed)
        .is_ok()
}

fn read_user(row: &sqlx::postgres::PgRow) -> Result<User, UserError> {
    use sqlx::Row;
    Ok(User {
        id: row.try_get("id").map_err(|_| UserError::StorageFailed)?,
        display_name: row
            .try_get("display_name")
            .map_err(|_| UserError::StorageFailed)?,
        city: row.try_get("city").map_err(|_| UserError::StorageFailed)?,
        region: row
            .try_get("region")
            .map_err(|_| UserError::StorageFailed)?,
        state: row.try_get("state").map_err(|_| UserError::StorageFailed)?,
        policy_version: row
            .try_get("policy_version")
            .map_err(|_| UserError::StorageFailed)?,
        policy_accepted_at: row
            .try_get("policy_accepted_at")
            .map_err(|_| UserError::StorageFailed)?,
        deleted_at: row
            .try_get("deleted_at")
            .map_err(|_| UserError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| UserError::StorageFailed)?,
    })
}

/// SQL expression computing the keyed phone lookup (deterministic HMAC-SHA256).
const PHONE_LOOKUP_EXPR: &str =
    "encode(hmac(convert_to($1, 'UTF8'), convert_to($2, 'UTF8'), 'sha256'), 'hex')";

/// Create one `pending` account inside the caller's transaction.
///
/// Fails with [`UserError::DuplicatePhone`] when the number already belongs to
/// a non-deleted account — checked first by the application and enforced again
/// by the partial unique index, so the database and the application agree.
///
/// Runtime-checked SQL keeps `cargo build` offline-capable.
///
/// # Errors
///
/// Returns [`UserError::InvalidPhone`]/[`UserError::InvalidProfile`] before any
/// write, [`UserError::DuplicatePhone`] for a held number, else
/// [`UserError::StorageFailed`]. Reasons are static.
pub async fn create_user(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user: NewUser,
    keys: PhoneKeys<'_>,
) -> Result<User, UserError> {
    check_profile(&user)?;
    let canonical = canonicalize_phone(&user.phone)?;
    if keys.lookup_key.is_empty() || keys.encryption_key.is_empty() {
        return Err(UserError::StorageFailed);
    }
    if find_user_by_phone(&mut **tx, &canonical, keys.lookup_key)
        .await?
        .is_some()
    {
        return Err(UserError::DuplicatePhone);
    }
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(
        r#"INSERT INTO users
            (display_name, city, region, policy_version, policy_accepted_at,
             phone_ciphertext, phone_lookup)
           VALUES ($1, $2, $3, $4, $5,
                   pgp_sym_encrypt($6, $7),
                   encode(hmac(convert_to($6, 'UTF8'), convert_to($8, 'UTF8'), 'sha256'), 'hex'))
           ON CONFLICT (phone_lookup) WHERE deleted_at IS NULL DO NOTHING
           RETURNING id, display_name, city, region, state, policy_version,
                     policy_accepted_at, deleted_at, created_at"#,
    )
    .bind(&user.display_name)
    .bind(&user.city)
    .bind(&user.region)
    .bind(&user.policy_version)
    .bind(user.policy_accepted_at)
    .bind(&canonical)
    .bind(keys.encryption_key)
    .bind(keys.lookup_key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| UserError::StorageFailed)?;
    match row {
        Some(row) => read_user(&row),
        // A concurrent transaction won the number after our check: the index
        // refused us without aborting our transaction.
        None => Err(UserError::DuplicatePhone),
    }
}

/// Find the live (non-deleted) account holding `phone`, if any.
///
/// Accepts any common formatting; deleted holders are invisible here.
///
/// # Errors
///
/// Returns [`UserError::InvalidPhone`] for malformed input, else
/// [`UserError::StorageFailed`]. Reasons are static.
pub async fn find_user_by_phone<'e, E>(
    executor: E,
    phone: &str,
    lookup_key: &str,
) -> Result<Option<User>, UserError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let canonical = canonicalize_phone(phone)?;
    if lookup_key.is_empty() {
        return Err(UserError::StorageFailed);
    }
    let row: Option<sqlx::postgres::PgRow> = sqlx::query(
        &[
            r#"SELECT id, display_name, city, region, state, policy_version,
                  policy_accepted_at, deleted_at, created_at
           FROM users
           WHERE phone_lookup = "#,
            PHONE_LOOKUP_EXPR,
            " AND deleted_at IS NULL",
        ]
        .concat(),
    )
    .bind(&canonical)
    .bind(lookup_key)
    .fetch_optional(executor)
    .await
    .map_err(|_| UserError::StorageFailed)?;
    row.map(|row| read_user(&row)).transpose()
}

/// Delete one account, keeping its row for safety records (INV-07).
///
/// Returns `true` when this call transitioned the account; `false` for
/// unknown ids and already-deleted accounts (indistinguishable by design).
///
/// # Errors
///
/// Returns [`UserError::StorageFailed`] on database failure only.
pub async fn delete_user(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: uuid::Uuid,
) -> Result<bool, UserError> {
    let affected = sqlx::query(
        "UPDATE users SET deleted_at = now(), state = 'deleted'
          WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| UserError::StorageFailed)?;
    Ok(affected.rows_affected() == 1)
}

/// Reveal one account's current destination with the encryption key.
///
/// Purpose-limited administrative read (contact handoff in its owning card
/// revalidates full eligibility first and never caches this value).
///
/// # Errors
///
/// Returns [`UserError::StorageFailed`] when the account is missing, deleted,
/// or the key does not decrypt. Reasons are static; no plaintext leaks.
pub async fn reveal_phone<'e, E>(
    executor: E,
    user_id: uuid::Uuid,
    encryption_key: &str,
) -> Result<String, UserError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    if encryption_key.is_empty() {
        return Err(UserError::StorageFailed);
    }
    sqlx::query_scalar(
        "SELECT pgp_sym_decrypt(phone_ciphertext, $2)::text FROM users
          WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .bind(encryption_key)
    .fetch_one(executor)
    .await
    .map_err(|_| UserError::StorageFailed)
}

/// Create one session for `user_id`, persisting only the Argon2 digest.
///
/// The plaintext `token` (opaque, caller-generated) is hashed in memory and
/// never written anywhere.
///
/// # Errors
///
/// Returns [`UserError::InvalidSecret`] for an empty or oversized token, else
/// [`UserError::StorageFailed`]. Reasons are static.
pub async fn create_session(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: uuid::Uuid,
    token: &str,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> Result<Session, UserError> {
    check_secret(token)?;
    let digest = hash_secret(token)?;
    let row: sqlx::postgres::PgRow = sqlx::query(
        r#"INSERT INTO sessions (user_id, session_digest, expires_at)
           VALUES ($1, $2, $3)
           RETURNING id, user_id, expires_at, revoked_at, created_at"#,
    )
    .bind(user_id)
    .bind(&digest)
    .bind(expires_at)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| UserError::StorageFailed)?;
    use sqlx::Row;
    Ok(Session {
        id: row.try_get("id").map_err(|_| UserError::StorageFailed)?,
        user_id: row
            .try_get("user_id")
            .map_err(|_| UserError::StorageFailed)?,
        expires_at: row
            .try_get("expires_at")
            .map_err(|_| UserError::StorageFailed)?,
        revoked_at: row
            .try_get("revoked_at")
            .map_err(|_| UserError::StorageFailed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| UserError::StorageFailed)?,
    })
}

/// Find the live session matching `token`, if any.
///
/// Compares against stored Argon2 digests; expired or revoked sessions never
/// match. The scan is linear over live sessions by design in this card; an
/// indexed opaque session identifier lands with the session lifecycle card.
///
/// # Errors
///
/// Returns [`UserError::InvalidSecret`] for an empty token, else
/// [`UserError::StorageFailed`]. Reasons are static.
pub async fn find_session_by_token<'e, E>(
    executor: E,
    token: &str,
) -> Result<Option<Session>, UserError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    check_secret(token)?;
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(
        r#"SELECT id, user_id, session_digest, expires_at, revoked_at, created_at
           FROM sessions WHERE revoked_at IS NULL AND expires_at > now()"#,
    )
    .fetch_all(executor)
    .await
    .map_err(|_| UserError::StorageFailed)?;
    use sqlx::Row;
    for row in rows {
        let digest: String = row
            .try_get("session_digest")
            .map_err(|_| UserError::StorageFailed)?;
        if verify_secret(token, &digest) {
            return Ok(Some(Session {
                id: row.try_get("id").map_err(|_| UserError::StorageFailed)?,
                user_id: row
                    .try_get("user_id")
                    .map_err(|_| UserError::StorageFailed)?,
                expires_at: row
                    .try_get("expires_at")
                    .map_err(|_| UserError::StorageFailed)?,
                revoked_at: row
                    .try_get("revoked_at")
                    .map_err(|_| UserError::StorageFailed)?,
                created_at: row
                    .try_get("created_at")
                    .map_err(|_| UserError::StorageFailed)?,
            }));
        }
    }
    Ok(None)
}

/// Revoke one session. Returns `true` on transition; `false` for unknown or
/// already-revoked ids (indistinguishable by design).
///
/// # Errors
///
/// Returns [`UserError::StorageFailed`] on database failure only.
pub async fn revoke_session(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    session_id: uuid::Uuid,
) -> Result<bool, UserError> {
    let affected = sqlx::query(
        "UPDATE sessions SET revoked_at = now()
          WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(session_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| UserError::StorageFailed)?;
    Ok(affected.rows_affected() == 1)
}

/// A new phone verification challenge for one canonical lookup.
#[derive(Debug, Clone)]
pub struct NewChallenge {
    /// Phone in any common formatting; canonicalized before storage.
    pub phone: String,
    /// Provider code plaintext; only its Argon2 digest is stored.
    pub code: String,
    /// Challenge validity horizon.
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// Issue one challenge for `phone`, storing only digests.
///
/// # Errors
///
/// Returns [`UserError::InvalidPhone`]/[`UserError::InvalidSecret`] before any
/// write, else [`UserError::StorageFailed`]. Reasons are static.
pub async fn create_challenge(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    challenge: NewChallenge,
    lookup_key: &str,
) -> Result<uuid::Uuid, UserError> {
    let canonical = canonicalize_phone(&challenge.phone)?;
    check_secret(&challenge.code)?;
    if lookup_key.is_empty() {
        return Err(UserError::StorageFailed);
    }
    let digest = hash_secret(&challenge.code)?;
    sqlx::query_scalar(
        &[
            r#"INSERT INTO phone_challenges (phone_lookup, challenge_digest, expires_at)
           VALUES ("#,
            PHONE_LOOKUP_EXPR,
            ", $3, $4)
           RETURNING id",
        ]
        .concat(),
    )
    .bind(&canonical)
    .bind(lookup_key)
    .bind(&digest)
    .bind(challenge.expires_at)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| UserError::StorageFailed)
}

/// Consume one live challenge matching `phone` and `code`.
///
/// Returns `true` exactly once per challenge: live, unexpired, unconsumed,
/// and verified. Anything else — wrong code, expired, consumed, unknown —
/// returns `false` without distinguishing (anti-enumeration).
///
/// # Errors
///
/// Returns [`UserError::InvalidPhone`]/[`UserError::InvalidSecret`] for
/// malformed input, else [`UserError::StorageFailed`]. Reasons are static.
pub async fn consume_challenge(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    phone: &str,
    code: &str,
    lookup_key: &str,
) -> Result<bool, UserError> {
    let canonical = canonicalize_phone(phone)?;
    check_secret(code)?;
    if lookup_key.is_empty() {
        return Err(UserError::StorageFailed);
    }
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(
        &[
            r#"SELECT id, challenge_digest
           FROM phone_challenges
           WHERE phone_lookup = "#,
            PHONE_LOOKUP_EXPR,
            r#"
             AND consumed_at IS NULL AND expires_at > now()
           ORDER BY created_at DESC"#,
        ]
        .concat(),
    )
    .bind(&canonical)
    .bind(lookup_key)
    .fetch_all(&mut **tx)
    .await
    .map_err(|_| UserError::StorageFailed)?;
    use sqlx::Row;
    for row in rows {
        let id: uuid::Uuid = row.try_get("id").map_err(|_| UserError::StorageFailed)?;
        let digest: String = row
            .try_get("challenge_digest")
            .map_err(|_| UserError::StorageFailed)?;
        if verify_secret(code, &digest) {
            sqlx::query("UPDATE phone_challenges SET consumed_at = now() WHERE id = $1")
                .bind(id)
                .execute(&mut **tx)
                .await
                .map_err(|_| UserError::StorageFailed)?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// True when a block from `blocker_id` toward `blocked_id` is recorded.
///
/// The ledger starts empty; block commands and cascades arrive in P10.
///
/// # Errors
///
/// Returns [`UserError::StorageFailed`] on database failure only.
pub async fn blocks_relation_exists<'e, E>(
    executor: E,
    blocker_id: uuid::Uuid,
    blocked_id: uuid::Uuid,
) -> Result<bool, UserError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let found: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM user_blocks WHERE blocker_id = $1 AND blocked_id = $2")
            .bind(blocker_id)
            .bind(blocked_id)
            .fetch_optional(executor)
            .await
            .map_err(|_| UserError::StorageFailed)?;
    Ok(found.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_shapes_one_canonical_value() {
        let canonical = canonicalize_phone("+55 11 98765-4321").expect("shapes");
        assert_eq!(canonical, "+5511987654321");
        assert_eq!(
            canonicalize_phone("+55(11)98765.4321").expect("shapes"),
            canonical
        );
        assert_eq!(
            canonicalize_phone("  +5511987654321  ").expect("shapes"),
            canonical
        );
    }

    #[test]
    fn malformed_phones_are_refused() {
        for input in [
            "",
            "not-a-phone",
            "5511987654321",
            "+55",
            "+55119876543210000",
        ] {
            assert_eq!(canonicalize_phone(input), Err(UserError::InvalidPhone));
        }
    }

    #[test]
    fn secret_round_trip_verifies() {
        let digest = hash_secret("synthetic-token").expect("hashes");
        assert!(!digest.contains("synthetic-token"));
        assert!(verify_secret("synthetic-token", &digest));
        assert!(!verify_secret("other-token", &digest));
    }

    #[test]
    fn error_display_carries_no_values() {
        // Variants carry no payload by construction (Copy unit variants), so
        // rendering one can never echo a phone, token, URL, or key.
        let rendered = format!(
            "{} {} {} {} {}",
            UserError::InvalidPhone,
            UserError::InvalidProfile,
            UserError::DuplicatePhone,
            UserError::InvalidSecret,
            UserError::StorageFailed
        );
        assert!(!rendered.contains("+55"));
        assert!(!rendered.contains("canary"));
        assert!(!rendered.contains("postgres://"));
    }
}
