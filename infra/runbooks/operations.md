# Operations runbook (restart, workers, outages, observability)

**Status:** Live procedure for the current phase. Companion scripts:
`scripts/test-ops.sh` (disposable restart proof) and `scripts/smoke.sh`
(schema plus minimal flow probe, GET only). Backup and restore live in
`backups.md`; deploy and rollback in `deployment.md`/`rollback.md`.

## Served process

The application image runs the serving binary: validated settings, a bounded
pool, schema agreement, one composed router, and graceful shutdown. Startup is
refused, with a named reason and no value, when a setting is missing, the
database is unreachable, or the applied schema differs from the tracked
migrations. A refused startup never binds a port.

- Health: `GET /health/live` (process-local) and `GET /health/ready`
  (pool-backed; 503 with the dependency name only while the database is
  unusable). The container health check runs
  `procurali-backend --health-check`, which probes readiness once and
  reports the result as its exit code.
- Shutdown: SIGTERM (or an interrupt) drains in-flight requests and exits
  cleanly; the container stop path is bounded by the compose stop grace
  period.
- Configuration: `infra/env.example` lists every setting, including the
  phone identity keys, which must stay stable across restarts. The
  deterministic fake provider and its proof code are refused outside
  disposable stacks.
- Proof: `scripts/build-artifact.sh --serve-smoke` runs the built image
  against a disposable database and requires real liveness, readiness,
  public traffic, a refused cookie-less staff read, a healthy container,
  and a clean drain.

## Restart procedure

1. Restart the database container (`docker restart <container>`); wait for
   the health status to report `healthy` before any further step.
2. Confirm the container actually restarted (started-at timestamp changed)
   and the project answers on its loopback port.
3. Re-run the operational suite (`cargo test --test ops_restart` via
   `scripts/test-ops.sh`); identical business-row outcomes are required.
4. Inspect sessions, state, and job claims after any restart:
   - sessions still authenticate against current server state only;
   - request/offer/contact rows keep their lifecycle state (an outage or
     restart never flips a lifecycle, refunds a quota, or completes work);
   - `background_jobs` rows keep exactly one holder per claim; rows whose
     lease lapsed without settlement return to `queued` for any worker,
     and a replayed completion is refused as stale.

## Worker crash recovery

- A worker that dies between claim and effect settles nothing. Its row stays
  `claimed` until the lease lapses; then any worker may reclaim it, attempts
  counted, and the business effect executes exactly once.
- Repeated sweeps (expiry, prompts, retention) converge on one business
  result: the transition runs once, the fact is recorded once, and the owner
  notice is deduplicated per (event, recipient). Reruns observe the settled
  row instead of writing again.
- Never retry a non-idempotent write without the durable deduplication row,
  and never call an external service inside a retryable transaction.

## Dependency outages

- Provider outage: challenge dispatch and code confirmation fail with the
  static `verification provider unavailable` reason. The failure is the
  proof — no challenge is anchored, no account activates, and no code or
  destination appears in the error. Retry only after the provider recovers.
- Database outage: every guarded operation (contact, worker claim, sweep)
  fails with a static storage reason. No contact is granted, no destination
  is disclosed, and no mutation is saved; restored connections observe the
  identical counts and lifecycle states as before the outage.
- A cached contact destination must never be served after a block,
  suspension, deletion, withdrawal, or expired deadline; retries revalidate
  current eligibility before returning anything.

## Observability (no private data)

Operational indicators carry names and numbers only — readiness words,
pool sizes, job states and ages, schema versions, and stable error codes.
Phone numbers, precise addresses, report identities, private offer terms,
codes, tokens, cookies, and URLs never enter metrics, logs, or smoke
output (`backend/src/operations/metrics.rs`; correlation follows the
redacted telemetry contract).

- Readiness: process liveness plus dependency names (`database`) only.
- Resources: pool size/idle gauges; saturation is inferred from queueing
  and job age, never from invented language capacity numbers.
- Errors: counters keyed by stable codes (`internal`, `unavailable`, ...);
  caller text can never become a series.
- Work age: oldest due unclaimed job and failed-job backlog per
  `job_backlog`; a synthetic failure moves exactly its own indicator.
- Thresholds are host settings (`PROCURALI_OPS_MAX_JOB_AGE_SECS`,
  `PROCURALI_OPS_MAX_FAILED_JOBS`), documented starting points for this
  single host — not performance promises. `scripts/smoke.sh --self-test`
  proves the probe logic; `scripts/smoke.sh --database-url ... --base-url
  ...` validates one real target (GET only).

## Operational response

- Database or pool saturation: stop writes at the proxy if needed, inspect
  pool gauges and the oldest due job age, drain through the bounded worker,
  and scale the host — never bypass eligibility, quotas, or the provider
  boundary to shed load.
- Worker lag: a due backlog older than the host job-age threshold pages a
  human; recovery is reclaim-after-lease with attempts counted, never a
  duplicate effect. Failed rows stay failed with their static reason until
  a human dispositions them.
- Provider errors: verification and handoff stay in the failed-proof state
  (no anchor, no activation, no disclosure) until the provider recovers;
  retries revalidate current eligibility first.
- Failed backups: keep serving from the live database, preserve the last
  proven artifact, and re-run `scripts/backup.sh` plus
  `scripts/restore-test.sh` before claiming recoverability again.
- Moderation backlog: review queues are worked oldest-first by accountable
  staff through the audit-backed moderation writers (actor, reason, scope,
  time, and previous/resulting eligibility on every decision); backlogs
  never auto-approve, auto-ban, or silently drop reports.

## What to record

Every incident note names the failing command, the observed persisted
state (row counts, job states, event counts), the recovery command, and
the post-recovery suite result. Missing live credentials, staging hosts,
or backup destinations block only their own gates and are recorded as
pending — never faked.
