---
document_id: DOC-07
status: recommended-business-baseline
scope: mixed
source_sections: ["7"]
last_updated: 2026-10-07
---

# 7. Business rules and invariants

### 7.1 Identity, ownership, and permissions

- **INV-01:** Every request has exactly one registered author; ownership cannot be transferred in the MVP.
- **INV-02:** Every offer has exactly one seller and one existing request, cycle, and revision.
- **INV-03:** A user cannot offer on their own request, including through a professional profile of the same account.
- **INV-04:** Pending, suspended, banned, or deleted users cannot publish, renew, offer, or initiate contact. Restricted users retain only specifically allowed safety and account-management actions.
- **INV-05:** Role labels never replace resource ownership, state, or block checks.
- **INV-06:** Payment cannot grant administrative powers or override another user's consent.
- **INV-07:** Current phone uniqueness is enforced across non-deleted accounts; recovery cannot expose a recycled number's previous owner.

### 7.2 Request integrity

- **INV-08:** An active request has complete valid fields, a category allowed for its current cycle, an enabled city, and a future deadline. A category retired only for new use may finish an existing cycle; prohibition never allows such an exception.
- **INV-09:** A request cannot accept new offers or new contact when completed, cancelled, expired, suspended, removed, or otherwise restricted.
- **INV-10:** Budget is positive, uses the launch currency, and contains no more than two decimal places.
- **INV-11:** A request has one item need, not a bundled shopping list of unrelated categories.
- **INV-12:** Renewal requires explicit current intent, starts a new cycle, and never rewrites initial publication history.
- **INV-13:** Completion, cancellation, and expiry have distinct meanings and distinct events.
- **INV-14:** Material changes preserve the prior revision and invalidate incompatible historical assumptions by retiring live offers.
- **INV-15:** Editing, renewal, removal, or recreating content does not reset rolling abuse counts.
- **INV-16:** A buyer's default-locality change never silently moves a published request.

### 7.3 Offer integrity

- **INV-17:** Offer creation requires an active request and eligible buyer and seller at the time of submission.
- **INV-18:** Each seller has at most one offer slot for a request cycle; withdrawal or rejection does not free another slot.
- **INV-19:** Offer price is positive, in the request's currency, and no greater than the current maximum budget.
- **INV-20:** Offer condition is new or used, and must match the request unless the buyer accepts either.
- **INV-21:** An offer concerns the requested item, with a truthful, specific description; unsupported substitutions are subject to decline and moderation.
- **INV-22:** New offers must satisfy the selected city scope, or an explicitly allowed future geographic extension.
- **INV-23:** An offer is never a reservation or an enforceable platform order; no inventory exclusivity is implied.
- **INV-24:** Offer changes create new terms history; past contact snapshots remain intact.
- **INV-25:** Terminal offers are not eligible for new contact, including after the seller withdraws following contact.
- **INV-26:** Renewal never makes previous-cycle offers live again.

### 7.4 Contact and privacy integrity

- **INV-27:** Only the current request author can initiate a marketplace contact on that request's live offer.
- **INV-28:** Contact requires both parties eligible, the offer live and current, no block, and no content restriction.
- **INV-29:** Contact belongs to an offer, request cycle, buyer, and seller; there is no unrestricted user-phone lookup.
- **INV-30:** Offer submission alone never reveals the buyer's phone or unlocks unsolicited seller contact.
- **INV-31:** Public pages, share messages, and discovery do not expose phone numbers, precise addresses, report identities, or private offer terms.
- **INV-32:** A WhatsApp handoff necessarily reveals its destination to the initiating buyer. It cannot be represented as permanent concealment of the seller's number.
- **INV-33:** A recorded handoff does not imply message delivery, a real conversation, or a sale.
- **INV-34:** Repeated execution of one action does not duplicate a contact, subscription interval, outcome, or moderation decision.

### 7.5 Safety and commercial integrity

- **INV-35:** Active blocks prevent new interactions in both directions while preserving legitimate historical evidence.
- **INV-36:** Reports remain allegations until assessed; raw report volume alone cannot cause a permanent ban.
- **INV-37:** Important administrative changes require actor, reason, scope, time, and previous/resulting eligibility.
- **INV-38:** Lifting one restriction cannot override an independent restriction, expired period, deletion, or ban.
- **INV-39:** Professional declaration, paid access, phone control, and reputation are separate facts.
- **INV-40:** Paid features expire by entitlement time; basic marketplace access remains free subject to ordinary limits.
- **INV-41:** One Radar/request-cycle pair generates at most one initial match, and every alert rechecks current eligibility.
- **INV-42:** Reputation and aggregate measurements exclude duplicate events and identified abusive activity while preserving traceable corrections.
- **INV-43:** No public reputation claim labels buyer-reported completion as a verified transaction.
- **INV-44:** Deleting an account stops current interaction immediately; preservation of limited safety records does not permit public reuse of deleted personal data.

### 7.6 Duplicate intent and abuse limits

Refuse an exact repeated open request from the same buyer when its normalized item title, category, condition, budget, city, and region are identical. Offer editing or renewal of the existing request instead. Near-duplicates that change wording may be legitimate; show a reminder and allow moderation rather than declaring automated certainty.

For this exact-duplicate comparison, trim leading/trailing title whitespace, collapse repeated internal whitespace, and ignore letter case. Do not translate, remove words, infer synonyms, or use semantic similarity to declare an exact duplicate. Compare category and locality by their stable business identity and budget by its monetary value. Minor wording variations remain possible near-duplicates for review, not automatic certainty.

Deleting and recreating the same need within its unexpired seven-day window is treated as repetitive publication if intended to regain discovery position. Legitimate corrections should use editing. Repeated replacement requests consume activation allowance and can trigger a warning, temporary publishing restriction, then moderator review for repeated abuse. No permanent ban follows merely from hitting a limit.

Every refused action explains its business reason and the next legitimate action, such as editing, renewing, withdrawing, waiting for an allowance to recover, or requesting review. A refusal does not consume a successful-action quota.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [Principles and policies](../product/principles-and-policies.md)
- [Entities](entities.md)
- [State transitions](state-transitions.md)
- [Permissions](permissions.md)
