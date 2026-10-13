#!/usr/bin/env bash
# Restart, worker, and dependency-outage validation (P18-T05).
# Usage:
#   TEST_DATABASE_URL=postgres://...@127.0.0.1:PORT/procurali_test_bootstrap \
#     scripts/test-ops.sh --suffix SUFFIX --port PORT
#   scripts/test-ops.sh --self-test
# Proves against the disposable test stack only (loopback,
# `procurali_test_*` databases; anything else is refused):
#   1. the ops_restart cargo suite passes (crash recovery, repeated sweeps,
#      provider outage, database outage);
#   2. a real database container restart preserves the stack: the project is
#      healthy again and the suite still passes with identical business-row
#      outcomes.
# Credentials never appear in output; only host:port/database labels.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

SUFFIX=""
PORT=""
SELF_TEST=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --suffix) SUFFIX="${2:-}"; shift 2 || exit 2 ;;
    --port) PORT="${2:-}"; shift 2 || exit 2 ;;
    --self-test) SELF_TEST=1; shift ;;
    *) echo "usage: $(basename "$0") --suffix SUFFIX --port PORT | --self-test" >&2; exit 2 ;;
  esac
done

if [ "$SELF_TEST" -eq 1 ]; then
  echo "self-test: missing TEST_DATABASE_URL fails instead of succeeding"
  if TEST_DATABASE_URL="" ./scripts/test-ops.sh --suffix t --port 5439 2>/dev/null; then
    echo "self-test: missing url wrongly accepted" >&2
    exit 1
  fi
  echo "self-test: non-disposable database is refused"
  if TEST_DATABASE_URL="postgres://u@127.0.0.1:5439/procurali_prod" ./scripts/test-ops.sh --suffix t --port 5439 2>/dev/null; then
    echo "self-test: production database wrongly accepted" >&2
    exit 1
  fi
  echo "self-test: non-loopback host is refused"
  if TEST_DATABASE_URL="postgres://u@db.example.com:5439/procurali_test_x" ./scripts/test-ops.sh --suffix t --port 5439 2>/dev/null; then
    echo "self-test: remote host wrongly accepted" >&2
    exit 1
  fi
  echo "test-ops self-test: OK"
  exit 0
fi

[ -n "$SUFFIX" ] || { echo "test-ops: --suffix is required (disposable project only)" >&2; exit 2; }
[ -n "$PORT" ] || { echo "test-ops: --port is required (explicit loopback port)" >&2; exit 2; }
URL="${TEST_DATABASE_URL:-}"
[ -n "$URL" ] || { echo "test-ops: TEST_DATABASE_URL is required (explicit disposable naming)" >&2; exit 1; }
case "$URL" in
  postgres://*@127.0.0.1:*/*procurali_test_* | postgres://*@localhost:*/*procurali_test_*) ;;
  *) echo "test-ops: refusing non-disposable or non-loopback TEST_DATABASE_URL" >&2; exit 2 ;;
esac
LABEL="$(printf '%s' "$URL" | sed -E 's|.*@([^/]+)/.*|\1|')/$(printf '%s' "$URL" | sed 's|.*/||; s|?.*||')"
echo "test-ops: target $LABEL (credentials never printed)"

CONTAINER="procurali-test-db-$SUFFIX"
docker ps --format '{{.Names}}' | grep -qx "$CONTAINER" || {
  echo "test-ops: no running test container '$CONTAINER' (start it with scripts/infra.sh up-test --suffix $SUFFIX --port $PORT)" >&2
  exit 1
}

echo "test-ops: (1/3) ops_restart suite before restart"
TEST_DATABASE_URL="$URL" cargo test --manifest-path backend/Cargo.toml --test ops_restart

STARTED_BEFORE="$(docker inspect -f '{{.State.StartedAt}}' "$CONTAINER")"
echo "test-ops: (2/3) restarting database container $CONTAINER"
docker restart "$CONTAINER" >/dev/null
echo "test-ops: waiting for health"
for _ in $(seq 1 60); do
  if [ "$(docker inspect -f '{{.State.Health.Status}}' "$CONTAINER" 2>/dev/null)" = "healthy" ]; then
    break
  fi
  sleep 2
done
[ "$(docker inspect -f '{{.State.Health.Status}}' "$CONTAINER")" = "healthy" ] || {
  echo "test-ops: container did not return to healthy (failed recovery, not success)" >&2
  exit 1
}
STARTED_AFTER="$(docker inspect -f '{{.State.StartedAt}}' "$CONTAINER")"
[ "$STARTED_BEFORE" != "$STARTED_AFTER" ] || {
  echo "test-ops: container did not actually restart (failed recovery, not success)" >&2
  exit 1
}
echo "test-ops: container restarted ($STARTED_BEFORE -> $STARTED_AFTER) and is healthy"

echo "test-ops: (3/3) ops_restart suite after restart (identical outcomes required)"
TEST_DATABASE_URL="$URL" cargo test --manifest-path backend/Cargo.toml --test ops_restart
echo "test-ops: OK (restart preserved the stack; worker/outage assertions hold)"
