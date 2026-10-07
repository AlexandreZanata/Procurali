---
document_id: DOC-11
status: recommended-business-baseline
scope: mvp
source_sections: ["11"]
last_updated: 2026-10-07
---

# 11. WhatsApp contact workflow

### 11.1 Eligibility and buyer choice

The buyer chooses “Talk on WhatsApp” on a live offer. No formal acceptance, checkout, mutual-approval ceremony, or payment is required. Buyers may contact multiple sellers, subject to abuse controls, and are not required to select a single winner.

Immediately before handing off, recheck request ownership, both eligible verified accounts, active/unexpired request, current offer and revision, current seller destination, no blocks, and no content restriction. If the offer changed since it was displayed, show current terms and require a fresh contact choice. If closure or expiry occurred first, contact is refused even if the buyer had the offer open.

### 11.2 Contextual message

Recommended prefilled message:

> Hello! I saw your offer on Procuro Aí for my refrigerator request. You offered a used Consul 340L for BRL 520. Is it still available?

Use the current request item and current offer item, price, and condition. Do not include a precise address, buyer phone, internal moderation data, or a public link to private offer details. The buyer can change or discard the message in WhatsApp; the marketplace neither reads nor stores the resulting conversation.

### 11.3 What is recorded

Record a contact initiation only when a valid handoff is made available following an intentional buyer action. The first such event for a buyer/seller/request-cycle/offer combination is a unique contact. Later valid handoffs are repeat-contact events. A retry of the same unfinished action returns its existing result rather than manufacturing new unique contacts.

Store the business context: parties, request cycle and revision, offer revision, item and price snapshot, initiation time, and entry source. Keep any destination information administrative/private and purpose-limited. If the app cannot prepare a usable handoff, record a handoff failure separately without counting a contact initiation.

The platform cannot prove WhatsApp opened, that a message was sent, or that the seller answered. User-facing language must say “Contact initiated,” not “Conversation confirmed” or “Sale completed.”

### 11.4 Privacy boundary

The seller's number is private before a valid handoff and unavailable through public browsing or offer-list data. At handoff, the interested buyer necessarily receives the WhatsApp destination. The seller learns the buyer's WhatsApp identity only if the buyer sends a message or otherwise communicates in WhatsApp. Sending an offer grants no automatic buyer-number disclosure.

A user can block further in-platform interactions or withdraw an offer, but cannot retract an already delivered destination. The platform should communicate this limitation when users consent to external contact and provide reporting/blocking actions afterwards.

### 11.5 Abuse protection

Allow up to **ten distinct seller contacts per request cycle** in the MVP. This supports comparison while reducing bulk destination collection; repeat handoffs to an already contacted seller do not consume another slot. Review legitimate buyers hitting this cap and confirmed collection abuse. New contact still requires an offer the seller deliberately submitted to that request. Staff may impose a lower temporary contact restriction based on documented abuse.

If either participant is suspended, banned, deleted, or actively blocked, no new handoff occurs. A buyer can report or record an outcome concerning a previous interaction without restoring contact.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [State transitions](../domain/state-transitions.md)
- [Invariants](../domain/invariants.md)
- [Offers](offers.md)
- [Outcomes](outcomes.md)
- [Privacy and security](../trust/privacy-and-security.md)
