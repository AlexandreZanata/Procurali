---
document_id: DOC-02
status: recommended-business-baseline
scope: mixed
source_sections: ["2"]
last_updated: 2026-10-07
---

# 2. Business principles and decision baseline

### 2.1 Product principles

1. **Purchase intent is the central object.** Every offer and marketplace contact belongs to a request.
2. **Simplicity precedes feature breadth.** Each mandatory capability must support publishing, discovery, offering, comparison, contact, outcome, or the safety of those actions.
3. **Local liquidity precedes geographic expansion.** Default discovery remains within the selected city.
4. **Fresh demand matters.** Active requests require an unexpired publication period and explicit renewal.
5. **Buyer choice controls contact.** Sending an offer never unlocks the buyer's phone number.
6. **An offer is an expression of availability.** It is neither a binding sale nor formal order acceptance.
7. **WhatsApp is intentional.** Negotiation continues there without an internal chat requirement.
8. **Privacy is the default.** Public content contains approximate location and no phone number or exact address.
9. **Rules must be explainable.** Deterministic relevance, progressive restrictions, and reviewable moderation precede complex prediction.
10. **Measurement must reflect evidence.** Declared outcomes, contact initiations, and confirmed feedback are separate facts.
11. **Payment does not buy trust.** Professional status or a subscription cannot bypass safety restrictions or imply verified identity.
12. **Commercial rules remain replaceable.** Prices and benefits can change without redefining requests, offers, or contacts.

The decision test is: **Does this rule help someone find a nearby person who has what they need, quickly and safely?**

### 2.2 Recommended policy defaults

The following are launch recommendations, not permanent limits. Changes apply prospectively unless explicitly stated. Each change records its effective date and reason; historical measurements use the policy that applied at the time.

- **Request duration: seven consecutive 24-hour periods from publication or renewal.** This balances realistic local discovery with stale demand. Review after observing response times and expired requests with unresolved demand.
- **Concurrent demand limit: three open requests per account in the free MVP.** An open request is active or suspended with an unexpired period. Three allows several legitimate household needs while limiting feed domination. Review blocked publication attempts and confirmed spam, including professional buyer use.
- **Publication or renewal allowance: six successful activations per rolling 24 hours per account.** Initial publication and renewal both consume an activation; deletion does not refund it. This supports three legitimate replacements without allowing rapid republishing. Review legitimate limit complaints and repeated-request abuse.
- **Offer allowance: one offer slot per seller per request activation cycle; ten successful submissions or explicit resubmissions per rolling 24 hours on the free plan.** Resubmission after a buyer's material revision uses the existing cycle slot but consumes one daily submission. Ordinary offer editing and withdrawing do not consume or refund submissions. Ten is a conservative trial limit for casual sellers; review legitimate seller demand, buyer-driven revision friction, and offer quality before increasing it.
- **Offer expiry: the earliest of the request cycle's deadline or seller unavailability.** No independent offer timer is needed in the MVP.
- **Strict budget and condition compatibility.** Above-budget or wrong-condition offers are refused. Alternatives are accepting mismatches with warnings or allowing negotiation of every mismatch; the strict choice keeps early relevance understandable. Sellers may offer after the buyer changes the request.
- **Material request changes retire existing live offers.** Budget, category, condition, item identity, or locality changes require sellers to submit for the new revision. Alternatives are retaining potentially invalid offers or checking each one individually; retiring them avoids stale assumptions.
- **Buyer-only initial contact.** Alternatives are seller-initiated contact or mutual contact approval. Buyer initiation requires fewer steps while protecting buyers from unsolicited outreach.
- **First-contact outcome prompt after 24 hours, plus an expiry prompt.** This gives negotiation time without requiring a conversation tracker. Review response rates and buyer fatigue.
- **No raw complaint-count bans.** A moderator decides permanent bans; temporary containment requires a documented reason. This reduces coordinated-report abuse.
- **Commercial capabilities follow the core MVP.** Implementing subscriptions before validating supply and demand would add unrelated obligations to the first release.

### 2.3 Policy values and interpretation

Money uses BRL at launch. Budgets and offered prices are positive amounts with at most two decimal places; zero and negative values are invalid. No artificial universal maximum price is introduced: a reviewable operational ceiling may be added only with a category-specific rationale. Prices represent the item price. Delivery and additional charges are outside the marketplace transaction flow; sellers must disclose known mandatory extra costs in their offer and must not advertise a misleading item price to conceal them.

Text limits are **120 characters for request titles and offer descriptions, 500 for optional request or offer notes, 80 for display names, and 1,000 for report details**. These starting bounds keep essential information readable and prevent large unsolicited content. Shorter valid text is allowed if it identifies the item; meaningless or unrelated text may be moderated. Review truncation complaints before changing these limits.

For active requests, allow **three material revisions per rolling 24 hours**, including the proposed successful revision. This reduces repeated disruption of existing offers; harmless spelling corrections and draft edits do not consume this allowance. Repeated nonmaterial changes intended to manipulate visibility are still abuse. Renewal never changes the original first-publication date or moves an existing request ahead of newer demand.

Rolling periods use actual elapsed time. A deadline is exclusive: at or after the deadline, the request is expired for every action even if the recorded expiration event has not yet been processed. City-local calendar dates are used for human-facing reporting; elapsed-time limits do not reset at midnight.

### 2.4 Additional decision register

- **Registration friction versus identity assurance.** Options: public browsing with phone confirmation only at participation, registration before browsing, or extensive identity checks for everyone. Recommend public browsing and minimal verified participation; visitors understand demand before committing, while consequential actions remain attributable.
- **Expiration of offers.** Options: separate seller-set deadlines, indefinite offers, or alignment with the request cycle. Recommend cycle alignment and immediate seller withdrawal for unavailability; it provides one understandable deadline without claiming inventory verification.
- **Renewal and existing offers.** Options: revive all offers, ask each seller to reconfirm automatically, or start with no live prior-cycle offers. Recommend a new cycle with fresh seller responses; old availability must not be assumed and renewal requires no extra seller workflow.
- **Discovery recency after renewal.** Options: rank by renewal time, original publication time, or a weighted score. Recommend original publication time with a factual renewal indicator; stale demand cannot repeatedly displace genuinely new needs.
- **Several interested sellers.** Options: require one accepted offer, automatically choose the lowest price, or allow buyer-selected multiple contacts. Recommend multiple contacts within a clear allowance; the platform helps comparison without becoming checkout or an auction.
- **Professional classification.** Options: force payment after a volume threshold, infer status invisibly, or accept declaration with evidence-based review. Recommend declaration and review; legitimate casual use is not penalized and paid access remains optional.
- **Reputation presentation.** Options: stars and open reviews, an opaque score, or labeled facts and structured observations. Recommend facts and limited structured feedback; the initial product needs less subjective moderation and makes fewer unsupported trust claims.
- **Account/content restoration.** Options: restore everything automatically, permanently discard everything, or restore explicitly while preserving original deadlines. Recommend explicit resource restoration; an account appeal cannot bring back stale or independently prohibited offers.
- **Commercial launch timing.** Options: sell contacts at launch, build subscriptions before liquidity, or validate the free loop before selling Radar convenience. Recommend the free loop first and Radar afterwards; monetization depends on real relevant demand rather than restricting its initial flow.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [State transitions](../domain/state-transitions.md)
- [Invariants](../domain/invariants.md)
- [Requests](../workflows/requests.md)
- [Offers](../workflows/offers.md)
- [Whatsapp contact](../workflows/whatsapp-contact.md)
- [Monetization](../commercial/monetization.md)
- [Metrics](../measurement/metrics.md)
