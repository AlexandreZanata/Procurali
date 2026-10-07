---
document_id: DOC-27
status: recommended-business-baseline
scope: mixed
source_sections: ["27.1", "27.2", "27.3", "27.4"]
last_updated: 2026-10-07
---

# 27. Business readiness checklist

Use this checklist before implementation planning. A checked item means its business behavior is understood and assigned, not that software already exists.

### 27.1 Domain and core workflow

- [ ] The product is organized around purchase intent and the request–offer–contact–outcome chain.
- [ ] Buyer and seller are contextual roles on one account; professional classification is separate from paid access.
- [ ] Required registration fields, number uniqueness, proof of control, recovery, recycled-number handling, and phone changes are understood.
- [ ] Every conceptual entity has responsibilities, business attributes, states, relationships, actions, and invariants.
- [ ] The seven-day exclusive deadline, rolling allowances, and original publication age have one consistent interpretation.
- [ ] Material and nonmaterial request changes, offer resubmission, and contact snapshots are distinguishable.
- [ ] Renewals create cycles and never revive prior-cycle offers.
- [ ] Completed, cancelled, expired, suspended, and removed requests have distinct effects.
- [ ] Seller withdrawal, buyer decline, expiry, invalidation, and suspension cannot be confused with purchase acceptance.
- [ ] Budget, condition, category, locality, self-offer, and ownership validations apply at the action's effective time.
- [ ] The buyer can contact more than one eligible seller without checkout or item reservation.
- [ ] Contact initiation, actual conversation, buyer-reported resolution, and platform attribution have different meanings.
- [ ] A missing outcome response is kept as unknown, and “Not yet” does not automatically renew.

### 27.2 Trust, privacy, and operation

- [ ] Public content excludes phones, exact addresses, private offers, and report identities.
- [ ] The seller understands destination disclosure at WhatsApp handoff; the buyer number is not revealed by offer submission.
- [ ] Blocks are bilateral for future platform interaction and do not promise to retract external knowledge.
- [ ] Report reasons, duplicate grouping, severity, review targets, appeals, and decision audit are defined.
- [ ] Permanent bans require authorized review rather than complaint-count automation.
- [ ] Prohibited categories are extensible and their changes affect existing active content consistently.
- [ ] Account suspension/deletion effects are defined for both buyer and seller resources.
- [ ] Restoration checks each independent restriction and never extends elapsed deadlines silently.
- [ ] The permission groups incorporate ownership, state, relationship, time, scope, and purpose.
- [ ] Staff grants and support/review coverage have accountable operational owners.
- [ ] Retention defaults and applicable exceptions receive pre-launch approval and understandable user notices.
- [ ] Reputation labels distinguish declarations, feedback, contact counts, and validated moderation findings.
- [ ] Every EC-01 through EC-38 has a single expected behavior rather than a hidden technical assumption.

### 27.3 Growth and measurement

- [ ] Shared links preserve request context through registration and respect current visibility.
- [ ] Share intent, landing, registration attribution, and offer attribution are separate observable facts.
- [ ] Each successful business action maps to a recorded event without duplicate retry inflation.
- [ ] First-publication and renewal-cycle metrics are separate.
- [ ] Cohort maturity, denominators, unknown outcomes, sample sizes, exclusions, and zero-denominator behavior are defined.
- [ ] The North Star is unique contact initiations per request, paired with bounded request-contact coverage.
- [ ] Operational liquidity distinguishes historical response time from current live availability.
- [ ] Provisional quotas have documented rationale and are reviewed against legitimate-user friction.
- [ ] The launch city and initial six category scopes are confirmed before public operation.
- [ ] MVP experiment duration, minimum sample, and provisional product-validation targets are recorded by the launch owner.

### 27.4 Scope and commercial gates

- [ ] AC-01 through AC-44 and AC-48 through AC-50 are adopted as mandatory business acceptance behavior.
- [ ] AC-45 through AC-47 are reserved for the later commercial release.
- [ ] No paid feature is required for basic request, offer, contact, or outcome behavior.
- [ ] Radar matching is deterministic, local, explainable, and deduplicated per cycle.
- [ ] Proposed BRL 19.90 pricing is treated as a hypothesis, not an approved tariff.
- [ ] Commercial plan terms, remedies, retention, notification consent, and access intervals are approved before sales begin.
- [ ] Downgrade preserves legitimate basic offers/history and pauses paid monitoring prospectively.
- [ ] Future demand intelligence has aggregate-only scope and a reviewed small-group suppression policy.
- [ ] Explicitly excluded features are not added as prerequisites for the free MVP.
- [ ] Technology and implementation decisions begin only after this business baseline is reviewed.



The original brief is mapped to this collection in [Source-brief coverage](brief-coverage.md).

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [Principles and policies](../product/principles-and-policies.md)
- [Invariants](../domain/invariants.md)
- [Permissions](../domain/permissions.md)
- [Acceptance criteria](acceptance-criteria.md)
- [Mvp scope](../product/mvp-scope.md)
- [Brief coverage](brief-coverage.md)
