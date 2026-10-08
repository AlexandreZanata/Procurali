//! Redacted structured observability.
//!
//! Privacy boundary (INV-31): logs carry only typed, allowlisted fields — a
//! correlation id, a stable error code, a route label, and numeric status/latency.
//! Raw request bodies, cookies, phone destinations, verification codes, session
//! material, keys, SQL binds, and caller-controlled free text never enter an event.
//! Caller-supplied correlation/route text passes a strict token grammar; anything
//! else is replaced, so a hostile value cannot inject extra log lines.
//!
//! The module is independent of marketplace DTO serialization: it uses only
//! primitives and never imports HTTP or domain DTO types.

use super::config::ConfigError;
use std::fmt;

/// Maximum accepted length for caller-supplied correlation/route tokens.
pub const MAX_TOKEN_LEN: usize = 64;

/// Stable machine-readable error codes for log and failure output.
/// These seed the contract registry (`contracts/domain-vectors.json`); the HTTP
/// error mapping extends them in a later card without renaming these strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// Unexpected internal failure. No detail leaves the process.
    Internal,
    /// A required setting is missing.
    MissingSetting,
    /// A supplied setting fails validation.
    InvalidSetting,
    /// A dependency needed for the action is unavailable.
    Unavailable,
}

impl ErrorCode {
    /// Stable wire string. Never renamed without a contract revision.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::MissingSetting => "missing_setting",
            Self::InvalidSetting => "invalid_setting",
            Self::Unavailable => "unavailable",
        }
    }

    /// Map a configuration refusal to its log code.
    #[must_use]
    pub const fn from_config(error: &ConfigError) -> Self {
        match error {
            ConfigError::Missing(_) => Self::MissingSetting,
            ConfigError::Invalid(_) => Self::InvalidSetting,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Bounded request-correlation identifier.
///
/// Well-formed caller tokens (1–64 chars of `[A-Za-z0-9_-]`) are preserved so
/// distributed callers can correlate; anything else yields a fresh random id so
/// hostile text (newlines, JSON fragments, canary secrets) never reaches the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrelationId(String);

impl CorrelationId {
    /// Fresh random identifier (32 lowercase hex chars).
    #[must_use]
    pub fn generate() -> Self {
        let hi = rand::random::<u64>();
        let lo = rand::random::<u64>();
        Self(format!("{hi:016x}{lo:016x}"))
    }

    /// Accept a caller-supplied candidate only when it matches the token grammar.
    #[must_use]
    pub fn sanitize(candidate: &str) -> Self {
        if is_safe_token(candidate) {
            Self(candidate.to_owned())
        } else {
            Self::generate()
        }
    }

    /// The identifier text. Safe by construction for log output.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CorrelationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Route label for log grouping. Unknown or hostile input becomes the fixed
/// string `unknown-route`; no caller text is ever reflected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteLabel(&'static str);

impl RouteLabel {
    /// Build from caller text without reflecting it.
    #[must_use]
    pub fn sanitize(candidate: &str) -> Self {
        if !is_safe_token(candidate) {
            return Self("unknown-route");
        }
        match candidate {
            "health-live" => Self("health-live"),
            "health-ready" => Self("health-ready"),
            _ => Self("unknown-route"),
        }
    }

    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

/// A request-scoped failure for logs and caller-visible formatting.
/// Carries correlation + code only; details stay out of every rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestError {
    correlation: CorrelationId,
    code: ErrorCode,
}

impl RequestError {
    #[must_use]
    pub fn new(correlation: CorrelationId, code: ErrorCode) -> Self {
        Self { correlation, code }
    }

    #[must_use]
    pub fn correlation(&self) -> &CorrelationId {
        &self.correlation
    }

    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "request {} failed: {}", self.correlation, self.code)
    }
}

impl std::error::Error for RequestError {}

/// Emit one structured event with allowlisted fields only.
///
/// `detail` is a static label chosen by the call site, never caller input.
/// Bodies, cookies, phones, codes, keys, and binds have no parameter here,
/// so they cannot be logged by mistake.
pub fn log_event(
    correlation_candidate: &str,
    code: ErrorCode,
    route_candidate: &str,
    status: u16,
    latency_ms: u64,
    detail: &'static str,
) {
    let correlation = CorrelationId::sanitize(correlation_candidate);
    let route = RouteLabel::sanitize(route_candidate);
    tracing::info!(
        correlation_id = correlation.as_str(),
        error_code = code.as_str(),
        route = route.as_str(),
        status,
        latency_ms,
        detail,
    );
}

/// Strict token grammar: 1–64 chars of letters, digits, `-`, `_`, containing at
/// least one ASCII letter. Newlines, quotes, braces, spaces, `+` (phone-shaped
/// input), `=`/`;` (cookie-shaped input), and purely numeric strings (which
/// resemble verification codes) are rejected, so hostile or secret-shaped text
/// can neither reflect into logs nor inject extra log lines.
fn is_safe_token(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate.len() <= MAX_TOKEN_LEN
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        && candidate.bytes().any(|byte| byte.is_ascii_alphabetic())
}
