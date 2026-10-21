# Project status

**Updated:** October 21, 2026  
**Phase:** Implementation verification on the construction branch `codex/backend-mvp` (mandatory phases P00–P19, P21, and P22 implemented; optional P20 Redis deferred)  
**Business baseline:** Recommended version 1.1

## Available artifacts

- The [business specification collection](../README.md) covers the original 47 brief items in the original 27-section order.
- Thirty-one canonical business topic files preserve 44 invariant definitions, 38 edge cases, and 50 acceptance scenarios.
- The [original entry point](../BUSINESS_LOGIC_SPECIFICATION.md) is a navigation index; business text lives in linked topic files.
- Root/documentation agent instructions, a document manifest, task routes, maintenance guidance, templates, and change history support agent-maintained work.

## Implementation status

The free-MVP backend (Rust/Axum/PostgreSQL), reproducible one-host
infrastructure, and the minimal componentized web interface are
implemented on the construction branch and verified from tracked files
only: full tier registry (unit, integration on real PostgreSQL, api,
concurrency, privacy, web component, e2e over real HTTP, ops, bounded
load, critical-mutations), plus restart/backup/restore/smoke proofs and
the release gate. The application also serves itself: one production
composition root merges every business boundary into one router, and the
binary serves it over real HTTP with typed startup refusals, pool-backed
readiness, and a graceful drain. See [release status](../engineering/release-status.md)
for the tested revision and results, and the [root README](../../README.md)
for checkout, check, and operation commands.

Specifications describe intended behavior; implementation evidence lives
in the verification tiers above, not in documentation alone.

Known construction follow-ups (not launch blockers for local
correctness): renewal-path duplicate enforcement, test-cleanup hardening,
the live phone-verification transport, and the scheduling of background
jobs from business flows. Mobile apps, Radar Pro, subscriptions, and paid
passes remain future work.

## Open operational choices before public launch

These are identified launch-owner decisions, not missing core request/offer/contact behavior:

- Select the launch city and confirm category scope.
- Assign accountable operations/moderation ownership and staffed support/review hours.
- Review provisional thresholds and retention defaults, required user notices, and any documented obligation-based exceptions.
- Define the product-validation experiment duration, minimum observed cohort, and provisional success targets.
- Verify the mandatory business acceptance behavior once implementation exists.

See the [readiness checklist](../quality/readiness-checklist.md) for the full gate.

## Later commercial gate

Radar Pro, subscription sales, and temporary passes have not launched. BRL 19.90/month remains a pricing hypothesis. Commercial terms, remedies, notification consent, interval behavior, and commercial record retention require their own later release gate. They do not block the free MVP's implementation.

See [monetization](../commercial/monetization.md), [Radar Pro](../commercial/radar-pro.md), and [MVP scope](../product/mvp-scope.md).

## Maintaining this status

Update this page from repository evidence or an explicit product decision. Distinguish proposed, specified, implemented, verified, and released states. Record material changes in the [changelog](changelog.md); keep temporary task details in a substantive handoff instead of turning this page into a chat transcript.
