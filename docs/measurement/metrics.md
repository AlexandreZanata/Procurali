---
document_id: DOC-20
status: recommended-business-baseline
scope: mixed
source_sections: ["20.6", "20.7", "20.8"]
last_updated: 2026-10-07
---

# 20. Business metrics

Event definitions live in [Business events](events.md), and operational classifications live in [Liquidity](liquidity.md).

### 20.6 Measurement conventions

Use distinct request identities for new-demand counts and separate request-cycle counts for renewal behavior. Use original city/category snapshots for publication cohorts, and report current locality/category separately when requirements change. Exclude staff tests, recognized bots, and confirmed abusive activity from product-health aggregates; preserve a documented exclusion count rather than silently changing totals.

For first-publication cohort metrics, use a **seven-day interaction window** from initial publication and a **14-day outcome window** from initial publication. This allows an additional week for buyers to declare a resolution after expiry. Mark younger cohorts incomplete; do not compare them directly to mature cohorts. Renewal-cycle reports use their own seven-day interaction window and are labeled cycle metrics.

If the denominator is zero, report “Not applicable,” not zero performance. Report sample size and city/category alongside rates. Use medians and the 90th percentile alongside arithmetic means for response times. Unanswered requests are shown separately because averaging only answered requests can disguise low liquidity.

### 20.7 North Star and core conversion metrics

**North Star: unique valid contact initiations within the first seven days ÷ distinct requests first published in a mature cohort.**

This is a measurable approximation of “contacts initiated per published request.” It measures usable buyer/seller connections, not proven real conversations. Multiple sellers can be contacted for one request, so the value can exceed 1 and must not be displayed as a bounded success percentage. Pair it with the request-contact coverage rate to detect a few requests dominating activity.

- **Published requests:** Distinct requests first published in the selected cohort. Shows demand volume; renewals are separate.
- **Offer coverage:** Requests receiving at least one relevant offer in seven days ÷ published requests. Shows whether demand attracts suitable supply.
- **First-offer time:** Elapsed time from first publication to the first relevant offer, for answered requests; show unanswered count. Shows supply responsiveness.
- **Offers per request:** Distinct seller offers in the interaction window ÷ published requests, with median and distribution. Resubmission revisions do not add distinct sellers. Shows comparison depth.
- **Offer-view rate:** Relevant offers first viewed by the buyer within the cohort interaction window ÷ relevant offers submitted in that same window. Shows buyer engagement; flag offers submitted near the window's end with little viewing time.
- **Offer-to-contact conversion:** Distinct relevant offers receiving a unique contact in the interaction window ÷ relevant offers submitted in that window. Shows whether offers lead to buyer interest.
- **Request-to-contact conversion:** Distinct published requests with at least one unique contact in seven days ÷ published requests. Shows contact coverage and is bounded from 0% to 100%.
- **Completed requests:** Requests in the cohort with buyer-reported resolution by day 14. Shows declared needs resolved, with source breakdown.
- **Resolution rate:** Buyer-reported completed requests by day 14 ÷ all published requests in that mature cohort. Shows reported overall resolution; unknown outcomes reduce observed coverage but must be disclosed.
- **Platform-attributed resolution rate:** Completed requests explicitly linked to a valid historical contact by day 14 ÷ published requests. Shows declared marketplace contribution rather than all outside resolution.
- **Respondent success rate:** Completed requests ÷ requests reporting a final resolved or no-longer-needed outcome by day 14. This excludes unknown outcomes and must be shown beside outcome-response rate to expose selection bias.
- **Outcome-response rate:** Requests with any buyer outcome answer by day 14 ÷ published requests. Shows observability; “Not yet” counts as a response but not final success.
- **Requests with no offer after 24 hours:** Requests still active at hour 24 with no relevant offer received by then ÷ requests still active at hour 24. Also show the count of requests closed before that observation point. Shows early liquidity gaps.

“Request served” means at least one relevant offer within the declared response window; use offer coverage as its explicit definition. “Need resolved” means a buyer-declared completion. These are different metrics and must not share an ambiguous “success” label.

### 20.8 Retention, sharing, professional, and safety metrics

- **Recurring buyers:** Distinct buyers publishing in both the current and immediately preceding 30-day window ÷ buyers publishing in the preceding window. Shows repeat demand participation without counting draft-only users.
- **Recurring sellers:** Distinct sellers submitting relevant offers in both those windows ÷ sellers submitting in the preceding window. Shows retained supply; measure contacted-seller retention separately.
- **Share intent volume:** Distinct recorded share actions, by channel and city. Shows use of the growth loop, not delivered messages.
- **Share landing conversion:** Attributable new registrations within seven days of eligible shared-link landing ÷ distinct attributable eligible visitors. Anonymous visitors without reliable distinct identity are reported separately as visits, not fabricated unique people.
- **Share-to-offer conversion:** Distinct attributed external visitors who submit an offer within seven days ÷ distinct attributable eligible visitors. Shows whether shared demand recruits supply.
- **Active professionals:** Declared professional accounts making at least one eligible offer or opening a relevant Radar opportunity in 30 days. Distinguish paid professionals, free professionals, and merely registered profiles.
- **Radar adoption:** Eligible paid sellers with at least one active Radar ÷ sellers with effective Radar access. Shows activation after purchase.
- **Radar open rate:** Matches opened within seven days of alert release ÷ alerts released in a mature seven-day alert cohort. External delivery uncertainty is reported separately.
- **Radar-assisted offers/contacts:** Offers linked to an opened Radar opportunity, and their unique contact initiations. Uses recorded attribution; does not claim causal proof.
- **Paid renewal rate:** Subscriptions with a new confirmed interval ÷ subscriptions whose previous interval ended in the observation period. Separate voluntary cancellation, payment failure, and safety revocation.
- **Valid-incident rate:** Reviewed valid incidents ÷ eligible marketplace interactions in the same defined period, with reporting delay disclosed. Raw report count alone is not the harm rate.
- **Moderation health:** Pending cases, time to first review by priority, restriction reversals, and recurring substantiated abuse. Shows operational load and decision quality.
- **Allowance friction:** Legitimate users refused by each allowance, later successful action, and reviewed appeals. Helps tune provisional thresholds rather than increasing them blindly.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [Overview](../product/overview.md)
- [Principles and policies](../product/principles-and-policies.md)
- [Whatsapp contact](../workflows/whatsapp-contact.md)
- [Outcomes](../workflows/outcomes.md)
- [Sharing](../workflows/sharing.md)
- [Reputation](../trust/reputation.md)
- [Radar pro](../commercial/radar-pro.md)
- [Events](events.md)
- [Liquidity](liquidity.md)
