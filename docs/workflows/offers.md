---
document_id: DOC-10
status: recommended-business-baseline
scope: mvp
source_sections: ["10"]
last_updated: 2026-10-07
---

# 10. Offer workflow

### 10.1 Submission

An eligible seller chooses “I have this” on an active request. The seller supplies a short item/model description, positive price, condition of new or used, and optional notes. The seller confirms the item is currently available and can be made available in the request's city.

Validate both account states, request deadline and visibility, current cycle/revision, absence of a block, self-offer prohibition, unique seller slot, daily allowance, category/item compatibility, condition, and budget. All required checks must pass before the offer becomes sent. The buyer gets a private offer notice; the seller sees submission confirmation without buyer phone access.

Because item wording can be ambiguous, the seller declares relevance and the buyer can decline or report an unrelated offer. Clear category, condition, price, and locality mismatches are deterministic refusals; no sophisticated semantic matching is a prerequisite.

### 10.2 Offer privacy and comparison

Only the buyer, the offer's seller, and authorized staff with a relevant purpose can inspect the full offer. Other sellers cannot see competitors' prices or a buyer's private comparison activity. Received offers are ordered by submission time, newest first, with a stable identifier tie-breaker; the buyer can choose a simple price sort. Editing does not move an offer to the top.

Each offer displays item description, price, condition, declared seller name and professional classification, approximate locality, and factual reputation where available. It shows an update indicator if terms changed after viewing. Unavailable or terminal offers remain in a collapsed historical context for the parties, with contact disabled. Live offers are the actionable comparison set.

### 10.3 Viewing and declining

Opening an offer's details records the first buyer view and marks sent as viewed. Merely receiving a notification does not mean viewed. Repeated views are not additional first-view conversions. Sellers can see their own offer's broad status; buyer identity data remains unchanged.

The buyer may decline an offer without selecting another offer or explaining a reason. Decline means “I will not pursue this offer,” not a negative reputation judgment. A declined seller cannot create another offer in that same cycle; a later renewed cycle creates a new opportunity.

### 10.4 Seller editing and withdrawal

The seller may edit a live offer while the request remains eligible. New terms must still meet the current budget, condition, item, and locality requirements. Changes create an offer revision, keep its original submission time, and notify a buyer who previously viewed or contacted it. A buyer initiating contact after a price change must see the latest terms and acknowledge that the offer changed since their earlier view.

The seller can withdraw or mark the item unavailable at any time, including after contact. Withdrawal disables future marketplace contact immediately and keeps prior contact history. It cannot recall phone numbers or messages already shared through WhatsApp. No negative reputation follows from withdrawal alone; deceptive repeated bait-and-switch behavior can be reported and reviewed.

Offering the same item to multiple requests is allowed because inventory is not reserved or tracked. The seller is responsible for truthful availability and timely withdrawal if it is sold. The platform does not promise exclusivity to any buyer.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [Principles and policies](../product/principles-and-policies.md)
- [State transitions](../domain/state-transitions.md)
- [Invariants](../domain/invariants.md)
- [Requests](requests.md)
- [Whatsapp contact](whatsapp-contact.md)
