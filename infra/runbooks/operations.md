# Operations runbook (restart, workers, outages)

**Status:** Live procedure for the current phase. Companion script:
`scripts/test-ops.sh` (disposable test stacks only). Backup and restore
live in `backups.md`; deploy and rollback in `deployment.md`/`rollback.md`.

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

## What to record

Every incident note names the failing command, the observed persisted
state (row counts, job states, event counts), the recovery command, and
the post-recovery suite result. Missing live credentials, staging hosts,
or backup destinations block only their own gates and are recorded as
pending — never faked.
