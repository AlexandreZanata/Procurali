---
document_id: DOC-05
status: recommended-business-baseline
scope: mixed
source_sections: ["5.1\u20135.16"]
last_updated: 2026-10-07
---

# 5. Conceptual domain entities and capabilities

These are business entities and value concepts, not database tables. “Identifier” means a stable conceptual identity. Business state and moderation visibility may coexist; Section [6](state-transitions.md) defines how they interact.

### 5.1 User

- **Responsibility:** Own identity, account eligibility, contact destination, and marketplace resources.
- **Attributes:** Identifier, display name, current verified phone, verification time, default locality, account state, registration time, last meaningful activity, professional classification, accepted policy version.
- **States:** Pending verification, active, suspended, banned, deleted.
- **Relations:** Authors requests and offers; initiates contacts as a buyer; creates reports; owns blocks, a professional profile, and commercial access when applicable.
- **Actions and rules:** Register, confirm phone, change profile or phone, change default locality, declare professional use, recover access, request deletion. Only the owner or explicitly authorized staff may perform relevant changes.
- **Invariants:** One current phone per account; one account per assigned current phone; marketplace actions require an eligible verified owner.

### 5.2 Purchase Request

- **Responsibility:** Express one buyer's current purchase need and govern the eligibility of its offers.
- **Attributes:** Identifier, author, title, category, maximum budget and currency, accepted condition, city, approximate region, optional notes, revision, cycle number, original publication time, current cycle start and deadline, lifecycle state, visibility, closure reason.
- **States:** Draft, active, completed, expired, cancelled, suspended; owner removal is a visibility condition.
- **Relations:** One author, one category and locality, many historical revisions, activation cycles, offers, contacts, shares, and at most one current final outcome.
- **Actions and rules:** Create, edit, publish, renew, complete, cancel, remove, suspend, restore. Material edits invalidate live offers before the new revision is offerable.
- **Invariants:** Exactly one author; one coherent physical-item need; active status is time-bounded; terminal completion and cancellation cannot be renewed.

### 5.3 Request Revision and Activation Cycle

- **Responsibility:** Preserve what sellers responded to and distinguish renewed intent from the initial request.
- **Attributes:** Request identity, revision or cycle number, effective time, actor, material-change reason, requirement snapshot; a cycle also has its start and deadline.
- **States:** A revision is current or superseded; a cycle is current or ended.
- **Relations:** Belong to exactly one request; offers name their target cycle and revision.
- **Actions and rules:** Start a cycle, record a revision, supersede requirements, end a cycle. Renewal creates a new cycle without creating a new request or erasing original age.
- **Invariants:** Numbers increase; history is not overwritten; renewal does not revive offers from an earlier cycle.

### 5.4 Offer

- **Responsibility:** Describe a seller's currently available item for one request cycle and revision.
- **Attributes:** Identifier, request, target cycle and revision, seller, item description or model, item price and currency, condition, optional notes, seller locality snapshot, offer revision, timestamps, state, moderation visibility, terminal reason.
- **States:** Sent, viewed, contacted, withdrawn, rejected, expired, invalidated, suspended.
- **Relations:** Exactly one seller and request; many revisions and handoffs; zero or more reports.
- **Actions and rules:** Submit, view, edit, decline, withdraw, mark unavailable, suspend, restore. A materially invalidated offer may be explicitly resubmitted for the latest request revision within its existing seller/request-cycle slot. Withdrawn or rejected offers cannot be resubmitted in the same cycle.
- **Invariants:** Seller is not the buyer; price is within budget; condition and locality match; one offer slot per seller per cycle; no contact after a terminal state or safety restriction.

### 5.5 Contact

- **Responsibility:** Record the buyer's intentional attempt to start a WhatsApp conversation and the context shown at that moment.
- **Attributes:** Identifier, buyer, seller, request, cycle, offer, request and offer revision snapshots, initiation time, handoff attempt identity, current destination reference at initiation, repeat indicator, entry source.
- **States:** Recorded initiation; delivery or conversation status is unknown in the MVP.
- **Relations:** One valid offer at initiation; possible outcome attribution and structured feedback.
- **Actions and rules:** Start a valid handoff or record a repeat handoff. Failure known before providing a handoff is recorded separately and is not a contact. Later withdrawal or moderation does not erase the historical initiation.
- **Invariants:** Initiator is the request author; parties differ; historical price and item terms do not change after an offer edit; retries of the same action do not create multiple facts.

### 5.6 Request Outcome

- **Responsibility:** Separate buyer-reported resolution from abandonment and missing information.
- **Attributes:** Request, buyer, answer, submission time, optional linked contacted offer, resolution source, correction reference and reason where applicable.
- **States:** Unresolved response, resolved final outcome, no-longer-needed final outcome.
- **Relations:** Belongs to a request; may reference a contact for attribution.
- **Actions and rules:** Record “Yes,” “Not yet,” or “I no longer need it.” A resolved answer asks, optionally, whether it was through a platform offer or elsewhere. Staff can correct an erroneous historical declaration with a recorded reason; correction does not reopen the request.
- **Invariants:** Only the buyer reports the need's outcome; a completed request is not proof of a platform sale; unknown outcomes are not treated as failures or successes.

### 5.7 Block

- **Responsibility:** Prevent future interactions between users.
- **Attributes:** Blocking user, blocked user, creation time, active status, removal time.
- **States:** Active, removed.
- **Relations:** Two distinct users and their marketplace interactions.
- **Actions and rules:** Block or unblock; either direction blocks new offers, discovery between the pair, and contact. Unblocking does not restore previously invalidated offers.
- **Invariants:** No self-block; account deletion does not expose the relationship; staff may inspect it only for a relevant safety purpose.

### 5.8 Report

- **Responsibility:** Capture an allegation and its disposition without assuming guilt.
- **Attributes:** Identifier, reporter, target type and identity, reason, optional bounded details, relevant historical context, severity, created time, review state, decision, linked case and moderator.
- **States:** Open, under review, resolved valid, resolved invalid, duplicate, withdrawn.
- **Relations:** Targets a user, request, offer, or a contact through its participants; may join a moderation case and actions.
- **Actions and rules:** Submit, add relevant information, triage, review, withdraw, decide, reopen for new evidence. Withdrawal does not force staff to stop reviewing a serious allegation.
- **Invariants:** Reporter cannot report their own content; repeated reports about the same incident add information instead of multiplying complaint weight; reporter identity is not disclosed to the target.

### 5.9 Moderation Case and Moderation Action

- **Responsibility:** Group related evidence and document proportionate decisions.
- **Attributes:** Case identity, affected parties/resources, evidence references, severity, reviewer; action identity, actor, action type, reason, scope, effective time, duration, previous and resulting eligibility, policy version, reversal reference.
- **States:** Cases are open, investigating, or closed. Actions are effective, elapsed, or reversed; history persists.
- **Relations:** Reports, users, requests, offers, policy categories, and appeals.
- **Actions and rules:** Warn, hide content, suspend, ban, restore, reject an allegation, or reverse a prior decision. No silent administrative changes.
- **Invariants:** A restriction requires an accountable actor and reason; restoration cannot override an unrelated restriction; elapsed account suspension does not revive stale content.

### 5.10 Category and Prohibited-Content Policy

- **Responsibility:** Define eligible product classes and maintain an extensible platform restriction policy.
- **Attributes:** Stable category identity, English name, status, scope definition, policy version, effective date, restriction reason, handling of existing content.
- **States:** Allowed, unavailable for new use, prohibited. A prohibited item class may apply across multiple allowed categories.
- **Relations:** Requests, offers, moderation decisions, and future Radar filters.
- **Actions and rules:** Enable, retire, prohibit, or reinstate under documented policy. A newly prohibited class hides affected active content and disables contact immediately after classification.
- **Invariants:** Reclassification never silently legitimizes prohibited content; policy changes preserve historical category meaning for metrics.

### 5.11 Location

- **Responsibility:** Describe locality without collecting a meeting address.
- **Attributes:** Stable city identity, display name, neighborhood or approximate region, supported-region labels, optional explicitly selected additional cities for future use.
- **States:** A city is enabled or unavailable for new publication; a region may be current or renamed.
- **Relations:** User defaults, request-locality snapshots, seller offer snapshots, Radar scope.
- **Actions and rules:** Select a city and approximate region; update defaults; deliberately choose another discovery city. A profile move does not change existing requests or offers.
- **Invariants:** Every published request has one city and approximate region; automatic cross-city expansion is absent from the MVP.

### 5.12 Professional Profile

- **Responsibility:** Identify declared commercial activity transparently.
- **Attributes:** User, public business/display name, business type, declared operating categories, approximate locality, declaration date, optional future verification claims.
- **States:** Declared active, withdrawn; account restrictions override visibility.
- **Relations:** One user; separate from subscriptions and reputation.
- **Actions and rules:** Declare, update, withdraw commercial classification. Repeated commercial use after withdrawal may require review.
- **Invariants:** Payment is not professional verification; no second account is required; optional business details do not expose a private address or personal phone.

### 5.13 Plan, Commercial Entitlement, and Subscription — Post-MVP

- **Responsibility:** Define purchasable benefits and their effective access period independently of core marketplace rights.
- **Attributes:** Plan/version, benefit definitions, price and currency, commercial interval, effective dates; entitlement owner, benefit and access interval; subscription owner, plan version, agreed terms, current paid interval, renewal preference, state.
- **States:** Plans are available or retired. Subscriptions are pending, active, cancellation scheduled, ended, or revoked. Entitlements are not yet effective, effective, elapsed, or revoked.
- **Relations:** User, professional profile when applicable, Radar settings, commercial purchase record.
- **Actions and rules:** Start after confirmed commercial purchase, schedule cancellation, renew after confirmation, end, or revoke with reason. A failed renewal grants no new paid interval; current already-paid access lasts to its deadline unless revoked for a documented reason.
- **Invariants:** A new price does not rewrite an existing paid interval; cancelled renewal does not remove paid-through access; safety restrictions always win over paid access.

### 5.14 Radar and Radar Match — Post-MVP

- **Responsibility:** Turn saved seller preferences into explainable opportunities.
- **Attributes:** Radar owner, selected categories, explicit city/regions, inclusive budget bounds, accepted item conditions, enabled state, criteria revision; match identity, Radar, request cycle, matched revision, reason, generated time, alert state.
- **States:** Radar is active, user-paused, entitlement-paused, restricted, or archived. A match is generated, alerted, opened, or invalidated; alert preparation can be pending or failed without implying delivery.
- **Relations:** User, entitlement, request cycle, notification preference.
- **Actions and rules:** Save filters, enable, pause, evaluate new eligible demand, alert, open a request. No offer is sent automatically.
- **Invariants:** One initial match per Radar/request-cycle pair; no private phone in alerts; subscriptions do not authorize offers or contact otherwise prohibited.

### 5.15 Share Attribution

- **Responsibility:** Measure share-driven acquisition without claiming delivery or reading private conversations.
- **Attributes:** Request/cycle, sharer, channel, intent time, privacy-safe source marker, attributable landing time, optional later registration and offer references.
- **States:** Share intent recorded; landing observed; subsequent attributed conversion observed.
- **Relations:** One request, optional user and acquisition events.
- **Actions and rules:** Prepare a share message, copy a link, record a supported landing. A sharing action is not proof a message was sent or read.
- **Invariants:** No group name, recipient list, conversation content, phone, or exact address is collected for attribution.

### 5.16 Reputation Observation and Business Policy

- **Responsibility:** Preserve structured feedback and explain current operating rules.
- **Attributes:** Observation author, contacted party, contact, question, answer, time, review status; policy identity/version, effective date, named thresholds and rationale.
- **States:** Feedback is submitted, disputed, reviewed, or excluded. Policies are proposed, effective, or superseded.
- **Relations:** Contacts, user reputation summaries, moderation cases, actions evaluated under a policy version.
- **Actions and rules:** Submit one feedback set per buyer/seller/request cycle, correct within the feedback window, dispute, review, derive summaries; approve prospective policy changes.
- **Invariants:** Repeated contact cannot create repeated reputation credit; an unreviewed negative answer is not a validated report.



Domain operations spanning entities are specified in [Domain capabilities](capabilities.md).

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [Glossary](../product/glossary.md)
- [State transitions](state-transitions.md)
- [Invariants](invariants.md)
- [Permissions](permissions.md)
- [Capabilities](capabilities.md)
