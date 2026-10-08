# API contract notes (proposed)

**Status:** Proposed. The inventory lives in `contracts/openapi.yaml`; the refusal
matrix in `contracts/authorization-cases.json`. Business meaning stays in `docs/`.

## Conventions

- Base path `/api/v1`. JSON request/response bodies. Money is an exact decimal string;
  instants are RFC3339 UTC; errors follow the stable `code` registry
  (`contracts/domain-vectors.json`).
- Unsafe operations use POST/PATCH/DELETE with same-origin CSRF validation and the
  session cookie. Safe public reads use GET and never create contacts, offers,
  events, or quota consumption.
- Cursor pagination for collections: `?cursor=<opaque>&limit=<n>` (default 20, max 50);
  responses carry `items` plus an opaque `next_cursor` or null. No offset paging.
- Mutations accept an `Idempotency-Key` header scoped per DEC-0003; same key with a
  changed body is refused with `idempotency_key_reuse`.
- Resource cycle/revision fields are explicit: requests expose `cycle` and
  `revision`; offers expose `request_revision`; submissions send the revision they
  were composed against (EC-34).

## DTO discipline (allowlist summary)

| Audience | Sees | Never sees |
|---|---|---|
| Public (anonymous/visitor) | Request id, title, budget string, condition, category, city/region label, cycle dates, share metadata | Phone, exact address, received offers, prices of others, reporter identities |
| Owner (buyer of the request) | Above plus own draft fields, own received offers with seller display names, notices, allowed actions | Other buyers' data, seller phones before handoff, report-reporter identities |
| Participant (seller of an offer) | Own offer terms/history, current request requirements | Competing offers/prices, buyer phone before author-initiated handoff |
| Staff (moderator/admin, granted role) | Report queues, evidence, restriction controls | Bulk phone export; no unrestricted user-phone lookup (INV-29) |

Contact handoff is a separate `POST /contacts` response with `Cache-Control: no-store`,
never embedded in offer DTOs or public pages. A handoff records a measurable
initiation, not proof of WhatsApp delivery.

## Ownership and state guards (summary)

Request writes: author only, terminal states immutable. Offer writes: seller owns
exactly one slot per request cycle (INV-02); self-offers refused. Contact: only the
current request author on a live offer with eligible parties (INV-27/INV-28); anyone
else refused regardless of role (AC-22, INV-05). Outcomes/notices: owner or intended
recipient only. Staff: granted roles only, scoped actions, audited; paid status never
overrides a restriction (AC-43/AC-44).

## Deferred surface

No Radar, subscription, pass, payment, internal-chat, or mobile-specific routes exist
in the mandatory MVP (AC-45..AC-47 explicitly deferred). Professional declaration is
a free profile attribute, not a paywall (AC-44).
