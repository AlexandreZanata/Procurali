---
document_id: DOC-20-EVENTS
status: recommended-business-baseline
scope: mixed
source_sections: ["20.1", "20.2", "20.3", "20.4", "20.5"]
last_updated: 2026-10-07
---

# 20. Business events, metrics, and liquidity

### 20.1 Event-recording contract

Record significant successful business facts, not merely attempted buttons. Each event identifies its business fact, occurrence time, responsible actor or time/policy cause, affected conceptual resource, request cycle/revision where applicable, source context, and policy version. Preserve request/offer terms snapshots where the meaning depends on historical terms.

An action retry produces one fact, not duplicated events. Failed or refused actions belong to separate operational observations with refusal reasons; they do not inflate publications, offers, contacts, or revenue. Event records and analytical access must follow the visibility and retention boundaries in Section [15](../trust/privacy-and-security.md). Measurement does not justify collecting private WhatsApp conversation content.

### 20.2 Account and profile events

- **user_registered:** A new account is created. Supports acquisition counts; not proof of marketplace participation.
- **user_phone_verified:** Phone control is confirmed. Supports verification conversion and time to eligibility.
- **user_phone_changed:** A verified destination changes. Supports security review and destination-history accuracy; analytics need no raw number.
- **user_locality_changed:** The account's default city/region changes. Supports geographic retention without rewriting resource locations.
- **professional_status_declared / professional_status_withdrawn:** Commercial classification changes. Supports professional adoption independently of payment.
- **user_suspended / user_reactivated / user_banned / user_ban_reversed:** Account eligibility changes with an accountable action. Supports restriction volume, recurrence, and appeals.
- **user_deleted:** Marketplace identity is withdrawn. Supports attrition and immediate resource closure checks.

### 20.3 Request events

- **request_created:** A private draft is created. Supports draft-to-publication conversion.
- **request_updated:** A revision changes requirements or wording; identify materiality and revision. Supports editing friction and offer invalidation analysis.
- **request_published:** First successful activation occurs. This is the denominator for distinct-demand publication cohorts.
- **request_renewed:** A new activation cycle starts. Count separately from new demand and identify the previous cycle.
- **request_viewed:** An eligible public detail is opened. Distinguish owner, staff, signed-in third-party, and anonymous views; repeated views are not distinct people automatically.
- **request_share_initiated:** A permitted sharing or copying action occurs. It is an intent, not proof of delivery.
- **shared_request_landed:** An observable shared/direct-request landing occurs with known or unknown attribution.
- **request_expired:** A cycle ends at its deadline, including hidden suspended cycles.
- **request_completed:** Buyer reports resolution, with platform/elsewhere/unknown source and optional contact attribution.
- **request_cancelled:** Request ends without a resolution claim; include abandonment, removal, ban, or deletion reason.
- **request_removed:** Owner removes public content; separate visibility from any closure outcome.
- **request_suspended / request_restored:** Resource-specific visibility eligibility changes; account reactivation alone is insufficient.
- **request_outcome_prompted:** An eligible outcome or expiry prompt is shown. Supports response rate without claiming response.
- **request_outcome_reported:** Buyer answers yes, not yet, or no longer needed. May accompany one lifecycle event; it is not another publication or completion.
- **request_outcome_corrected:** An authorized correction changes historical classification with a reason. Supports consistent recomputation.

### 20.4 Offer, contact, and feedback events

- **offer_submitted:** A first valid seller offer is created for a request cycle/revision. Supports supply, time to first offer, and offer coverage.
- **offer_resubmitted:** A seller requalifies an invalidated offer after a material request edit. Supports recovery from changing demand; does not create another distinct seller slot.
- **offer_updated:** Live terms change and a new offer revision becomes current. Supports transparency and bait-and-switch investigation.
- **offer_viewed:** The buyer first opens that offer's details. Supports offer-view conversion; repeated openings are separate optional observations.
- **offer_rejected:** Buyer declines an offer. Supports comparison outcomes without treating decline as misconduct.
- **offer_withdrawn:** Seller withdraws or marks unavailable. Supports availability attrition and prompt-withdrawal behavior.
- **offer_expired:** Its request cycle ends. Supports offer freshness.
- **offer_invalidated:** A current revision, block, closure, ban, deletion, or policy change removes eligibility. Include reason rather than treating all invalidations as seller failures.
- **offer_suspended / offer_restored:** A scoped temporary restriction changes offer eligibility. Supports case handling.
- **contact_initiated:** First valid handoff for the unique contact combination is made available. Supports the primary liquidity/contact metric.
- **contact_handoff_repeated:** Another eligible handoff for an existing contact occurs. Supports usability but is excluded from unique-contact counts.
- **contact_handoff_failed:** A known preparation failure prevents a usable handoff. Supports reliability and is excluded from initiation conversion.
- **interaction_feedback_submitted / interaction_feedback_corrected:** A structured answer or correction is recorded. Supports evidence-based reputation with one observation set per pair/cycle.

### 20.5 Safety and commercial events

- **report_created / report_information_added / report_withdrawn:** An allegation is submitted, supplemented, or withdrawn. Supports case intake while excluding duplicates from incident counts.
- **report_review_started / report_resolved / report_reopened:** Triage or evidence review progresses. Supports review targets, upheld incident counts, and appeal corrections.
- **moderation_action_applied / moderation_action_reversed:** A scoped decision takes effect or is reversed. Supports the audit trail and corrected safety indicators.
- **user_blocked / user_unblocked:** A relationship restriction changes. Supports self-protection and interaction eligibility.
- **content_policy_changed:** An approved category or prohibition rule changes with an effective version. Supports explainable historical classification.
- **business_policy_changed:** A threshold or business policy changes prospectively. Supports evaluation of rule experiments.
- **subscription_started / subscription_renewed:** A confirmed paid interval begins. Supports paid adoption and retention, never from an unconfirmed attempt.
- **subscription_cancellation_scheduled / subscription_ended / subscription_revoked:** Renewal or entitlement status changes. Supports churn and paid-through correctness.
- **radar_created / radar_updated / radar_paused / radar_activated:** Saved demand monitoring changes. Supports configuration and active usage.
- **radar_match_generated / radar_match_invalidated:** An eligible deduplicated opportunity is recognized or later becomes unavailable.
- **radar_alert_released / radar_alert_failed / radar_alert_opened:** An alert is made available, fails, or is opened. External delivery is unknown unless supported by explicit evidence; release is not assumed reading.
- **temporary_radar_started / temporary_radar_ended:** A future seven-day entitlement begins or ends. Supports pass use independently of monthly subscription totals.

Commercial events are planned for their later release. The free MVP requires the core request, offer, contact, outcome, account, share, safety, and policy events only.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [Metrics](metrics.md)
- [Liquidity](liquidity.md)
- [Whatsapp contact](../workflows/whatsapp-contact.md)
- [Reports and moderation](../trust/reports-and-moderation.md)
