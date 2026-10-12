#!/usr/bin/env bash
# Migration-safe disposable deploy and rollback (P18-T03).
# Usage:
#   scripts/deploy.sh --target disposable --suffix SUFFIX --revision SHA [--image TAG] [--port PORT] [--migrations-dir DIR] [--env-file FILE] [--dry-run]
#   scripts/deploy.sh rollback --target disposable --suffix SUFFIX --revision SHA [same options]
# Only the `disposable` target exists: any other target fails loudly (no
# authorized staging/production target is configured; P18-T07 owns that
# gate). Migrations run as a separate controlled job before services
# start; application readiness waits for the compatible schema version
# (HTTP readiness belongs to the server-merge follow-up). Rollback
# redeploys a previous artifact tag without touching migrations or
# volumes: applied migrations are never rewritten and live volumes are
# never erased (no code path here does either).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

ACTION="deploy"
TARGET=""
SUFFIX=""
REVISION=""
IMAGE=""
PORT="5440"
MIGRATIONS_DIR="$REPO_ROOT/backend/migrations"
ENV_FILE="infra/env.deploy"
DRY_RUN=0
if [ "${1:-}" = "rollback" ]; then
  ACTION="rollback"
  shift
fi
while [ "$#" -gt 0 ]; do
  case "$1" in
    --target) TARGET="${2:-}"; shift 2 || exit 2 ;;
    --suffix) SUFFIX="${2:-}"; shift 2 || exit 2 ;;
    --revision) REVISION="${2:-}"; shift 2 || exit 2 ;;
    --image) IMAGE="${2:-}"; shift 2 || exit 2 ;;
    --port) PORT="${2:-}"; shift 2 || exit 2 ;;
    --migrations-dir) MIGRATIONS_DIR="${2:-}"; shift 2 || exit 2 ;;
    --env-file) ENV_FILE="${2:-}"; shift 2 || exit 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    *) echo "usage: $(basename "$0") [rollback] --target disposable --suffix SUFFIX --revision SHA [--image TAG] [--port PORT] [--migrations-dir DIR] [--env-file FILE] [--dry-run]" >&2; exit 2 ;;
  esac
done
for required in TARGET SUFFIX REVISION; do
  if [ -z "${!required}" ]; then
    echo "deploy: --target/--suffix/--revision are all required" >&2
    exit 2
  fi
done
if [ "$TARGET" != "disposable" ]; then
  echo "deploy: target '$TARGET' is not authorized (only 'disposable' exists; staging/production need P18-T07 inputs)" >&2
  exit 2
fi
case "$SUFFIX" in
  dev | prod | staging | production | *[^a-z0-9-]* | "")
    echo "deploy: suffix '$SUFFIX' refused (lowercase alphanumerics and dashes only, never environment names)" >&2
    exit 2
    ;;
esac
if [ -z "$IMAGE" ]; then
  IMAGE="procurali-app:$REVISION"
fi
if [ ! -d "$MIGRATIONS_DIR" ]; then
  echo "deploy: migrations dir missing: $MIGRATIONS_DIR" >&2
  exit 2
fi
EXPECTED_VERSION="$(ls "$MIGRATIONS_DIR"/*.sql 2>/dev/null | wc -l)"
COMPOSE="docker compose -f infra/compose.deploy.yaml -p procurali-deploy-$SUFFIX --env-file $ENV_FILE"
if [ ! -f "$ENV_FILE" ]; then
  echo "deploy: env file missing: $ENV_FILE (copy infra/env.deploy.example, never commit secrets)" >&2
  exit 2
fi
set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a
export APP_IMAGE="$IMAGE"
export DEPLOY_SUFFIX="$SUFFIX"
export POSTGRES_PORT="$PORT"
export PGPASSWORD="$POSTGRES_PASSWORD"
export DEPLOY_ENV_FILE="$ENV_FILE"

echo "deploy: action=$ACTION target=$TARGET suffix=$SUFFIX revision=$REVISION image=$IMAGE"
if [ "$DRY_RUN" -eq 1 ]; then
  echo "deploy: dry run (plan only, nothing started)"
  echo "deploy: 1. verify image $IMAGE exists locally"
  echo "deploy: 2. start db service, wait healthy"
  echo "deploy: 3. run $EXPECTED_VERSION migration(s) from $MIGRATIONS_DIR as a separate job"
  echo "deploy: 4. wait schema version $EXPECTED_VERSION before services"
  echo "deploy: 5. start app/proxy pinned to $IMAGE (never latest)"
  exit 0
fi

echo "deploy: verifying image $IMAGE exists"
docker image inspect "$IMAGE" >/dev/null || {
  echo "deploy: image '$IMAGE' not present locally (build it with scripts/build-artifact.sh)" >&2
  exit 1
}

echo "deploy: starting db service"
# The deployed topology publishes no database ports at all: the
# migration job runs as a container inside the internal network, so no
# loopback overlay or host exposure exists at any point.
$COMPOSE up -d db
echo "deploy: waiting for db health"
for _ in $(seq 1 24); do
  if $COMPOSE exec -T db pg_isready -U "${POSTGRES_USER:-procurali}" >/dev/null 2>&1; then
    break
  fi
  sleep 5
done
$COMPOSE exec -T db pg_isready -U "${POSTGRES_USER:-procurali}" || {
  echo "deploy: db never became healthy" >&2
  exit 1
}

echo "deploy: running migrations as a separate controlled job"
if [ "$MIGRATIONS_DIR" != "$REPO_ROOT/backend/migrations" ]; then
  echo "deploy: custom migrations dir is for failure drills only: $MIGRATIONS_DIR" >&2
fi
# Operator sqlx 0.9 (pinned) runs inside a postgres container on the
# internal network: the database publishes no ports, and no new image
# is required. Prebuilt migrate images are the production hardening.
SQLX_BIN="$(command -v sqlx)" || {
  echo "deploy: pinned sqlx-cli 0.9 is required on the operator host" >&2
  exit 2
}
docker run --rm \
  --network "procurali-backend-${SUFFIX}" \
  -v "$SQLX_BIN:/usr/local/bin/sqlx:ro" \
  -v "$MIGRATIONS_DIR:/migrations:ro" \
  -e "DATABASE_URL=postgres://${POSTGRES_USER:-procurali}:${POSTGRES_PASSWORD:?}@db:5432/${POSTGRES_DB:-procurali_deploy}" \
  --entrypoint /usr/local/bin/sqlx \
  postgres:18.6@sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336 \
  migrate run --source /migrations || {
  echo "deploy: migration job FAILED; services stay down, readiness never reported" >&2
  $COMPOSE stop app proxy 2>/dev/null || true
  exit 1
}

echo "deploy: waiting for compatible schema version $EXPECTED_VERSION"
SCHEMA_OK=0
for _ in $(seq 1 24); do
  VERSION="$($COMPOSE exec -T db psql -U "${POSTGRES_USER:-procurali}" -d "${POSTGRES_DB:-procurali_deploy}" -tAc "SELECT count(*) FROM _sqlx_migrations;" 2>/dev/null || echo -1)"
  if [ "$VERSION" = "$EXPECTED_VERSION" ]; then
    SCHEMA_OK=1
    break
  fi
  sleep 5
done
if [ "$SCHEMA_OK" -ne 1 ]; then
  echo "deploy: schema version never reached $EXPECTED_VERSION; readiness withheld" >&2
  exit 1
fi

echo "deploy: starting app/proxy pinned to $IMAGE"
COMPOSE="docker compose -f infra/compose.deploy.yaml -p procurali-deploy-$SUFFIX --env-file $ENV_FILE"
# Force recreation: env-file content changes do not invalidate the
# cached container config, so a plain `up` could keep running stale
# settings (or none at all). The database is never recreated here.
$COMPOSE up -d --force-recreate app proxy
echo "deploy: asserting steady state publishes no database ports"
if [ -n "$(docker port "procurali-deploy-$SUFFIX-db-1" 2>/dev/null || true)" ]; then
  echo "deploy: database container publishes host ports in steady state" >&2
  exit 1
fi
RUNNING_IMAGE="$($COMPOSE ps --format json 2>/dev/null | python3 -c "
import json,sys
for line in sys.stdin:
    if not line.strip():
        continue
    service = json.loads(line)
    if service.get('Service') == 'app':
        print(service.get('Image',''))
")"
echo "deploy: app running image: $RUNNING_IMAGE"
if [ "$RUNNING_IMAGE" != "$IMAGE" ]; then
  echo "deploy: running app image '$RUNNING_IMAGE' is not the selected artifact '$IMAGE'" >&2
  exit 1
fi
echo "deploy: OK action=$ACTION revision=$REVISION schema=$EXPECTED_VERSION (volumes and migrations untouched)"
