---
document_id: DOC-04
status: recommended-business-baseline
scope: mvp
source_sections: ["4"]
last_updated: 2026-10-07
---

# 4. User types and account lifecycle

### 4.1 One account, multiple marketplace roles

Buyer and seller are contextual roles. The same account can buy and sell simultaneously. A professional seller may also publish personal requests. No duplicate account is required for another role, a second city, or a professional profile. Moderator and administrator capabilities are explicitly granted operational permissions, never self-selected marketplace roles.

### 4.2 Minimum registration

Require a display name, a telephone number usable for WhatsApp contact, a city, and a neighborhood or approximate region. A region such as “Central area” is acceptable when a precise neighborhood is unavailable. The user confirms control of the phone number before publishing, offering, or contacting. Browsing public eligible requests does not require registration.

Do not require a tax identifier, full address, biography, birth date, photograph, or extensive documents in the MVP. The owner must accept the marketplace rules and understand that WhatsApp contact shares the seller's contact destination with an interested buyer.

**Phone validation:** Require an international country code and a structurally valid telephone number. Treat different formatting of the same number as the same identity value. Confirmation proves control of the number, not legal identity, item ownership, or permanent WhatsApp availability. The user declares that the number can receive WhatsApp conversations; invalid or reported unusable destinations require correction before further contact.

**Duplicate accounts:** One current phone number belongs to at most one non-deleted account. A registration using an existing number follows account recovery rather than silently creating another account. Evidence of number recycling requires review; possession of a recycled number must not expose a previous owner's history. Deleted-account identifiers remain separated from a later new account. Suspected abuse across multiple numbers requires evidence and moderation, not assumptions about a shared neighborhood or surname.

**Phone changes:** Require renewed proof of account control and confirmation of the new number. Keep the existing number until the change succeeds. A number already assigned to another account cannot be adopted through the edit flow. Future handoffs use the new number; earlier contact snapshots remain historical administrative records and are never silently rewritten. Notify the account owner of a successful change through the account's available notice channel.

### 4.3 Account states

- **Pending verification:** Registered but not yet eligible for publishing, offering, or initiating contact.
- **Active:** Verified and eligible, subject to ownership, resource state, limits, and safety rules.
- **Suspended:** Temporarily barred from marketplace mutations and new contact. The owner can inspect their own records, submit an appeal, and request account deletion.
- **Banned:** Indefinitely barred following a documented moderation decision. Recovery of marketplace access requires a separate administrative reversal, not a timer.
- **Deleted:** No marketplace access, discoverable identity, active request, or live offer. A minimal safety record may remain for the defined retention purpose.

Suspension hides the user's active requests and live offers, prevents all new contact involving them, and leaves original deadlines running. Restoring an account does not automatically republish content; resource restoration is an explicit, separately checked action. A ban cancels active buyer requests, invalidates seller offers, and removes public visibility.

### 4.4 Future verification

Additional professional identity checks may later create a separate verification claim with its own method, date, scope, and revocation. Neither phone confirmation nor subscription payment may be labeled identity verification or proof of trustworthy inventory.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [State transitions](state-transitions.md)
- [Privacy and security](../trust/privacy-and-security.md)
- [Reports and moderation](../trust/reports-and-moderation.md)
- [Permissions](permissions.md)
