//! Deadline-vector acceptance: the frozen time vectors from
//! `contracts/domain-vectors.json` plus seven-day cycles, rolling windows,
//! preserved publication age, and the production/test clock contract.
//! Fixed instants only — no multi-day sleeps. The final test needs a live
//! database (proves the production clock agrees with PostgreSQL time).

use procurali_backend::application::clock::{Clock, FixedClock, SystemClock};
use procurali_backend::domain::time::{Instant, CYCLE_SECONDS};
use serde_json::Value;
use std::time::Duration;

fn instant(text: &str) -> Instant {
    Instant::parse_rfc3339(text).expect("frozen test instant parses")
}

#[test]
fn contract_deadline_vectors_hold_exactly() {
    let text = std::fs::read_to_string("../contracts/domain-vectors.json")
        .expect("frozen domain vectors are readable");
    let contract: Value = serde_json::from_str(&text).expect("vectors parse");
    let deadlines = &contract["deadlines"];
    let deadline = instant(deadlines["deadline"].as_str().expect("deadline"));
    let cases = deadlines["cases"].as_array().expect("boundary cases");
    assert_eq!(cases.len(), 3, "before/equal/after triple");
    for case in cases {
        let at = instant(case["at"].as_str().expect("instant"));
        let eligible = case["eligible"].as_bool().expect("verdict");
        assert_eq!(
            Instant::is_eligible_at(at, deadline),
            eligible,
            "exclusive boundary at {}",
            case["at"].as_str().unwrap_or_default()
        );
    }
    // Equality denies, through a frozen clock too.
    let frozen = FixedClock::new(instant("2026-10-15T12:00:00Z"));
    assert!(!Instant::is_eligible_at(frozen.now(), deadline));

    for bad in [
        "15/10/2026",
        "2026-10-15 12:00:00",
        "2026-10-15",
        "not-a-time",
        "",
    ] {
        assert!(
            Instant::parse_rfc3339(bad).is_err(),
            "{bad:?} is refused without guessing"
        );
    }
    // Explicit offsets normalize to the same UTC instant.
    assert_eq!(
        instant("2026-10-15T14:00:00+02:00"),
        instant("2026-10-15T12:00:00Z")
    );
}

#[test]
fn seven_day_cycle_needs_no_waiting() {
    assert_eq!(CYCLE_SECONDS, 7 * 24 * 60 * 60);
    let published = instant("2026-10-08T12:00:00Z");
    let deadline = published.cycle_deadline().expect("cycle computes");
    assert_eq!(deadline.format(), "2026-10-15T12:00:00Z");
    let clock = FixedClock::new(published);
    assert!(Instant::is_eligible_at(clock.now(), deadline));
    // Six days, 23:59:59 later: still eligible; one second more: expired.
    clock.advance_by(Duration::from_secs(
        6 * 24 * 60 * 60 + 23 * 60 * 60 + 59 * 60 + 59,
    ));
    assert!(Instant::is_eligible_at(clock.now(), deadline));
    clock.advance_by(Duration::from_secs(1));
    assert_eq!(clock.now(), deadline);
    assert!(!Instant::is_eligible_at(clock.now(), deadline));
}

#[test]
fn rolling_windows_use_actual_elapsed_time() {
    let now = instant("2026-10-08T12:00:00Z");
    let day = Duration::from_secs(24 * 60 * 60);
    // Exactly 24h ago is outside (exclusive start); one second later is inside.
    assert!(!Instant::is_within_window(
        instant("2026-10-07T12:00:00Z"),
        now,
        day
    ));
    assert!(Instant::is_within_window(
        instant("2026-10-07T12:00:01Z"),
        now,
        day
    ));
    // Future events never count, even inside the span.
    assert!(!Instant::is_within_window(
        instant("2026-10-08T12:00:01Z"),
        now,
        day
    ));
    // Six activations in 24h: the sixth fits, the seventh does not (counting
    // stays with the caller; the window primitive decides membership).
    let events = [
        "2026-10-07T13:00:00Z",
        "2026-10-07T15:00:00Z",
        "2026-10-07T18:00:00Z",
        "2026-10-07T21:00:00Z",
        "2026-10-08T06:00:00Z",
        "2026-10-08T11:00:00Z",
    ];
    let inside = events
        .iter()
        .filter(|each| Instant::is_within_window(instant(each), now, day))
        .count();
    assert_eq!(inside, 6);
}

#[test]
fn renewal_keeps_original_publication_age() {
    // First publication T0; renewal at T0+3d starts a fresh cycle, while age
    // still counts from T0 (composition of two independent primitives).
    let first_publication = instant("2026-10-01T12:00:00Z");
    let renewal = instant("2026-10-04T12:00:00Z");
    let second_deadline = renewal.cycle_deadline().expect("renewal cycle");
    assert_eq!(second_deadline.format(), "2026-10-11T12:00:00Z");
    assert_eq!(
        second_deadline.elapsed_seconds_since(first_publication),
        10 * 24 * 60 * 60,
        "age counts from first publication, not renewal"
    );
    assert_eq!(
        second_deadline.elapsed_seconds_since(second_deadline),
        0,
        "a fresh deadline has consumed nothing"
    );
}

#[test]
fn system_clock_takes_no_input_and_fixed_clock_stays_local() {
    // Two production reads are ordered and close together in real time.
    let first = SystemClock.now();
    let second = SystemClock.now();
    assert!(first <= second);
    // Manipulating a fixed clock cannot move the production clock.
    let frozen = FixedClock::new(instant("2001-01-01T00:00:00Z"));
    frozen.advance_by(Duration::from_secs(24 * 60 * 60));
    assert_eq!(frozen.now().format(), "2001-01-02T00:00:00Z");
    assert!(SystemClock.now() > frozen.now());
    // Production configuration refuses test-only clock switches; that gate is
    // proven by `test_clock_switch_is_rejected_outside_test_doubles`.
}

#[tokio::test]
async fn system_clock_agrees_with_postgresql_time() {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point at a disposable database");
    assert!(
        url.contains("/procurali_test_"),
        "refusing non-disposable TEST_DATABASE_URL"
    );
    let pool = sqlx::PgPool::connect(&url)
        .await
        .expect("test postgres reachable");
    let db_now: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&pool)
        .await
        .expect("database clock reads");
    pool.close().await;
    let db_instant =
        Instant::parse_rfc3339(&db_now.to_rfc3339()).expect("database timestamp parses");
    let local = SystemClock.now();
    let drift = local
        .elapsed_seconds_since(db_instant)
        .abs()
        .max(db_instant.elapsed_seconds_since(local));
    assert!(
        drift <= 5,
        "production clock agrees with PostgreSQL within 5s (drift {drift}s)"
    );
}
