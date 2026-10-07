---
document_id: DOC-15
status: recommended-business-baseline
scope: mixed
source_sections: ["15"]
last_updated: 2026-10-07
---

# 15. Privacy and security

### 15.1 Visibility classes

**Public, while the request is eligible:** Request item, category, budget, accepted condition, approximate city/region, optional sanitized notes, buyer display name, original age, current availability, and permitted factual profile information. Public profiles have no phone or exact address.

**Authenticated owner/participant only:** Own account details and private phone management; buyer's received offers; seller's own offer and its status; the parties' relevant historical contact context; own reports, blocks, and professional settings. Signing in alone does not unlock all offers or phone numbers.

**Interaction-gated:** Seller WhatsApp destination only for an eligible buyer initiating contact on that seller's live offer. The buyer's destination is not revealed by the app to the seller; WhatsApp may reveal it after the buyer sends a message.

**Administrative, purpose-limited:** Phone history required for review, report details and reporter identity, restricted content snapshots, eligibility decisions, audit records, and abuse evidence. Aggregate commercial insights never inherit unrestricted access to these personal details.

### 15.2 Minimal and progressive safeguards

- Confirm phone control before marketplace actions and check duplicate current numbers.
- Apply publication, offer, revision, and contact allowances with clear refusal messages.
- Keep bulk browsing and repeated access bounded by a configurable abuse policy; excessive automated activity can receive a temporary restriction and review. Browsing-volume limits must be calibrated against normal seller discovery rather than invented as universal user quotas.
- Do not provide bulk phone access, public offer feeds, or unbounded user-directory discovery.
- Refuse unsolicited external links and contact destinations in request/offer free text for the MVP. Optional product details should be expressible without opening a third-party site.
- Detect repeated identical content and rapid creation/deletion patterns using explainable evidence.
- Prohibit unattended offer blasts, misleading template offers, and attempts to evade account restrictions. Radar alerts do not imply permission to automate offers.
- Escalate repeated abuse from explanation to warning, scoped temporary restriction, and human review. Permanent bans require a documented decision.
- Provide report and block actions wherever a relevant request, offer, or past interaction can be inspected.

Do not require extensive identity paperwork from every legitimate user to address hypothetical abuse. Extra verification can be requested for a specific documented risk or future professional claim.

### 15.3 Account deletion and retention baseline

Deletion immediately removes public account identity, stops contact, cancels unresolved buyer requests, invalidates seller offers, ends paid renewal when commercial features exist, and disables Radars. Notify affected participants that the relevant request or offer is unavailable without disclosing the deletion reason.

Recommended product retention policy, to be validated before launch:

- **Ordinary removed-content and contact context:** Keep access-limited records for up to 90 days after closure or deletion to support recent disputes, then remove unnecessary personal content and destination data. Owner-facing history for a still-active account can retain non-sensitive summaries longer at the owner's choice.
- **Open safety incidents:** Retain only incident-relevant evidence while the case remains open; review necessity every 90 days. Closed safety evidence has a recommended maximum of 180 days after decision unless a separately documented obligation requires retention.
- **Moderation audit:** Retain the minimum decision trail needed to explain restrictions; after personal evidence is no longer necessary, remove or de-identify personal content. Every retained restriction must have a stated purpose and a periodic necessity review.
- **Measurement:** Keep non-identifying aggregate counts after removing identifying links. A removed phone number must never remain available for marketing or marketplace contact.
- **Commercial records, post-MVP:** Define a separate approved retention policy before selling access; no unspecified permanent retention by default.

The 90-day ordinary window supports recent local disputes without indefinite contact storage; the longer safety window supports review of a documented incident. These are proposed business defaults, not legal retention determinations. Launch requires confirmation of applicable obligations and approved user notices. A recorded exception names its basis, scope, owner, and review/end condition.

### 15.4 Limits of external-contact control

In-platform blocks and deletion stop future in-platform disclosure. They cannot erase WhatsApp conversations, numbers already learned, copied share previews, or information another person voluntarily retained. Inform users of this concrete boundary without requiring an additional approval step for every contact.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [Users](../domain/users.md)
- [Whatsapp contact](../workflows/whatsapp-contact.md)
- [Reports and moderation](reports-and-moderation.md)
- [Permissions](../domain/permissions.md)
