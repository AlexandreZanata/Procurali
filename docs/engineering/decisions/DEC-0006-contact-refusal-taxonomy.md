# DEC-0006 — Contact refusal taxonomy for a restricted seller

**ID:** DEC-0006 | **Date:** 2026-10-21 | **Kind:** technical (refusal taxonomy)
**Status:** adopted (implementation repair; business rules unchanged)
**Scope:** mvp
**Supersedes:** nothing. **Superseded by:** nothing.

## Problem

`start_contact` mapped a seller that is no longer `active` (banned,
suspended, deleted) to `ContactError::ForbiddenState`. That variant is
documented in the same module as "the request side cannot take contact
(buyer restricted, or wrong lifecycle beyond expiry)", while
`ContactError::SellerNotActive` is documented as "the seller account is
missing, deleted, or not `active`" — the exact case. The replay path
(`contact_replay`) already returned `SellerNotActive` for the same
condition, so initiation and replay disagreed, and the two tracked suites
disagreed with each other: the moderation suite expected
`SellerNotActive` when a ban wins the contact race, while the concurrency
suite pinned `ForbiddenState` for a suspended seller. One of them could
not hold, so the moderation suite failed whenever the ban won the race —
an intermittent failure of the tracked api tier.

## Options

- **Keep the mapping and relax the moderation test** to accept either
  variant. The race becomes deterministic, but the taxonomy keeps two
  documented meanings for one condition, and the refusal message on the
  wire ("offer cannot take contact") keeps naming the offer for what is
  an account restriction.
- **Map a non-active seller to `SellerNotActive`** in `start_contact`, the
  meaning the enum already documents, the replay path already uses, and the
  moderation suite already expects. The request-side refusals (withdrawn or
  terminal offer, expired or restricted demand) keep `ForbiddenState`.
- **Introduce a new variant or a new wire code** for account restrictions.
  A new wire code is a contract change the business specification does not
  ask for.

## Recommendation

Adopt the second option: one condition, one meaning, one message family.
The wire code stays `forbidden_state` (the tracked contract registry pins
the code, not the internal variant), and the seller-restriction message
becomes the one the replay path already returns.

## Impact

- Business rules unchanged: a restricted seller still cannot take contact,
  the refusal still carries no destination, and no contact fact is written.
- Canonical paths: INV-09 (eligibility), INV-28 (no restricted disclosure),
  AC-25 (a restriction refuses without fabricating a handoff).
- Affected code: `backend/src/application/start_contact.rs` (one refusal
  mapping) and the two tracked suites that pinned the two meanings
  (`api_moderation_accounts`, `concurrency_contacts`).
- The wire code `forbidden_state` and the tracked contract registry are
  untouched; only the internal variant and its message change, for the
  seller-restriction case only.
