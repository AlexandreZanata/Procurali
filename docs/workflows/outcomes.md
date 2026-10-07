---
document_id: DOC-12
status: recommended-business-baseline
scope: mvp
source_sections: ["12"]
last_updated: 2026-10-07
---

# 12. Outcome and completion workflow

### 12.1 Prompt timing

Show one outcome prompt 24 hours after the first valid contact in a request cycle, provided the request is still unresolved. If no response is given, do not repeat it after every contact. At the cycle's expiry, show the renewal/outcome prompt. Combine simultaneous prompts so the buyer receives one coherent question.

The buyer can report an outcome at any time while the request is active, expired, or suspended. Prompt absence never prevents manual closure. No prompt is required for a cancelled or completed request.

### 12.2 Answers and effects

- **“Yes, I found it.”** The request becomes completed, new offers/contact stop, and remaining live offers are invalidated. Optional source: “Through a Procuro Aí offer,” “Elsewhere,” or “Prefer not to say.” If through the platform, the buyer may link a previously contacted offer; no contacted offer means platform attribution is unavailable.
- **“Not yet.”** If active, record an unresolved response and retain the current deadline. If expired, explain that renewal is necessary; renew only after explicit confirmation and ordinary checks. If suspended, retain the restriction. This answer never silently extends time.
- **“I no longer need it.”** The request becomes cancelled for buyer abandonment, removes discovery eligibility, and invalidates live offers. It is not counted as successful resolution.

The buyer can complete a request even if it never received an offer, because the need may have been solved elsewhere. Completion without platform attribution does not give a seller transaction credit. Not responding produces an unknown outcome, not a presumed cancellation or completion.

### 12.3 Ideal complete journey

1. A person opens the product and understands the local demand model.
2. They create or recover a minimal verified account.
3. They publish one valid local need and see its seven-day period.
4. They share the direct request link.
5. Local sellers discover it through the link or local discovery.
6. Eligible sellers submit matching item offers.
7. The buyer receives notices and compares private offers.
8. The buyer chooses a seller and initiates WhatsApp contact.
9. Negotiation happens in WhatsApp, independently of the platform.
10. The buyer reports resolution and optionally attributes it to a contacted offer.
11. The request closes; relevant metrics and structured reputation observations update without claiming a verified sale.

### 12.4 Alternate journeys

- **No offers:** The request remains available until its deadline; the buyer can share, materially revise, or close it. At expiry, offer explicit renewal; never fabricate seller activity.
- **Buyer gives up:** Cancellation immediately stops new interaction and preserves the reason separately from expiry.
- **No outcome response:** Expiry makes the request inactive with an unknown resolution. Do not leave it active indefinitely.
- **Invalid offer:** Buyer declines it or reports it. Decline alone does not penalize the seller; reviewed deception may do so.
- **Seller reported:** A report opens review; contact eligibility changes only under an explicit safety restriction or other existing rule.
- **Buyer renews:** A new cycle opens, old offers remain unavailable, and sellers may respond afresh.

## Related documents

- [Documentation index](../README.md)
- [Agent maintenance workflow](../maintenance/agent-workflow.md)
- [State transitions](../domain/state-transitions.md)
- [Whatsapp contact](whatsapp-contact.md)
- [Reputation](../trust/reputation.md)
- [Metrics](../measurement/metrics.md)
