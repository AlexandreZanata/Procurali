---
document_id: DOC-05-CAPABILITIES
status: recommended-business-baseline
scope: mixed
source_sections: ["5.17"]
last_updated: 2026-10-07
---

# 5.17 Domain capabilities


The following operations coordinate rules across several entities. They specify business responsibilities, not technical implementations.

- **Register and verify an account.** Inputs: name, phone, locality, proof of control, policy acceptance. Validate required fields, phone uniqueness, current restrictions, and supported launch locality. Result: eligible active account or a clear pending/recovery outcome.
- **Evaluate action eligibility and limits.** Inputs: actor, action, relevant resource, current time, effective policy. Validate account state, ownership, blocks, category, lifecycle, deadlines, and rolling counts. Result: permission or a specific refusal reason; no partial business mutation.
- **Publish a request.** Inputs: author and complete draft. Validate requirements, limits, duplicate intent, category, and locality. Result: first active cycle, deadline, publication event, and shareable eligible request.
- **Revise a request.** Inputs: author, current revision, proposed changes. Classify materiality, evaluate limits, and identify live offers. Result: a new revision, necessary offer invalidations, and notices, or refusal if the request changed meanwhile.
- **Renew a request.** Inputs: author, eligible current request, renewed-intent confirmation. Validate status, visibility, account, limits, and content policy. Result: new seven-day cycle with no revived offers and a renewal event.
- **Discover compatible requests.** Inputs: selected city, optional region/category/budget/condition filters, viewer context. Exclude unavailable, expired, removed, blocked, or prohibited resources. Result: deterministically ordered eligible requests.
- **Submit or resubmit an offer.** Inputs: seller, request cycle/revision, item terms, local availability. Validate compatibility, unique slot, quotas, and restrictions. Result: live offer for the current requirements, visible only to its parties and authorized staff.
- **Initiate contact.** Inputs: buyer, offer and current terms. Recheck both parties, resource eligibility, seller destination, and ownership. Result: a recorded initiation and contextual WhatsApp handoff, or a refusal without a contact fact.
- **Record an outcome.** Inputs: buyer, request, answer, optional attribution. Validate ownership, current status, and any linked contact. Result: unresolved response, completion, or cancellation, with associated lifecycle effects.
- **Apply or remove a block.** Inputs: owner, other user. Validate distinct parties. Result: interaction restrictions, affected offer invalidations, and preserved history; unblocking does not undo those invalidations.
- **Moderate an incident.** Inputs: authorized staff, case, evidence, reason, scoped decision. Validate permission and proportional scope. Result: auditable action, eligibility changes, and appropriate notices.
- **Match Radar demand.** Inputs: active Radar, entitled seller, newly eligible request cycle/revision. Validate explicit matching criteria and all discovery restrictions. Result: deduplicated match with an explanation, or no match.
- **Derive liquidity and reputation.** Inputs: eligible historical events, observation window, locality, policy version. Exclude duplicates and identified abuse; distinguish unknown outcomes. Result: traceable aggregate indicators without phone or exact-address disclosure.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [Entities](entities.md)
- [State transitions](state-transitions.md)
- [Invariants](invariants.md)
- [Permissions](permissions.md)
