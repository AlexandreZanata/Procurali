# DEC-0005 — Optional Redis acceleration deferred for lack of a measured use case

**ID:** DEC-0005 | **Date:** 2026-10-13 | **Kind:** technical (optional acceleration)
**Status:** proposed (planning recommendation; re-entry needs measured evidence or explicit owner instruction)
**Scope:** post-mvp, optional (P20 Redis phase; the mandatory baseline is unaffected)
**Supersedes:** nothing. **Superseded by:** nothing.

## Problem

The baseline permits an optional Redis acceleration phase only after
mandatory correctness is green and a measured, bounded, non-sensitive use
case selects it (INV-09, INV-28, INV-31: no cache may decide contact
disclosure, ownership, lifecycle, quotas, or expired visibility). The
bounded load run gives the first real numbers; this record decides
whether any cache use case meets the bar.

## Baseline evidence (P19-T02, disposable host, real TCP/DB)

101 calls, p50 1.05s / p95 9.09s / p99 11.24s; zero infrastructure
failures; exact business counts. Latency is dominated by authenticated
round-trips: session lookup is a documented linear Argon2 scan, so
parallel logins queue behind hashing. No query, resource, or pool
pressure attributable to cacheable data was observed.

## Options considered and rejected

1. **Session/auth lookups.** The dominant cost — but sessions are auth
   decisions, explicitly outside cache authority. Rejected.
2. **Catalog reads (cities/categories/regions).** Tiny indexed tables,
   seeded once; no measured pain in the mix. No evidence, rejected.
3. **Discovery/listing queries.** Not stressed by the measured mix; no
   baseline slowness observed. Rejected without measurements.
4. **Staff metric aggregates.** Rarely computed over small data; no
   evidence. Rejected.
5. **Request/offer/contact bodies.** Ownership, lifecycle, and visibility
   must be checked against the authoritative database on every read
   (INV-05/09/28/31); caching them adds invalidation complexity on every
   lifecycle event with no measured need. Rejected.

## Recommendation

**Defer the optional P20 Redis phase.** No bounded use case satisfies
all three gates (actual baseline evidence, non-sensitive scope, never
cache-authoritative). No implementation commit follows from this record;
P20-T02 and later stay unstarted.

## Re-entry criteria

Revisit only on (a) measured p95 on a real host attributable to a
cacheable, non-sensitive, non-authoritative read with a defined
key/TTL/invalidation/fallback, or (b) explicit owner instruction. A
revisit re-runs the three gates above; deferral hides no mandatory test
(the full tier registry is green without Redis).

## Impact

No canonical business path changes; no invariant, transition,
permission, event, or metric changes. `docs/engineering/performance.md`
records the deferral alongside the numbers.

## Verification and follow-through

- Plan validator: structurally clean (only expected plan-time vs
  execution-state deviations); public-document consistency holds.
- Next action: none unless the re-entry criteria are met.
