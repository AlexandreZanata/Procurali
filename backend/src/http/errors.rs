//! Structured errors: stable codes, exact statuses, secret-free messages.
//!
//! Canonical rules: INV-01 (ownership server-side — errors never echo
//! ownership claims), INV-05 (roles never authorize — refusals name the rule,
//! not the role), INV-31 (no private disclosure — messages and fields carry no
//! phone, address, reporter, token, or configuration value), AC-05 (invalid
//! input refused with the requirement), AC-19 (private comparison — error
//! bodies disclose nothing cross-account).
//!
//! [`Code`] is closed: every variant maps to one registry entry in
//! `contracts/domain-vectors.json` and one HTTP status. Adding a code needs a
//! contract revision; renaming one is breaking. `From` impls translate each
//! domain failure without callers inventing strings.

use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::Serialize;
use std::collections::BTreeMap;

/// Closed stable error code. Wire string and status are fixed per variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Code {
    /// Required input absent.
    #[serde(rename = "missing_field")]
    MissingField,
    /// Input fails shape or vocabulary rules.
    #[serde(rename = "invalid_field")]
    InvalidField,
    /// Money fails the exact-decimal grammar.
    #[serde(rename = "malformed_amount")]
    MalformedAmount,
    /// Zero or negative money.
    #[serde(rename = "nonpositive_amount")]
    NonPositiveAmount,
    /// More than two money decimals. Never rounded.
    #[serde(rename = "overprecision")]
    Overprecision,
    /// Money exceeds the integer-cent range.
    #[serde(rename = "amount_too_large")]
    AmountTooLarge,
    /// Text exceeds its scalar bound.
    #[serde(rename = "text_too_long")]
    TextTooLong,
    /// Unknown city code.
    #[serde(rename = "unknown_city")]
    UnknownCity,
    /// Unknown category code.
    #[serde(rename = "unknown_category")]
    UnknownCategory,
    /// Retired category code.
    #[serde(rename = "retired_category")]
    RetiredCategory,
    /// Prohibited category code.
    #[serde(rename = "prohibited_category")]
    ProhibitedCategory,
    /// Deadline passed for the action.
    #[serde(rename = "expired")]
    Expired,
    /// Rolling allowance exhausted.
    #[serde(rename = "quota_exceeded")]
    QuotaExceeded,
    /// Identical intent already recorded.
    #[serde(rename = "duplicate_intent")]
    DuplicateIntent,
    /// Authenticated non-owner.
    #[serde(rename = "forbidden_owner")]
    ForbiddenOwner,
    /// Authenticated wrong capability.
    #[serde(rename = "forbidden_role")]
    ForbiddenRole,
    /// Action invalid in the current state.
    #[serde(rename = "forbidden_state")]
    ForbiddenState,
    /// Stale revision submitted.
    #[serde(rename = "conflict_revision")]
    ConflictRevision,
    /// Same key reused with different input.
    #[serde(rename = "idempotency_key_reuse")]
    IdempotencyKeyReuse,
    /// No usable session.
    #[serde(rename = "unauthenticated")]
    Unauthenticated,
    /// Phone proof failed (attempts consumed).
    #[serde(rename = "verification_failed")]
    VerificationFailed,
    /// Challenge/attempt rate exceeded.
    #[serde(rename = "rate_limited")]
    RateLimited,
    /// Route or resource absent (no existence oracle).
    #[serde(rename = "not_found")]
    NotFound,
    /// Unexpected internal failure. No detail leaves the process.
    #[serde(rename = "internal")]
    Internal,
    /// A dependency needed for the action is unavailable.
    #[serde(rename = "unavailable")]
    Unavailable,
}

impl Code {
    /// Stable wire string (registry `contracts/domain-vectors.json`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingField => "missing_field",
            Self::InvalidField => "invalid_field",
            Self::MalformedAmount => "malformed_amount",
            Self::NonPositiveAmount => "nonpositive_amount",
            Self::Overprecision => "overprecision",
            Self::AmountTooLarge => "amount_too_large",
            Self::TextTooLong => "text_too_long",
            Self::UnknownCity => "unknown_city",
            Self::UnknownCategory => "unknown_category",
            Self::RetiredCategory => "retired_category",
            Self::ProhibitedCategory => "prohibited_category",
            Self::Expired => "expired",
            Self::QuotaExceeded => "quota_exceeded",
            Self::DuplicateIntent => "duplicate_intent",
            Self::ForbiddenOwner => "forbidden_owner",
            Self::ForbiddenRole => "forbidden_role",
            Self::ForbiddenState => "forbidden_state",
            Self::ConflictRevision => "conflict_revision",
            Self::IdempotencyKeyReuse => "idempotency_key_reuse",
            Self::Unauthenticated => "unauthenticated",
            Self::VerificationFailed => "verification_failed",
            Self::RateLimited => "rate_limited",
            Self::NotFound => "not_found",
            Self::Internal => "internal",
            Self::Unavailable => "unavailable",
        }
    }

    /// Exact HTTP status per code.
    #[must_use]
    pub const fn status(self) -> StatusCode {
        match self {
            Self::MissingField | Self::InvalidField | Self::MalformedAmount | Self::TextTooLong => {
                StatusCode::BAD_REQUEST
            }
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::ForbiddenOwner | Self::ForbiddenRole => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::DuplicateIntent
            | Self::ConflictRevision
            | Self::IdempotencyKeyReuse
            | Self::ForbiddenState => StatusCode::CONFLICT,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::NonPositiveAmount
            | Self::Overprecision
            | Self::AmountTooLarge
            | Self::UnknownCity
            | Self::UnknownCategory
            | Self::RetiredCategory
            | Self::ProhibitedCategory
            | Self::Expired
            | Self::QuotaExceeded
            | Self::VerificationFailed => StatusCode::UNPROCESSABLE_ENTITY,
        }
    }
}

/// Structured error body: code, human message, optional field-to-code map.
/// Messages are static text; values (phones, tokens, URLs) have no parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApiError {
    code: Code,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    fields: Option<BTreeMap<&'static str, Code>>,
}

impl ApiError {
    /// Build from a closed code and a static message.
    #[must_use]
    pub const fn new(code: Code, message: &'static str) -> Self {
        Self {
            code,
            message,
            fields: None,
        }
    }

    /// Attach one field-specific code (field names only, never values).
    #[must_use]
    pub fn with_field(mut self, field: &'static str, code: Code) -> Self {
        self.fields
            .get_or_insert_with(BTreeMap::new)
            .insert(field, code);
        self
    }

    /// The stable code string.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code.as_str()
    }

    /// Unexpected internal failure with a generic message.
    #[must_use]
    pub const fn internal() -> Self {
        Self::new(Code::Internal, "internal error")
    }

    /// Unavailable dependency with a generic message.
    #[must_use]
    pub const fn unavailable() -> Self {
        Self::new(Code::Unavailable, "dependency unavailable")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (self.code.status(), Json(self)).into_response()
    }
}

/// Translate money refusals without inventing strings.
impl From<crate::domain::money::MoneyError> for ApiError {
    fn from(error: crate::domain::money::MoneyError) -> Self {
        use crate::domain::money::MoneyError as E;
        match error {
            E::Empty => Self::new(Code::MissingField, "amount is required"),
            E::Malformed => Self::new(
                Code::MalformedAmount,
                "amount must be digits with at most two decimals",
            ),
            E::NonPositive => Self::new(Code::NonPositiveAmount, "amount must be positive"),
            E::Overprecision => {
                Self::new(Code::Overprecision, "amount must have at most two decimals")
            }
            E::Overflow => Self::new(Code::AmountTooLarge, "amount exceeds the supported range"),
        }
    }
}

/// Translate condition refusals.
impl From<crate::domain::condition::ConditionError> for ApiError {
    fn from(_: crate::domain::condition::ConditionError) -> Self {
        Self::new(Code::InvalidField, "unknown condition")
    }
}

/// Translate text refusals.
impl From<crate::domain::text::TextError> for ApiError {
    fn from(_: crate::domain::text::TextError) -> Self {
        Self::new(Code::TextTooLong, "text exceeds its length bound")
    }
}

/// Translate locality refusals.
impl From<crate::domain::location::LocationError> for ApiError {
    fn from(error: crate::domain::location::LocationError) -> Self {
        use crate::domain::location::LocationError as E;
        match error {
            E::Empty => Self::new(Code::MissingField, "locality code is required"),
            E::Invalid => Self::new(Code::InvalidField, "unknown locality code"),
        }
    }
}

/// Translate time refusals.
impl From<crate::domain::time::TimeError> for ApiError {
    fn from(error: crate::domain::time::TimeError) -> Self {
        use crate::domain::time::TimeError as E;
        match error {
            E::Empty => Self::new(Code::MissingField, "instant is required"),
            E::Malformed => Self::new(
                Code::InvalidField,
                "instant must be RFC3339 with an explicit offset",
            ),
        }
    }
}

/// Translate configuration refusals (server-side faults stay generic).
impl From<crate::operations::config::ConfigError> for ApiError {
    fn from(_: crate::operations::config::ConfigError) -> Self {
        Self::internal()
    }
}

/// Translate pool failures (no values; outage maps to 503).
impl From<crate::persistence::pool::PoolError> for ApiError {
    fn from(error: crate::persistence::pool::PoolError) -> Self {
        use crate::persistence::pool::PoolError as E;
        match error {
            E::InvalidUrl | E::ForbiddenDatabase => Self::internal(),
            E::ConnectionFailed | E::QueryFailed => Self::unavailable(),
        }
    }
}

/// Translate log-level codes to HTTP outcomes.
impl From<crate::operations::telemetry::ErrorCode> for ApiError {
    fn from(error: crate::operations::telemetry::ErrorCode) -> Self {
        use crate::operations::telemetry::ErrorCode as E;
        match error {
            E::Internal => Self::internal(),
            E::MissingSetting | E::InvalidSetting => Self::internal(),
            E::Unavailable => Self::unavailable(),
        }
    }
}
