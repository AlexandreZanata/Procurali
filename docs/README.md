# Documentation index

**Language:** English  
**Business baseline:** Version 1.1, October 7, 2026  
**Current phase:** Business specification and documentation organization

This is the entry point for agents and human reviewers. Each business topic has one canonical file. The [original specification entry point](BUSINESS_LOGIC_SPECIFICATION.md) retains the 27-section reading order, and the [manifest](manifest.json) provides machine-readable discovery.

## First read

1. [Repository instructions](../AGENTS.md) and [documentation instructions](AGENTS.md).
2. [Project status](maintenance/project-status.md) for what exists and what has not been implemented.
3. [Product overview](product/overview.md), [principles and policies](product/principles-and-policies.md), and [glossary](product/glossary.md).
4. [MVP scope](product/mvp-scope.md) and [exclusions](product/out-of-scope.md).
5. The relevant task route below, followed by the [agent workflow](maintenance/agent-workflow.md) before editing.

Do not load every document for a narrow task. Read the task's topic, its related rules, and relevant acceptance/edge-case identifiers. Broaden the context when the change crosses domain boundaries.

For a numerical limit, timing window, or commercial interval, use the [policy index](product/policy-index.md) to find its canonical definition without searching the entire collection.

## Find documentation by task

- **Registration, phone, account states, or deletion:** [Users](domain/users.md), [privacy and security](trust/privacy-and-security.md), [permissions](domain/permissions.md), and [moderation](trust/reports-and-moderation.md).
- **Request publication, budget, category, editing, renewal, or closure:** [Request workflow](workflows/requests.md), [policy defaults](product/principles-and-policies.md), [lifecycles](domain/state-transitions.md), and [invariants](domain/invariants.md).
- **City, region, filters, relevance, or discovery ordering:** [Discovery](workflows/discovery.md), [request locality](workflows/requests.md), and [liquidity](measurement/liquidity.md).
- **Offer validity, seller allowance, comparison, editing, or withdrawal:** [Offer workflow](workflows/offers.md), [invariants](domain/invariants.md), [lifecycles](domain/state-transitions.md), and [permissions](domain/permissions.md).
- **WhatsApp, contact privacy, retries, or contact counts:** [Contact workflow](workflows/whatsapp-contact.md), [privacy](trust/privacy-and-security.md), [events](measurement/events.md), and [metrics](measurement/metrics.md).
- **Completion, cancellation, attribution, or buyer prompts:** [Outcomes](workflows/outcomes.md), [reputation](trust/reputation.md), [lifecycles](domain/state-transitions.md), and [metrics](measurement/metrics.md).
- **Sharing, external visitor acquisition, or registration continuity:** [Sharing](workflows/sharing.md), [discovery](workflows/discovery.md), and [events](measurement/events.md).
- **Fraud, spam, reports, blocking, bans, or prohibited goods:** [Moderation](trust/reports-and-moderation.md), [privacy/security](trust/privacy-and-security.md), [permissions](domain/permissions.md), and [edge cases](quality/edge-cases.md).
- **Professional sellers, Radar, subscription, or downgrade:** [Professional sellers](commercial/professional-sellers.md), [Radar Pro](commercial/radar-pro.md), [monetization](commercial/monetization.md), and [scope](product/mvp-scope.md).
- **Analytics, event names, cohort windows, or liquidity:** [Events](measurement/events.md), [metrics](measurement/metrics.md), [liquidity](measurement/liquidity.md), and [privacy](trust/privacy-and-security.md).
- **Future implementation planning or review:** [Entities](domain/entities.md), [capabilities](domain/capabilities.md), [invariants](domain/invariants.md), [acceptance criteria](quality/acceptance-criteria.md), and [readiness checklist](quality/readiness-checklist.md).
- **Documentation changes or agent handoff:** [Agent workflow](maintenance/agent-workflow.md), [decision guidance](decisions/README.md), [templates](#templates), and [changelog](maintenance/changelog.md).

For every behavior change, also inspect the relevant entries in [edge cases](quality/edge-cases.md) and [acceptance criteria](quality/acceptance-criteria.md).

## Canonical business documents

### Product

- [DOC-01 — Overview](product/overview.md): Purpose, hypothesis, and central marketplace chain.
- [DOC-02 — Principles and policies](product/principles-and-policies.md): Recommended decisions and initial operating limits.
- [DOC-03 — Glossary](product/glossary.md): Shared domain vocabulary.
- [DOC-24 — MVP scope](product/mvp-scope.md): Required behavior and delivery gates.
- [DOC-25 — Roadmap](product/roadmap.md): Near-term and future capabilities.
- [DOC-26 — Exclusions](product/out-of-scope.md): Explicit boundaries for the first release and this specification phase.

### Domain

- [DOC-04 — Users](domain/users.md): Roles, registration, verification, and account eligibility.
- [DOC-05 — Entities](domain/entities.md): Conceptual responsibilities, attributes, relations, and invariants.
- [DOC-05-CAPABILITIES — Capabilities](domain/capabilities.md): Operations spanning multiple entities.
- [DOC-06 — State transitions](domain/state-transitions.md): Request, offer, and planned commercial lifecycles.
- [DOC-07 — Invariants](domain/invariants.md): The canonical definitions of INV-01 through INV-44.
- [DOC-21 — Permissions](domain/permissions.md): Contextual capability groups and ownership restrictions.

### Workflows

- [DOC-08 — Requests](workflows/requests.md): Publication through maintenance and closure.
- [DOC-09 — Discovery](workflows/discovery.md): Deterministic local matching and visibility.
- [DOC-10 — Offers](workflows/offers.md): Seller representation and private buyer comparison.
- [DOC-11 — WhatsApp contact](workflows/whatsapp-contact.md): Controlled handoff and observable business facts.
- [DOC-12 — Outcomes](workflows/outcomes.md): Resolution, abandonment, and unknown results.
- [DOC-13 — Sharing](workflows/sharing.md): Links and the acquisition loop.

### Trust and safety

- [DOC-14 — Reputation](trust/reputation.md): Factual evidence and structured feedback.
- [DOC-15 — Privacy and security](trust/privacy-and-security.md): Visibility, safeguards, deletion, and retention baseline.
- [DOC-16 — Reports and moderation](trust/reports-and-moderation.md): Incidents, blocking, prohibitions, decisions, and audit.

### Commercial planning

- [DOC-17 — Professional sellers](commercial/professional-sellers.md): Declared business use, independent of payment.
- [DOC-18 — Radar Pro](commercial/radar-pro.md): Post-MVP saved demand matching and alerts.
- [DOC-19 — Monetization](commercial/monetization.md): Free access, proposed plans, entitlement periods, and future products.

### Measurement

- [DOC-20-EVENTS — Events](measurement/events.md): Named business facts and recording meaning.
- [DOC-20 — Metrics](measurement/metrics.md): Cohorts, formulas, denominators, and evidence limitations.
- [DOC-20-LIQUIDITY — Liquidity](measurement/liquidity.md): Operational demand-health classification.

### Quality and traceability

- [DOC-22 — Edge cases](quality/edge-cases.md): EC-01 through EC-38.
- [DOC-23 — Acceptance criteria](quality/acceptance-criteria.md): AC-01 through AC-50, including three later commercial scenarios.
- [DOC-27 — Readiness checklist](quality/readiness-checklist.md): Business and operating readiness, not proof of implemented software.
- [DOC-27-COVERAGE — Brief coverage](quality/brief-coverage.md): Traceability for all 47 original brief items.

## Agent maintenance

- [Agent workflow](maintenance/agent-workflow.md): How to find, change, verify, and hand off documentation.
- [Project status](maintenance/project-status.md): Evidence-based current phase and open launch/commercial gates.
- [Changelog](maintenance/changelog.md): Significant document and behavior changes.
- [Decision records](decisions/README.md): How to record new consequential decisions without duplicating the baseline.

### Templates

- [Topic document](templates/topic-document.md).
- [Decision record](templates/decision-record.md).
- [Task handoff](templates/task-handoff.md).

## Search and stable references

Use filenames and manifest summaries to locate a subject. Search for stable identifiers such as `INV-27`, `AC-21`, or `EC-07` to find exact constraints and scenarios. Event names such as `contact_initiated` locate measurement semantics. Section numbers are retained for source traceability rather than used as file naming requirements.

`DOC-*` identifies a canonical business document; `INV-*` identifies an invariant; `AC-*` identifies an acceptance scenario; `EC-*` identifies an edge case; future `DEC-*` identifies a decision record. File paths may change with updated navigation, but these identities remain stable.

The manifest stores discovery metadata, not a second copy of business rules. `related` paths are relative to `docs/`; `path_base` defines the base for top-level manifest paths. A `mixed` scope means a file explicitly contains more than one release scope; it does not make every described capability mandatory for the MVP.
