---
document_id: DOC-20-LIQUIDITY
status: recommended-business-baseline
scope: mixed
source_sections: ["20.9"]
last_updated: 2026-10-07
---

# 20.9 Operational liquidity classification


Measure historical response and current actionable supply separately. Historical relevant offers remain response evidence after an honest later withdrawal; known false or prohibited offers are excluded through traceable corrections. Current live relevant offers determine what the buyer can act on now.

At any observation time, a visible active request has one of these response states:

- **Awaiting supply:** Younger than 24 hours and no relevant offer yet.
- **No-offer gap:** At least 24 hours old and no relevant offer received.
- **Low liquidity:** First relevant offer arrived after 24 hours, or earlier offers existed but there are no current live relevant offers.
- **Healthy:** At least one relevant offer arrived within 24 hours and at least one current live relevant offer remains.
- **High seller competition:** Healthy, with at least three distinct sellers' current live relevant offers. This is a supply-depth signal, not guaranteed purchase success.

Apply the highest applicable state: high competition first, then healthy, then low liquidity, then no-offer gap or awaiting supply. A later first offer does not erase the missed 24-hour response target. For renewed demand, evaluate a separate cycle indicator from the renewal time while keeping the initial response record intact.

Twenty-four hours provides a meaningful early local response target; three sellers provide actual choice without requiring a dense marketplace. Both are provisional operating definitions, reviewed after observing each city's volumes and buyer response times.

Use these indicators operationally to identify underserved categories or regions, recruitment opportunities, stale availability, and the right time to expand geographic coverage. Future product features may suggest sharing or clearer requirements for weak demand. Paid plans must not be required to rescue a basic buyer's liquidity.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [Metrics](metrics.md)
- [Events](events.md)
- [Discovery](../workflows/discovery.md)
- [Mvp scope](../product/mvp-scope.md)
