# DEC-0001 — Mandatory MVP implementation stack

**ID:** DEC-0001 | **Date:** 2026-10-08 | **Kind:** technical (implementation stack)
**Status:** proposed (planning recommendation; approved only by explicit owner decision at build start)
**Scope:** mvp (backend, infrastructure, minimal web UI)
**Supersedes:** nothing. **Superseded by:** nothing.

## Problem

The business baseline (`docs/`, v1.1) specifies behavior, not technology. Implementation
needs one bounded, reproducible stack so that economical agents produce compatible code,
tests, and infrastructure without redesigning the system per task. The choice must keep
the free MVP free of paid services, keep phone/session secrets out of the database
plaintext, and keep deployment reproducible on one host.

## Options

1. **Rust (Axum/Tokio/SQLx) + PostgreSQL + plain TypeScript + Compose/Caddy (recommended).**
   One compiled package, parameterized SQL, redacted structured logs, reproducible
   local/test/production-parity containers. Smallest operational surface that still
   gives serializable transactions, durable uniqueness, and real HTTP authorization tests.
2. **TypeScript full-stack (Node + ORM).** Faster UI iteration, but ORM abstraction
   hides the transaction/Retry/D DL behavior this plan proves with real PostgreSQL,
   and invites generic repository shortcuts the architecture baseline forbids.
3. **Managed backend (BaaS).** Outsourcing identity/database trades away the
   server-owned eligibility, quota-guard, and audit requirements (INV-04, INV-15,
   INV-34) and introduces a vendor account the owner has not approved.

## Recommendation

Adopt option 1 with the pins in [dependencies.md](../dependencies.md):
Axum 0.8.x, Tokio 1.x, SQLx 0.8.x, PostgreSQL 18.6, plain TypeScript 7.x (dev-only),
Playwright 1.x (dev-only), Compose + Caddy. Rationale:

- Serializable multi-resource transactions with unique constraints and bounded retry
  map directly onto INV-14/INV-15/INV-34 and the race inventory in the test strategy.
- SQLx migrations keep schema changes reviewable as SQL files; native UUIDv7 avoids
  an ID-generation service.
- Plain TS with no runtime framework keeps the minimal UI auditable and the
  contract (`contracts/openapi.yaml`) the single source of API truth.
- Compose/Caddy reproduces local, test, and one-host staging topologies from
  tracked files; database ports stay internal in deployed operation.

Redis 8.10.2 is gated optional (P20): it may cache non-sensitive derived data after
the baseline is green and measured, and never decides disclosure, ownership,
lifecycle, or bans.

## Effective timing and existing resources

Applies from P01-T01 (first Rust package) onward. No existing implementation,
users, or data exist, so no migration or historical correction is needed.
Reversible at planning cost only: switching stacks later discards P01+ code but no
production data. SQL schema intent (documented per migration) survives a rewrite.

## Impact

- Canonical business documents: unchanged; this decision constrains implementation only.
- Affected plan areas: `backend/`, `web/`, `contracts/`, `infra/`, `scripts/`,
  `.github/workflows/` (see architecture baseline).
- Invariants/transitions/permissions/events: behavior stays server-owned; SQLx must
  use parameterized queries exclusively (INV-31 privacy surface).
- Deferred: native mobile apps, Radar sales, subscriptions, paid passes (out of scope).

## Verification and follow-through

- P00-T02 assertions: every pin has an official source (crates.io / npm / postgresql.org);
  no `latest` tag, Git branch, or invented version/digest; optional scope explicit.
  Verified 2026-10-08 via `cargo search`/`cargo info`/`npm view` (axum 0.8.9, tokio 1.53.2,
  sqlx 0.8.6, serde 1.0.229, typescript 7.0.2, @playwright/test 1.64.0) plus plan
  references (PostgreSQL 18.6, Redis 8.10.2).
- Next action: P01-T01 resolves exact patches into `backend/Cargo.lock`;
  P02-T01 records pulled image digests. Unresolved: owner approval of this
  recommendation before public launch (recorded as proposed, not approved).
