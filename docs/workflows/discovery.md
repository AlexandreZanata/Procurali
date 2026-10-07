---
document_id: DOC-09
status: recommended-business-baseline
scope: mvp
source_sections: ["9"]
last_updated: 2026-10-07
---

# 9. Local discovery

### 9.1 Default eligibility and ordering

Public discovery shows active, visible, unexpired, allowed requests in the selected city. A signed-in user's default city is suggested; an anonymous viewer deliberately selects a city or opens a direct request link. Do not infer a precise location or silently search nationwide.

Offer filters for category, region, minimum/maximum request budget, and accepted condition. Filter bounds are inclusive and positive when supplied. An inverted price range is refused. Leaving a region unspecified shows the whole city, preserving opportunities between nearby neighborhoods.

Recommended default order is:

1. Apply selected city and explicit filters as eligibility conditions.
2. If a region preference is selected without strict region filtering, place requests in that region first, then the rest of the city.
3. Within each group, order by original first-publication time, newest first.
4. Resolve identical timestamps using a stable request identifier.

Renewal does not create a recency boost. An eligible request can display “Still looking; renewed recently” as a factual business label. Editing does not change discovery rank. Paid sellers do not receive privileged placement for their buyer requests in the MVP.

### 9.2 Geographic flexibility

The MVP requires seller availability in the request's city. A seller with a different default profile city can deliberately select the buyer's city and confirm they can make the item available there; the offered locality records that commitment. This avoids treating a profile move as proof of physical impossibility.

There is no required exact address, distance calculation, radius, or neighborhood adjacency prediction. Buyers and sellers handle meeting feasibility on WhatsApp. Users can manually select another city for discovery; it never becomes an automatic fallback when the current city is empty. A future radius or explicit neighboring-city rule requires a separate policy decision.

### 9.3 Public request detail

A direct visitor can understand the item, budget, condition, category, approximate region, buyer display name, and current availability, and find the “I have this” action. The visitor does not see received offers, private notes intended for administration, or phone numbers.

Selecting “I have this” requires an eligible account before submission. Preserve the intended request and any unsent permitted offer details through registration so the seller can continue without rediscovering the request. Recheck availability after registration because it may have changed.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [Principles and policies](../product/principles-and-policies.md)
- [Invariants](../domain/invariants.md)
- [Requests](requests.md)
- [Sharing](sharing.md)
