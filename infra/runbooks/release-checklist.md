# Release checklist (final baseline acceptance)

**Status:** Live procedure for the release cut. Companion status:
`docs/engineering/release-status.md`. Staging detail:
`staging-validation.md`. Backup detail: `backups.md`.

## 1. Release check (offline gates first)

Run `scripts/check-release.sh` on the final revision: exact lockfiles,
no unsound/yanked/unmaintained or high/critical advisories (one recorded
exception), no strong-copyleft surprise, and production configuration
that refuses fakes and missing secrets. Any failure stops the cut.

## 2. Full tier registry on a fresh stack

From a fresh disposable stack on the final revision, run every
mandatory tier with real PostgreSQL where required: unit, integration,
api, concurrency, privacy, web, e2e, ops, and critical-mutations, plus
`scripts/check.sh fast`. E2E needs a live disposable backend over HTTP;
specs requiring the external harness seeder (fixtures file, harness
accounts) stay pending with the exact missing input instead of failing
the cut — the harness seeder is a recorded follow-up, not a silent skip.
Zero discovered tests, skips, or `#[ignore]` anywhere fail the cut.

The serving proof belongs to the same cut: `scripts/build-artifact.sh
--serve-smoke` starts the built image against a disposable database and
requires real liveness, readiness, public traffic, a refused cookie-less
staff read, a healthy container, and a clean SIGTERM drain.

## 3. Restore rehearsal on the final revision

Seed representative business rows through the real API on a disposable
database (accounts, demand, offer, contact), take a backup with
`scripts/backup.sh`, restore it into an isolated target with
`scripts/restore-test.sh`, and confirm exact snapshot and migration
equality. Then rerun core invariants against the restored data: schema
agreement plus live probes (`scripts/smoke.sh`) and one idempotent
contact replay converging on the pre-restore contact. Any drift fails
the cut.

## 4. Reconcile and record

Update `docs/engineering/release-status.md` with the tested SHA, tier
results, the restore evidence, and the external gates — each pending
gate naming its missing owner input. Unchecked or unavailable checks
remain pending; nothing pending becomes a pass without a new run after
its input lands. Push only through the policy-required phase path.
