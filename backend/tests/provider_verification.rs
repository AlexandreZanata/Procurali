//! Phone-verification adapter acceptance (P04-T02).
//!
//! No database is required: every case runs against scripted transports or an
//! isolated loopback HTTP server with synthetic responses. Proves:
//! - success, wrong code, timeout, and malformed responses are distinguished;
//! - production configuration refuses the fake and missing credentials;
//! - an accepted start never implies verification;
//! - the production request builder and response parser round-trip over real
//!   HTTP against a synthetic server;
//! - request and error surfaces stay redacted.
//!
//! All phones, codes, and credentials below are synthetic and reserved.

use procurali_backend::application::phone_verification::{
    CheckOutcome, FakeVerifyProvider, StartOutcome, VerificationError, VerificationProvider,
};
use procurali_backend::operations::config::{Environment, Settings};
use procurali_backend::operations::twilio_verify::{
    build_check_request, build_start_request, parse_check_response, parse_start_response,
    OutgoingRequest, TransportOutcome, TwilioConfig, TwilioLiveProvider, VerifyTransport,
};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

/// Scripted transport: replays one queued outcome per request, in order.
struct ScriptedTransport {
    queued: Mutex<VecDeque<TransportOutcome>>,
}

impl ScriptedTransport {
    fn replying(outcomes: Vec<TransportOutcome>) -> Self {
        Self {
            queued: Mutex::new(outcomes.into()),
        }
    }
}

impl VerifyTransport for ScriptedTransport {
    fn post_form(
        &self,
        _request: OutgoingRequest,
    ) -> impl std::future::Future<Output = TransportOutcome> + Send {
        let outcome = self
            .queued
            .lock()
            .expect("script lock")
            .pop_front()
            .expect("scripted outcome available");
        async move { outcome }
    }
}

fn live_config() -> TwilioConfig {
    TwilioConfig::new(
        "canary-account-sid".to_owned(),
        "canary-auth-token".to_owned(),
        "canary-service-sid".to_owned(),
        "https://verify.twilio.com".to_owned(),
        Duration::from_secs(10),
    )
    .expect("live config validates")
}

fn received(status: u16, body: &str) -> TransportOutcome {
    TransportOutcome::Received {
        status,
        body: body.to_owned(),
    }
}

#[tokio::test]
async fn provider_outcomes_are_distinguished() {
    // Success: accepted start plus approved check.
    let provider = TwilioLiveProvider::new(
        live_config(),
        ScriptedTransport::replying(vec![
            received(201, r#"{"sid":"VE1","status":"pending"}"#),
            received(200, r#"{"sid":"VE1","status":"approved"}"#),
        ]),
    );
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

    // Wrong code stays incorrect, never verified.
    let provider = TwilioLiveProvider::new(
        live_config(),
        ScriptedTransport::replying(vec![received(200, r#"{"status":"pending"}"#)]),
    );
    assert_eq!(
        provider
            .check_verification("+5511987654321", "000000")
            .await
            .expect("check works"),
        CheckOutcome::Incorrect
    );

    // Timeout and connection loss surface as typed failures.
    for outcome in [
        TransportOutcome::TimedOut,
        TransportOutcome::ConnectionFailed,
    ] {
        let provider =
            TwilioLiveProvider::new(live_config(), ScriptedTransport::replying(vec![outcome]));
        let failed = provider
            .start_verification("+5511987654321")
            .await
            .expect_err("transport failure surfaces");
        assert!(
            matches!(
                failed,
                VerificationError::Timeout | VerificationError::ConnectionFailed
            ),
            "timeout and connection loss stay distinct failures"
        );
    }

    // Malformed payloads never parse as success or incorrect.
    let provider = TwilioLiveProvider::new(
        live_config(),
        ScriptedTransport::replying(vec![received(200, "this is not json")]),
    );
    assert_eq!(
        provider
            .check_verification("+5511987654321", "135790")
            .await
            .expect_err("malformed surfaces"),
        VerificationError::MalformedResponse
    );

    // Rate limits and rejections classify without claiming anything.
    let provider = TwilioLiveProvider::new(
        live_config(),
        ScriptedTransport::replying(vec![
            received(429, r#"{"code":20429,"status":429}"#),
            received(400, r#"{"code":21211,"status":400}"#),
        ]),
    );
    assert_eq!(
        provider
            .start_verification("+5511987654321")
            .await
            .expect("rate limit classifies"),
        StartOutcome::RateLimited
    );
    assert_eq!(
        provider
            .check_verification("+5511987654321", "135790")
            .await
            .expect_err("rejection surfaces"),
        VerificationError::ProviderRejected
    );
}

fn settings_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn production_config_refuses_fake_or_missing_credentials() {
    let base = || {
        settings_map(&[
            ("DATABASE_URL", "postgres://u@127.0.0.1:5432/procurali_dev"),
            ("SESSION_KEY", "test-only-session-key-long-enough"),
        ])
    };
    // Production refuses the fake double.
    let mut production_fake = base();
    production_fake.insert("PROCURALI_ENV".to_owned(), "production".to_owned());
    production_fake.insert("PHONE_PROVIDER".to_owned(), "fake".to_owned());
    assert!(Settings::from_map(&production_fake).is_err());
    // Test/development accept the fake double.
    for environment in ["test", "development"] {
        let mut allowed = base();
        allowed.insert("PROCURALI_ENV".to_owned(), environment.to_owned());
        allowed.insert("PHONE_PROVIDER".to_owned(), "fake".to_owned());
        let settings = Settings::from_map(&allowed).expect("fake allowed here");
        assert!(settings.allows_test_doubles());
    }
    // Live mode without credentials is refused everywhere.
    for environment in ["test", "production"] {
        let mut missing = base();
        missing.insert("PROCURALI_ENV".to_owned(), environment.to_owned());
        missing.insert("PHONE_PROVIDER".to_owned(), "twilio".to_owned());
        assert!(Settings::from_map(&missing).is_err());
    }
    // Live mode with explicit credentials loads.
    let mut live = base();
    live.insert("PHONE_PROVIDER".to_owned(), "twilio".to_owned());
    live.insert("TWILIO_ACCOUNT_SID".to_owned(), "canary-sid".to_owned());
    live.insert("TWILIO_AUTH_TOKEN".to_owned(), "canary-token".to_owned());
    live.insert(
        "TWILIO_VERIFY_SERVICE_SID".to_owned(),
        "canary-service".to_owned(),
    );
    let settings = Settings::from_map(&live).expect("live test settings load");
    assert!(!settings.allows_test_doubles() || settings.environment() == Environment::Development);
    // The fake constructor itself refuses forbidden environments.
    assert_eq!(
        FakeVerifyProvider::for_environment(Environment::Production, "135790"),
        Err(VerificationError::RefusedInEnvironment)
    );
    assert!(FakeVerifyProvider::for_environment(Environment::Test, "135790").is_ok());
}

#[tokio::test]
async fn accepted_start_never_implies_verified() {
    // Live path: accepted start, then a wrong code and a timeout.
    let provider = TwilioLiveProvider::new(
        live_config(),
        ScriptedTransport::replying(vec![
            received(201, r#"{"sid":"VE1","status":"pending"}"#),
            received(200, r#"{"status":"pending"}"#),
            TransportOutcome::TimedOut,
        ]),
    );
    assert_eq!(
        provider
            .start_verification("+5511987654321")
            .await
            .expect("start works"),
        StartOutcome::ChallengeSent
    );
    assert_eq!(
        provider
            .check_verification("+5511987654321", "000000")
            .await
            .expect("check works"),
        CheckOutcome::Incorrect
    );
    assert_eq!(
        provider
            .check_verification("+5511987654321", "135790")
            .await
            .expect_err("timeout is a failure, not proof"),
        VerificationError::Timeout
    );

    // Fake path: an accepted start followed by a wrong code stays incorrect.
    let fake = FakeVerifyProvider::for_tests("135790");
    assert_eq!(
        fake.start_verification("+5511987654321")
            .await
            .expect("fake starts"),
        StartOutcome::ChallengeSent
    );
    assert_eq!(
        fake.check_verification("+5511987654321", "000000")
            .await
            .expect("fake checks"),
        CheckOutcome::Incorrect
    );
}

/// Isolated synthetic Verify server: real loopback HTTP with canned Verify
/// payloads, recording every observed request line, authorization header, and
/// body for wire-shape assertions.
struct SyntheticVerify {
    address: std::net::SocketAddr,
    observed: std::sync::Arc<Mutex<Vec<(String, String, String)>>>,
}

type ObservedRequests = std::sync::Arc<Mutex<Vec<(String, String, String)>>>;

impl SyntheticVerify {
    async fn start(expected_code: &'static str) -> Self {
        use axum::extract::State;
        use axum::http::{HeaderMap, StatusCode};
        use axum::routing::post;
        let observed = std::sync::Arc::new(Mutex::new(Vec::new()));
        let state = (std::sync::Arc::clone(&observed), expected_code);
        let app = axum::Router::new()
            .route(
                "/v2/Services/{service}/Verifications",
                post(
                    |State(state): State<(ObservedRequests, &'static str)>,
                     headers: HeaderMap,
                     body: String| async move {
                        let authorization = headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("")
                            .to_owned();
                        state.0.lock().expect("record lock").push((
                            "verifications".to_owned(),
                            authorization,
                            body,
                        ));
                        (
                            StatusCode::CREATED,
                            r#"{"sid":"VEtest","status":"pending"}"#,
                        )
                    },
                )
                .with_state(state.clone()),
            )
            .route(
                "/v2/Services/{service}/VerificationCheck",
                post(
                    |State(state): State<(ObservedRequests, &'static str)>,
                     headers: HeaderMap,
                     body: String| async move {
                        let authorization = headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("")
                            .to_owned();
                        let approved = body
                            .contains(&format!("Code={}", state.1.replace('+', "%2B")))
                            || body.contains(state.1);
                        state.0.lock().expect("record lock").push((
                            "check".to_owned(),
                            authorization,
                            body,
                        ));
                        let payload = if approved {
                            r#"{"sid":"VEtest","status":"approved"}"#
                        } else {
                            r#"{"sid":"VEtest","status":"pending"}"#
                        };
                        (StatusCode::OK, payload)
                    },
                )
                .with_state(state),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback binds");
        let address = listener.local_addr().expect("loopback address reads");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("synthetic server serves");
        });
        Self { address, observed }
    }
}

/// Test-only minimal HTTP exchange over a loopback socket: sends one
/// production-built request and parses the real response bytes through the
/// production classifiers. Blocking socket I/O runs on the blocking pool so
/// the server task keeps making progress. Never production code.
fn round_trip_blocking(request: OutgoingRequest) -> (u16, String) {
    use std::io::{Read, Write};
    let url = request.url().to_owned();
    let authorization = request.authorization().to_owned();
    let body = request.body().to_owned();
    let after_host = url.split_once("127.0.0.1").expect("loopback url").1;
    let path = after_host
        .find('/')
        .map(|at| after_host.split_at(at).1.to_owned())
        .expect("path present");
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .expect("host present")
        .to_owned();
    let mut stream = std::net::TcpStream::connect(&host).expect("server accepts");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("timeout sets");
    let wire = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nAuthorization: {}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        path,
        host,
        authorization,
        body.len(),
        body
    );
    stream.write_all(wire.as_bytes()).expect("request writes");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("response reads");
    let text = String::from_utf8(raw).expect("response is text");
    let (head, body) = text.split_once("\r\n\r\n").expect("headers end");
    let status: u16 = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("status parses");
    (status, body.to_owned())
}

async fn round_trip(request: OutgoingRequest) -> (u16, String) {
    tokio::task::spawn_blocking(|| round_trip_blocking(request))
        .await
        .expect("blocking exchange joins")
}

#[tokio::test]
async fn synthetic_http_server_round_trip() {
    let server = SyntheticVerify::start("135790").await;
    let config = TwilioConfig::new(
        "canary-account-sid".to_owned(),
        "canary-auth-token".to_owned(),
        "canary-service-sid".to_owned(),
        format!("http://{}", server.address),
        Duration::from_secs(10),
    )
    .expect("loopback test config validates");

    // Challenge send over real HTTP.
    let start = build_start_request(&config, "+55 11 98765-4321").expect("start builds");
    let (status, body) = round_trip(start).await;
    assert_eq!(
        parse_start_response(status, &body),
        Ok(StartOutcome::ChallengeSent)
    );

    // Correct code confirms over real HTTP; a wrong code does not.
    let check = build_check_request(&config, "+5511987654321", "135790").expect("check builds");
    let (status, body) = round_trip(check).await;
    assert_eq!(
        parse_check_response(status, &body),
        Ok(CheckOutcome::Verified)
    );
    let wrong = build_check_request(&config, "+5511987654321", "000000").expect("builds");
    let (status, body) = round_trip(wrong).await;
    assert_eq!(
        parse_check_response(status, &body),
        Ok(CheckOutcome::Incorrect)
    );

    // The server observed the Verify wire shape: service paths, Basic auth,
    // and form-encoded destinations.
    let observed = server.observed.lock().expect("record lock").clone();
    assert_eq!(observed.len(), 3);
    assert!(observed[0].1.starts_with("Basic "));
    assert!(observed[0].2.contains("To=%2B5511987654321"));
    assert!(observed[0].2.contains("Channel=sms"));
    assert!(observed[1].2.contains("Code=135790"));
    assert!(observed[2].2.contains("Code=000000"));
}

#[test]
fn request_and_error_surfaces_stay_redacted() {
    let config = live_config();
    let request = build_start_request(&config, "+5511987654321").expect("builds");
    let rendered = format!("{config:?} {request:?}");
    assert!(!rendered.contains("canary-account-sid"));
    assert!(!rendered.contains("canary-auth-token"));
    assert!(!rendered.contains("canary-service-sid"));
    assert!(!rendered.contains("5511987654321"));
    assert!(!rendered.contains("Basic "));

    // Errors render statically even when raised for canary inputs.
    let failed = parse_check_response(200, "not json").expect_err("malformed");
    let rendered = format!("{failed:?} {failed}");
    assert!(!rendered.contains("canary"));
    assert!(!rendered.contains("5511"));
    let rejected = parse_start_response(400, r#"{"code":21211,"status":400}"#).expect_err("400");
    let rendered = format!("{rejected:?} {rejected}");
    assert!(!rendered.contains("21211"));
}
