---
document_id: DOC-18
status: recommended-business-baseline
scope: post-mvp
source_sections: ["18"]
last_updated: 2026-10-07
---

# 18. Radar Pro

### 18.1 Purpose and criteria

Radar Pro lets professionals receive relevant opportunities without repeatedly searching. It is planned for near-term post-MVP, with no dependency on it for the initial request–offer–contact loop.

An owner chooses one or more allowed categories, one explicit city, optional regions within it, an optional inclusive request-budget range, and accepted item conditions. The filter targets what buyers are requesting, not an inferred product catalog or a promised sale price. Future extra filters require a named business meaning.

If the region set is empty, match the entire selected city. If supplied, it is an exact region set; no hidden geographic expansion. For a seller interested only in used products, buyer requests accepting used or either match; new-only requests do not. Empty category selection is invalid to avoid accidental all-market alerts.

### 18.2 Deterministic match rules

A match requires all of the following:

1. Radar enabled, owner active and verified, and an effective Radar entitlement.
2. Request newly published, renewed, or materially revised after Radar activation, and currently active, visible, unexpired, and allowed.
3. Request not authored by the Radar owner, with no block between the parties.
4. Matching city and any explicitly selected regions.
5. Matching category, request-budget bounds, and accepted condition.
6. No earlier initial match for that Radar/request-cycle pair.

If a material revision newly makes a request match, one match may be created if none exists for that Radar and cycle. If it already matched earlier, the revision updates eligibility/context but does not create a second initial alert. A renewal may generate a fresh match, clearly labeled as renewed demand, not a newly acquired buyer.

### 18.3 Alerts and failure behavior

Example alert:

> New local request: someone is looking for a washing machine for up to BRL 800 in your monitored city.

The alert contains only permitted public details and a request link. Recheck current eligibility before releasing an alert. If the request closes, expires, becomes prohibited, or is blocked first, invalidate the pending match. If a delivered alert later becomes stale, its link shows the current status and no disabled action.

Use the owner's explicitly selected permitted notification destination. In-product notices are the default; optional external notification channels require a separate consent and availability policy. The initial Radar plan does not need to send WhatsApp marketing messages to buyers or automatically contact them.

Saved criteria persist when a subscription ends, but alerts pause at entitlement expiry. Reactivation starts matching new eligible demand from that time; there is no automatic historical alert flood. Notification failures do not count as delivered alerts or automatically grant a refund; service-remedy terms must be defined before commercial launch.

### 18.4 Radar measurement and limits

Measure matching requests, alerted matches, opened alerts, offers submitted after opening, unique contact initiations on those offers, and renewals of paid access. Interpret a Radar-assisted contact as attributed usage, not proof the alert exclusively caused the negotiation.

Before selling the first plan, publish the permitted number of Radars, selected cities/regions, categories, notification preferences, and any commercial offer allowance. Recommend **one Radar per account with multiple categories in one city** for the first plan: it supports local professional use without complicated packaging. Review genuine multi-category and multi-region needs before introducing tiers.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [Discovery](../workflows/discovery.md)
- [Professional sellers](professional-sellers.md)
- [Monetization](monetization.md)
- [Metrics](../measurement/metrics.md)
- [Permissions](../domain/permissions.md)
