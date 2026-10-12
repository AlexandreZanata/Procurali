# Backup and restore runbook (disposable stacks)

Backups are consistent `pg_dump -Fc` artifacts with a manifest
(checksum, time, revision, schema version, row counts). Credentials
never enter manifests or logs. Backup data and keys never enter Git
or the public web. Off-host destinations do not exist yet: any
`offhost:` destination is refused, and the live off-host gate stays
blocked until P18-T07 approves a destination.

## Take a backup

```sh
# Deploy stack (no ports involved; dump travels over docker exec):
scripts/backup.sh --suffix <name> --env-file <env-file> --out /tmp/backups
# Reachable disposable database:
scripts/backup.sh --database-url <procurali_test_* URL> --out /tmp/backups
```

## Prove a restore

```sh
# Into a reachable disposable database:
scripts/restore-test.sh --database-url <other-procurali_test_* URL> \
  --backup /tmp/backups/manifest.json
# Into a deploy stack database (stack must exist; data there is replaced):
scripts/restore-test.sh --suffix <name> --env-file <env-file> \
  --backup /tmp/backups/manifest.json
```

The script fails loudly (never reports success) when the manifest or
dump is missing, the checksum mismatches (corrupt artifact), the
restore errors, or any snapshot/migration count differs.

## Notes

- `pg_restore --clean --if-exists` replays into the target; the target
  is disposable by construction (refused otherwise).
- Manifest `counts` pin `requests`, `offers`, `contacts`, and applied
  migrations; equality is exact, zeros included.
- Keep backup directories out of the repository; the scanner
  (`scripts/check-secrets.sh`) treats backup data like any other
  secret-adjacent material.
