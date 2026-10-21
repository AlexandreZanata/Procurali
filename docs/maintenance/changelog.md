# Documentation changelog

This records significant documentation and business-baseline changes. It is not evidence of application implementation or a substitute for the canonical topic text.

## October 20, 2026 — The served application on `codex/backend-mvp`

- Implemented the serving-binary follow-up recorded since P18-T01: one production composition root merges every HTTP boundary into one router under the production body/timeout bounds, the privacy headers, and the stable unknown-route refusal; the binary serves it over real HTTP with validated settings, a bounded pool, schema agreement, pool-backed readiness, typed value-free startup refusals, and a graceful SIGTERM drain. No business rule changed and PostgreSQL remains the only authority.
- Proved the serving behavior on disposable stacks only: the served binary answers liveness, readiness, public traffic, and a refused cookie-less staff read; the built image answers the same probes against a disposable database (`scripts/build-artifact.sh --serve-smoke`) and reports a healthy container; the operational smoke flow passes against a served target.
- Recorded what is not claimed: no live phone verification, staging HTTPS, off-host restore, or public launch (all pending on owner-supplied inputs); the live Verify HTTPS transport is still absent, so the served binary refuses the live provider instead of pretending a proof was delivered; business flows still do not schedule background jobs; the permanent e2e harness seeder, renewal-path duplicate enforcement, and test-cleanup hardening remain open.

## October 13, 2026 — Backend MVP construction delivered on `codex/backend-mvp`

- Implemented and verified the mandatory free MVP (Request → Offers → Contact → Outcome with safety and administration) as a Rust/Axum/PostgreSQL backend, reproducible one-host infrastructure, and a minimal TypeScript web interface; verification ran from tracked files only (unit, integration on real PostgreSQL, api, concurrency, privacy, web component, e2e over real HTTP, ops, bounded load, critical-mutations, plus restart/backup/restore/smoke proofs and the release gate).
- Published the operator handoff: real setup/check/operation commands in the root README, the full engineering document set in the documentation index and manifest, evidence-based project status, release status with the tested revision and pending live gates, and runbooks for operations, deployment, rollback, backups, smoke, staging validation, and release cuts.
- Recorded what is not claimed: no live phone verification, staging HTTPS, off-host restore, or public launch (all pending on owner-supplied inputs); mobile apps, Radar Pro, subscriptions, and paid passes remain future; the serving-binary merge, the permanent e2e harness seeder, renewal-path duplicate enforcement, and test-cleanup hardening are filed construction follow-ups. Pushing the construction branch is distinct from merging to `main` or deploying anything.
- E2E over real HTTP: 20 of 27 specs pass; 7 stay pending on the external e2e harness seeder (fixtures file, harness accounts) — recorded, not skipped.

## October 7, 2026 — Version 1.1: Modular documentation for AI maintenance

- Split the original 27-section specification into 31 canonical business files grouped by product, domain, workflows, trust, commercial planning, measurement, and quality.
- Extracted cross-entity capabilities, business events, liquidity classifications, and source-brief coverage into focused files.
- Retained the original specification path and all original numbered section anchors as a navigation-only compatibility entry point.
- Added repository/documentation agent instructions, a thin Cursor rule entry point, task routes, a policy-location index, a machine-readable manifest, maintenance workflow, project status, and reusable documentation/decision/handoff templates.
- Preserved all 44 invariant definitions, 38 edge cases, and 50 acceptance scenarios and their identifiers.
- Clarified one checklist sentence to say that a missing outcome response is kept as unknown, matching the existing outcome and measurement rules. This is an editorial clarification, not a business-policy change.
- Updated links and cross-section references for the modular collection. No application feature or technology decision was introduced.
- Verified preservation of the original business text and stable identifiers, all 27 legacy section anchors, canonical metadata consistency, complete coverage of documentation files by the 46-entry manifest, and local links/heading fragments. This verifies documentation organization, not application behavior.

## October 7, 2026 — Version 1.0: Initial business baseline

- Created the English business specification from the 47-item product brief.
- Defined the purchase-intent domain, lifecycle, policies, contextual permissions, privacy/safety behavior, commercial planning, events, metrics, and acceptance scenarios.
- Kept implementation technology, application code, infrastructure, and visual design outside the specification's mandate.
