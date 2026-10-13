# Release status

**Updated:** 2026-10-13  
**Branch:** `codex/backend-mvp` (construction; not a launch authorization)  
**Head tested:** `9664da2` (`P18-T06: Configure observability and
operational response`)  
**Live readiness:** NOT READY — no deployment or live verification is
claimed. Every live gate below is pending on owner-supplied inputs.

## Verified locally (disposable stacks, synthetic fixtures)

- Full tier registry green on the tested head: unit, integration (real
  PostgreSQL), api, concurrency, privacy, web component, ops, and the
  critical-mutations guard-sensitivity probes (see
  `docs/engineering/testing.md` for tier definitions).
- Restart/worker/outage recovery, observability wiring, backup/restore
  proofs, migration-safe deploy/rollback procedure, and image/proxy
  topology proven against disposable targets only.
- Known construction follow-up (not a live gate): the serving binary
  merge (production router composition in `main.rs`) is still open, so
  live-HTTP validation runs through explicitly scoped harnesses until it
  lands. Nothing serving production traffic exists.

## Live gates (all pending; each names its missing input)

| Gate | Status | Missing input |
|---|---|---|
| Live phone verification (real provider proof, not fake) | pending | Authorized Verify account/service, credentials, budget, destination scope, consent configuration, one test destination |
| Staging host with real HTTPS origin and cookie transport | pending | Authorized Linux host, access method, domain/DNS, TLS configuration, runtime secrets |
| Off-host backup restore from the actual destination | pending | Approved destination, encryption/key custody, retention, restore credentials, schedule |
| Public launch authorization | pending | Launch city, moderation ownership and hours, user notices, explicit release authorization, pilot targets |

Procedure for each gate: `infra/runbooks/staging-validation.md`. A local
fake, a local restore, or a pushed branch never satisfies a live gate;
missing inputs keep prior local results valid and block only their own
gate.
