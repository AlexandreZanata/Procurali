---
document_id: DOC-16
status: recommended-business-baseline
scope: mvp
source_sections: ["16"]
last_updated: 2026-10-07
---

# 16. Reports and moderation

### 16.1 Reasons and submission

Initial reasons are suspected fraud, spam, false information, prohibited item, inappropriate behavior, inappropriate content, and other. “Other” requires a short explanation within the report-detail limit. Other reasons allow optional contextual detail. A report may concern visible current content or the reporter's own historical interaction, including after request closure.

Require an authenticated account and a relevant target. A suspended user may report or appeal concerning their own historical interaction; a banned user may submit an appeal or incident concerning their own history without marketplace privileges. Do not allow reporting one's own content, fabricated identifiers, or irrelevant inaccessible private offers.

A duplicate reporter/target/incident report becomes additional information in the existing case. Reports from different users may be grouped, but each retains its source and context. Report withdrawal is recorded; serious review may continue independently.

### 16.2 Triage and priorities

- **Critical:** Credible imminent harm, severe prohibited content, or evidence of active coordinated fraud. Place first for staffed review; use scoped temporary containment where needed. Target first review within four staffed hours.
- **High:** Specific fraud evidence, stolen-goods allegations, repeated substantiated deception, or deliberate destination harvesting. Target first review within one business day.
- **Normal:** Ordinary spam, misleading descriptions, unsuitable offers, or behavior complaints without urgent harm. Target first review within three business days.
- **Low:** Duplicates, unsupported general complaints, or minor categorization issues. Target first review within five business days.

These are operating targets that require an accountable moderation owner and published support hours; they are not a promise of continuous staffing. Record missed targets and oldest unresolved cases. Severity derives from evidence and potential impact, not the accused user's plan or a raw complaint count.

### 16.3 Decisions

The moderator inspects relevant content and history, distinguishes an allegation from a finding, and chooses no violation/invalid report, duplicate, corrective notice, content removal, temporary scoped suspension, account suspension, or permanent ban. The decision identifies applicable policy, evidence, reason, duration if temporary, and any appeal route.

Recommended first ordinary account suspension is 24 hours for repeated low-severity abuse after a warning; serious credible harm may justify immediate suspension pending review. Alternatives are fixed penalties for every case or escalating blindly by count; the recommended approach uses documented severity and prior substantiated behavior. Staff must justify a different duration and review pending-investigation restrictions within seven days.

A suspension's account restriction can end at its specified deadline unless another restriction remains. Content restoration remains explicit and observes original request deadlines. Bans are indefinite until formally reversed. Invalid allegations do not penalize the target. Demonstrably malicious reporting may receive the same progressive treatment as other abuse; an unproven report is not automatically malicious.

### 16.4 Appeals and audit

Offer one appeal per decision, with additional review when material new evidence arises. A reviewer distinct from the original decision-maker is preferable when staffing allows it. Record whether a decision was upheld, narrowed, or reversed, and correct affected derived reputation. Never erase the original action or falsely restore expired commercial access or request cycles.

Staff actions must include actor identity, permission context, target, previous state, resulting state, reason, policy version, timestamp, duration, evidence references, and related reversal. Merely inspecting sensitive contact data also records actor and purpose. Notify the affected user of actionable restrictions and appeal options, without exposing reporter identities or another participant's confidential data.

### 16.5 Blocking between users

Blocking is mandatory basic protection in the MVP. Either party can block the other after discovering a relevant profile or interaction. The other party is not given the block owner's private reason.

An active block hides each user's eligible requests from the other signed-in user's discovery, prevents new offers in either direction, invalidates live offers between the pair, and disables all future contact handoffs between them. Past events remain in a limited private historical summary, and reporting remains available. Unblocking permits future ordinary eligibility but does not resurrect invalidated offers or previously shared phone secrecy.

### 16.6 Extensible prohibited-content policy

Initial platform prohibitions cover weapons and weapon components; illegal drugs; controlled medication and medication requests outside MVP scope; stolen goods; counterfeit or illicit products; identity documents; bank accounts or financial-account access; credentials; service accounts; items whose sale violates applicable restrictions; exploitative or inappropriate material; and any item class later prohibited by a documented platform policy.

These are platform exclusions, not a legal determination that every item in each broad class is unlawful. A policy entry states its item class, plain-language scope, examples, effective date, rationale, and treatment of existing requests/offers. Staff can add a class without changing core request/offer semantics.

A newly prohibited class immediately stops new publication and offerings. Identified active requests/offers in that class become hidden and ineligible for contact, and owners receive the relevant policy explanation. Paid status supplies no exception. Retired-but-not-prohibited categories may finish existing eligible cycles; new publication and renewal are unavailable until an allowed recategorization occurs.

### 16.7 Minimum administrative operations

Authorized staff can inspect users, requests, offers, reports, and relevant history; filter open incidents by severity/age; suspend and restore accounts or content; ban and formally reverse bans; inspect current liquidity and abuse indicators; and approve prospective business-policy changes. Bulk disclosure of private phones is not an ordinary administrative capability.

Moderators handle content and scoped safety actions. Administrators additionally approve bans, reverse bans, grant operational permissions, and approve commercial/policy changes. A sole launch operator may hold both grants, but actions still identify the grant and purpose. Staff do not edit seller prices, impersonate user contact, or fabricate a buyer's outcome; administrative correction is explicit and audited.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [Users](../domain/users.md)
- [State transitions](../domain/state-transitions.md)
- [Invariants](../domain/invariants.md)
- [Privacy and security](privacy-and-security.md)
- [Permissions](../domain/permissions.md)
