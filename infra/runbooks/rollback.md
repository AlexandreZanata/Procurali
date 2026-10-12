# Rollback runbook (disposable stacks)

Rollback is a forward redeploy of a previous compatible artifact, not
a migration downgrade and not a volume reset.

## Procedure

```sh
# 1. Record the current state (revision, schema version, probe counts).
# 2. Redeploy the previous artifact tag through the same gate:
scripts/deploy.sh rollback --target disposable --suffix <name> \
  --revision <previous-sha> --image procurali-app:<previous-tag>
# 3. Re-check schema version and row counts; they must equal step 1.
```

## Rules

- Never `docker compose down -v`: volumes hold the only copy of the
  data in a disposable stack, and erasing them is data loss, not
  rollback.
- Never edit, reorder, or delete an applied migration file to
  "un-apply" it. Forward-only schema history is load-bearing for
  audit and retention.
- A rollback across an incompatible schema boundary is refused by the
  schema-version wait: the old services stay down and the failure is
  visible instead of serving a split-brain stack.
- Verify with row counts before and after (see P18-T03 evidence for
  the probe-table pattern); equal counts plus equal schema version
  close the rollback.
