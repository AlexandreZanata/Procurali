# Testing guide (proposed)

**Status:** Proposed expectations with live scaffold registration. `scripts/` wrappers
invoke these tiers from tracked files only — never from `.local/`.

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
| critical-mutations | `./scripts/test.sh critical-mutations` | Fault-injection sensitivity probes in an isolated copy (P17): every guard in `contracts/critical-guards.json` must fail its named suites with the fault applied and pass without it; survivors fail the gate |
| load | `./scripts/test.sh load` | Bounded load + correctness probes on an explicitly selected disposable/staging target |

`./scripts/test.sh all` runs every registered tier (currently unit, api, privacy,
web) and lists tiers without registered suites; explicitly requesting an unregistered tier,
an unknown tier, an unknown task, or a task with no tests in the tier exits nonzero.
Every target fails on zero discovered tests. `./scripts/check.sh fast` is the universal
quick gate (format, Clippy deny-warnings, unit tier, private-file guard).

Registered tier contents (P17-T01 reconciliation; every name is a
`cargo test --test <target>` suite except `web`, which runs the
component project):
- unit: `bin`, `configuration`, `privacy_logging`, `api_scaffold`
- api: all 41 `api_*` suites (routes, payloads, cookie auth, matrices)
- integration: all 25 `postgres_*` suites plus `deadline_requests`,
  `request_revision`, `retention_policy` (real PostgreSQL only)
- concurrency: all 4 `concurrency_*` suites (barrier-coordinated races)
- privacy: `privacy_logging`, `privacy_surfaces`, `provider_verification`
- web: pinned install, `tsc --noEmit`, build, component project
- e2e: Playwright e2e project against a real disposable stack
  (serial workers; fresh database per verification run — session
  lookup is a documented linear scan, so shared databases slow
  authenticated round-trips)
Pure-vector suites (`domain_vectors`, `deadline_vectors`,
`liquidity_vectors`) stay directly runnable and join a tier with the
final verification card.

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

`contracts/scenario-tests.json` (v2, reconciled 2026-10-11 by P17-T01):
132 scenarios — 50 AC (47 mandatory plus future AC-45/46/47), 38 EC
(35 mandatory plus future paid EC-18/29/30), 44 INV (42 mandatory plus
future INV-40/41) — every mandatory ID naming its real discovered
assertions (`target::test`, `lib:module::test`, or `web:spec#title`),
plus the 14 race cases with their proving tests. Future entries name
only the standing guard test that forbids inventing the feature. Tiers
above re-run them; a mandatory ID without a real assertion fails the
gate.

## Continuous integration

`.github/workflows/verify.yml` runs `./scripts/ci.sh` on pushes to `main`
and pull requests to `main`: pinned `actions/checkout@v4`, stable Rust
via `rustup` (matching `backend/rust-toolchain.toml`), system Node with
the Playwright shell browser, then locked fetch/build, `fmt --check`,
`clippy --all-targets -- -D warnings`, the registered unit/api/privacy
tiers, the full integration and concurrency tiers against a real
PostgreSQL service, the web tier, the critical-mutations tier, the
release gate (lockfiles, advisories, licenses, secrets, production
config refusals), and the private-path guard. The workflow reads
tracked files only (it fails if any `.local/` path is tracked or
present) and uploads no artifacts; logs stay secret-free by
construction. The database service uses the same pinned
`postgres:18.6` image+digest as local stacks, with a synthetic
test-only password and an explicit disposable `TEST_DATABASE_URL`; a
missing or unreachable database fails the run instead of skipping.
The e2e tier stays out of `ci.sh`: it needs an orchestrated
disposable stack with a serving binary (a recorded follow-up), so
clean-checkout proofs run it explicitly with the batch harness.
