#!/usr/bin/env bash
# Restore proof against a separate disposable target (P18-T04).
# Usage:
#   scripts/restore-test.sh --suffix SUFFIX --env-file FILE --backup MANIFEST
#   scripts/restore-test.sh --database-url URL --backup MANIFEST
#   scripts/restore-test.sh --self-test
# --suffix restores into a deploy stack database via container exec (the
# stack must exist with an empty migrated database); --database-url
# restores into a directly reachable disposable database
# (`procurali_test_*` only; anything else is refused). After restore,
# migration state and business-row counts must equal the manifest
# exactly; corrupt or missing artifacts fail loudly instead of
# reporting success.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

SUFFIX=""
ENV_FILE="infra/env.deploy"
DATABASE_URL=""
MANIFEST=""
SELF_TEST=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --suffix) SUFFIX="${2:-}"; shift 2 || exit 2 ;;
    --env-file) ENV_FILE="${2:-}"; shift 2 || exit 2 ;;
    --database-url) DATABASE_URL="${2:-}"; shift 2 || exit 2 ;;
    --backup) MANIFEST="${2:-}"; shift 2 || exit 2 ;;
    --self-test) SELF_TEST=1; shift ;;
    *) echo "usage: $(basename "$0") (--suffix SUFFIX --env-file FILE | --database-url URL) --backup MANIFEST [--self-test]" >&2; exit 2 ;;
  esac
done

if [ "$SELF_TEST" -eq 1 ]; then
  echo "self-test: missing artifact fails instead of succeeding"
  if ./scripts/restore-test.sh --database-url "postgres://u@127.0.0.1:1/procurali_test_x" --backup /tmp/does-not-exist.json 2>/dev/null; then
    echo "self-test: missing artifact wrongly accepted" >&2
    exit 1
  fi
  echo "restore self-test: OK"
  exit 0
fi

if [ -z "$MANIFEST" ] || [ ! -f "$MANIFEST" ]; then
  echo "restore-test: backup manifest missing: ${MANIFEST:-<none>} (failed restore, not success)" >&2
  exit 1
fi
DUMP="$(dirname "$MANIFEST")/$(python3 -c "import json,sys; print(json.load(open('$MANIFEST'))['dump'])")"
if [ ! -s "$DUMP" ]; then
  echo "restore-test: dump artifact missing or empty: $DUMP (failed restore, not success)" >&2
  exit 1
fi
EXPECTED_SHA="$(python3 -c "import json; print(json.load(open('$MANIFEST'))['sha256'])")"
ACTUAL_SHA="$(sha256sum "$DUMP" | cut -d' ' -f1)"
if [ "$EXPECTED_SHA" != "$ACTUAL_SHA" ]; then
  echo "restore-test: checksum mismatch (corrupt artifact, refusing)" >&2
  exit 1
fi
EXPECTED_COUNTS="$(python3 -c "import json; print(' '.join(json.load(open('$MANIFEST'))['counts']))")"

restore_via_exec() {
  local container="$1" user="$2" db="$3" dump="$4"
  docker cp "$dump" "$container:/tmp/restore.dump"
  docker exec -i "$container" pg_restore -U "$user" -d "$db" --clean --if-exists /tmp/restore.dump || {
    echo "restore-test: pg_restore failed (failed restore, not success)" >&2
    return 1
  }
  docker exec "$container" rm -f /tmp/restore.dump
}

check_counts() {
  local query="$1"
  local got
  got="$(eval "$query" 2>/dev/null || echo unavailable)"
  local want
  want="$(printf '%s' "$EXPECTED_COUNTS" | tr ' ' '\n' | sort)"
  local have
  have="$(printf '%s' "$got" | tr ' ' '\n' | sort)"
  if [ "$want" != "$have" ]; then
    echo "restore-test: row counts differ. want: $want ; have: $have" >&2
    return 1
  fi
  echo "restore-test: snapshots match: $have"
}

if [ -n "$SUFFIX" ]; then
  [ -f "$ENV_FILE" ] || { echo "restore-test: env file missing: $ENV_FILE" >&2; exit 2; }
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
  DB_CONTAINER="$(docker ps --format '{{.Names}} {{.State}}' | awk '$1 == "procurali-deploy-'"$SUFFIX"'-db-1" && $2 == "running" {print $1}')"
  [ -n "$DB_CONTAINER" ] || { echo "restore-test: no running db container for suffix $SUFFIX" >&2; exit 1; }
  restore_via_exec "$DB_CONTAINER" "${POSTGRES_USER:?}" "${POSTGRES_DB:?}" "$DUMP" || exit 1
  check_counts "docker exec -i $DB_CONTAINER psql -U ${POSTGRES_USER:?} -d ${POSTGRES_DB:?} -tAc \"SELECT 'requests:' || count(*) FROM requests UNION ALL SELECT 'offers:' || count(*) FROM offers UNION ALL SELECT 'contacts:' || count(*) FROM contacts UNION ALL SELECT 'migrations:' || count(*) FROM _sqlx_migrations;\"" || exit 1
elif [ -n "$DATABASE_URL" ]; then
  case "$DATABASE_URL" in
    *@*/*procurali_test_*) ;;
    *) echo "restore-test: refusing non-disposable database URL" >&2; exit 2 ;;
  esac
  PG_IMAGE="postgres:18.6@sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336"
  DB_USER="$(printf '%s' "$DATABASE_URL" | sed -E 's|.*://([^:]+):.*|\1|')"
  DB_PASS="$(printf '%s' "$DATABASE_URL" | sed -E 's|.*://[^:]+:([^@]+)@.*|\1|')"
  DB_HOSTPORT="$(printf '%s' "$DATABASE_URL" | sed -E 's|.*@([^/]+)/.*|\1|')"
  DB_NAME="$(printf '%s' "$DATABASE_URL" | sed 's|.*/||; s|?.*||')"
  docker run --rm --network host \
    -e "PGPASSWORD=$DB_PASS" \
    "$PG_IMAGE" createdb -h "${DB_HOSTPORT%%:*}" -p "${DB_HOSTPORT##*:}" -U "$DB_USER" --maintenance-db=postgres "$DB_NAME" 2>&1 | grep -v "already exists" || true
  docker run --rm --network host \
    -v "$DUMP:/tmp/restore.dump:ro" \
    -e "PGPASSWORD=$DB_PASS" \
    "$PG_IMAGE" pg_restore --clean --if-exists -h "${DB_HOSTPORT%%:*}" -p "${DB_HOSTPORT##*:}" -U "$DB_USER" -d "$DB_NAME" /tmp/restore.dump || {
    echo "restore-test: pg_restore failed (failed restore, not success)" >&2
    exit 1
  }
  check_counts "docker run --rm --network host -e \"PGPASSWORD=$DB_PASS\" $PG_IMAGE psql -h ${DB_HOSTPORT%%:*} -p ${DB_HOSTPORT##*:} -U $DB_USER -d $DB_NAME -tAc \"SELECT 'requests:' || count(*) FROM requests UNION ALL SELECT 'offers:' || count(*) FROM offers UNION ALL SELECT 'contacts:' || count(*) FROM contacts UNION ALL SELECT 'migrations:' || count(*) FROM _sqlx_migrations;\"" || exit 1
else
  echo "restore-test: one of --suffix or --database-url is required" >&2
  exit 2
fi
echo "restore-test: OK (snapshots and migration state match)"
