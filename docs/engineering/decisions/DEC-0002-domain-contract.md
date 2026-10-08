# DEC-0002 — Money, time, text, and refusal representation contract

**ID:** DEC-0002 | **Date:** 2026-10-08 | **Kind:** technical (domain representation)
**Status:** proposed (planning recommendation; business rules unchanged)
**Scope:** mvp
**Supersedes:** nothing. **Superseded by:** nothing.

## Problem

Implementation agents need exact parsing, comparison, and refusal behavior before writing
money/time/text code. Ambiguity here produces floating-point rounding, silent truncation,
timezone-dependent deadlines, or inconsistent length enforcement between Rust and
TypeScript. Canonical behavior: INV-08 (complete valid fields, allowed category, enabled
city, future deadline), INV-10 (positive BRL, at most two decimals), INV-19 (offer price
positive, request currency, at most current maximum budget), AC-05 (invalid publication
refused with the requirement, no event), AC-10 (deadline exclusive; absent from active
discovery even before the expiry notice).

## Options

- **Money:** exact decimal strings on the wire + integer minor units internally, versus
  binary floating point. Only the first preserves INV-10/INV-19 exactly.
- **Time:** exclusive UTC-instants compared at the guarded mutation, versus inclusive or
  local-midnight deadlines. The baseline mandates exclusive rolling-period deadlines
  (principles §2.3: "at or after the deadline, the request is expired for every action").
- **Text:** Unicode scalar-value counting in both languages, versus UTF-8 bytes, UTF-16
  units, or grapheme clusters. Scalar values are the only count expressible identically
  in Rust (`str::chars().count()`) and TypeScript (`[...s].length`); `docs/` states no
  counting rule, so this is recorded as a technical recommendation, not a business change.

## Recommendation

1. API money matches `^\d+(\.\d{1,2})?$` (BRL, launch currency). Internally and in
   persisted values it is integer minor units (cents). Zero, negatives, exponents,
   `NaN`, missing integer digits, whitespace, grouping separators, and currency symbols
   are rejected with stable codes; overprecision (`600.001`) is rejected, never rounded.
   `600.00` maps to exactly `60000`. No universal maximum is introduced: an operational
   ceiling requires a category-specific rationale (principles §2.3).
2. Instants serialize as RFC3339 UTC with explicit offset. Deadlines are exclusive and
   evaluated from the effective time at the guarded mutation, not page-open or arrival
   time. A seven-day cycle is seven consecutive 24-hour periods from publication/renewal;
   rolling windows use actual elapsed time, never midnight resets. Equal-to is denied.
3. Text limits follow the canonical bounds (titles/descriptions 120, notes 500, display
   names 80, report details 1000), counted in Unicode scalar values in both languages.
   Shorter valid text stays allowed; meaningfulness remains a moderation judgment.
4. Refusals use the stable `code` registry in `contracts/domain-vectors.json`
   (`missing_field`, `overprecision`, `expired`, `quota_exceeded`, ...). Codes are never
   renamed without a contract revision; messages stay human-readable and never contain
   secrets or phone numbers.
5. Vectors in `contracts/domain-vectors.json` are the independent expected values for
   P03 parser tests. A conflict between a vector and `docs/` resolves in favor of `docs/`.

## Effective timing and existing resources

Applies from P03-T01 (money) and P03-T02/T03 (time/text) onward. No existing data or
implementation exists; nothing to migrate. Changing canonical limits later is a business
decision with prospective effect (principles §2.2).

## Impact

- Canonical paths: principles §2.2/§2.3 (values), INV-08/INV-10/INV-19, AC-05/AC-10.
- Later tasks: P03-T01..T04 implement and prove these vectors; P05-T04/P07-T02 enforce
  them at publication/submission.
- No business rule is inferred from an implementation helper: scalar counting and the
  strict money grammar are representation choices under existing rules.

## Verification and follow-through

- P00-T04 assertions: `600.00` -> `60000` verified in the vector table above;
  `600.001` rejected without rounding; deadline equal-to denied with before/after
  vectors; Unicode vectors carry independently written scalar counts (verified with
  actual codepoint counts on 2026-10-08).
- Next action: P03-T01..T04 turn these vectors into failing-then-passing parser tests.
  Unresolved: none in this card; a future product decision may refine text counting,
  recorded separately if ever requested.
