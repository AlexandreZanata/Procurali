//! Contextual handoff preparation: safe messages, honest failures.
//!
//! Canonical rules: INV-30 with INV-31 (no buyer phone, address, or offer
//! link in handoff context — the builders take item terms only, so private
//! material cannot enter by construction), INV-32 (the handoff necessarily
//! reveals the destination to its initiating buyer — and nobody else),
//! INV-33 with AC-24 (preparation that cannot produce a usable destination
//! records its failure separately without incrementing initiation metrics
//! or reputations, and claims no external opening or delivery — the URL
//! opens a chat draft; sending happens in WhatsApp, outside the platform).
//!
//! Preparation precedes commitment: callers validate destination usability
//! before recording a contact, and record known failures through
//! [`record_preparation_failure`] instead. Responses carrying handoffs must
//! send `Cache-Control: no-store` (see [`CACHE_DIRECTIVE`]); the URL and
//! message are transient buyer-only values, never persisted.

use crate::persistence::users::canonicalize_phone;

/// Cache directive for every response carrying handoff context: nothing
/// initiates from a stored or shared copy.
pub const CACHE_DIRECTIVE: &str = "no-store";

/// Preparation failure kinds, recorded as static reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparationFailure {
    /// The destination does not decrypt with the current keys.
    Undecryptable,
    /// The destination decrypts but is not a usable phone.
    UnusableNumber,
}

impl PreparationFailure {
    /// Stable fact string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Undecryptable => "undecryptable",
            Self::UnusableNumber => "unusable_number",
        }
    }
}

/// Typed preparation failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparationError {
    /// The destination cannot back a handoff; carries its static kind.
    Unusable(PreparationFailure),
    /// The lookup or write failed.
    StorageFailed,
}

impl std::fmt::Display for PreparationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unusable(_) => f.write_str("destination cannot back a handoff"),
            Self::StorageFailed => f.write_str("handoff preparation failed"),
        }
    }
}

impl std::error::Error for PreparationError {}

/// Percent-encode one message for a URL query value (RFC 3986): UTF-8
/// bytes outside the unreserved set become uppercase `%XX`. No dependency,
/// no ambiguity, no platform-specific form encoding.
#[must_use]
pub fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Build the contextual message from current item terms: description, exact
/// price, and condition. Takes no buyer, address, link, or contact
/// material, so none can appear — verified by unit test, not by review.
#[must_use]
pub fn handoff_message(description: &str, price_cents: i64, condition: &str) -> String {
    format!(
        "Hi! I'm interested in your {} listed at {} ({}). Is it still available?",
        description.trim(),
        crate::persistence::requests::format_budget(price_cents),
        condition
    )
}

/// Build the buyer-only chat URL for a verified E.164 destination: the
/// `wa.me` host with digits only plus the encoded message. Transient by
/// contract — callers never persist or share it.
#[must_use]
pub fn whatsapp_url(destination_e164: &str, message: &str) -> String {
    let digits: String = destination_e164
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    format!("https://wa.me/{digits}?text={}", percent_encode(message))
}

/// Verify one stored destination ciphertext decrypts to a usable phone
/// with the current keys, returning its canonical form. Anything else —
/// undecryptable bytes or a malformed number — refuses without recording
/// anything: the caller records the failure separately and initiates no
/// contact.
///
/// # Errors
///
/// Returns [`PreparationError::Unusable`] for undecryptable or malformed
/// destinations, else [`PreparationError::StorageFailed`].
pub async fn prepare_destination<'e, E>(
    executor: E,
    ciphertext: &[u8],
    encryption_key: &str,
) -> Result<String, PreparationError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    if encryption_key.is_empty() || ciphertext.is_empty() {
        return Err(PreparationError::Unusable(
            PreparationFailure::Undecryptable,
        ));
    }
    let decrypted: Option<String> = sqlx::query_scalar("SELECT pgp_sym_decrypt($1, $2)::text")
        .bind(ciphertext)
        .bind(encryption_key)
        .fetch_optional(executor)
        .await
        .map_err(|_| PreparationError::StorageFailed)?;
    match decrypted {
        // Canonicalization is pure: its only failure is a malformed
        // number, which is exactly the unusable case.
        Some(number) => canonicalize_phone(&number)
            .map_err(|_| PreparationError::Unusable(PreparationFailure::UnusableNumber)),
        None => Err(PreparationError::Unusable(
            PreparationFailure::Undecryptable,
        )),
    }
}

/// Record one known preparation failure for a buyer/offer pair without
/// creating any contact: initiation metrics and reputations stay exactly
/// as they were.
///
/// # Errors
///
/// Returns [`PreparationError::StorageFailed`] on database failure only.
pub async fn record_preparation_failure<'e, E>(
    executor: E,
    buyer_id: uuid::Uuid,
    offer_id: uuid::Uuid,
    failure: PreparationFailure,
) -> Result<(), PreparationError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query(
        "INSERT INTO business_events
            (actor_id, resource_kind, resource_id, effective_at, kind,
             policy, source, payload)
         VALUES ($1, 'offer', $2, now(), 'contact.preparation_failed',
                 'mvp-free', 'api', jsonb_build_object('reason', $3::text))",
    )
    .bind(buyer_id)
    .bind(offer_id)
    .bind(failure.as_str())
    .execute(executor)
    .await
    .map_err(|_| PreparationError::StorageFailed)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_carries_current_terms_only() {
        let message = handoff_message("Frost-free 300L", 52_000, "used");
        assert!(message.contains("Frost-free 300L"));
        assert!(message.contains("520.00"));
        assert!(message.contains("used"));
        for absent in [
            "+55", "90000", "http", "wa.me", "offer/", "phone", "address", "token",
        ] {
            assert!(!message.contains(absent), "no {absent} in handoff message");
        }
    }

    #[test]
    fn encoding_is_exact_and_url_shaped() {
        assert_eq!(percent_encode("a b+c"), "a%20b%2Bc");
        assert_eq!(percent_encode("~unreserved-_.x"), "~unreserved-_.x");
        let url = whatsapp_url("+55 11 98765-4321", "Hi! Tudo bem? & você");
        assert!(url.starts_with("https://wa.me/5511987654321?text="));
        assert!(!url.contains(' '));
        assert!(!url.contains("++"));
    }

    #[test]
    fn cache_and_failure_strings_are_stable() {
        assert_eq!(CACHE_DIRECTIVE, "no-store");
        assert_eq!(PreparationFailure::Undecryptable.as_str(), "undecryptable");
        assert_eq!(
            PreparationFailure::UnusableNumber.as_str(),
            "unusable_number"
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            PreparationError::Unusable(PreparationFailure::Undecryptable),
            PreparationError::Unusable(PreparationFailure::UnusableNumber),
            PreparationError::StorageFailed,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
            assert!(!rendered.contains("+55"));
        }
    }
}
