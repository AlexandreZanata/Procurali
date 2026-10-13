//! Operational indicators and response thresholds without private data.
//!
//! Canonical rules: INV-31 (public pages, logs, and metrics expose no phone
//! numbers, precise addresses, report identities, or private offer terms —
//! every counter here carries indicator names and numbers only) and INV-37
//! (administrative changes record actor, reason, scope, time, and
//! previous/resulting eligibility — the response runbook points at the
//! existing audit-backed moderation writers; this module contributes the
//! redacted operational half).
//!
//! Thresholds are actual-host operational settings read from the
//! environment, never language performance promises: they name how long one
//! deployment tolerates an unclaimed job or a failed backlog before a human
//! looks, with explicit defaults documented as starting points for the
//! single host. A synthetic failure must move exactly its own indicator;
//! rendering never reflects caller text, phones, or secrets.

use super::telemetry::ErrorCode;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

/// Default ceiling for an unclaimed due job before it pages a human, in
/// seconds. Starting point for the single host; override per host.
pub const DEFAULT_MAX_JOB_AGE_SECS: i64 = 900;

/// Default ceiling for failed jobs kept for inspection before a human
/// looks, in rows. Starting point for the single host; override per host.
pub const DEFAULT_MAX_FAILED_JOBS: i64 = 50;

/// Bounds for host overrides: below the floor pages on normal backoff, past
/// the ceiling indicates misconfiguration. Digits only — a leading `+`
/// (phone-shaped input) never parses as a threshold.
pub const MIN_JOB_AGE_SECS: i64 = 60;
/// See [`MIN_JOB_AGE_SECS`].
pub const LIMIT_JOB_AGE_SECS: i64 = 2_592_000;
/// See [`MIN_JOB_AGE_SECS`].
pub const MIN_FAILED_JOBS: i64 = 1;
/// See [`MIN_JOB_AGE_SECS`].
pub const LIMIT_FAILED_JOBS: i64 = 100_000;

/// Environment source for [`OperationalThresholds::from_env`].
pub const MAX_JOB_AGE_ENV: &str = "PROCURALI_OPS_MAX_JOB_AGE_SECS";
/// Environment source for [`OperationalThresholds::from_env`].
pub const MAX_FAILED_JOBS_ENV: &str = "PROCURALI_OPS_MAX_FAILED_JOBS";

/// Host operational settings: how long this deployment tolerates trouble
/// before a human looks. Numbers only; no performance promises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationalThresholds {
    /// Unclaimed due-job age that pages a human, in seconds (at least 60).
    pub max_job_age_secs: i64,
    /// Failed-job rows that page a human (at least 1).
    pub max_failed_jobs: i64,
}

impl OperationalThresholds {
    /// Host defaults: starting points, not capacity claims.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            max_job_age_secs: DEFAULT_MAX_JOB_AGE_SECS,
            max_failed_jobs: DEFAULT_MAX_FAILED_JOBS,
        }
    }

    /// Read host overrides from the environment. Missing keys keep defaults;
    /// malformed or out-of-range values are refused with a static reason.
    ///
    /// # Errors
    ///
    /// Returns [`ThresholdError::InvalidValue`] for a non-numeric or
    /// out-of-range override. Names the variable, never its value.
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Result<Self, ThresholdError> {
        let mut thresholds = Self::defaults();
        if let Some(raw) = get(MAX_JOB_AGE_ENV) {
            thresholds.max_job_age_secs = parse_threshold(&raw, MAX_JOB_AGE_ENV)?;
            if !(MIN_JOB_AGE_SECS..=LIMIT_JOB_AGE_SECS).contains(&thresholds.max_job_age_secs) {
                return Err(ThresholdError::InvalidValue(MAX_JOB_AGE_ENV));
            }
        }
        if let Some(raw) = get(MAX_FAILED_JOBS_ENV) {
            thresholds.max_failed_jobs = parse_threshold(&raw, MAX_FAILED_JOBS_ENV)?;
            if !(MIN_FAILED_JOBS..=LIMIT_FAILED_JOBS).contains(&thresholds.max_failed_jobs) {
                return Err(ThresholdError::InvalidValue(MAX_FAILED_JOBS_ENV));
            }
        }
        Ok(thresholds)
    }
}

/// Parse a digits-only threshold. Signs, spaces inside the digits, decimals,
/// and empty input are refused: a phone-shaped value must never parse.
fn parse_threshold(raw: &str, variable: &'static str) -> Result<i64, ThresholdError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ThresholdError::InvalidValue(variable));
    }
    trimmed
        .parse()
        .map_err(|_| ThresholdError::InvalidValue(variable))
}

/// Typed threshold failure. Names the variable, never its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThresholdError {
    /// An override is non-numeric or out of range; carries the variable name.
    InvalidValue(&'static str),
}

impl std::fmt::Display for ThresholdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidValue(_) => f.write_str("invalid operational threshold"),
        }
    }
}

impl std::error::Error for ThresholdError {}

/// In-memory error counters keyed by static codes only. Caller text has no
/// parameter here, so hostile or secret-shaped input cannot become a series.
#[derive(Debug, Default)]
pub struct ErrorCounters {
    counts: Mutex<HashMap<&'static str, u64>>,
}

impl ErrorCounters {
    /// Empty counters.
    #[must_use]
    pub fn new() -> Self {
        Self {
            counts: Mutex::new(HashMap::new()),
        }
    }

    /// Record one failure under its stable code.
    pub fn record(&self, code: ErrorCode) {
        let mut counts = self.counts.lock().expect("counters lock");
        *counts.entry(code.as_str()).or_insert(0) += 1;
    }

    /// Current counts by stable code name.
    #[must_use]
    pub fn snapshot(&self) -> BTreeMap<&'static str, u64> {
        self.counts
            .lock()
            .expect("counters lock")
            .iter()
            .map(|(code, count)| (*code, *count))
            .collect()
    }
}

/// Readiness with dependency names only — never values or credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyReadiness {
    /// True when every prerequisite is usable.
    pub ready: bool,
    /// Unavailable dependency names (e.g. `"database"`).
    pub unavailable: Vec<&'static str>,
}

/// Connection-pool gauges: sizes only, no URLs or credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolGauges {
    /// Open connections.
    pub size: u32,
    /// Idle connections.
    pub idle: u32,
}

/// Background-work backlog: states and oldest due age only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobBacklog {
    /// Rows waiting for a worker.
    pub queued: i64,
    /// Rows held by a worker.
    pub claimed: i64,
    /// Rows settled as failed.
    pub failed: i64,
    /// Age of the oldest due unclaimed job, in seconds, if any is due.
    pub oldest_due_age_secs: Option<i64>,
}

/// One redacted operational snapshot: names and numbers only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationalSnapshot {
    /// Dependency readiness.
    pub readiness: DependencyReadiness,
    /// Pool gauges.
    pub pool: PoolGauges,
    /// Work backlog.
    pub jobs: JobBacklog,
    /// Applied schema version (migration count agreement is separate).
    pub schema_version: i64,
    /// Error counts by stable code.
    pub errors: BTreeMap<&'static str, u64>,
}

impl OperationalSnapshot {
    /// Indicator names currently past their host thresholds. A synthetic
    /// failure moves exactly its own indicator: an old due job breaches
    /// only `"job-age"`, a failed backlog only `"failed-jobs"`, and a down
    /// dependency only `"readiness"`.
    #[must_use]
    pub fn breaches(&self, thresholds: &OperationalThresholds) -> Vec<&'static str> {
        let mut breached = Vec::new();
        if !self.readiness.ready {
            breached.push("readiness");
        }
        if self
            .jobs
            .oldest_due_age_secs
            .is_some_and(|age| age > thresholds.max_job_age_secs)
        {
            breached.push("job-age");
        }
        if self.jobs.failed > thresholds.max_failed_jobs {
            breached.push("failed-jobs");
        }
        breached
    }

    /// Render `indicator=value` lines with numbers and fixed words only.
    /// There is no caller-text parameter anywhere in this module, so no
    /// phone, token, cookie, or secret can reach the output by construction.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut lines = vec![
            format!(
                "readiness={}",
                if self.readiness.ready {
                    "ready"
                } else {
                    "not-ready"
                }
            ),
            format!("unavailable={}", self.readiness.unavailable.join(",")),
            format!("pool_size={}", self.pool.size),
            format!("pool_idle={}", self.pool.idle),
            format!("jobs_queued={}", self.jobs.queued),
            format!("jobs_claimed={}", self.jobs.claimed),
            format!("jobs_failed={}", self.jobs.failed),
            format!(
                "oldest_due_age_secs={}",
                self.jobs
                    .oldest_due_age_secs
                    .map_or_else(|| "none".to_owned(), |age| age.to_string())
            ),
            format!("schema_version={}", self.schema_version),
        ];
        for (code, count) in &self.errors {
            lines.push(format!("error_{code}={count}"));
        }
        lines.join("\n")
    }
}

/// Typed metrics failure. Static reasons only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricsError {
    /// The lookup failed.
    StorageFailed,
}

impl std::fmt::Display for MetricsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("operational metrics unavailable")
    }
}

impl std::error::Error for MetricsError {}

/// Read pool gauges from a live pool. Sizes only.
#[must_use]
pub fn pool_gauges(pool: &sqlx::PgPool) -> PoolGauges {
    PoolGauges {
        size: pool.size(),
        idle: u32::try_from(pool.num_idle()).unwrap_or(u32::MAX),
    }
}

/// Read the background-work backlog and the oldest due age in seconds.
/// Due means queued past `not_before` with remaining budget, at `now`.
/// Claim-held rows are counted, never stolen or mutated here.
///
/// # Errors
///
/// Returns [`MetricsError::StorageFailed`] when the lookup fails.
pub async fn job_backlog(
    pool: &sqlx::PgPool,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<JobBacklog, MetricsError> {
    let row: (i64, i64, i64, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT
           count(*) FILTER (WHERE state = 'queued'),
           count(*) FILTER (WHERE state = 'claimed'),
           count(*) FILTER (WHERE state = 'failed'),
           min(not_before) FILTER (
             WHERE state = 'queued' AND not_before <= $1 AND attempts < max_attempts
           )
         FROM background_jobs",
    )
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(|_| MetricsError::StorageFailed)?;
    Ok(JobBacklog {
        queued: row.0,
        claimed: row.1,
        failed: row.2,
        oldest_due_age_secs: row.3.map(|due| (now - due).num_seconds().max(0)),
    })
}

/// Read the applied schema version (highest recorded migration).
///
/// # Errors
///
/// Returns [`MetricsError::StorageFailed`] when the lookup fails.
pub async fn schema_version(pool: &sqlx::PgPool) -> Result<i64, MetricsError> {
    sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations")
        .fetch_one(pool)
        .await
        .map_err(|_| MetricsError::StorageFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn thresholds_default_and_parse_host_overrides() {
        let defaults = OperationalThresholds::from_env(env_of(&[])).expect("defaults parse");
        assert_eq!(defaults, OperationalThresholds::defaults());
        assert_eq!(defaults.max_job_age_secs, DEFAULT_MAX_JOB_AGE_SECS);
        let custom = OperationalThresholds::from_env(env_of(&[
            (MAX_JOB_AGE_ENV, "120"),
            (MAX_FAILED_JOBS_ENV, "3"),
        ]))
        .expect("overrides parse");
        assert_eq!(custom.max_job_age_secs, 120);
        assert_eq!(custom.max_failed_jobs, 3);
        for bad in [
            "sixty",
            "",
            "-5",
            "0",
            "+120",
            "12.5",
            "  ",
            "99999999999999999999",
        ] {
            assert!(
                OperationalThresholds::from_env(env_of(&[(MAX_JOB_AGE_ENV, bad)])).is_err(),
                "non-numeric, signed, or out-of-range job age refused: {bad}"
            );
        }
        assert!(OperationalThresholds::from_env(env_of(&[(MAX_FAILED_JOBS_ENV, "0")])).is_err());
    }

    #[test]
    fn synthetic_failure_moves_exactly_its_own_indicator() {
        let thresholds = OperationalThresholds::defaults();
        let fresh = OperationalSnapshot {
            readiness: DependencyReadiness {
                ready: true,
                unavailable: vec![],
            },
            pool: PoolGauges { size: 5, idle: 4 },
            jobs: JobBacklog {
                queued: 1,
                claimed: 0,
                failed: 0,
                oldest_due_age_secs: Some(10),
            },
            schema_version: 18,
            errors: BTreeMap::new(),
        };
        assert!(fresh.breaches(&thresholds).is_empty());
        let old = OperationalSnapshot {
            jobs: JobBacklog {
                oldest_due_age_secs: Some(DEFAULT_MAX_JOB_AGE_SECS + 1),
                ..fresh.jobs
            },
            ..fresh.clone()
        };
        assert_eq!(old.breaches(&thresholds), vec!["job-age"]);
        let failed = OperationalSnapshot {
            jobs: JobBacklog {
                failed: DEFAULT_MAX_FAILED_JOBS + 1,
                ..fresh.jobs
            },
            ..fresh.clone()
        };
        assert_eq!(failed.breaches(&thresholds), vec!["failed-jobs"]);
        let down = OperationalSnapshot {
            readiness: DependencyReadiness {
                ready: false,
                unavailable: vec!["database"],
            },
            ..fresh.clone()
        };
        assert_eq!(down.breaches(&thresholds), vec!["readiness"]);
    }

    #[test]
    fn counters_accept_stable_codes_only() {
        let counters = ErrorCounters::new();
        counters.record(ErrorCode::Unavailable);
        counters.record(ErrorCode::Unavailable);
        counters.record(ErrorCode::Internal);
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.get("unavailable"), Some(&2));
        assert_eq!(snapshot.get("internal"), Some(&1));
        assert_eq!(snapshot.len(), 2);
    }

    #[test]
    fn rendering_carries_no_private_material() {
        let canary_phone = "+5511987654321";
        let snapshot = OperationalSnapshot {
            readiness: DependencyReadiness {
                ready: true,
                unavailable: vec![],
            },
            pool: PoolGauges { size: 5, idle: 4 },
            jobs: JobBacklog {
                queued: 0,
                claimed: 0,
                failed: 0,
                oldest_due_age_secs: None,
            },
            schema_version: 18,
            errors: ErrorCounters::new().snapshot(),
        };
        // The module accepts no caller text except threshold numbers, so a
        // phone-shaped override must surface as a static refusal naming the
        // variable — never the value.
        let refused = OperationalThresholds::from_env(env_of(&[(MAX_JOB_AGE_ENV, canary_phone)]))
            .expect_err("phone-shaped threshold refused");
        let rendered = format!("{snapshot:?} {} {refused:?} {refused}", snapshot.to_text());
        assert!(!rendered.contains(canary_phone), "no destination in output");
        assert!(!rendered.contains("token"), "no secret field in output");
        assert!(
            !rendered.contains("cookie"),
            "no session material in output"
        );
        assert!(snapshot.to_text().contains("readiness=ready"));
        for text in [
            format!(
                "{:?} {}",
                ThresholdError::InvalidValue(MAX_JOB_AGE_ENV),
                ThresholdError::InvalidValue(MAX_JOB_AGE_ENV)
            ),
            format!(
                "{:?} {}",
                MetricsError::StorageFailed,
                MetricsError::StorageFailed
            ),
        ] {
            assert!(!text.contains(canary_phone));
            assert!(!text.contains("https://"));
        }
    }
}
