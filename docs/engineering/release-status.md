# Release status

**Updated:** 2026-10-20  
**Branch:** `codex/backend-mvp` (construction; not a launch authorization)  
**Head tested:** `5340d60` (`P21-T02: Prove live HTTP serving on a
disposable stack`; the release-status commit that follows adds these
handoff records only — application code identical)  
**Live readiness:** NOT READY — no deployment or live verification is
claimed. Every live gate below is pending on owner-supplied inputs.
**Delivery:** phase P21 closed with verified GitHub evidence (see the
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
- E2E over real HTTP against a disposable backend: 20 of 27 specs pass,
  including the full buyer/seller loop with two contacts and the safety
  alternatives. The remaining 7 need the external e2e harness seeder
  (fixtures file, harness accounts) and stay pending — recorded, not
  skipped.
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

## Known construction follow-ups (not live gates)

- The permanent e2e harness seeder, renewal-path duplicate enforcement,
  and test-cleanup hardening remain open.
- The live Twilio Verify HTTPS transport is not part of this build: the
  served binary refuses `PHONE_PROVIDER=twilio` instead of pretending a
  proof was delivered. Production startup therefore still waits on that
  transport and on the authorized staging inputs.
- Business flows still do not enqueue background jobs: expiry notices and
  outcome prompts are implemented and proven at the application level, but
  nothing schedules them yet, so the served worker currently has no work
  to claim.
- The integration tier shows load-sensitive flakes in three race suites
  (timing assertions and suite database cleanup) on this workstation; the
  same suites pass in isolation and the same failures reproduce on the
  unmodified baseline revision.
- Nothing serving production traffic exists.

## Live gates (all pending; each names its missing input)

| Gate | Status | Missing input |
|---|---|---|
| Live phone verification (real provider proof, not fake) | pending | Authorized Verify account/service, credentials, budget, destination scope, consent configuration, one test destination, and the live HTTPS transport for it |
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
