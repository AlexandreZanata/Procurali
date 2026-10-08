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

## Later phases

Migration runners (P02-T03), per-suite fixture isolation (P02-T05), and CI
PostgreSQL (P02-T06) build on these identities and guards; this file grows with
them instead of duplicating their rules.
