# Deployment runbook (disposable stacks)

`scripts/deploy.sh` deploys one pinned artifact to one disposable
single-host stack. Only the `disposable` target exists; anything else
(including any staging or production name) fails loudly until P18-T07
configures authorized inputs.

## Prerequisites

- Docker with Compose v2, `cargo sqlx` 0.9 (`cargo install sqlx-cli --locked` if missing).
- `infra/env.deploy`: copy `infra/env.deploy.example` and fill every value.
  The file is git-ignored and never committed.
- The artifact image present locally (`scripts/build-artifact.sh`).

## Deploy

```sh
scripts/deploy.sh --target disposable --suffix <name> --revision <sha> \
  --image procurali-app:<tag> --port <loopback-port>
```

Order inside the script (each step gates the next):

1. Refuse non-disposable targets, bad suffixes, missing image, and
   missing migrations dir.
2. Start only the `db` service; wait for `pg_isready` health.
3. Run `cargo sqlx migrate run` as a separate controlled job. Any
   migration failure aborts before services start; readiness is never
   reported and the error names the failing migration.
4. Poll `_sqlx_migrations` until the applied count equals the
   migration files; services wait for the compatible schema. The served
   application refuses to start against a different schema, so a
   mismatched database can never serve traffic.
5. Start `app` (pinned tag, never `latest`) and `proxy`; assert the
   running app container image equals the requested tag.

The application image serves traffic: its entrypoint is the serving
binary, its health check is `procurali-backend --health-check`, and
`scripts/build-artifact.sh --serve-smoke` proves that behavior against a
disposable database. A production deployment still needs the live phone
verification transport and the authorized staging inputs before it can
start; the deterministic fake provider is refused outside disposable
stacks by design.

`--dry-run` prints this plan and changes nothing.

## Rollback

Rollback redeploys a previous artifact tag through the same command
(`rollback` action). Applied migrations are never rewritten and
volumes are never erased — there is no code path here that does
either. See `rollback.md`.
