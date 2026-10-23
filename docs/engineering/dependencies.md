# Engineering dependencies (planned pins)

**Status:** Planned; no application package exists yet. Exact resolved versions are locked
in `backend/Cargo.lock` (P01-T01) and `web/package-lock.json` (P15).
**Verified:** October 8, 2026 against crates.io and the npm registry from this workstation;
the TLS dependency patch bumps of October 20, 2026 (`rustls` 0.23.46,
`rustls-webpki` 0.103.16, `webpki-roots` 1.0.10) and the `reqwest` 0.12.28
transport addition of October 23, 2026 were verified the same way.
**Scope:** Free MVP backend, reproducible infrastructure, minimal plain-TypeScript web UI.
Mobile, commercial, and optional Redis-acceleration dependencies are excluded unless marked optional.

## Rust toolchain

| Item | Pin | Source |
|---|---|---|
| Rust stable | >= 1.89 (MSRV 1.89); workstation runs 1.96.0 | rust-lang stable release train |
| `rust-toolchain.toml` | `channel = "stable"` recorded in P01-T01; no pinned nightly | project convention |

MSRV 1.89 is the oldest toolchain the dependency set below is expected to build with.
P01-T01 records the exact toolchain file; CI uses stable.

## Backend crates (planned; added to `backend/Cargo.toml` in P01-T01)

| Crate | Pinned line (verified 2026-10-08) | Purpose | License |
|---|---|---|---|
| axum | 0.8.x (latest observed 0.8.9) | HTTP routing, extractors | MIT |
| tokio | 1.x (latest observed 1.54.1), features `full` restricted to net/time/signal/macros/rt | Async runtime | MIT |
| sqlx | 0.8.x (latest observed 0.8.6; 0.9.0 exists but is not adopted until proven compatible) | Parameterized PostgreSQL access, migrations; features `runtime-tokio`, `postgres`, `uuid`, `chrono`, `migrate` | MIT OR Apache-2.0 |
| serde / serde_json | 1.x (latest observed 1.0.229) | Explicit request/response DTOs | MIT OR Apache-2.0 |
| uuid | 1.x (latest observed 1.27.0), features `v7`, `serde` | UUIDv7 identifiers | MIT OR Apache-2.0 |
| chrono | 0.4.x (latest observed 0.4.45) | UTC instants, deadline arithmetic | MIT OR Apache-2.0 |
| tower-http | 0.7.x (latest observed 0.7.1) | Request limits, security headers, CORS/same-origin handling | MIT |
| argon2 | 0.6.x (latest observed 0.6.0) | Opaque session-token digest storage (password-hashing grade KDF; no home-grown crypto) | MIT OR Apache-2.0 |
| thiserror | 2.x | Structured domain/HTTP error types | MIT OR Apache-2.0 |
| tracing / tracing-subscriber | 0.1.x / 0.3.x with `json`, `env-filter` | Redacted structured logs | MIT |
| config | 0.15.x | Validated environment configuration | MIT OR Apache-2.0 |
| reqwest | 0.12.28, `rustls-tls` only (no default features) | Live Twilio Verify HTTPS transport; bounded timeout, no redirects | MIT OR Apache-2.0 |

No Git development branch and no `latest` tag is used. Exact patches are resolved by Cargo
in P01-T01 and frozen in `Cargo.lock`. SQLx 0.9 is deferred until its PostgreSQL runtime
behavior is proven compatible with the pinned Tokio/Axum combination.

## Web toolchain (planned; `web/package.json` created in P15)

| Item | Pin (verified 2026-10-08) | Purpose |
|---|---|---|
| Node.js | 22 LTS minimum for builds; workstation runs v26.3.1 | TS compilation, browser-test runner |
| TypeScript | 7.x (latest observed 7.0.2), dev-only | Plain-TS compilation to JavaScript, no runtime UI framework |
| @playwright/test | 1.x (latest observed 1.64.0), dev-only | Browser tests against real disposable backend/database |

TypeScript and Playwright are development dependencies only. No runtime UI framework
(React/Vue/Svelte) is planned for the MVP.

## Databases and container images

| Image | Pin | Notes |
|---|---|---|
| `postgres` | `18.6` (official Docker Hub tag) | Authority for state, quotas, events, audit; native UUIDv7 |
| `redis` (optional, P20 only) | `8.10.2` (examined at planning) | Acceleration of non-sensitive derived data only; never authoritative |

Image digests are not invented here. P02-T01 records the digest of the actually pulled
image (`docker inspect`) alongside the tag in `infra/compose.yaml` comments.
The core MVP runs correctly without Redis; Redis absence or failure changes nothing authoritative.

## Release checks (P17-T05)

`scripts/check-release.sh` gates exact lockfiles (`cargo fetch
--locked`, `npm install --dry-run`), dependency advisories, licenses,
and production configuration refusals; `scripts/check-secrets.sh`
gates staged/tracked private paths, high-confidence secret material,
and production-bypass markers (test-only fixtures allowlisted).
`scripts/test.sh ops` stays unregistered until the infrastructure
phase owns deployment targets.

Known advisory exceptions (recorded, not hidden):
[RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071)
(rsa 0.9.10, Marvin timing sidechannel, medium severity) has no fixed
upgrade available. The release gate ignores exactly this ID and
otherwise tolerates zero unassessed advisories: anything else fails
loudly for human triage (record or repair). Revisit when an upgrade
lands.

[RUSTSEC-2026-0381](https://rustsec.org/advisories/RUSTSEC-2026-0381)
(webpki-roots bundled root certificates) is ignored for one package only:
`webpki-roots 0.26.11` is the published semver shim over the 1.0 line
("this crate version is a semver trick to webpki-roots v1"), pulled in by
`sqlx-core 0.8.6`, which pins the 0.26 line. The lockfile resolves the
shim to `webpki-roots 1.0.10`, which carries the fixed root store, so the
vulnerable roots are not in the build; `cargo audit` matches by package
version and cannot see that. The shim line itself has no patched release,
and the only way to drop it is the SQLx 0.9 upgrade that this manifest
defers until its PostgreSQL runtime behavior is proven compatible. Revisit
when that upgrade happens.

License policy: strong copyleft (GPL/AGPL, including LGPL-only)
without a permissive alternative fails the gate. The single
`MIT OR Apache-2.0 OR LGPL-2.1-or-later` entry picks a permissive
alternative and passes. Production configuration refusals (fake
provider, non-HTTPS origin, short session key, test/prod database
equality) re-run their suites inside the gate.

## Explicitly out of scope for this manifest

Native mobile SDKs, payment processors, internal-chat infrastructure, map tiles,
AI/recommendation libraries, and subscription billing are not pinned because they are
not part of the mandatory free MVP. See [DEC-0001](decisions/DEC-0001-stack.md).
