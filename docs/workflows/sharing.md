---
document_id: DOC-13
status: recommended-business-baseline
scope: mvp
source_sections: ["13"]
last_updated: 2026-10-07
---

# 13. Sharing and acquisition

### 13.1 Shareable content

After valid publication or renewal, an eligible buyer may share through WhatsApp, link copying, or the device's sharing capability. Sharing is a core acquisition action, not a secondary promotional feature.

Example message:

> I am looking for a used refrigerator for up to BRL 600 in Jardim Primavera. Have one to sell? See my request on Procuro Aí: [request link]

Use only public request details. The stable direct link resolves the current request status and requirements; it never permanently freezes an earlier budget or grants offer privileges. A share preview may become stale outside the platform, so the live request detail is the authority.

### 13.2 Returning and external visitors

The acquisition loop is:

**Publish → share → external seller opens the request → seller understands demand → seller registers only when offering → offer submitted → seller discovers more local requests.**

Preserve the request context through registration, keep required seller fields minimal, and suggest eligible same-city requests after submission. Avoid forcing registration before the visitor can understand the need or placing contact behind a subscription.

### 13.3 Link behavior after a lifecycle change

- **Active and eligible:** Show public request details and the offer action.
- **Expired, completed, or cancelled:** Show a limited status summary, no offer/contact action, and other eligible local requests. Do not reveal private offers or an outcome's seller attribution.
- **Suspended, prohibited, deleted-account, or owner-removed:** Show a generic unavailable result without protected request details or allegations. Do not distinguish safety reasons to strangers.
- **Blocked pair:** A signed-in blocked party receives the unavailable result. Anonymous public browsing cannot enforce a personal block reliably; it still exposes no contact destination or offer capability.

Sharing controls are disabled for currently unavailable requests. A previously copied link cannot be revoked from someone else's messages, but its destination respects current visibility.

### 13.4 Attribution policy

Record share intent, observable direct-link landing, and attributable registration or offer submission separately. Recommended acquisition window: seven elapsed days after a shared-link landing. Attribute a new user's registration to the most recent identifiable eligible shared-request landing in that window. If attribution is missing or ambiguous, mark it unknown rather than guessing.

This seven-day starting window matches the initial request lifecycle and is simple to explain; review observed time-to-registration before changing it. Copied links without an identifiable source can be counted as direct request visits but cannot prove which person or sharing channel caused a conversion.

Never collect recipient lists, group names, WhatsApp message delivery, or private conversation contents to measure sharing.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [Requests](requests.md)
- [Discovery](discovery.md)
- [Whatsapp contact](whatsapp-contact.md)
- [Privacy and security](../trust/privacy-and-security.md)
- [Metrics](../measurement/metrics.md)
