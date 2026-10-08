# Engineering architecture (planned baseline)

**Status:** Proposed implementation baseline; no application code exists yet.
**Scope:** Free MVP backend, reproducible infrastructure, minimal plain-TypeScript web UI.
Business behavior is specified in `docs/`; this file constrains implementation shape only.

## Shape

One Rust application in `backend/`, organized into module boundaries (not separate crates):

- `domain/`: pure value types and rules (BRL money, clock/deadline semantics, condition,
  text normalization, locality values). No I/O.
- `application/`: one operation per business capability (register, publish, submit offer,
  start contact, record outcome, moderate, ...). Owns eligibility checks and transaction
  boundaries. Calls persistence and provider adapters; never trusts client-supplied
  ownership, state, or version.
- `persistence/`: SQLx repositories, migrations, pool, transaction retry, idempotency store,
  event/notice stores. Parameterized queries exclusively.
- `http/`: Axum router, handlers, DTO allowlists, structured errors, session/CSRF handling,
  health/readiness. Handlers parse, authenticate, call one application operation, map one
  explicit public result.
- `operations/`: validated configuration, redacted structured logging, lifecycle/shutdown,
  bounded PostgreSQL-backed worker (expiry, outcome prompts, retention), provider clients
  (Twilio Verify live adapter behind a narrow trait; deterministic fake for tests/dev only).

PostgreSQL 18.6 is the authority for resource state, ownership, verification/session
eligibility, quotas, business events, and audit. Native UUIDv7 identifiers.

## Web

`web/` holds plain TypeScript compiled to JavaScript, semantic HTML, native CSS, and small
autonomous custom elements where they simplify reuse. No runtime UI framework.
TypeScript/build/browser-test tools are development dependencies. `contracts/openapi.yaml`
is the single API truth; generated types never invent business behavior.

## Delivery and runtime

Docker Compose reproduces local, test, and one-host staging topologies from tracked
`infra/` files; Caddy terminates HTTPS on the same origin. Database ports are internal
in deployed operation. Optional Redis (gated, post-baseline) caches only non-sensitive
derived data and never decides disclosure, ownership, lifecycle, or bans.

No native app, payment processor, internal chat, complex map, or AI recommendation
in this delivery.

## Data and identity conventions (summary)

- BRL amounts: integer minor units internally and persisted; API money is an exact
  decimal string (e.g. `"520.00"`), never binary floating point. Overprecision rejected.
- UTC instants stored explicitly; UUIDv7 is an identifier, not authorization.
- Counts/quotas derive from successful facts inside guarded transactions; removal does
  not refund historical allowance.
- Request cycle and material revision stored separately; offer snapshots and contact
  history are append-only and never mutated retroactively.
- Public DTOs are allowlists; phone, challenge, session, reporter, audit, and
  contact-destination internals never derive into public serialization.
- Phone identity is canonical E.164 via a maintained validator; destination stored
  encrypted (vetted AEAD, key outside the database) with a keyed lookup digest.
- Contact handoff is a separate no-store response, never part of public offer DTOs.
  A retry revalidates current eligibility before any replay; a handoff records a
  measurable initiation, not proof of WhatsApp delivery.

## Background work

A small worker from the same package claims PostgreSQL-backed jobs durably
(expiry notices, outcome prompts, retention). Jobs are idempotent, safe under two
workers, and recoverable after a crash. Expired resources are unavailable by
time checks even if the job is delayed.

See [dependencies](dependencies.md) for pins and [decisions](decisions/DEC-0001-stack.md)
for the stack rationale.
