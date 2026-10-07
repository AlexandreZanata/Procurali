---
document_id: DOC-22
status: recommended-business-baseline
scope: mixed
source_sections: ["22"]
last_updated: 2026-10-07
---

# 22. Edge cases

### EC-01 — Buyer deletes an account with active requests

Cancel unresolved requests with an account-deletion reason, hide public identity/content, invalidate live offers, stop contact, and end owned monitoring/renewal where applicable. Sellers receive an unavailable status. Preserve only the limited incident/history records described in Section [15](../trust/privacy-and-security.md).

### EC-02 — Seller deletes an account with live offers

Invalidate their offers and remove seller destination access immediately. Buyers retain limited historical contact context and reporting rights. A buyer who already learned the number through WhatsApp cannot have that external knowledge recalled.

### EC-03 — Buyer closes a request after receiving offers

Complete or cancel according to the buyer's answer. Stop new offers and handoffs, invalidate remaining live offers, and notify affected sellers. No seller is automatically selected or credited with a sale.

### EC-04 — Seller withdraws after contact

Mark withdrawn, disable future handoffs, preserve historical terms and contact, and notify the buyer. The prior contact remains counted if valid when initiated. Deception, if alleged, goes through review rather than automatic punishment.

### EC-05 — Seller is banned after offering

Hide seller content, invalidate their live offers, stop all new handoffs, and preserve incident evidence. Notify affected buyers only that offers are unavailable, with an appropriate safety notice if needed and approved by staff.

### EC-06 — Buyer is banned

Cancel their unresolved requests for a moderation reason, hide public demand, invalidate related live offers, and stop new contact. Past legitimate seller actions remain historical facts, subject to correction if the buyer activity is confirmed abusive.

### EC-07 — Deadline is reached during an action

The action is eligible only if its business effect occurs before the exclusive deadline. An offer or contact that loses to expiry is refused; it does not produce a successful event. The user gets the current status and available next steps.

### EC-08 — Buyer renews a request

Start a new eligible cycle, expire previous-cycle live offers, retain original request age and history, and allow fresh seller responses. Renewal does not reset account allowances or turn a renewed request into newly acquired demand.

### EC-09 — Buyer changes the budget

Treat either increase or decrease as material. Retire live offers, preserve their earlier prices, and require explicit seller resubmission against the latest revision. Any new price must fit the updated ceiling.

### EC-10 — Seller changes a price

Require a live eligible offer and a within-budget price. Record a new offer revision, notify a buyer who viewed/contacted, and show latest terms before the next handoff. Earlier contact snapshots remain unchanged; exceeding the budget is refused.

### EC-11 — Item is sold before buyer contact

Seller marks unavailable, equivalent to withdrawal. Refuse any later handoff. If the seller failed to update availability, the buyer can provide structured feedback or report a misleading offer; the platform never guaranteed inventory.

### EC-12 — Several buyers contact the same seller about one item

Permit independent valid contacts from their own requests. No reservation or exclusivity exists. Seller updates affected offers when the item is unavailable; no buyer is labeled the winning purchaser by the platform.

### EC-13 — Several sellers answer the same request

Preserve each seller's independent private offer. Buyer can compare, decline, or contact multiple sellers within the contact allowance. No auction, automatic lowest-price winner, or required single acceptance is introduced.

### EC-14 — A report arrives after request closure

Accept it when the reporter has relevant public or historical interaction context. Use authorized retained evidence. Closing or removal does not by itself dismiss an allegation, restore contact, or prevent safety review.

### EC-15 — Someone shares a suspended request

Refuse new sharing through the product. Existing links return generic unavailability to strangers without disclosing content or report reasons. Their existence outside the platform does not override moderation.

### EC-16 — A category becomes prohibited

Reject new requests/offers/renewals in that class; hide identified existing affected content and disable contact. Notify owners with the new policy basis. Retain the historical classification in metrics and audit actions.

### EC-17 — User moves to another city

Update account defaults only. Existing request and offer locality snapshots remain as originally declared. Moving a current request requires an explicit material revision; offering in the old city requires truthful availability there.

### EC-18 — Professional returns to the free plan

Pause paid Radar at the entitlement deadline, keep saved criteria, and retain valid basic offers and history. Do not remove a professional declaration merely because payment ends. Future expanded quotas end prospectively, without retroactively deleting legitimate offers.

### EC-19 — Suspended seller is restored after request expiry

Restore account eligibility if authorized, but keep the offer expired. Restoration never extends the buyer's request or offers. A new eligible cycle permits a fresh offer.

### EC-20 — Suspended buyer reports that the need was resolved

Accept the owner outcome and complete the request while keeping moderation visibility restrictions. No new offer/contact is allowed and no pending investigation is dismissed.

### EC-21 — Seller changes their phone after contact

Require phone-change verification. Future eligible handoffs use the new verified destination; historical initiation context records the destination valid at that time. Notify a returning buyer that contact details changed without exposing the old number publicly.

### EC-22 — Current phone is already assigned to another account

Do not create a duplicate or transfer ownership through a normal profile edit. Offer recovery or reviewed recycled-number handling. A newly assigned telephone subscriber cannot inspect an unrelated previous owner's records.

### EC-23 — Buyer blocks seller after contact

Disable all new handoffs and invalidate live offers between them. Preserve a limited historical summary and report access. The platform cannot stop messages already possible in WhatsApp; the user can also use WhatsApp's own block controls.

### EC-24 — User unblocks someone

Remove the relationship restriction only. Previously invalidated offers stay unavailable; future otherwise eligible interactions may proceed, including a fresh offer in a later cycle.

### EC-25 — Duplicate contact or outcome action is retried

Return the existing business result for the same action. Do not duplicate reputation, unique contact counts, closure events, or attribution. A genuinely later handoff is a repeat event only if current eligibility still holds.

### EC-26 — Buyer completes a request without platform offers

Allow completion as resolution elsewhere or with unknown source. Count overall declared resolution, not platform-attributed resolution or a seller's success. No fictional contact is created.

### EC-27 — Buyer accidentally completes or cancels a request

Keep the terminal state; the owner can create a new need subject to limits. If a historical outcome was wrong, staff may record a correction with evidence, but cannot revive the old cycle or manufacture seller credit.

### EC-28 — Suspended content is edited to remove a violation

Store the private revision, preserve evidence, and keep the restriction until authorized review. If the cycle has expired, restoration returns expired availability rather than an active extension.

### EC-29 — Radar alert is prepared before closure

Recheck before release; invalidate a no-longer-eligible match. If already released, the request link shows current unavailability. Alert preparation alone is not a delivery/open event.

### EC-30 — Radar entitlement ends while a match is pending

Do not release a new paid alert after expiry. Retain only permitted historical usage/context and pause the Radar. A later repurchase starts new matching from activation, without stale backlog.

### EC-31 — A coordinated group reports one user

Group evidence, preserve sources, and prioritize based on credibility and potential harm. Use documented scoped containment if warranted. Raw report volume cannot produce a permanent ban or public negative trust badge.

### EC-32 — The free account's open-request limit is reached

Refuse another publication with an explanation. The buyer can close a genuinely resolved/unwanted request or wait for expiry. Owner removal frees an open slot but does not refund rolling activations or authorize duplicate feed refreshing.

### EC-33 — Seller repeatedly resubmits after buyer revisions

Allow only explicit matching resubmission within the existing slot, subject to daily offer allowance and any restriction. Record one distinct seller offer for supply-depth metrics, with multiple revision facts rather than inflated competition.

### EC-34 — Budget or category changes during an unsent offer

Reject stale submission and show current requirements. The seller confirms or revises terms before attempting again; there is no automatic acceptance against a hidden new budget or category.

### EC-35 — A business region or category label is renamed

Preserve its stable conceptual identity if scope is unchanged, and record the label change. A substantive scope change is versioned; historical cohorts do not get silently moved to a different city or item meaning.

### EC-36 — Staff lifts one of several restrictions

Leave independent restrictions effective. An offer remains unavailable if the seller is still suspended, the pair remains blocked, the request expired, or the content is prohibited. Restoration is never a blanket permission override.

### EC-37 — A shared preview displays an old budget

The live request is authoritative. Show current requirements on landing and validate the current revision for offering. A preview outside the product is not a guaranteed current quote.

### EC-38 — Account deletion occurs during an open fraud case

Stop all marketplace access and public identity immediately. Preserve only incident-relevant evidence under the stated review/retention policy; communicate the purpose and deletion limits in the account policy. Do not use retained data for commercial outreach.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [State transitions](../domain/state-transitions.md)
- [Invariants](../domain/invariants.md)
- [Permissions](../domain/permissions.md)
- [Acceptance criteria](acceptance-criteria.md)
