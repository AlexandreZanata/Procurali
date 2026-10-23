# Release status

**Updated:** 2026-10-23  
**Branch:** `codex/backend-mvp` (construction; not a launch authorization)  
**Head tested:** `7c3474e` (`P22-T01: Seed the harness state and orchestrate
the disposable e2e stack`; the release-status commit that follows adds these
handoff records only — application code identical)  
**Live readiness:** NOT READY — no deployment or live verification is
claimed. Every live gate below is pending on owner-supplied inputs.
**Delivery:** phase P22 closed with verified GitHub evidence (see the
[changelog](../maintenance/changelog.md)); merging to `main` or
deploying any environment is a separate, unauthorized-by-default step.

## Verified locally (disposable stacks, synthetic fixtures)

- Full tier registry on the tested head with real PostgreSQL where
  required: unit, integration, api, concurrency, privacy, web component,
  ops, load (bounded mix with restriction/expiry/idempotency probes), and
  the critical-mutations guard-sensitivity probes (all 6 guards killed;
  see `docs/engineering/testing.md` for tier definitions).
  `scripts/check-release.sh` (lockfiles, advisories, licenses, production
  refusals) is green.
- E2E over real HTTP against the served application: all 27 registered
  browser specs pass over a disposable database. The permanent harness
  (`scripts/e2e-stack.sh`) starts a disposable database, applies the tracked
  migrations, serves the real application over it, seeds the harness state
  (harness accounts, a withdrawn offer, a suspended demand, and the fixtures
  file the specs read), runs the browser journey, and tears the stack down.
  The specs that previously stayed pending for a missing harness input now
  run: withdrawn-slot permanence, withdrawn-supply refusals, suspended
  visibility, and the staff moderation journey.
- Restore rehearsal on the tested head: API-seeded business rows backed
  up and restored into an isolated target with exact snapshot and
  migration equality; schema agreement, live probes, and one idempotent
  contact replay (same contact, `repeat: true`) all re-verified against
  the restored data.
- Restart/worker/outage recovery, observability wiring, backup/restore
  proofs, migration-safe deploy/rollback procedure, and image/proxy
  topology proven against disposable targets only.
- Served application proven on the tested head: one production
  composition root merges every business boundary into one router under
  the production body/timeout bounds, the privacy headers, and the stable
  unknown-route refusal; the binary serves real HTTP over a disposable
  PostgreSQL, refuses invalid settings, an unreachable database, and a
  mismatched schema with value-free typed refusals, answers liveness and
  pool-backed readiness, drains on SIGTERM, and reports its own health
  through `procurali-backend --health-check`. The built image answers the
  same probes against a disposable database (`scripts/build-artifact.sh
  --serve-smoke`), and the operational smoke flow passes against a served
  target (`scripts/smoke.sh --database-url ... --base-url ...`).

## Changes after the tested matrix

- The request-creation limits now apply where requests are created:
  publication enforces the three-open-slot and rolling activation
  allowance inside its guarded transaction, and renewal refuses to reopen
  an identical open need. The constructor tested this on `148dd89` with
  `check.sh fast` plus the integration, api, concurrency, ops, and privacy
  tiers; the full release matrix and e2e run above remain recorded at
  `7c3474e`.
- The owner notices reach the owner: publication, renewal, and the expiry
  sweep schedule their notices and prompts, and the served worker delivers
  them over real HTTP.

## Known construction follow-ups (not live gates)

- The served binary carries the live Twilio Verify HTTPS transport
  (bounded timeout, no redirects) and serves the live adapter when
  `PHONE_PROVIDER=twilio` is configured. Production startup still waits
  on the owner's Verify credentials and on the authorized staging inputs.
- The owner notices now reach the owner: a publication or renewal schedules
  its cycle's expiry sweep, the first contact of a cycle schedules the
  outcome prompt, and the expiry sweep schedules the renewal/outcome prompt.
  The served worker processes the due jobs, and the owner reads the expiry
  notice and the prompt over real HTTP. One job commits with the business
  fact that schedules it, and the prompt notice stands on the cycle's own
  fact so one fact notifies its recipient once.
- The load-sensitive failures the api and integration tiers showed before
  are resolved on this revision: a restricted seller now refuses contact
  with the documented account-side meaning (so the ban-versus-contact race
  has one defined outcome), the commit-visibility proof waits for the
  commit instead of assuming a fixed delay, and the disposable database
  cleanup terminates the suite's own leftover sessions before dropping it.
  Measured on the tested head: `postgres_atomicity` 0 failures in 10
  consecutive runs (7 in 10 on the unmodified baseline `afd48f9`), and the
  tracked CI entry point (`scripts/ci.sh`) green twice in a row with the
  e2e tier included.
- Nothing serving production traffic exists.

## Live gates (all pending; each names its missing input)

| Gate | Status | Missing input |
|---|---|---|
| Live phone verification (real provider proof, not fake) | pending | Authorized Verify account/service, credentials, budget, destination scope, consent configuration, and one test destination (the live HTTPS transport ships in the build) |
| Staging host with real HTTPS origin and cookie transport | pending | Authorized Linux host, access method, domain/DNS, TLS configuration, runtime secrets |
| Off-host backup restore from the actual destination | pending | Approved destination, encryption/key custody, retention, restore credentials, schedule |
| Public launch authorization | pending | Launch city, moderation ownership and hours, user notices, explicit release authorization, pilot targets |

Procedure for each gate: [`infra/runbooks/staging-validation.md`](../../infra/runbooks/staging-validation.md). A local
fake, a local restore, or a pushed branch never satisfies a live gate;
missing inputs keep prior local results valid and block only their own
gate.

## Integration handoff

A new checkout needs only tracked files: the [root
README](../../README.md) carries the setup, check, test, and operation
commands; [`infra/runbooks/`](../../infra/runbooks/) carries the procedures;
[`contracts/`](../../contracts/) carries the API contract and the scenario
registry. No private workspace is required or referenced. Scope verified is
the mandatory free MVP; mobile apps, Radar Pro sales, subscriptions, and
paid passes remain future. No automatic public deployment follows from
this status.
