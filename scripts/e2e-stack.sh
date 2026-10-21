#!/usr/bin/env bash
# Disposable e2e stack (P22-T01): one script, one real journey.
#
# Usage:
#   scripts/e2e-stack.sh --suffix SUFFIX [--db-port PORT] [--app-port PORT]
#   scripts/e2e-stack.sh --self-test
#
# Starts a disposable PostgreSQL container on a disposable network, applies
# the tracked migrations, serves the real application over it, seeds the
# permanent harness state (fixtures file plus harness accounts), runs the
# Playwright e2e project against the served target, and tears everything
# down. Every value is synthetic and loopback-only; the database is named
# `procurali_test_*` and refuses any other name. A missing tool, a refused
# input, an unreachable target, or a failing spec fails the run loudly:
# there is no silent skip and no production target.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Pinned PostgreSQL image (same pin as the Compose stacks).
E2E_PG_IMAGE="postgres:18.6@sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336"
E2E_DB_NAME="procurali_test_e2e"
E2E_DB_USER="procurali_test"
E2E_DB_PASSWORD="e2e-only-not-a-secret"
E2E_FAKE_CODE="135790"
E2E_LOOKUP_KEY="e2e-harness-only-lookup-key"
E2E_ENCRYPTION_KEY="e2e-harness-only-encryption-key"
E2E_SESSION_KEY="e2e-harness-only-session-key"

SUFFIX=""
DB_PORT=""
APP_PORT=""
SELF_TEST=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --suffix) SUFFIX="${2:-}"; shift 2 || exit 2 ;;
    --db-port) DB_PORT="${2:-}"; shift 2 || exit 2 ;;
    --app-port) APP_PORT="${2:-}"; shift 2 || exit 2 ;;
    --self-test) SELF_TEST=1; shift ;;
    *) echo "usage: $(basename "$0") --suffix SUFFIX [--db-port PORT] [--app-port PORT] | --self-test" >&2; exit 2 ;;
  esac
done

free_port() {
  python3 -c "import socket; s=socket.socket(); s.bind(('127.0.0.1', 0)); print(s.getsockname()[1]); s.close()"
}

e2e_self_test() {
  echo "self-test: missing suffix fails"
  if ./scripts/e2e-stack.sh >/dev/null 2>&1; then
    echo "self-test: a run without an explicit suffix wrongly started" >&2
    exit 1
  fi
  echo "self-test: environment-like suffix is refused"
  if ./scripts/e2e-stack.sh --suffix prod >/dev/null 2>&1; then
    echo "self-test: an environment-like suffix wrongly started" >&2
    exit 1
  fi
  echo "self-test: unknown flag fails"
  if ./scripts/e2e-stack.sh --bogus >/dev/null 2>&1; then
    echo "self-test: unknown flag wrongly accepted" >&2
    exit 1
  fi
  echo "e2e-stack self-test: OK"
}

if [ "$SELF_TEST" -eq 1 ]; then
  e2e_self_test
  exit 0
fi

if [ -z "$SUFFIX" ]; then
  echo "e2e-stack: --suffix is required (no default disposable target)" >&2
  exit 2
fi
case "$SUFFIX" in
  *[!A-Za-z0-9_-]*)
    echo "e2e-stack: refusing suffix '$SUFFIX' (allowed: letters, digits, -, _)" >&2
    exit 2
    ;;
  dev | development | prod | production | staging | main | test)
    echo "e2e-stack: refusing environment-like suffix '$SUFFIX'" >&2
    exit 2
    ;;
esac
for port in "$DB_PORT" "$APP_PORT"; do
  case "$port" in
    "" | *[!0-9]*) ;;
    *) ;;
  esac
done
[ -n "$DB_PORT" ] || DB_PORT="$(free_port)"
[ -n "$APP_PORT" ] || APP_PORT="$(free_port)"

NETWORK="procurali-e2e-net-$SUFFIX"
DB_CONTAINER="procurali-e2e-db-$SUFFIX"
FIXTURES="/tmp/procurali-e2e-fixtures-$SUFFIX.json"
SERVER_LOG="/tmp/procurali-e2e-server-$SUFFIX.log"
SERVER_PID=""

cleanup() {
  if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill -TERM "$SERVER_PID" 2>/dev/null || true
    for _ in $(seq 1 20); do
      kill -0 "$SERVER_PID" 2>/dev/null || break
      sleep 1
    done
    kill -KILL "$SERVER_PID" 2>/dev/null || true
  fi
  docker rm -f "$DB_CONTAINER" >/dev/null 2>&1 || true
  docker network rm "$NETWORK" >/dev/null 2>&1 || true
}
trap cleanup EXIT

command -v docker >/dev/null || { echo "e2e-stack: docker is required" >&2; exit 2; }
command -v curl >/dev/null || { echo "e2e-stack: curl is required" >&2; exit 2; }
command -v node >/dev/null || { echo "e2e-stack: node is required" >&2; exit 2; }

echo "e2e-stack: disposable database on 127.0.0.1:$DB_PORT, served application on 127.0.0.1:$APP_PORT"
docker network create "$NETWORK" >/dev/null
docker run -d --name "$DB_CONTAINER" --network "$NETWORK" --network-alias db \
  -p "127.0.0.1:$DB_PORT:5432" \
  -e "POSTGRES_USER=$E2E_DB_USER" -e "POSTGRES_PASSWORD=$E2E_DB_PASSWORD" \
  -e "POSTGRES_DB=$E2E_DB_NAME" \
  "$E2E_PG_IMAGE" >/dev/null

echo "e2e-stack: waiting for the disposable database"
READY=0
for _ in $(seq 1 60); do
  if docker exec "$DB_CONTAINER" pg_isready -U "$E2E_DB_USER" -d "$E2E_DB_NAME" >/dev/null 2>&1; then
    READY=1
    break
  fi
  sleep 1
done
[ "$READY" -eq 1 ] || { echo "e2e-stack: the disposable database never became ready" >&2; exit 1; }

echo "e2e-stack: building the web bundle and the application binaries"
if [ ! -f "$REPO_ROOT/web/dist/main.js" ]; then
  (cd "$REPO_ROOT/web" && npm ci --no-audit --no-fund >/dev/null && npm run build >/dev/null)
fi
# Release binaries only: the served application verifies session digests
# with Argon2, and a debug build makes the documented slow-auth path slow
# enough to time the browser suite out.
cargo build --locked --release --manifest-path "$REPO_ROOT/backend/Cargo.toml" --bins >/dev/null

echo "e2e-stack: applying the tracked migrations"
DATABASE_URL="postgres://$E2E_DB_USER:$E2E_DB_PASSWORD@127.0.0.1:$DB_PORT/$E2E_DB_NAME" \
  "$REPO_ROOT/backend/target/release/procurali-migrate" >/dev/null || {
  echo "e2e-stack: the migration job failed" >&2
  exit 1
}

DATABASE_URL="postgres://$E2E_DB_USER:$E2E_DB_PASSWORD@127.0.0.1:$DB_PORT/$E2E_DB_NAME"
echo "e2e-stack: starting the served application"
PROCURALI_ENV=test \
PHONE_PROVIDER=fake \
FAKE_VERIFY_CODE="$E2E_FAKE_CODE" \
SESSION_KEY="$E2E_SESSION_KEY" \
PHONE_LOOKUP_KEY="$E2E_LOOKUP_KEY" \
PHONE_ENCRYPTION_KEY="$E2E_ENCRYPTION_KEY" \
APP_BIND_ADDRESS="127.0.0.1:$APP_PORT" \
APP_ASSETS_DIR="$REPO_ROOT/web/dist" \
DATABASE_URL="$DATABASE_URL" \
  "$REPO_ROOT/backend/target/release/procurali-backend" >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!

echo "e2e-stack: waiting for the served application"
READY=0
for _ in $(seq 1 60); do
  if curl -sS -m 5 -o /dev/null "http://127.0.0.1:$APP_PORT/health/ready" 2>/dev/null; then
    READY=1
    break
  fi
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "e2e-stack: the served application exited before becoming ready" >&2
    tail -5 "$SERVER_LOG" >&2 || true
    exit 1
  fi
  sleep 1
done
[ "$READY" -eq 1 ] || {
  echo "e2e-stack: the served application never became ready" >&2
  tail -5 "$SERVER_LOG" >&2 || true
  exit 1
}

echo "e2e-stack: seeding the harness state"
PHONE_LOOKUP_KEY="$E2E_LOOKUP_KEY" \
PHONE_ENCRYPTION_KEY="$E2E_ENCRYPTION_KEY" \
E2E_HARNESS_DATABASE_URL="$DATABASE_URL" \
E2E_FIXTURES_PATH="$FIXTURES" \
  cargo test --locked --manifest-path "$REPO_ROOT/backend/Cargo.toml" --test e2e_harness -- --nocapture || {
  echo "e2e-stack: the harness seeder failed" >&2
  exit 1
}
[ -f "$FIXTURES" ] || { echo "e2e-stack: the harness wrote no fixtures file" >&2; exit 1; }

echo "e2e-stack: running the browser suite against the served target"
cd "$REPO_ROOT/web"
PROCURALI_E2E_BASE_URL="http://127.0.0.1:$APP_PORT" \
PROCURALI_E2E_FAKE_CODE="$E2E_FAKE_CODE" \
E2E_FIXTURES_PATH="$FIXTURES" \
TEST_DATABASE_URL="$DATABASE_URL" \
  npx playwright test --project=e2e
echo "e2e-stack: OK (the served application answered the browser journey)"
