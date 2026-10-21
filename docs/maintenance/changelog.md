# Documentation changelog

This records significant documentation and business-baseline changes. It is not evidence of application implementation or a substitute for the canonical topic text.

## October 21, 2026 — The permanent e2e harness on `codex/backend-mvp`

- Gave the browser tier a permanent harness: a seeder that reaches the harness state through the real business operations (registration and proof flow, audited staff grant, published demand, withdrawn offer, suspended demand) and writes the fixtures file the specs read, plus one script that starts a disposable database, applies the tracked migrations, serves the real application over it, seeds, runs the browser journey, and tears the stack down. The harness refuses any database that is not explicitly disposable and loopback.
- Closed the harness-blocked browser specs: all 27 registered e2e specs now run against the served application over a disposable database, including withdrawn-slot permanence, withdrawn-supply refusals, suspended visibility, and the staff moderation journey. The e2e tier no longer needs an external database tool or an operator-supplied target, and it is registered in the tracked CI entry point.
- Repaired a refusal-taxonomy inconsistency the api tier kept failing on: a seller that is no longer active now refuses contact as the account restriction it is (the documented `SellerNotActive` meaning, the same one the replay path already used) instead of the request-side refusal, so initiation and replay agree and the ban-versus-contact race has one defined outcome. The wire code stays `forbidden_state` and the business rule is unchanged ([DEC-0006](../engineering/decisions/DEC-0006-contact-refusal-taxonomy.md)).
- Recorded what is not claimed: no live phone verification, staging HTTPS, off-host restore, or public launch (all pending on owner-supplied inputs); the live Verify HTTPS transport is still absent; business flows still do not schedule background jobs; renewal-path duplicate enforcement and test-cleanup hardening remain open.

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
