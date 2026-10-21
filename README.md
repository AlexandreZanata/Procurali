# Procuro Aí

Procuro Aí is a local reverse marketplace: buyers publish what they need and their maximum budget, nearby sellers submit offers, and buyers choose whom to contact through WhatsApp.

**Product promise:** “Say what you are looking for and how much you want to pay. People who have it for sale can find you.”

## Project documentation

Start at the [documentation index](docs/README.md). Documentation is split into focused, canonical files so AI agents can find and update only the context relevant to their task.

- [Agent instructions](AGENTS.md) define repository-wide working conventions.
- [Agent maintenance workflow](docs/maintenance/agent-workflow.md) explains discovery, editing, verification, and handoffs.
- [Document manifest](docs/manifest.json) provides machine-readable document identities, paths, scope, and relationships.
- [Business Logic Specification index](docs/BUSINESS_LOGIC_SPECIFICATION.md) preserves the original 27-section reading order.
- [Project status](docs/maintenance/project-status.md) distinguishes specified behavior from implemented behavior.

The business baseline deliberately makes no technology, programming language, framework, database, or infrastructure decisions and contains no implementation code or visual screen designs.

## Initial scope

The mandatory MVP validates one complete loop:

**Purchase request → seller offers → buyer-initiated WhatsApp contact → buyer-reported outcome.**

Individuals use the basic marketplace for free. Professional monetization is planned through Radar Pro, which helps sellers discover relevant demand automatically without charging for basic contact access.

## How to use the specification

1. Read the repository instructions and documentation index.
2. Follow the task routes to the relevant canonical topic and related domain rules.
3. Preserve existing invariant, acceptance, and edge-case identifiers when editing.
4. Update related topics, navigation, and change history when a change crosses documents.
5. Keep mandatory MVP, post-MVP, and future scope explicit and separate.

All numeric thresholds in the specification are recommended starting policies, with a rationale and a review condition. They are not claims of validated product-market fit.

## Implementation checkout (construction branch `codex/backend-mvp`)

The free-MVP backend (Rust/Axum/PostgreSQL), reproducible infrastructure,
and the minimal TypeScript web interface are implemented and verified
from tracked files only — no private workspace is needed:

- Start disposable infrastructure: `scripts/infra.sh up-test --suffix NAME --port PORT`
  (development: `scripts/infra.sh up-dev`).
- Quick gate: `./scripts/check.sh fast` (format, Clippy as errors, unit
  tier, private-path guard).
- Tiered suites: `./scripts/test.sh <unit|integration|api|concurrency|privacy|web|e2e|ops|load|critical-mutations|all>`
  with an explicit disposable `TEST_DATABASE_URL`; `e2e` also needs
  `PROCURALI_E2E_BASE_URL`, `load` needs `LOAD_BASE_URL`.
- Operations: `scripts/smoke.sh` (schema + minimal flow probe, GET only),
  `scripts/test-ops.sh` (restart proof), `scripts/backup.sh` /
  `scripts/restore-test.sh` (backup and restore proof),
  `scripts/deploy.sh` (disposable deploy/rollback), `scripts/load-test.sh`
  (bounded load + correctness).
- Serving the application: `cargo run --bin procurali-backend` (validated
  settings, migrated schema, real HTTP; see
  `docs/engineering/local-operation.md`) and
  `scripts/build-artifact.sh --serve-smoke` (the built image serving real
  traffic against a disposable database).
- Release state: `docs/engineering/release-status.md` (tested revision,
  tier results, pending live gates); procedures in `infra/runbooks/`.

Verified scope is the mandatory free MVP (Request → Offers → Contact →
Outcome with safety and administration). Native mobile apps, Radar Pro
sales, subscriptions, and paid passes remain future work. Live phone
verification, staging HTTPS, off-host restore, and public launch stay
pending on owner-supplied inputs — a tested commit never implies a live
deployment.
