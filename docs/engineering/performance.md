# Performance observations (bounded load, actual host)

**Status:** Measured observations for one disposable host, not a capacity
promise. Runner: `scripts/load-test.sh`; mix: `contracts/load-scenarios.json`.

## Observed run (2026-10-13)

- **Revision:** `f99805e` plus the P19-T02 working tree (load script, duplicate
  wiring, load tier registration). No production traffic exists.
- **Environment:** this workstation, loopback only; disposable PostgreSQL
  18.6 (pinned image) on `127.0.0.1:5439`; temporary full-API server over
  real TCP with the deterministic fake provider (deleted after the run).
- **Mix:** 4 buyers × 3 sellers, 1 demand per buyer, parallelism 8;
  registration, publication, offers, contacts, then idempotent-replay,
  expired-demand, and duplicate-intent probes. Elapsed 67s (cap 600s).

| Signal | Observed |
|---|---|
| Requests | 101 (33×200, 49×201, 16×202, 1×409, 2×422) |
| Latency | p50 1.05s, p95 9.09s, p99 11.24s |
| Final counts | requests 5 (4 active + 1 refused duplicate draft), offers 12, contacts 12 |
| Duplicates | none (12 contacts for 12 distinct pairs; replay converged) |
| Leaks | none (no destination outside handoffs; no 10+ digit runs in public payloads) |
| Quota overflow | none (inside free-plan allowances) |
| Infrastructure failures | 0 (expected refusals: 1×409 duplicate, 2×422 expired offer/contact) |

Authenticated round-trips dominate latency: session lookup is a documented
linear Argon2 scan, so 8-way parallel logins queue behind hashing. That
cost is expected on this host and is not a product defect.

## Load-found defect fixed in this card

The duplicate-intent probe initially published an exact duplicate (201):
the P05-T06 check existed and was unit-proven but never called by the
publication writer. The writer now enforces it (409 `duplicate_intent`),
locked by `identical_second_publication_refuses_as_duplicate_intent` and
the load probe itself. Renewal was left unchanged (separate follow-up);
one pre-existing renewal fixture used a second identical demand and now
uses a distinct title. No other suite regressed (full api/integration
tiers re-run).

## What this does not claim

No throughput promise beyond this host and mix; no staging/production
capacity statement; live provider, staging host, and off-host restore
remain pending owner inputs (see `release-status.md`).
