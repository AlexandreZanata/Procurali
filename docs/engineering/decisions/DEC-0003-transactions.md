# DEC-0003 — Transaction, linearization, retry, and idempotency behavior

**ID:** DEC-0003 | **Date:** 2026-10-08 | **Kind:** technical (persistence concurrency)
**Status:** proposed (planning recommendation; business rules unchanged)
**Scope:** mvp
**Supersedes:** nothing. **Superseded by:** nothing.

## Problem

Concurrent buyers, sellers, workers, and staff race on the last quota slot, on expiry
boundaries, on blocks/restrictions, and on retried requests. Without a frozen protocol,
implementations duplicate events, resurrect withdrawn contact, or grant stale actions.
Canonical behavior: INV-14 (material changes preserve prior revision, retire live
offers), INV-15 (no rolling-count reset), INV-24 (offer-term history append-only,
contact snapshots intact), INV-28 (contact needs full current eligibility), INV-34 (repeat
execution duplicates nothing), INV-38 (lifting one restriction overrides nothing else),
EC-07 (effect must occur before the exclusive deadline), EC-25 (retry returns the
existing result; a later handoff counts only if still eligible), EC-34 (stale revision
rejected against current requirements), EC-36 (independent restrictions stay effective).

## Options

- **Isolation:** serializable transactions for state-changing multi-resource operations,
  versus read-committed with ad-hoc locking. Only serializable plus unique constraints
  makes last-slot races deterministic without inventing queue semantics.
- **Linearization point:** the guarded database mutation that establishes eligibility
  (effective clock read after guards, immediately at that mutation), versus
  request-arrival or page-open time. Only the mutation point satisfies EC-07 and makes
  "closure/restriction winning before the mutation prevents the action" testable.
- **Retry scope:** bounded retry of genuinely retryable errors only (serialization
  `40001`, deadlock `40P01`), versus unbounded or blind retry. Bound: initial attempt
  plus at most 3 retries; each retry starts fresh and rechecks current rules; no
  external provider call runs inside a retryable transaction. After the bound, record
  the final reason; never return phantom success.
- **Idempotency scope:** (actor, operation, key, input digest). Same key + same body
  returns the existing result (EC-25); same key + different body is refused with
  `idempotency_key_reuse`; no durable success is ever keyed by key alone.

## Recommendation

1. Every business mutation, its business event(s), affected notices, and deduplication
   record commit atomically where they are one business fact. A failure injected between
   mutation and event rolls back everything (P03-T05 proves this against real PostgreSQL).
2. Quota counters (open slots, activations, submissions, revisions) derive from
   successful facts under guarded transactions with account-scoped guards and unique
   constraints; removal never refunds historical allowance (INV-15).
3. Offer submission carries the request revision it was composed against; a revision
   mismatch is rejected with current requirements (EC-34). Material request changes
   retire live offers while preserving their history (INV-14/INV-24).
4. Contact replay revalidates full current eligibility (INV-28, INV-38) before any
   response: a prior successful contact retried after block/suspension/withdrawal/expiry
   returns a refusal with no phone destination — cached sensitive output is never
   re-served. A genuinely later eligible handoff is a new repeat event (EC-25).
5. Staff lifting one restriction changes only that restriction; every other guard is
   rechecked at the next mutation (EC-36/INV-38). Unblocking never resurrects old offers.
6. External verification (Twilio) runs outside retryable transactions with explicit
   before/after/failure states; a retryable database failure never repeats a provider
   request nor duplicates a business event.

## Effective timing and existing resources

Applies from P03-T05 (events) through P03-T06/T07 (retry/idempotency) onward. No
existing data; nothing to migrate.

## Impact

- Canonical paths: INV-14/INV-15/INV-24/INV-28/INV-34/INV-38, EC-07/EC-25/EC-34/EC-36.
- Later tasks: P03-T05..T07 implement; P05-T05/T06, P07-T03, P08-T04..T06 prove quota,
  duplicate-intent, allowance, and contact races; P06-T03/P09-T02 cover worker claims.
- Race inventory and boundary expectations live in the test strategy; this decision
  fixes only the mechanism, never the business outcome.

## Verification and follow-through

- P00-T05 assertions: serialization retry cannot duplicate events/grants (unique
  constraints + atomic commit); changed input under one key is refused; post-block
  contact replay discloses nothing. Cases frozen in `contracts/idempotency-cases.json`.
- Next action: P03-T05..T07 implement with real-PostgreSQL race tests; P08-T06 proves
  contact linearization with controlled schedules.
  Unresolved: none in this card.
