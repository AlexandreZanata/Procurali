//! Phone-verification provider boundary: one trait, two adapters.
//!
//! Canonical rules: INV-04 (only a provider-confirmed code activates an
//! account — submitting a challenge request never counts as proof) and AC-02
//! (minimum registration: no tax id, photo, address, or seller account).
//! Mechanism follows `docs/engineering/authentication.md`: a replaceable
//! provider boundary with a live Twilio Verify adapter (in
//! [`crate::operations::twilio_verify`]) and a deterministic fake restricted
//! to test/development configuration, which production startup refuses.
//!
//! The trait reports outcomes, never booleans: a successful `start` only means
//! a challenge was sent, and only [`CheckOutcome::Verified`] — produced solely
//! by a provider-confirmed code — proves control of the number. Retrying or
//! failing after an accepted start never upgrades to verified.

use crate::operations::config::Environment;

/// Outcome of requesting a verification challenge for one phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartOutcome {
    /// The provider accepted the challenge request.
    ChallengeSent,
    /// The provider rate-limited the request; back off without treating the
    /// number as verified.
    RateLimited,
}

/// Outcome of submitting one verification code for one phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckOutcome {
    /// The provider confirmed the code: control of the number is proven.
    Verified,
    /// The code did not match (or the challenge is gone); indistinguishable
    /// by design (anti-enumeration).
    Incorrect,
    /// The provider rate-limited the request; back off without treating the
    /// number as verified.
    RateLimited,
}

/// Typed verification failure. Static reasons only — phones, codes, tokens,
/// URLs, and credentials never appear in `Display` or `Debug`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationError {
    /// The phone or code argument is missing or malformed.
    InvalidInput,
    /// The provider request timed out.
    Timeout,
    /// The provider could not be reached.
    ConnectionFailed,
    /// The provider answered with an unparseable payload.
    MalformedResponse,
    /// The provider refused the request (unknown number, denied destination).
    ProviderRejected,
    /// A test double was requested where doubles are forbidden.
    RefusedInEnvironment,
}

impl std::fmt::Display for VerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput => f.write_str("invalid verification input"),
            Self::Timeout => f.write_str("verification provider timed out"),
            Self::ConnectionFailed => f.write_str("verification provider unreachable"),
            Self::MalformedResponse => f.write_str("verification provider malformed response"),
            Self::ProviderRejected => f.write_str("verification provider rejected request"),
            Self::RefusedInEnvironment => {
                f.write_str("test verification double refused in this environment")
            }
        }
    }
}

impl std::error::Error for VerificationError {}

/// Phone-proof provider: start challenges and check codes.
///
/// Both methods return outcomes or typed failures; neither returns a bare
/// boolean, so callers cannot mistake an accepted start for proof.
pub trait VerificationProvider {
    /// Request a challenge for `phone`. Success means sent, never proven.
    fn start_verification(
        &self,
        phone: &str,
    ) -> impl std::future::Future<Output = Result<StartOutcome, VerificationError>> + Send;

    /// Submit `code` for `phone`. Only [`CheckOutcome::Verified`] proves control.
    fn check_verification(
        &self,
        phone: &str,
        code: &str,
    ) -> impl std::future::Future<Output = Result<CheckOutcome, VerificationError>> + Send;
}

/// Deterministic fake provider for test/development configuration only.
///
/// `start_verification` always reports sent; `check_verification` reports
/// verified exactly when the submitted code equals the fixed code given at
/// construction. A fake success never proves real phone control; production
/// startup refuses this type through [`for_environment`](FakeVerifyProvider::for_environment).
#[derive(Debug, PartialEq)]
pub struct FakeVerifyProvider {
    expected_code: String,
}

impl FakeVerifyProvider {
    /// Build the fake with one fixed code. Prefer
    /// [`for_environment`](FakeVerifyProvider::for_environment), which refuses
    /// production explicitly.
    #[must_use]
    pub fn for_tests(code: &str) -> Self {
        Self {
            expected_code: code.to_owned(),
        }
    }

    /// Build the fake unless `environment` forbids test doubles.
    ///
    /// # Errors
    ///
    /// Returns [`VerificationError::RefusedInEnvironment`] in production (and
    /// anywhere else doubles are forbidden).
    pub fn for_environment(
        environment: Environment,
        code: &str,
    ) -> Result<Self, VerificationError> {
        if environment.allows_test_doubles() {
            Ok(Self::for_tests(code))
        } else {
            Err(VerificationError::RefusedInEnvironment)
        }
    }
}

impl VerificationProvider for FakeVerifyProvider {
    async fn start_verification(&self, phone: &str) -> Result<StartOutcome, VerificationError> {
        if !phone.trim().is_empty() && phone.chars().count() <= 32 {
            Ok(StartOutcome::ChallengeSent)
        } else {
            Err(VerificationError::InvalidInput)
        }
    }

    async fn check_verification(
        &self,
        phone: &str,
        code: &str,
    ) -> Result<CheckOutcome, VerificationError> {
        if phone.trim().is_empty() || code.is_empty() {
            Err(VerificationError::InvalidInput)
        } else if *code == *self.expected_code {
            Ok(CheckOutcome::Verified)
        } else {
            Ok(CheckOutcome::Incorrect)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_distinguishes_success_from_wrong_code() {
        let provider = FakeVerifyProvider::for_tests("135790");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime builds");
        runtime.block_on(async {
            assert_eq!(
                provider
                    .start_verification("+5511987654321")
                    .await
                    .expect("start works"),
                StartOutcome::ChallengeSent
            );
            assert_eq!(
                provider
                    .check_verification("+5511987654321", "135790")
                    .await
                    .expect("check works"),
                CheckOutcome::Verified
            );
            assert_eq!(
                provider
                    .check_verification("+5511987654321", "000000")
                    .await
                    .expect("check works"),
                CheckOutcome::Incorrect
            );
        });
    }

    #[test]
    fn fake_is_refused_where_doubles_are_forbidden() {
        assert!(FakeVerifyProvider::for_environment(Environment::Test, "135790").is_ok());
        assert!(FakeVerifyProvider::for_environment(Environment::Development, "135790").is_ok());
        assert_eq!(
            FakeVerifyProvider::for_environment(Environment::Production, "135790"),
            Err(VerificationError::RefusedInEnvironment)
        );
        assert_eq!(
            FakeVerifyProvider::for_environment(Environment::Staging, "135790"),
            Err(VerificationError::RefusedInEnvironment)
        );
    }

    #[test]
    fn errors_carry_no_values() {
        for error in [
            VerificationError::InvalidInput,
            VerificationError::Timeout,
            VerificationError::ConnectionFailed,
            VerificationError::MalformedResponse,
            VerificationError::ProviderRejected,
            VerificationError::RefusedInEnvironment,
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("canary"));
            assert!(!rendered.contains("+55"));
            assert!(!rendered.contains("https://"));
        }
    }
}
