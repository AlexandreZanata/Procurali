# Testing guide (proposed)

**Status:** Proposed expectations; no test suite exists yet. Later `scripts/` wrappers and
CI invoke these tiers from tracked files only — never from `.local/`.

## Tiers and commands

| Tier | Command (from P01/P02 onward) | What it proves |
|---|---|---|
| unit | `./scripts/test.sh unit` | Domain values, boundaries, policy arithmetic, snapshot expectations (deterministic clock/seams) |
| integration | `./scripts/test.sh integration` | Real PostgreSQL migrations, constraints, guarded transactions, events, rollback, reload |
| api | `./scripts/test.sh api` | HTTP routes, exact payloads/errors, real cookie auth, ownership/role matrix |
| concurrency | `./scripts/test.sh concurrency` | Barrier-coordinated races with exact persisted count/state/event assertions |
| privacy | `./scripts/test.sh privacy` | Canary non-disclosure, DTO allowlists, handoff gating, log/header/cache checks |
| web | `./scripts/test.sh web` | TS checks, component/unit behavior (no full-integration claim) |
| e2e | `./scripts/test.sh e2e` | Playwright buyer/seller/staff journeys on a real disposable backend/database |
| ops | `./scripts/test.sh ops` | Migration/restart/worker/config/backup-restore/deployment smoke on configured targets |
| critical-mutations | `./scripts/test.sh critical-mutations` | Fault-injection sensitivity probes in an isolated copy (P17) |
| load | `./scripts/test.sh load` | Bounded load + correctness probes on an explicitly selected disposable/staging target |

`./scripts/test.sh all` runs every tier available at the current phase; the final
baseline means all mandatory tiers. `./scripts/check.sh fast` is the universal quick
gate (format, Clippy deny-warnings, focused units, contract checks, private-file guard).

## Adding tests (`--task` filtering and discovery)

Each implementation task registers its named tests in `contracts/scenario-tests.json`
(scenario ID -> suite). Wrappers support `./scripts/test.sh <tier> --task <TASK-ID>`
to run one card's scope, and every tier fails on zero discovered tests. A new test is
meaningless unless it fails before the fix and passes after (P17 probes prove guards
are observed, not merely covered).

## Fixture isolation and the database contract

Integration and above require an explicit `TEST_DATABASE_URL` pointing at a disposable
`procurali_test_*` database; suites refuse the application/staging database. Each suite
gets an isolated schema/data (or a separate disposable database) with reliable cleanup
and synthetic fixtures of known identity. Database absence, version mismatch, missing
migration, or zero discovered tests is an error — never a skipped success, never
`#[ignore]`, never SQLite-or-memory substitution.

## Assertion standard

Every business writer needs at least: one valid case, invalid input, wrong owner/role,
ineligible account, forbidden resource state, applicable block/category/deadline
restriction, duplicate/retry behavior, persisted-state reload through a new connection,
exact event count, and absence of prohibited disclosure. Expectations are written from
canonical rules, never derived from the function under test. Money/state/count/
visibility values appear literally (e.g. one request with two distinct seller contacts
=> contacts/request 2, coverage 100%, zero inferred sales).

## Local versus live evidence

The deterministic fake provider, disposable databases, and local Compose prove local
correctness only. Live phone verification, actual-host deployment, and off-host backup
restore are separate gates requiring owner-supplied credentials/hosts/destinations;
their absence blocks only those gates and is recorded, never faked.

## Scenario registry

`contracts/scenario-tests.json` (v1, generated 2026-10-08 from the traceability
dispositions and task tiers): 132 scenarios — 47 mandatory AC, 35 mandatory EC,
42 mandatory INV (all `planned`, none passing) plus 8 explicitly future-deferred —
and the 14 mandatory race cases with their proving tasks. P17-T01 reconciles this
registry against actually discovered tests and fails the gate if any mandatory ID
lacks a real assertion.
