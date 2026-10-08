# Authentication and phone-verification protocol (proposed)

**Status:** Proposed technical protocol; no code exists yet. Business identity rules live
in `docs/` (INV-04/INV-05/INV-07, AC-02/AC-03, EC-22). This file fixes only mechanism
and technical thresholds, which are distinct from marketplace quotas.

## Registration and verification flow

1. Visitor supplies display name (max 80 scalar values), phone (any input format),
   city, and approximate region, and accepts the rules. The server creates a
   **pending** account: it can attempt verification and no other marketplace action
   (INV-04). No tax identifier, photograph, full address, or separate seller account
   is required or accepted (AC-02).
2. The phone is parsed to canonical E.164 with a maintained validator. Unparseable
   input is refused with `invalid_field`; no challenge is sent.
3. A verification challenge is issued through the provider adapter (live Twilio Verify
   in production; deterministic fake only in test/development configuration, which
   production startup refuses). Submitting a challenge request never counts as proof:
   only a provider-confirmed code activates the account.
4. On confirmation, the pending account becomes eligible exactly once; duplicate
   completion is idempotent under (account, operation, key, input digest).

## Sessions

- Opaque cryptographically random tokens (256-bit), persisted only as digests
  (Argon2). The token travels in an HTTP-only cookie; production sets `Secure`,
  `SameSite=Lax`, and same-origin CSRF validation for unsafe methods.
- Absolute maximum 7 days, idle maximum 24 hours (technical recommendations,
  configurable; unrelated to marketplace quotas). Every authenticated request checks
  current account state (pending/suspended/banned/deleted never act, INV-04) and
  never trusts client-supplied identity, role, or ownership (INV-05).
- No `X-User-Id` header, query-parameter identity, predictable token, universal fake
  code, default admin, or environment bypass exists in any configuration.

## Phone changes, recovery, recycled numbers

- A destination changes only after account control plus new-number confirmation; the
  reassignment is atomic and recorded in phone history. History keeps the destination
  valid at each past initiation; future handoffs use the new destination.
- A number assigned to a non-deleted account is refused elsewhere (INV-07, AC-03,
  EC-22) with a generic message plus a recovery/review offer that reveals no other
  owner's history.
- Recovery re-proves control of the destination and never exposes a recycled number's
  previous records. Recycled-number assignment to a new subscriber requires reviewed
  handling, never an automatic transfer through profile edit.
- No public or admin role self-assignment exists. Staff capabilities are granted only
  through an out-of-band operational grant verified at startup.

## Abuse controls (technical, not marketplace quotas)

Challenge window 5 minutes; 5 confirmation attempts per challenge; 60-second minimum
between challenge sends; 5 challenge starts per phone per hour; 20 per trusted IP per
hour. Rationale: SMS budgets are real money and codes are brute-forceable; these bounds
make bulk probing uneconomical while allowing legitimate retries. All values are
configuration with a documented review; production refuses to start with the fake
provider or with public test defaults.

## Anti-enumeration

Registration, challenge, login, and recovery responses are generic: they never reveal
whether a phone is registered, which account holds it, or why a challenge failed.
Timing differences between known/unknown phones are not a supported signal.

## Live-provider gate

Twilio Verify is the planned live adapter. Sending real verifications requires an
authorized account/service, credentials, budget, destination scope, and notice wording
from the owner. Missing credentials block live certification only; deterministic local
construction with the fake continues. A fake success never proves real phone control.

Boundary cases are frozen in `contracts/auth-cases.json`; the decision rationale is in
[DEC-0004](decisions/DEC-0004-auth.md).
