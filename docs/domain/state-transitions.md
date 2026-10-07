---
document_id: DOC-06
status: recommended-business-baseline
scope: mixed
source_sections: ["6"]
last_updated: 2026-10-07
---

# 6. States and state transitions

### 6.1 Request lifecycle

The following is a textual business diagram, not implementation code:

> Draft → Active: successful publication.  
> Draft → Cancelled: owner discards the draft.  
> Active → Completed: buyer reports the need resolved.  
> Active → Cancelled: buyer no longer needs it or eligible administrative cancellation.  
> Active → Expired: the current cycle deadline is reached.  
> Active → Suspended: temporary content or account restriction.  
> Active → Active, new cycle: eligible explicit renewal, ending the earlier cycle.  
> Expired → Active, new cycle: eligible explicit renewal.  
> Expired → Completed: buyer reports the already-resolved need after expiry.  
> Expired → Cancelled: buyer reports no longer needing it.  
> Suspended → Active: explicit authorized restoration while the original deadline is still future.  
> Suspended → Expired: original deadline reached, including during suspension.  
> Suspended → Completed or Cancelled: owner records an outcome; visibility restrictions persist.  
> Completed and Cancelled → no reactivation: a later need requires a new request.

Suspension stores the prior lifecycle context and restriction reasons. Expiry can change the lifecycle state while the moderation visibility remains hidden. Removing a request hides it; if its state is active, expired, suspended, or draft, removal cancels it with an owner-removal reason. Removal of an already completed or cancelled request preserves that outcome and hides its content. Restoration of removed content is not an MVP owner action.

There is no permanent active state, automatic renewal, completed-to-active transition, or cancellation-to-active transition. A new similar request may be created after terminal closure, subject to abuse limits.

### 6.2 Offer lifecycle

> Sent → Viewed: buyer opens the offer's details.  
> Sent or Viewed → Contacted: eligible buyer initiates contact.  
> Sent, Viewed, or Contacted → Withdrawn: seller withdraws or marks the item unavailable.  
> Sent, Viewed, or Contacted → Rejected: buyer declines the offer.  
> Sent, Viewed, or Contacted → Expired: request cycle ends through expiry or renewal.  
> Sent, Viewed, or Contacted → Invalidated: material request edit, request closure/removal, ban, active block, prohibited category, or deletion makes interaction unavailable.  
> Sent, Viewed, or Contacted → Suspended: scoped temporary restriction.  
> Suspended → previous live state: explicit restoration with the same current request cycle/revision and no remaining restrictions.  
> Suspended → Expired or Invalidated: deadline, cycle change, requirement change, or another disqualifying event occurs.  
> Invalidated for material request revision → Sent: seller explicitly resubmits matching terms for the latest revision within the existing slot.

Withdrawn, rejected, expired, and other invalidated offers cannot be reactivated in the same cycle. A later request cycle permits a fresh offer subject to current policy. Editing an eligible live offer preserves its engagement state and creates a new offer revision; it does not make a viewed or contacted offer appear unseen or erase contacts.

“Contacted” records historical engagement while allowing further eligible handoffs. It never means accepted, reserved, bought, or completed. Historical viewed and contacted timestamps survive later terminal states.

### 6.3 Closure effects and precedence

When a request completes or is cancelled, all remaining live offers become invalidated with the relevant request-closure reason. When it expires or renews, remaining live offers expire because their cycle ended. Already terminal offers keep their original terminal reason.

When several restrictions apply, eligibility follows this precedence: **account deletion or ban → prohibited content or active safety restriction → active block → owner removal → terminal request state or elapsed deadline → stale cycle/revision → ordinary action limits**. This determines the user-facing refusal; it does not discard other restriction reasons.

An operation evaluates eligibility at the moment the business change takes effect. If expiration or closure wins before an offer submission or contact, the action is refused. No actor can use an earlier opened request or offer to bypass current state.

### 6.4 Subscription and Radar lifecycle — Post-MVP

> Subscription Pending → Active: commercial purchase confirmed.  
> Active → Cancellation scheduled: renewal disabled, access retained through paid interval.  
> Active or Cancellation scheduled → Ended: paid interval ends without a new confirmed interval.  
> Any eligible subscription → Revoked: documented commercial or safety decision.  
> Ended → Active: a new confirmed paid interval; no retroactive access is implied.

> Radar Active → User-paused: owner chooses pause.  
> Radar Active → Entitlement-paused: paid access ends.  
> Radar Active → Restricted: account or safety restriction.  
> Paused or Restricted → Active: explicit activation after eligibility is restored.  
> Any Radar → Archived: owner stops using that saved configuration.

Reactivation evaluates new demand from that moment; it does not release a backlog of stale alerts.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [Users](users.md)
- [Invariants](invariants.md)
- [Requests](../workflows/requests.md)
- [Offers](../workflows/offers.md)
- [Whatsapp contact](../workflows/whatsapp-contact.md)
- [Reports and moderation](../trust/reports-and-moderation.md)
