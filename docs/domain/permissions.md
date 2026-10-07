---
document_id: DOC-21
status: recommended-business-baseline
scope: mixed
source_sections: ["21"]
last_updated: 2026-10-07
---

# 21. Permissions by role and resource context

The following is the permission matrix expressed as contextual capability groups. Every grant also checks current state, deadline, visibility, allowed category, relationship blocks, limits, and purpose.

### 21.1 Anonymous visitor

- May discover and read eligible public requests in a deliberately selected city.
- May open eligible direct share links and understand the product before registration.
- May start registration with request context preserved.
- May not submit offers, publish requests, read private offers, initiate contact, inspect phone numbers, report private resources, or use commercial account capabilities.

### 21.2 Active verified user acting as buyer

- May create and publish their own valid requests within limits.
- May edit, renew, complete, cancel, or remove only their own requests, under lifecycle rules.
- May read all offers on their own request, decline them, and initiate eligible contact on them.
- May report relevant content or interactions, block another user, and provide structured feedback after their own contact.
- May not edit seller offer terms, see another buyer's private offers, contact on another buyer's request, or move a request to another author.

### 21.3 Active verified user acting as seller

- May discover eligible requests and submit one valid offer slot per request cycle within allowance.
- May read, edit, withdraw, or mark unavailable only their own offers, under lifecycle rules.
- May read the public request context and their own offer's state, not competing private offers.
- May report relevant content/interactions and block another user.
- May not self-offer, reveal buyer phone through offer submission, initiate unsolicited platform contact with the buyer, or change the buyer's outcome.

### 21.4 Declared professional seller

- Has the same free contextual buyer/seller permissions as any eligible user.
- May maintain their own professional profile and factual business declaration.
- May configure and use Radar only with current commercial eligibility when that capability is released.
- May not use a professional label or payment to bypass limits not explicitly changed by the plan, restrictions, privacy, or relevance rules.

### 21.5 Pending or restricted account

- Pending users may manage minimal registration and verification and browse eligible public content.
- Suspended users may inspect their own limited history, correct suspended content privately, record an outcome on their own unresolved request, submit relevant reports/appeals, and request deletion. They cannot republish, renew, offer, or contact.
- Banned users may access the appeal/deletion path and incident reporting for their own history; marketplace mutation remains unavailable.
- Deleted accounts have no participant access. Another active participant retains only the limited non-sensitive history needed for their own record or report.

### 21.6 Moderator

- May inspect relevant users, resources, reports, and history for an assigned safety purpose.
- May triage, decide ordinary reports, warn, hide content, apply scoped or temporary suspensions, and restore eligible suspended content/accounts within granted scope.
- May not permanently ban or reverse a ban unless also explicitly granted administrator authority; may not impersonate users or silently rewrite marketplace terms.
- Inspection of sensitive details and important changes is audited.

### 21.7 Administrator

- Has granted moderation abilities plus permanent bans, ban reversals, operational permission management, policy approval, and later commercial-plan administration.
- May authorize recorded historical corrections, never undocumented outcome fabrication.
- May review aggregate operational indicators and approved incident-relevant sensitive information.
- Has no ordinary authority to initiate WhatsApp contact on a buyer's behalf or disclose participant phones unrelated to a documented purpose.

Ownership is necessary but insufficient: an owner cannot contact on an expired request, edit a withdrawn offer into a live one, or restore prohibited content. Staff authority is also scoped; an administrator's action must still leave an audit trail and respect independent restrictions.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [Users](users.md)
- [State transitions](state-transitions.md)
- [Invariants](invariants.md)
- [Privacy and security](../trust/privacy-and-security.md)
- [Reports and moderation](../trust/reports-and-moderation.md)
