# Local operation (development and disposable test stacks)

**Status:** Live runbook for the current phase. Commands below are implemented in
`scripts/infra.sh` with guards in `scripts/lib/test-environment.sh`.

## Identities

| Stack | Project (`-p`) | Container | Network | Volume | Port (loopback) |
|---|---|---|---|---|---|
| Development | `procurali-dev` | `procurali-dev-db` | `procurali-dev` | `procurali-dev-pgdata` | 5432 |
| Test `<suffix>` | `procurali-test-<suffix>` | `procurali-test-db-<suffix>` | `procurali-test-net-<suffix>` | `procurali-test-pgdata-<suffix>` | 5433 (or `--port`) |

Test identities always derive from an explicit `--suffix`; the default Compose
project is never used. Development is a single project; there is no destructive
dev/staging/production command in the helpers.

## Commands

- `scripts/infra.sh up-dev [--port PORT]`: starts the development database on
  loopback and waits for health (`service_healthy`) plus a bounded `pg_isready`
  probe. Refuses non-development `PROCURALI_ENV`.
- `scripts/infra.sh up-test --suffix SUFFIX [--port PORT]`: starts one isolated
  test project, waits for readiness, and requires an explicit disposable
  `TEST_DATABASE_URL` (postgres scheme, loopback host, `procurali_test_*`
  database, matching service user/port, different from `DATABASE_URL`).
  Refuses unknown environments, non-disposable names, non-loopback hosts,
  mismatched service identity, and environment-like suffixes
  (`dev`, `prod`, `staging`, ...).
- `scripts/infra.sh down-test --suffix SUFFIX`: tears down exactly that test
  project (`down -v`). The suffix is required and environment-like names are
  refused, so the development project can never match.

## Failure messages

Readiness failures name only `host:port/database`. Usernames, passwords, and full
URLs never appear in output, in any environment.

## Workstation notes

If host ports 5432/5433 are occupied by unrelated local projects, pass `--port`
with a free loopback port (e.g. `--port 55432`); committed defaults stay standard.
`TEST_DATABASE_URL` must use the same port.

## Running the served application locally

The backend binary serves the real application (one composed router over every
business boundary). It needs validated settings, a reachable database, and a
migrated schema; it never applies migrations itself.

```sh
cd backend
cargo run --bin procurali-backend
```

Required settings (see `infra/env.example` for the full list):

- `DATABASE_URL`: the application database.
- `PHONE_LOOKUP_KEY`, `PHONE_ENCRYPTION_KEY`: stable phone identity keys,
  required with no default.
- `SESSION_KEY`: required with no default.
- `PHONE_PROVIDER=fake` with `FAKE_VERIFY_CODE`: development and test only;
  production refuses the deterministic fake.
- `APP_BIND_ADDRESS` (default `127.0.0.1:8080`), `APP_ASSETS_DIR` (default
  `web/dist`).

Startup refuses, naming the reason and printing no value, when a setting is
missing, the database is unreachable, or the applied schema differs from the
tracked migrations. `procurali-backend --health-check` probes readiness once
and reports the result as its exit code; the container image uses it as its
health check.

## Later phases

Migration runners (P02-T03), per-suite fixture isolation (P02-T05), and CI
PostgreSQL (P02-T06) build on these identities and guards; this file grows with
them instead of duplicating their rules.
