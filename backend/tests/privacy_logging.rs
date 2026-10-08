//! Privacy acceptance for structured observability (INV-31).
//!
//! Synthetic canaries stand in for phone destinations, session material,
//! verification codes, and keys. Every canary must be absent from captured logs
//! and failure formatting, while correlation ids and safe codes stay observable.
//! Canaries are unique per test so parallel execution cannot cross-contaminate.

use procurali_backend::operations::config::ConfigError;
use procurali_backend::operations::telemetry::{
    log_event, CorrelationId, ErrorCode, RequestError, RouteLabel,
};
use std::io;
use std::sync::{Arc, Mutex, Once};

/// Shared byte sink for the JSON log subscriber installed once per process.
#[derive(Clone, Default)]
struct Capture {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buf
            .lock()
            .map_err(|_| io::Error::other("capture lock poisoned"))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn capture_handle() -> Capture {
    static INIT: Once = Once::new();
    static mut SINK: Option<Capture> = None;
    // Single global subscriber: `tracing` allows exactly one per process.
    // SAFETY: guarded by `INIT`; written once before any reader proceeds.
    unsafe {
        INIT.call_once(|| {
            let sink = Capture::default();
            let writer = sink.clone();
            let subscriber = tracing_subscriber::fmt()
                .json()
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .finish();
            let _ = tracing::subscriber::set_global_default(subscriber);
            SINK = Some(sink);
        });
        (*std::ptr::addr_of!(SINK))
            .clone()
            .expect("capture installed")
    }
}

fn captured_text() -> String {
    let sink = capture_handle();
    let guard = sink.buf.lock().expect("capture lock healthy");
    String::from_utf8_lossy(&guard).into_owned()
}

#[test]
fn correlation_and_safe_error_code_are_observable() {
    let _sink = capture_handle();
    let correlation = CorrelationId::sanitize("req-abc123_T03");
    assert_eq!(correlation.as_str(), "req-abc123_T03");
    log_event(
        correlation.as_str(),
        ErrorCode::InvalidSetting,
        "health-live",
        400,
        3,
        "scaffold-event",
    );
    let logs = captured_text();
    assert!(logs.contains("req-abc123_T03"));
    assert!(logs.contains("invalid_setting"));
    assert!(logs.contains("health-live"));
}

#[test]
fn canary_secrets_are_absent_from_logs_and_failure_formatting() {
    // Realistic secret shapes: an international phone, a session cookie, a
    // numeric verification code, and a base64-ish key. None satisfies the
    // caller-token grammar ('+', spaces, parens, '=', ';', pure digits).
    const PHONE: &str = "+55 (11) 90000-0000";
    const SESSION: &str = "procurali_session=abc123; Path=/; HttpOnly";
    const CODE: &str = "042017";
    const KEY: &str = "dGVzdC1rZXktUMDMvNDI+/=";
    let _sink = capture_handle();
    // Push every canary through every caller-controlled entry point.
    log_event(
        PHONE,
        ErrorCode::from_config(&ConfigError::Missing("SESSION_KEY")),
        SESSION,
        500,
        1,
        "canary-probe",
    );
    log_event(CODE, ErrorCode::Internal, KEY, 500, 1, "canary-probe");
    // The sanitizer must have replaced, not reflected, each hostile identifier.
    for canary in [PHONE, SESSION, CODE, KEY] {
        assert_ne!(
            CorrelationId::sanitize(canary).as_str(),
            canary,
            "sanitizer must replace secret-shaped input"
        );
    }
    let failure = RequestError::new(
        CorrelationId::sanitize(SESSION),
        ErrorCode::from_config(&ConfigError::Invalid("DATABASE_URL")),
    );
    let rendered = format!("{failure:?} {failure}");
    for canary in [PHONE, SESSION, CODE, KEY] {
        assert!(
            !rendered.contains(canary),
            "failure formatting leaked a canary"
        );
    }
    let logs = captured_text();
    for canary in [PHONE, SESSION, CODE, KEY] {
        assert!(!logs.contains(canary), "captured logs leaked a canary");
    }
    // The events themselves are still recorded with safe fields.
    assert!(logs.contains("missing_setting"));
    assert!(logs.contains("internal"));
}

#[test]
fn hostile_correlation_cannot_inject_extra_log_lines() {
    let _sink = capture_handle();
    // A caller-controlled value attempting a newline break plus a JSON fragment.
    let hostile = "abc\n{\"injected\":true}\nEVIL-T03-LINE";
    let id = CorrelationId::sanitize(hostile);
    assert_ne!(id.as_str(), hostile);
    assert!(!id.as_str().contains('\n'));
    log_event(
        hostile,
        ErrorCode::Unavailable,
        hostile,
        503,
        2,
        "injection-probe",
    );
    let logs = captured_text();
    assert!(!logs.contains("EVIL-T03-LINE"));
    assert!(!logs.contains("{\"injected\":true}"));
    // Unknown routes collapse to the fixed label instead of reflecting input.
    assert_eq!(RouteLabel::sanitize(hostile).as_str(), "unknown-route");
    assert_eq!(
        RouteLabel::sanitize("health-ready").as_str(),
        "health-ready"
    );
    assert!(logs.contains("unavailable"));
}
