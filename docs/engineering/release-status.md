# Release status

**Updated:** 2026-10-13  
**Branch:** `codex/backend-mvp` (construction; not a launch authorization)  
**Head tested:** `b3b4e55` (`P19-T04: Publish truthful engineering
completion and integration handoff`; the P19-T05 delivery commit adds
handoff records only — code identical)  
**Live readiness:** NOT READY — no deployment or live verification is
claimed. Every live gate below is pending on owner-supplied inputs.
**Delivery:** phase P19 closed with verified GitHub evidence (see the
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
- Known construction follow-ups (not live gates): the serving-binary
  merge (production router composition in `main.rs`) is still open, so
  live-HTTP validation runs through explicitly scoped harnesses until it
  lands; renewal-path duplicate enforcement and test-cleanup hardening
  are filed follow-ups. Nothing serving production traffic exists.

## Live gates (all pending; each names its missing input)

| Gate | Status | Missing input |
|---|---|---|
| Live phone verification (real provider proof, not fake) | pending | Authorized Verify account/service, credentials, budget, destination scope, consent configuration, one test destination |
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
commands; [`infra/runbooks/`](../../infra/runbooks/) carries the procedures; [`contracts/`](../../contracts/)
carries the API contract and the scenario registry. No private workspace
is required or referenced. Scope verified is the mandatory free MVP;
mobile apps, Radar Pro sales, subscriptions, and paid passes remain
future. No automatic public deployment follows from this status.
