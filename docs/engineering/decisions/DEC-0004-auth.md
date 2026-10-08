# DEC-0004 — Authentication sessions and provider boundary

**ID:** DEC-0004 | **Date:** 2026-10-08 | **Kind:** technical (identity mechanism)
**Status:** proposed (planning recommendation; business rules unchanged)
**Scope:** mvp
**Supersedes:** nothing. **Superseded by:** nothing.

## Problem

Identity tasks need a concrete session/challenge protocol that enforces INV-04
(restricted accounts act only where explicitly allowed), INV-05 (roles never replace
ownership/state/block checks), INV-07 (phone uniqueness; no recycled-history exposure),
AC-02 (minimal account, confirmed control), AC-03 (duplicate refused with safe
recovery offer), and EC-22 (no transfer via profile edit) — without introducing a
production bypass that tests could lean on.

## Options

- **Sessions:** opaque random tokens stored as digests in cookies, versus stateful
  JWT-style claims or client-supplied identity. Only digest-stored opaque tokens keep
  revocation server-side and make theft useless without the live session row.
- **Phone proof:** provider adapter (live Twilio Verify; deterministic fake for
  test/dev, refused by production startup), versus self-asserted codes or universal
  test codes accepted anywhere. Only the adapter boundary keeps "proof" meaningful
  while allowing deterministic local tests.
- **Challenge/session lifetimes:** challenge 5 min / 5 attempts / 60 s resend gap /
  5 starts per phone/hour / 20 per IP/hour; session absolute 7 days / idle 24 hours.
  These are technical cost/brute-force controls, recorded as configurable
  recommendations — never substitutes for marketplace quotas (3 open requests,
  6 activations, 10 submissions), which count successful business facts.

## Recommendation

Adopt the protocol in [authentication.md](../authentication.md): pending accounts,
E.164 canonicalization, provider-confirmed activation, digest-stored opaque sessions
with Secure/HTTP-only/SameSite cookies and same-origin CSRF, per-request current-state
checks, atomic proof-gated phone reassignment, recovery that hides previous owners,
generic anti-enumeration responses, no role self-assignment, and a hard live-provider
gate (missing credentials block live certification only).

## Effective timing and existing resources

Applies from P04-T01 (identity schema) onward. No existing accounts; nothing to migrate.

## Impact

- Canonical paths: INV-04/INV-05/INV-07, AC-02/AC-03, EC-22.
- Later tasks: P04-T01..T10 implement and prove registration, sessions, limits,
  phone change, recovery, and eligibility guards; P11/P12 reuse current-state checks.
- Technical thresholds are reviewable configuration, not business policy; changing a
  marketplace quota remains a business decision.

## Verification and follow-through

- P00-T06 assertions: thresholds are disjoint from marketplace quotas with boundary
  vectors in `contracts/auth-cases.json`; no X-User-Id/role/predictable-token/fake-code
  path is specified anywhere; missing live credentials block only live certification.
- Next action: P04-T01..T04 implement against the fake; P18-T07 attempts live proof
  once the owner supplies credentials. Unresolved: none in this card.
