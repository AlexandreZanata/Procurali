#!/usr/bin/env bash
# Safe local-stack helpers. See docs/engineering/local-operation.md.
# Usage:
#   scripts/infra.sh up-dev [--port PORT]
#   scripts/infra.sh up-test --suffix SUFFIX [--port PORT]
#   scripts/infra.sh down-test --suffix SUFFIX
# up-dev targets the single development project; up-test/down-test target only
# explicit disposable `procurali-test-<suffix>` projects. There is intentionally
# no destructive dev/staging/production command here.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/test-environment.sh
source "$REPO_ROOT/scripts/lib/test-environment.sh"

usage() {
  echo "usage:" >&2
  echo "  scripts/infra.sh up-dev [--port PORT]" >&2
  echo "  scripts/infra.sh up-test --suffix SUFFIX [--port PORT]" >&2
  echo "  scripts/infra.sh down-test --suffix SUFFIX" >&2
  exit 2
}

COMMAND="${1:-}"
if [ -z "$COMMAND" ]; then
  usage
fi
shift || true

SUFFIX=""
PORT=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --suffix)
      SUFFIX="${2:-}"
      shift 2 || usage
      ;;
    --port)
      PORT="${2:-}"
      shift 2 || usage
      ;;
    *) usage ;;
  esac
done

case "$PORT" in
  "" | *[!0-9]*)
    if [ -n "$PORT" ]; then
      echo "infra: refusing non-numeric --port '$PORT'" >&2
      exit 2
    fi
    ;;
esac

case "$COMMAND" in
  up-dev)
    procurali_require_local_env up-dev
    export POSTGRES_PORT="${PORT:-5432}"
    docker compose -f "$REPO_ROOT/infra/compose.yaml" -p procurali-dev up -d --wait
    procurali_wait_postgres procurali-dev \
      "${POSTGRES_USER:-procurali}" "${POSTGRES_DB:-procurali_dev}" 120
    ;;
  up-test)
    procurali_require_local_env up-test
    procurali_require_suffix "$SUFFIX"
    export TEST_POSTGRES_PORT="${PORT:-5433}"
    procurali_require_disposable_url "${TEST_POSTGRES_USER:-procurali_test}" "$TEST_POSTGRES_PORT"
    export TEST_CONTAINER_NAME="procurali-test-db-$SUFFIX"
    export TEST_NETWORK_NAME="procurali-test-net-$SUFFIX"
    export TEST_VOLUME_NAME="procurali-test-pgdata-$SUFFIX"
    docker compose -f "$REPO_ROOT/infra/compose.test.yaml" -p "procurali-test-$SUFFIX" up -d --wait
    procurali_wait_postgres "procurali-test-$SUFFIX" \
      "${TEST_POSTGRES_USER:-procurali_test}" "${TEST_POSTGRES_DB:-procurali_test_bootstrap}" 120
    ;;
  down-test)
    procurali_require_suffix "$SUFFIX"
    if [ -n "$PORT" ]; then
      echo "infra: --port is not accepted for down-test (suffix identifies the project)" >&2
      exit 2
    fi
    export TEST_CONTAINER_NAME="procurali-test-db-$SUFFIX"
    export TEST_NETWORK_NAME="procurali-test-net-$SUFFIX"
    export TEST_VOLUME_NAME="procurali-test-pgdata-$SUFFIX"
    docker compose -f "$REPO_ROOT/infra/compose.test.yaml" -p "procurali-test-$SUFFIX" down -v
    ;;
  *)
    echo "infra: unknown command '$COMMAND'" >&2
    usage
    ;;
esac
