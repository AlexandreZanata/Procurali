---
document_id: DOC-08
status: recommended-business-baseline
scope: mvp
source_sections: ["8"]
last_updated: 2026-10-07
---

# 8. Request creation and maintenance

### 8.1 Minimum buyer journey

The user may understand the marketplace and browse requests before registration. To publish, the user registers or returns to their existing verified account, supplies the need, confirms it, and receives a shareable request. Previously supplied account locality is suggested, but the buyer confirms the request's actual locality.

Required request fields:

- **What the buyer needs:** A clear item title, such as “Refrigerator.”
- **Category:** One allowed physical-product category.
- **Maximum budget:** A positive BRL amount; this is a ceiling, not a price promise or a guarantee of affordability.
- **Accepted condition:** New, used, or either. No preselected condition that could silently narrow the request.
- **City and approximate region:** Enough context for local availability, without a meeting address.
- **Optional notes:** Preferences, dimensions, or practical requirements, such as “Preferably frost-free.”

Account name and contact number are not republished as request-description fields. Public display uses the buyer's chosen display name without additional identity details. Reject phone numbers, exact street addresses, unsolicited external links, or requests to move contact outside the controlled offer flow when included in public text; explain how to proceed safely. A moderator can remove content that evades this rule.

The same public-content restrictions apply to display names, professional names, and offer descriptions/notes. A name cannot be used as a phone-directory entry or a concealed external-contact link. Legitimate product model identifiers are not treated as phone numbers merely because they contain digits; uncertain cases allow correction or review rather than silently rewriting the user's content.

### 8.2 Publication

Publication validates account eligibility, complete fields, category and prohibited-content policy, explicit city, duplicate-intent rules, open-request limit, and rolling activation allowance. A valid draft becomes active for seven days, starts cycle one, and receives its first-publication time and deadline. No approval queue is required for ordinary allowed content; clearly prohibited content is refused or held for review.

Drafts are private, accept no offers, and are not shareable. Saving a draft does not consume publication allowance. Repeated draft creation still falls under general anti-abuse restrictions. A second attempt to publish the same already-published draft returns the existing result rather than another request.

### 8.3 Editing

The author may edit a draft freely. The author may edit an active or expired request subject to validity and the active-request revision allowance. Editing an expired request does not republish it; explicit renewal is still required. Editing while suspended can correct reported content, but only authorized restoration can make it visible again. Completed or cancelled requests are read-only in the MVP, apart from owner removal and administrative correction of an erroneous record.

**Material changes:** Budget, category, accepted condition, city/region, changing the item being sought, or a note that changes suitability. A different appliance, required size, or new required feature is material even if only the notes field changes.

**Nonmaterial changes:** Spelling, punctuation, or clearer wording that does not change suitability. These changes preserve current offers, deadlines, ordering, and offer history.

Before a material change takes effect, the buyer is informed that current live offers will become unavailable and sellers must respond to the updated requirements. Successful revision invalidates those offers, records the new requirements, and gives affected sellers a contextual notice. An invalidated seller can resubmit the same offer identity for the current revision, matching the updated terms; that resubmission consumes the applicable daily offer allowance but not an additional seller/request-cycle slot. All resubmissions preserve past views and contacts as history.

Changing a budget upwards does not resurrect old offers. Changing it downwards does not rewrite past offered prices. A seller must explicitly reconfirm current terms after either material change. This uniform rule has fewer exceptions than retaining selected offers across revisions.

### 8.4 Renewal and expiry

An expired, visible, unresolved request may be renewed. An active request may be renewed during its last 24 hours; this preserves intent without encouraging repeated early refreshes. An earlier renewal attempt is refused with the eligible renewal time. Suspended or owner-removed requests cannot be renewed until appropriate restoration, if restoration remains allowed.

Renewal requires the buyer to confirm they still need the item and that its requirements are current. It consumes activation allowance, ends the previous cycle, expires its remaining offers, and starts a fresh seven-day cycle. It retains the request identity, original publication date, and historical outcomes or contacts. It does not imply that earlier sellers still have their items.

When the deadline arrives, the request is unavailable for new offers and contact immediately. A recorded expiry and an owner notice follow. The owner can choose “Renew,” “I found it,” or “Close request.” No answer keeps the request expired. Renewal is optional and never automatic.

### 8.5 Closing and removing

“I found it” completes the request. “I no longer need it” cancels it. “Remove request” hides public content and cancels a still-unresolved request with a removal reason; the action explains the loss of pending interaction. Completion and cancellation retain a private historical summary for the owner and a limited status summary for affected sellers. A report or administrative review can still reference historical content under safety access rules.

Removal is a business concealment operation, not permission to erase evidence of fraud or fabricate historical metrics. Account-deletion and retention rules are defined in Section [15](../trust/privacy-and-security.md).

### 8.6 Initial categories

Allowed launch categories are **Furniture, Home Appliances, Electronics, Bicycles, Baby and Children's Items, and Tools**. “Baby and Children's Items” covers permitted physical goods such as strollers and toys, not children-related services or restricted medical products. Bikes are included; motor vehicles are excluded. Category labels have short scope descriptions to reduce accidental misclassification.

Do not accept requests for real estate, motor vehicles, jobs, services, food, medication, or animals in the MVP. Excluded categories are unavailable, even where a particular item might otherwise be lawful. An item can also be prohibited within an allowed category. For example, an account credential is prohibited even if submitted as “Electronics.”

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [Principles and policies](../product/principles-and-policies.md)
- [State transitions](../domain/state-transitions.md)
- [Invariants](../domain/invariants.md)
- [Offers](offers.md)
- [Outcomes](outcomes.md)
