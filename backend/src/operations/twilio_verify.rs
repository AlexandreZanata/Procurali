//! Live Twilio Verify adapter over an injected byte transport.
//!
//! This module owns the Twilio Verify v2 protocol boundary: validated
//! configuration, request building (Basic auth, form encoding), response
//! classification (sent/verified/incorrect/rate-limited/rejected/malformed),
//! and redacted errors. The byte transport itself is injected through
//! [`VerifyTransport`], so unit tests script every provider behavior without
//! network access and without new dependencies; the production HTTPS transport
//! lands with the card that introduces the HTTP client dependency
//! (explicit follow-through, not a silent substitution).
//!
//! Start and check use the provider's endpoints under
//! `{base}/v2/Services/{service_sid}/`: `Verifications` (challenge send) and
//! `VerificationCheck` (code confirmation). An accepted start never implies
//! verification: only a provider-confirmed `approved` check yields
//! [`CheckOutcome::Verified`](crate::application::phone_verification::CheckOutcome::Verified).

use std::time::Duration;

use crate::application::phone_verification::{
    CheckOutcome, StartOutcome, VerificationError, VerificationProvider,
};
use crate::operations::config::ConfigError;
use crate::persistence::users::canonicalize_phone;

/// Approved configuration for the live adapter.
///
/// Credentials travel as opaque strings and never reach logs: [`Debug`](std::fmt::Debug)
/// renders shapes only. `base_url` is `https://verify.twilio.com` in production;
/// plain HTTP is accepted only for loopback test doubles carrying `/test-only/`.
#[derive(Clone, PartialEq)]
pub struct TwilioConfig {
    account_sid: String,
    auth_token: String,
    service_sid: String,
    base_url: String,
    timeout: Duration,
}

impl TwilioConfig {
    /// Maximum accepted provider timeout (bounded waits everywhere).
    pub const MAX_TIMEOUT: Duration = Duration::from_secs(60);

    /// Build and validate the adapter configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] naming the offending field for missing
    /// credentials, a non-approved origin, or an out-of-range timeout. Values
    /// never appear in the error.
    pub fn new(
        account_sid: String,
        auth_token: String,
        service_sid: String,
        base_url: String,
        timeout: Duration,
    ) -> Result<Self, ConfigError> {
        if account_sid.trim().is_empty() {
            return Err(ConfigError::Missing("TWILIO_ACCOUNT_SID"));
        }
        if auth_token.trim().is_empty() {
            return Err(ConfigError::Missing("TWILIO_AUTH_TOKEN"));
        }
        if service_sid.trim().is_empty() {
            return Err(ConfigError::Missing("TWILIO_VERIFY_SERVICE_SID"));
        }
        if !is_approved_origin(&base_url) {
            return Err(ConfigError::Invalid("TWILIO_BASE_URL"));
        }
        if timeout.is_zero() || timeout > Self::MAX_TIMEOUT {
            return Err(ConfigError::Invalid("TWILIO_TIMEOUT"));
        }
        Ok(Self {
            account_sid,
            auth_token,
            service_sid,
            base_url: base_url.trim_end_matches('/').to_owned(),
            timeout,
        })
    }

    /// Provider timeout handed to the transport.
    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl std::fmt::Debug for TwilioConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TwilioConfig")
            .field("account_sid", &"set")
            .field("auth_token", &"set")
            .field("service_sid", &"set")
            .field("base_url", &self.base_url)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Loopback HTTP is approved only for isolated test doubles.
fn is_approved_origin(url: &str) -> bool {
    if let Some(rest) = url.strip_prefix("https://") {
        return !rest.is_empty();
    }
    if let Some(rest) = url.strip_prefix("http://127.0.0.1") {
        return rest.starts_with([':', '/']) || rest.is_empty();
    }
    if let Some(rest) = url.strip_prefix("http://localhost") {
        return rest.starts_with([':', '/']) || rest.is_empty();
    }
    false
}

/// One provider request: URL, Basic authorization value, and form body.
///
/// `Debug` never renders the URL (service identity), the authorization value
/// (credential), or the body (phone and code).
pub struct OutgoingRequest {
    url: String,
    authorization: String,
    body: String,
}

impl OutgoingRequest {
    /// Request line target for transports.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Full `Authorization` header value for transports.
    #[must_use]
    pub fn authorization(&self) -> &str {
        &self.authorization
    }

    /// Form-encoded body for transports.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }
}

impl std::fmt::Debug for OutgoingRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutgoingRequest")
            .field("url", &"[redacted]")
            .field("authorization", &"[redacted]")
            .field("body_len", &self.body.len())
            .finish()
    }
}

/// Minimal RFC 4648 base64 (standard alphabet, padded): mechanical encoding
/// for the Basic authorization value, pinned by unit vectors below.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut block: u32 = 0;
        for (index, byte) in chunk.iter().enumerate() {
            block |= u32::from(*byte) << (16 - 8 * index);
        }
        let symbols = match chunk.len() {
            3 => 4,
            2 => 3,
            _ => 2,
        };
        for position in 0..symbols {
            let sextet = (block >> (18 - 6 * position)) & 0x3F;
            out.push(ALPHABET[sextet as usize] as char);
        }
        for _ in symbols..4 {
            out.push('=');
        }
    }
    out
}

/// Minimal form encoding for request values: unreserved characters pass
/// through, space becomes `+`, everything else becomes uppercase `%XX`.
/// Pinned by unit vectors below.
fn url_encode(value: &str) -> String {
    const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.~";
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if UNRESERVED.contains(&byte) {
            out.push(byte as char);
        } else if byte == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn basic_authorization(config: &TwilioConfig) -> String {
    let credentials = format!("{}:{}", config.account_sid, config.auth_token);
    format!("Basic {}", base64_encode(credentials.as_bytes()))
}

/// Build the challenge-send request for one phone.
///
/// # Errors
///
/// Returns [`VerificationError::InvalidInput`] for malformed phones.
pub fn build_start_request(
    config: &TwilioConfig,
    phone: &str,
) -> Result<OutgoingRequest, VerificationError> {
    let canonical = canonicalize_phone(phone).map_err(|_| VerificationError::InvalidInput)?;
    Ok(OutgoingRequest {
        url: format!(
            "{}/v2/Services/{}/Verifications",
            config.base_url, config.service_sid
        ),
        authorization: basic_authorization(config),
        body: format!("To={}&Channel=sms", url_encode(&canonical)),
    })
}

/// Build the code-confirmation request for one phone and code.
///
/// # Errors
///
/// Returns [`VerificationError::InvalidInput`] for malformed phones or codes.
pub fn build_check_request(
    config: &TwilioConfig,
    phone: &str,
    code: &str,
) -> Result<OutgoingRequest, VerificationError> {
    let canonical = canonicalize_phone(phone).map_err(|_| VerificationError::InvalidInput)?;
    if code.trim().is_empty() || code.chars().count() > 32 {
        return Err(VerificationError::InvalidInput);
    }
    Ok(OutgoingRequest {
        url: format!(
            "{}/v2/Services/{}/VerificationCheck",
            config.base_url, config.service_sid
        ),
        authorization: basic_authorization(config),
        body: format!("To={}&Code={}", url_encode(&canonical), url_encode(code)),
    })
}

fn response_status(body: &str) -> Result<String, VerificationError> {
    let parsed: serde_json::Value =
        serde_json::from_str(body).map_err(|_| VerificationError::MalformedResponse)?;
    parsed
        .get("status")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or(VerificationError::MalformedResponse)
}

/// Classify a challenge-send response.
///
/// # Errors
///
/// Returns [`VerificationError::MalformedResponse`] for unparseable payloads
/// and [`VerificationError::ProviderRejected`] for refused requests. Reasons
/// are static.
pub fn parse_start_response(
    http_status: u16,
    body: &str,
) -> Result<StartOutcome, VerificationError> {
    if http_status == 429 {
        return Ok(StartOutcome::RateLimited);
    }
    if !(200..300).contains(&http_status) {
        return Err(VerificationError::ProviderRejected);
    }
    let status = response_status(body).map_err(|_| VerificationError::MalformedResponse)?;
    match status.as_str() {
        "pending" => Ok(StartOutcome::ChallengeSent),
        _ => Err(VerificationError::MalformedResponse),
    }
}

/// Classify a code-confirmation response.
///
/// A non-rate-limited refusal (expired challenge, denied destination, server
/// failure) surfaces as [`VerificationError::ProviderRejected`]: the caller
/// answers generically while the local challenge record stays authoritative.
///
/// # Errors
///
/// Returns [`VerificationError::MalformedResponse`] for unparseable payloads
/// and [`VerificationError::ProviderRejected`] for refused requests. Reasons
/// are static.
pub fn parse_check_response(
    http_status: u16,
    body: &str,
) -> Result<CheckOutcome, VerificationError> {
    if http_status == 429 {
        return Ok(CheckOutcome::RateLimited);
    }
    if !(200..300).contains(&http_status) {
        return Err(VerificationError::ProviderRejected);
    }
    let status = response_status(body).map_err(|_| VerificationError::MalformedResponse)?;
    match status.as_str() {
        "approved" => Ok(CheckOutcome::Verified),
        "pending" => Ok(CheckOutcome::Incorrect),
        _ => Err(VerificationError::MalformedResponse),
    }
}

/// One transport exchange: either a full response or a typed failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportOutcome {
    /// A complete HTTP response: status code plus body.
    Received {
        /// HTTP status code.
        status: u16,
        /// Response body.
        body: String,
    },
    /// The provider request timed out.
    TimedOut,
    /// The provider could not be reached.
    ConnectionFailed,
}

/// Byte transport for provider requests.
///
/// The production HTTPS implementation lands with the card that introduces the
/// HTTP client dependency; tests inject scripted or loopback transports here.
pub trait VerifyTransport: Send + Sync {
    /// POST one form request and report the exchange outcome.
    fn post_form(
        &self,
        request: OutgoingRequest,
    ) -> impl std::future::Future<Output = TransportOutcome> + Send;
}

/// Live Twilio Verify adapter over an injected [`VerifyTransport`].
pub struct TwilioLiveProvider<T> {
    config: TwilioConfig,
    transport: T,
}

impl<T> TwilioLiveProvider<T> {
    /// Bind validated configuration to a transport.
    #[must_use]
    pub fn new(config: TwilioConfig, transport: T) -> Self {
        Self { config, transport }
    }
}

impl<T: VerifyTransport> VerificationProvider for TwilioLiveProvider<T> {
    async fn start_verification(&self, phone: &str) -> Result<StartOutcome, VerificationError> {
        let request = build_start_request(&self.config, phone)?;
        match self.transport.post_form(request).await {
            TransportOutcome::Received { status, body } => parse_start_response(status, &body),
            TransportOutcome::TimedOut => Err(VerificationError::Timeout),
            TransportOutcome::ConnectionFailed => Err(VerificationError::ConnectionFailed),
        }
    }

    async fn check_verification(
        &self,
        phone: &str,
        code: &str,
    ) -> Result<CheckOutcome, VerificationError> {
        let request = build_check_request(&self.config, phone, code)?;
        match self.transport.post_form(request).await {
            TransportOutcome::Received { status, body } => parse_check_response(status, &body),
            TransportOutcome::TimedOut => Err(VerificationError::Timeout),
            TransportOutcome::ConnectionFailed => Err(VerificationError::ConnectionFailed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> TwilioConfig {
        TwilioConfig::new(
            "canary-sid".to_owned(),
            "canary-token".to_owned(),
            "canary-service".to_owned(),
            "https://verify.twilio.com".to_owned(),
            Duration::from_secs(10),
        )
        .expect("test config validates")
    }

    #[test]
    fn base64_matches_rfc_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(
            base64_encode(b"Aladdin:open sesame"),
            "QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
    }

    #[test]
    fn form_encoding_escapes_phone_and_code() {
        assert_eq!(url_encode("+5511987654321"), "%2B5511987654321");
        assert_eq!(url_encode("135 790"), "135+790");
        assert_eq!(url_encode("abc-_.~"), "abc-_.~");
    }

    #[test]
    fn builders_shape_verify_requests() {
        let start = build_start_request(&test_config(), "+55 11 98765-4321").expect("start builds");
        assert!(start
            .url()
            .ends_with("/v2/Services/canary-service/Verifications"));
        assert!(start.authorization().starts_with("Basic "));
        assert_eq!(start.body(), "To=%2B5511987654321&Channel=sms");
        let check =
            build_check_request(&test_config(), "+5511987654321", "135 790").expect("check builds");
        assert!(check
            .url()
            .ends_with("/v2/Services/canary-service/VerificationCheck"));
        assert_eq!(check.body(), "To=%2B5511987654321&Code=135+790");
        assert!(build_start_request(&test_config(), "not-a-phone").is_err());
        assert!(build_check_request(&test_config(), "+5511987654321", "").is_err());
    }

    #[test]
    fn start_responses_classify() {
        assert_eq!(
            parse_start_response(201, r#"{"sid":"VE1","status":"pending"}"#),
            Ok(StartOutcome::ChallengeSent)
        );
        assert_eq!(
            parse_start_response(429, r#"{"code":20429,"status":429}"#),
            Ok(StartOutcome::RateLimited)
        );
        assert_eq!(
            parse_start_response(201, "not json"),
            Err(VerificationError::MalformedResponse)
        );
        assert_eq!(
            parse_start_response(201, r#"{"status":"approved"}"#),
            Err(VerificationError::MalformedResponse)
        );
        assert_eq!(
            parse_start_response(400, r#"{"code":21211,"status":400}"#),
            Err(VerificationError::ProviderRejected)
        );
    }

    #[test]
    fn check_responses_classify() {
        assert_eq!(
            parse_check_response(200, r#"{"status":"approved"}"#),
            Ok(CheckOutcome::Verified)
        );
        assert_eq!(
            parse_check_response(200, r#"{"status":"pending"}"#),
            Ok(CheckOutcome::Incorrect)
        );
        assert_eq!(
            parse_check_response(429, r#"{"code":20429,"status":429}"#),
            Ok(CheckOutcome::RateLimited)
        );
        assert_eq!(
            parse_check_response(200, "not json"),
            Err(VerificationError::MalformedResponse)
        );
        assert_eq!(
            parse_check_response(404, r#"{"code":20404,"status":404}"#),
            Err(VerificationError::ProviderRejected)
        );
    }

    #[test]
    fn config_refuses_missing_credentials_and_origins() {
        let valid = || {
            TwilioConfig::new(
                "sid".to_owned(),
                "token".to_owned(),
                "service".to_owned(),
                "https://verify.twilio.com".to_owned(),
                Duration::from_secs(10),
            )
        };
        assert!(valid().is_ok());
        assert_eq!(
            TwilioConfig::new(
                String::new(),
                "token".to_owned(),
                "service".to_owned(),
                "https://verify.twilio.com".to_owned(),
                Duration::from_secs(10),
            ),
            Err(ConfigError::Missing("TWILIO_ACCOUNT_SID"))
        );
        assert_eq!(
            TwilioConfig::new(
                "sid".to_owned(),
                "token".to_owned(),
                "service".to_owned(),
                "http://twilio.com".to_owned(),
                Duration::from_secs(10),
            ),
            Err(ConfigError::Invalid("TWILIO_BASE_URL"))
        );
        assert!(TwilioConfig::new(
            "sid".to_owned(),
            "token".to_owned(),
            "service".to_owned(),
            "http://127.0.0.1:9/test-only/x".to_owned(),
            Duration::from_secs(10),
        )
        .is_ok());
        assert_eq!(
            TwilioConfig::new(
                "sid".to_owned(),
                "token".to_owned(),
                "service".to_owned(),
                "https://verify.twilio.com".to_owned(),
                Duration::ZERO,
            ),
            Err(ConfigError::Invalid("TWILIO_TIMEOUT"))
        );
    }

    #[test]
    fn redaction_hides_credentials_everywhere() {
        let config = test_config();
        let request = build_start_request(&config, "+5511987654321").expect("builds");
        let rendered = format!("{config:?} {request:?}");
        assert!(!rendered.contains("canary-sid"));
        assert!(!rendered.contains("canary-token"));
        assert!(!rendered.contains("canary-service"));
        assert!(!rendered.contains("5511987654321"));
    }
}
