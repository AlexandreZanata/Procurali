---
document_id: DOC-23
status: recommended-business-baseline
scope: mixed
source_sections: ["23"]
last_updated: 2026-10-07
---

# 23. Business acceptance criteria

These scenarios define observable business behavior. They are specifications, not implementation tests or code. “Eligible” includes the ownership, lifecycle, phone-control, policy, block, locality, and allowance checks described above.

### AC-01 — Public understanding before registration

**Given** an anonymous visitor opens an eligible shared request, **when** they read it, **then** the item, budget, condition, approximate locality, and offer action are understandable, **and** no phone or received offer is disclosed.

### AC-02 — Minimal account creation

**Given** a new person supplies valid name, phone, city, and approximate region, **when** they confirm phone control and accept the rules, **then** one eligible account is created, **and** no tax identifier, photograph, full address, or separate seller account is required.

### AC-03 — Duplicate phone

**Given** a number already belongs to a non-deleted account, **when** registration or phone change tries to assign it elsewhere, **then** the assignment is refused, **and** recovery/review is offered without revealing another owner's history.

### AC-04 — Successful publication

**Given** an eligible buyer and a complete allowed request, **when** the buyer publishes within the allowances, **then** the request becomes active for seven days, **and** discovery and sharing become available, **and** one publication fact is recorded.

### AC-05 — Invalid required field

**Given** a draft lacks a city, category, item, accepted condition, or positive budget, **when** publication is attempted, **then** publication is refused with the missing/invalid requirement, **and** no active request or successful publication event is created.

### AC-06 — Open-request allowance

**Given** an account already has three open requests, **when** another publication is attempted, **then** it is refused with the applicable limit, **and** completed, cancelled, and expired requests do not count as open.

### AC-07 — Exact duplicate demand

**Given** the buyer already has an identical open need, **when** another identical request is published, **then** it is refused, **and** editing or renewing the existing request is suggested.

### AC-08 — Nonmaterial correction

**Given** an active request with live offers, **when** its author corrects a spelling error without changing requirements, **then** offers remain eligible, **and** deadlines and discovery order do not reset.

### AC-09 — Material request change

**Given** an active request with live offers, **when** the buyer confirms a valid material change, **then** the requirement revision is preserved, **and** prior live offers are invalidated, **and** sellers can explicitly resubmit matching terms within their existing slots.

### AC-10 — Automatic expiry

**Given** a request reaches its deadline, **when** any seller submission or buyer contact is attempted at or after that deadline, **then** it is refused, **and** the request is absent from active discovery even before any expiry notice is shown.

### AC-11 — Explicit renewal

**Given** an eligible expired request, **when** its buyer confirms continued need and passes current checks, **then** a new seven-day cycle begins, **and** previous offers remain expired, **and** original publication age is retained.

### AC-12 — No silent renewal

**Given** an expired request, **when** the buyer answers “Not yet” without confirming renewal, **then** the request remains expired, **and** no new activation is counted.

### AC-13 — Deterministic local discovery

**Given** a selected city and explicit filters, **when** a seller discovers requests, **then** only eligible matching requests appear, **and** region preference and original recency determine the stated order, **and** renewal does not jump ahead of newly published demand.

### AC-14 — Valid offer

**Given** a buyer has an active request, **when** a distinct eligible seller submits matching available-item terms within budget, **then** a sent offer appears privately among that request's received offers, **and** the seller does not receive the buyer's phone.

### AC-15 — Self-offer

**Given** a request belongs to the current user, **when** they attempt to offer using their personal or professional context, **then** submission is refused and no seller slot is created.

### AC-16 — Offer compatibility

**Given** a used-only request with a BRL 600 ceiling, **when** a seller submits a new item or a price above BRL 600, **then** the incompatible offer is refused with the specific condition/budget reason.

### AC-17 — One seller slot

**Given** a seller already offered in the current cycle, **when** they withdraw or are declined and try to create another offer, **then** submission is refused, **and** withdrawal or rejection does not reset the daily allowance or cycle slot.

### AC-18 — First buyer view

**Given** a sent offer, **when** the buyer opens its details, **then** its state becomes viewed and a first-view fact is recorded, **and** repeat openings do not inflate first-view conversion.

### AC-19 — Private comparison

**Given** multiple sellers offered to a request, **when** one seller inspects their offer, **then** competing offers, their prices, and buyer phone data are inaccessible.

### AC-20 — Changed offer terms

**Given** a buyer previously viewed an offer, **when** the seller changes its valid price, **then** a new offer revision and buyer notice are recorded, **and** the buyer sees and acknowledges current terms before a new handoff, **and** previous contact snapshots remain unchanged.

### AC-21 — Buyer-initiated WhatsApp handoff

**Given** a current live offer and eligible parties, **when** the request author selects “Talk on WhatsApp,” **then** one contextual handoff is made available and one contact initiation is recorded, **and** no formal purchase acceptance or payment is required.

### AC-22 — Another buyer cannot contact

**Given** a live offer on someone else's request, **when** another user attempts a handoff, **then** it is refused regardless of their seller or professional status.

### AC-23 — Contact retries and repeats

**Given** a valid recorded initiation, **when** the same action is retried, **then** no duplicate unique contact is created; **when** a later eligible handoff is requested, **then** it is a repeat event rather than another unique-contact conversion.

### AC-24 — Known handoff failure

**Given** an eligible contact choice but no usable destination/handoff can be prepared, **when** preparation fails, **then** the failure is recorded separately, **and** contact-initiation metrics and reputation are not incremented.

### AC-25 — Suspension prevents contact

**Given** a previously live offer, **when** either party is suspended before a handoff takes effect, **then** contact is refused and the destination is not newly disclosed.

### AC-26 — Withdrawal after contact

**Given** the buyer already initiated contact, **when** the seller withdraws, **then** future handoffs are disabled, **and** historical contact remains, **and** no claim is made that a previously shared number was recalled.

### AC-27 — Multiple seller choice

**Given** several eligible offers, **when** the buyer contacts more than one seller within allowance, **then** each unique valid contact is recorded, **and** no offer is automatically accepted or reserved.

### AC-28 — Buyer-reported completion

**Given** an unresolved owned request, **when** the buyer answers “Yes, I found it,” **then** the request becomes completed, **and** live offers become unavailable, **and** completion remains a declared outcome rather than proof of a sale.

### AC-29 — Platform attribution

**Given** the buyer reports resolution, **when** they link a previously contacted offer, **then** platform attribution references that historical contact; **when** no such contact exists, **then** no seller receives platform-attributed completion credit.

### AC-30 — Cancellation differs from expiry

**Given** an active request, **when** the buyer says they no longer need it, **then** it becomes cancelled with that reason, **and** it is not classified as expired or successfully resolved.

### AC-31 — Unknown outcome

**Given** a request expires and the buyer gives no outcome, **when** metrics are derived, **then** the outcome is unknown, **and** it is not fabricated as either success or buyer cancellation.

### AC-32 — External seller registration continuity

**Given** an external visitor chooses “I have this” from a shared request, **when** they finish registration, **then** they return to that request and permitted unsent offer context, **and** current availability is rechecked before submission.

### AC-33 — Share privacy

**Given** an eligible request, **when** the buyer prepares a share message or copies its link, **then** only permitted public context appears, **and** the recorded fact is sharing intent rather than message delivery.

### AC-34 — Suspended shared link

**Given** a previously shared request is suspended or removed, **when** a stranger opens the existing link, **then** a generic unavailable result appears, **and** protected content and new offer/contact actions are absent.

### AC-35 — Blocking

**Given** relevant interaction between two users, **when** either blocks the other, **then** signed-in mutual discovery and new offers/contact stop, **and** live pair offers are invalidated, **and** history and reporting remain available.

### AC-36 — No automatic restoration after unblock

**Given** an offer was invalidated by blocking, **when** the relationship is unblocked, **then** the old offer stays unavailable, **and** future otherwise eligible activity follows normal cycle and allowance rules.

### AC-37 — Report after closure

**Given** a buyer had a relevant historical contact, **when** they report suspected fraud after closure, **then** the report can be reviewed using permitted evidence, **and** closure does not dismiss it or reopen contact.

### AC-38 — Duplicate reports and coordinated allegations

**Given** repeated allegations about one incident, **when** triage evaluates them, **then** duplicates are grouped with preserved context, **and** permanent bans cannot result solely from their raw count.

### AC-39 — Audit and reversal

**Given** an authorized moderator applies or reverses a restriction, **when** the decision takes effect, **then** actor, reason, scope, time, and prior/resulting eligibility are recorded, **and** unrelated restrictions remain effective.

### AC-40 — Account deletion

**Given** an account with unresolved requests or live offers, **when** deletion takes effect, **then** public identity and new contact disappear, **and** affected resources become unavailable, **and** only purpose-limited retained evidence remains.

### AC-41 — Locality update

**Given** a user has published resources, **when** their profile city changes, **then** the existing resource localities remain unchanged until an explicit valid resource revision.

### AC-42 — Structured reputation feedback

**Given** one recorded contact and an open feedback window, **when** the buyer provides structured feedback, **then** one observation set is recorded per pair/cycle, **and** a negative answer is not automatically a valid report or permanent penalty.

### AC-43 — New prohibited class

**Given** an approved prohibition becomes effective, **when** affected live content is identified, **then** it becomes hidden/ineligible for contact, **and** owners receive a policy explanation, **and** paid status provides no exception.

### AC-44 — Free professional access

**Given** a declared professional has no paid plan, **when** they manually discover and submit a basic valid offer within free allowances, **then** the action succeeds, **and** buyer-initiated contact is not paywalled.

### AC-45 — Radar match, post-MVP

**Given** an active entitled Radar, **when** a newly eligible request meets all explicit criteria, **then** one explainable match is generated per Radar/request cycle, **and** no automatic offer or buyer-phone disclosure occurs.

### AC-46 — Stale Radar match, post-MVP

**Given** a generated match, **when** the request closes or paid access ends before alert release, **then** no new alert is released under invalid eligibility, **and** it is not counted as delivered or opened.

### AC-47 — Subscription cancellation, post-MVP

**Given** a confirmed active paid interval, **when** the owner disables renewal, **then** paid access lasts to its agreed deadline unless separately revoked, **and** basic valid marketplace resources remain afterwards.

### AC-48 — Metric semantics

**Given** one published request receives contacts from two distinct sellers, **when** its mature cohort metrics are calculated, **then** contacts per request can be 2 while request-contact coverage is 100%, **and** neither value is labeled two completed sales.

### AC-49 — Expiry during suspension

**Given** a request is hidden by suspension, **when** its original deadline passes, **then** it becomes expired while remaining hidden, **and** later account restoration does not reactivate it.

### AC-50 — Policy thresholds change

**Given** a limit or commercial plan receives an approved new version, **when** new actions are evaluated after its effective date, **then** current rules apply prospectively, **and** historical events, agreed paid intervals, and old cohort definitions remain traceable.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [Invariants](../domain/invariants.md)
- [Permissions](../domain/permissions.md)
- [Edge cases](edge-cases.md)
- [Mvp scope](../product/mvp-scope.md)
