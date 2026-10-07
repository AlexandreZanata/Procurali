# Procuro Aí — Business Logic Specification

**Version:** 1.1 — modular organization  
**Date:** October 7, 2026  
**Status:** Recommended business baseline for implementation planning  
**Language:** English  
**Scope:** Product behavior and conceptual domain only

This specification makes business recommendations where the product brief leaves choices open. These recommendations are sufficiently concrete to implement later, but remain subject to product validation. No section selects technologies, describes technical infrastructure, defines a database schema, writes implementation code, or designs screens.

The product name is provisional. Portuguese place names and the product's proper name may remain unchanged; all explanatory content, example messages, and business rules are in English. Documentation language does not determine the application's eventual localization.

The specification now lives in focused canonical documents. This file preserves the original reading order and section anchors; edit the linked topic file instead of copying business rules here. Start with the [documentation index](README.md) for task-oriented navigation or [document manifest](manifest.json) for machine-readable discovery.

## Contents

1. [Executive summary](#1-executive-summary)
2. [Business principles and decision baseline](#2-business-principles-and-decision-baseline)
3. [Domain glossary](#3-domain-glossary)
4. [User types and account lifecycle](#4-user-types-and-account-lifecycle)
5. [Conceptual domain entities and capabilities](#5-conceptual-domain-entities-and-capabilities)
6. [States and state transitions](#6-states-and-state-transitions)
7. [Business rules and invariants](#7-business-rules-and-invariants)
8. [Request creation and maintenance](#8-request-creation-and-maintenance)
9. [Local discovery](#9-local-discovery)
10. [Offer workflow](#10-offer-workflow)
11. [WhatsApp contact workflow](#11-whatsapp-contact-workflow)
12. [Outcome and completion workflow](#12-outcome-and-completion-workflow)
13. [Sharing and acquisition](#13-sharing-and-acquisition)
14. [Reputation](#14-reputation)
15. [Privacy and security](#15-privacy-and-security)
16. [Reports and moderation](#16-reports-and-moderation)
17. [Professional sellers](#17-professional-sellers)
18. [Radar Pro](#18-radar-pro)
19. [Monetization](#19-monetization)
20. [Business events, metrics, and liquidity](#20-business-events-metrics-and-liquidity)
21. [Permissions by role and resource context](#21-permissions-by-role-and-resource-context)
22. [Edge cases](#22-edge-cases)
23. [Business acceptance criteria](#23-business-acceptance-criteria)
24. [Mandatory MVP](#24-mandatory-mvp)
25. [Near-term post-MVP and future scope](#25-near-term-post-mvp-and-future-scope)
26. [Out of MVP scope](#26-out-of-mvp-scope)
27. [Business readiness checklist](#27-business-readiness-checklist)

## 1. Executive summary

[Product purpose and the reverse marketplace hypothesis](product/overview.md).

## 2. Business principles and decision baseline

[Principles, policy defaults, thresholds, and recommended business decisions](product/principles-and-policies.md).

## 3. Domain glossary

[Shared terminology and meanings used throughout the domain](product/glossary.md).

## 4. User types and account lifecycle

[Contextual roles, registration, phone control, and account eligibility](domain/users.md).

## 5. Conceptual domain entities and capabilities

[Conceptual entities, ownership, responsibilities, and relationships](domain/entities.md).

Additional file: [Cross-entity domain capabilities](domain/capabilities.md).

## 6. States and state transitions

[Lifecycles, transition guards, closure effects, and restriction precedence](domain/state-transitions.md).

## 7. Business rules and invariants

[INV-01 through INV-44 and duplicate-intent rules](domain/invariants.md).

## 8. Request creation and maintenance

[Request publication, revision, renewal, closure, and launch categories](workflows/requests.md).

## 9. Local discovery

[Local filters, deterministic ordering, and public request access](workflows/discovery.md).

## 10. Offer workflow

[Submission, private comparison, viewing, editing, and withdrawal](workflows/offers.md).

## 11. WhatsApp contact workflow

[Buyer-initiated handoff, contextual message, privacy, and deduplication](workflows/whatsapp-contact.md).

## 12. Outcome and completion workflow

[Outcome prompts, completion, attribution, and complete/alternate journeys](workflows/outcomes.md).

## 13. Sharing and acquisition

[Share actions, direct links, registration continuity, and acquisition attribution](workflows/sharing.md).

## 14. Reputation

[Factual evidence, structured feedback, and manipulation controls](trust/reputation.md).

## 15. Privacy and security

[Visibility, progressive safeguards, deletion, and proposed retention policy](trust/privacy-and-security.md).

## 16. Reports and moderation

[Reports, severity, decisions, appeals, blocking, prohibitions, and staff operations](trust/reports-and-moderation.md).

## 17. Professional sellers

[Commercial declaration and professional/free access distinctions](commercial/professional-sellers.md).

## 18. Radar Pro

[Saved criteria, deterministic matching, alerts, and entitlement effects](commercial/radar-pro.md).

## 19. Monetization

[Free access, proposed plans, intervals, downgrade, temporary passes, and future intelligence](commercial/monetization.md).

## 20. Business events, metrics, and liquidity

[Measurement windows, formulas, contact evidence, retention, and professional indicators](measurement/metrics.md).

Additional files: [Business events](measurement/events.md) and [Operational liquidity](measurement/liquidity.md).

## 21. Permissions by role and resource context

[Capabilities constrained by actor, ownership, state, relationship, and purpose](domain/permissions.md).

## 22. Edge cases

[EC-01 through EC-38 and their required behavior](quality/edge-cases.md).

## 23. Business acceptance criteria

[AC-01 through AC-50, with mandatory and later commercial scenarios](quality/acceptance-criteria.md).

## 24. Mandatory MVP

[Mandatory capabilities, delivery gates, and validation boundaries](product/mvp-scope.md).

## 25. Near-term post-MVP and future scope

[Near-term additions and separately scoped future capabilities](product/roadmap.md).

## 26. Out of MVP scope

[Explicit MVP exclusions and the business-only documentation mandate](product/out-of-scope.md).

## 27. Business readiness checklist

[Pre-implementation business, operation, measurement, and commercial gates](quality/readiness-checklist.md).

Additional file: [Source-brief coverage](quality/brief-coverage.md).
