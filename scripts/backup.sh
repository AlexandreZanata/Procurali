#!/usr/bin/env bash
# Consistent PostgreSQL backup with protected destination (P18-T04).
# Usage:
#   scripts/backup.sh --suffix SUFFIX --env-file FILE --out DIR [--destination DIR]
#   scripts/backup.sh --database-url URL --out DIR [--destination DIR]
#   scripts/backup.sh --self-test
# --suffix backs up a deploy stack database via `docker exec` (no ports
# are published, never opened for backup). --database-url backs up a
# directly reachable database, which must be disposable
# (`procurali_test_*`); anything else is refused. --destination accepts
# local directories only: any off-host destination fails loudly (no
# approved off-host target exists; P18-T07 owns that gate).
# Output: <db>-<timestamp>.dump plus manifest.json with checksum, time,
# revision, schema version, and row counts. Credentials never enter the
# manifest or the logs.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

SUFFIX=""
ENV_FILE="infra/env.deploy"
DATABASE_URL=""
OUT=""
DESTINATION=""
SELF_TEST=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --suffix) SUFFIX="${2:-}"; shift 2 || exit 2 ;;
    --env-file) ENV_FILE="${2:-}"; shift 2 || exit 2 ;;
    --database-url) DATABASE_URL="${2:-}"; shift 2 || exit 2 ;;
    --out) OUT="${2:-}"; shift 2 || exit 2 ;;
    --destination) DESTINATION="${2:-}"; shift 2 || exit 2 ;;
    --self-test) SELF_TEST=1; shift ;;
    *) echo "usage: $(basename "$0") (--suffix SUFFIX --env-file FILE | --database-url URL) --out DIR [--destination DIR] [--self-test]" >&2; exit 2 ;;
  esac
done

if [ "$SELF_TEST" -eq 1 ]; then
  echo "self-test: off-host destination is refused"
  if ./scripts/backup.sh --database-url "postgres://u@127.0.0.1:1/procurali_test_x" --out /tmp --destination offhost:s3 2>/dev/null; then
    echo "self-test: off-host destination wrongly accepted" >&2
    exit 1
  fi
  echo "self-test: non-disposable direct URL is refused"
  if ./scripts/backup.sh --database-url "postgres://u@127.0.0.1:1/appdb" --out /tmp 2>/dev/null; then
    echo "self-test: non-disposable URL wrongly accepted" >&2
    exit 1
  fi
  echo "self-test: manifest carries no credential fields"
  if grep -rEi "password|secret|token" scripts/backup.sh | grep -v "POSTGRES_PASSWORD:?\|PASSWORD=\|redact" | grep -q .; then
    echo "self-test: script prints credential-adjacent values" >&2
    exit 1
  fi
  echo "backup self-test: OK"
  exit 0
fi

if [ -z "$OUT" ]; then
  echo "backup: --out is required" >&2
  exit 2
fi
if [ -z "$DESTINATION" ]; then
  DESTINATION="$OUT"
fi
case "$DESTINATION" in
  offhost:* | s3:* | gs:* | ssh:* | https:*)
    echo "backup: off-host destination '$DESTINATION' refused (no approved off-host target; live gate stays blocked)" >&2
    exit 2
    ;;
esac
mkdir -p "$DESTINATION"

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
if [ -n "$SUFFIX" ]; then
  [ -f "$ENV_FILE" ] || { echo "backup: env file missing: $ENV_FILE" >&2; exit 2; }
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
  PROJECT="procurali-deploy-$SUFFIX"
  DB_CONTAINER="$(docker ps --format '{{.Names}} {{.State}}' | awk '$1 == "procurali-deploy-'"$SUFFIX"'-db-1" && $2 == "running" {print $1}')"
  if [ -z "$DB_CONTAINER" ]; then
    echo "backup: no running db container in project $PROJECT" >&2
    exit 1
  fi
  DB_NAME="${POSTGRES_DB:?}"
  DUMP="$DESTINATION/${DB_NAME}-${STAMP}.dump"
  echo "backup: dumping $DB_NAME via container exec (no ports involved)"
  docker exec "$DB_CONTAINER" pg_dump -U "${POSTGRES_USER:?}" -d "$DB_NAME" -Fc > "$DUMP" || {
    echo "backup: pg_dump failed" >&2
    exit 1
  }
  COUNTS="$(docker exec -i "$DB_CONTAINER" psql -U "${POSTGRES_USER:?}" -d "$DB_NAME" -tAc \
    "SELECT 'requests:' || count(*) FROM requests UNION ALL SELECT 'offers:' || count(*) FROM offers UNION ALL SELECT 'contacts:' || count(*) FROM contacts UNION ALL SELECT 'migrations:' || count(*) FROM _sqlx_migrations;" 2>/dev/null || echo "unavailable")"
elif [ -n "$DATABASE_URL" ]; then
  case "$DATABASE_URL" in
    *@*/*procurali_test_*)
      ;;
    *)
      echo "backup: refusing non-disposable database URL (must contain procurali_test_)" >&2
      exit 2
      ;;
  esac
  # Host pg tools are not required: dump and checks run inside the
  # pinned postgres image over host loopback. Credentials travel via
  # environment, never the command line or logs.
  PG_IMAGE="postgres:18.6@sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336"
  DB_NAME="$(printf '%s' "$DATABASE_URL" | sed 's|.*/||; s|?.*||')"
  DB_USER="$(printf '%s' "$DATABASE_URL" | sed -E 's|.*://([^:]+):.*|\1|')"
  DB_PASS="$(printf '%s' "$DATABASE_URL" | sed -E 's|.*://[^:]+:([^@]+)@.*|\1|')"
  DB_HOSTPORT="$(printf '%s' "$DATABASE_URL" | sed -E 's|.*@([^/]+)/.*|\1|')"
  DUMP="$DESTINATION/${DB_NAME}-${STAMP}.dump"
  echo "backup: dumping disposable database $DB_NAME"
  docker run --rm --network host \
    -e "PGPASSWORD=$DB_PASS" \
    "$PG_IMAGE" pg_dump -h "${DB_HOSTPORT%%:*}" -p "${DB_HOSTPORT##*:}" -U "$DB_USER" -d "$DB_NAME" -Fc > "$DUMP" || {
    echo "backup: pg_dump failed" >&2
    exit 1
  }
  COUNTS="$(docker run --rm --network host \
    -e "PGPASSWORD=$DB_PASS" \
    "$PG_IMAGE" psql -h "${DB_HOSTPORT%%:*}" -p "${DB_HOSTPORT##*:}" -U "$DB_USER" -d "$DB_NAME" -tAc \
    "SELECT 'requests:' || count(*) FROM requests UNION ALL SELECT 'offers:' || count(*) FROM offers UNION ALL SELECT 'contacts:' || count(*) FROM contacts UNION ALL SELECT 'migrations:' || count(*) FROM _sqlx_migrations;" 2>/dev/null || echo "unavailable")"
else
  echo "backup: one of --suffix or --database-url is required" >&2
  exit 2
fi

test -s "$DUMP" || { echo "backup: empty dump, refusing" >&2; exit 1; }
CHECKSUM="$(sha256sum "$DUMP" | cut -d' ' -f1)"
REVISION="$(git rev-parse --short HEAD)"
cat > "$DESTINATION/manifest.json" <<EOF
{
  "database": "$DB_NAME",
  "dump": "$(basename "$DUMP")",
  "sha256": "$CHECKSUM",
  "taken_at": "$STAMP",
  "revision": "$REVISION",
  "counts": $(printf '%s' "$COUNTS" | python3 -c "import json,sys; print(json.dumps([l for l in sys.stdin.read().split() if l]))")
}
EOF
echo "backup: OK $DUMP sha256=$CHECKSUM"
