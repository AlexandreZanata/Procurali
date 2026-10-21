#!/usr/bin/env bash
# Reproducible application image (P18-T01) with the serving-binary proof
# (P21-T02). See docs/engineering/testing.md.
# Usage:
#   scripts/build-artifact.sh [--tag TAG] [--revision SHA]
#   scripts/build-artifact.sh --verify-only [--tag TAG]
#   scripts/build-artifact.sh --serve-smoke [--tag TAG]
#   scripts/build-artifact.sh --self-test
# Builds the compiled web bundle, then the pinned multi-stage image with
# revision labels, then verifies: metadata (non-root user, exposed port,
# labels), absence of private/secret/test paths and secret material in
# the exported filesystem, and per-file content hashes for rebuild
# comparison. --serve-smoke is the real serving proof: the built image runs
# against a disposable database on a disposable network and must answer
# liveness, readiness, and public traffic, refuse a cookie-less staff read,
# and report a healthy container.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

TAG="procurali-app:local"
REVISION=""
VERIFY_ONLY=0
SERVE_SMOKE=0
SELF_TEST=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --tag) TAG="${2:-}"; shift 2 || exit 2 ;;
    --revision) REVISION="${2:-}"; shift 2 || exit 2 ;;
    --verify-only) VERIFY_ONLY=1; shift ;;
    --serve-smoke) SERVE_SMOKE=1; shift ;;
    --self-test) SELF_TEST=1; shift ;;
    *) echo "usage: $(basename "$0") [--tag TAG] [--revision SHA] [--verify-only] [--serve-smoke] [--self-test]" >&2; exit 2 ;;
  esac
done
if [ -z "$TAG" ]; then
  echo "build-artifact: --tag needs a value" >&2
  exit 2
fi
if [ -z "$REVISION" ]; then
  REVISION="$(git rev-parse --short HEAD)"
  if [ -n "$(git status --porcelain -- backend/ web/src web/public contracts/ infra/Dockerfile)" ]; then
    REVISION="${REVISION}-dirty"
  fi
fi

# Pinned PostgreSQL image (same pin as the Compose stacks).
ARTIFACT_PG_IMAGE="postgres:18.6@sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336"

image_labels() {
  docker inspect --format '{{.Config.User}}|{{.Config.ExposedPorts}}|{{index .Config.Labels "org.opencontainers.image.revision"}}' "$1"
}

verify_image() {
  local tag="$1"
  local meta
  meta="$(image_labels "$tag")"
  local user="${meta%%|*}"
  local rest="${meta#*|}"
  local ports="${rest%%|*}"
  local revision="${rest#*|}"
  [ "$user" = "appuser" ] || { echo "build-artifact: image user is '$user', want appuser" >&2; return 1; }
  case "$ports" in
    *8080*) ;;
    *) echo "build-artifact: port 8080 not exposed ($ports)" >&2; return 1 ;;
  esac
  [ -n "$revision" ] && [ "$revision" != "<no value>" ] || { echo "build-artifact: revision label missing" >&2; return 1; }
  echo "build-artifact: metadata OK (user=$user revision=$revision)"

  local export_dir
  export_dir="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap "rm -rf '$export_dir'" RETURN
  docker export "$(docker create "$tag")" | tar -x -C "$export_dir"
  for forbidden in ".local" "node_modules" "target" "test-results" ".git"; do
    if [ -e "$export_dir/app/$forbidden" ] || [ -e "$export_dir/$forbidden" ]; then
      echo "build-artifact: forbidden path present in image: $forbidden" >&2
      return 1
    fi
  done
  if grep -rEn -I --exclude-dir=proc --exclude-dir=sys --exclude-dir=dev \
      'BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----|AKIA[0-9A-Z]{16}|sk-live-[A-Za-z0-9]+' \
      "$export_dir/app" 2>/dev/null | grep -q .; then
    echo "build-artifact: secret material present in image" >&2
    return 1
  fi
  echo "build-artifact: filesystem clean (no private paths, no secret material)"
  (cd "$export_dir/app" && find . -type f -exec sha256sum {} + | sort -k2) > "/tmp/procurali-artifact-manifest.txt"
  echo "build-artifact: content manifest at /tmp/procurali-artifact-manifest.txt"
}

# One free loopback port for the published container port.
free_port() {
  python3 -c "import socket; s=socket.socket(); s.bind(('127.0.0.1', 0)); print(s.getsockname()[1]); s.close()"
}

# The real serving proof: the built image serves real traffic against a
# disposable database. Disposable network and database only; every value is
# synthetic and no message, contact, or WhatsApp handoff can fire (GET only).
SERVE_SMOKE_NETWORK=""
SERVE_SMOKE_DB=""
SERVE_SMOKE_APP=""

serve_smoke_cleanup() {
  [ -n "$SERVE_SMOKE_APP" ] && docker rm -f "$SERVE_SMOKE_APP" >/dev/null 2>&1 || true
  [ -n "$SERVE_SMOKE_DB" ] && docker rm -f "$SERVE_SMOKE_DB" >/dev/null 2>&1 || true
  [ -n "$SERVE_SMOKE_NETWORK" ] && docker network rm "$SERVE_SMOKE_NETWORK" >/dev/null 2>&1 || true
  SERVE_SMOKE_APP=""
  SERVE_SMOKE_DB=""
  SERVE_SMOKE_NETWORK=""
}

serve_smoke() {
  local tag="$1"
  SERVE_SMOKE_NETWORK="procurali-serve-smoke-$$"
  SERVE_SMOKE_DB="procurali-serve-smoke-db-$$"
  SERVE_SMOKE_APP="procurali-serve-smoke-app-$$"
  local network="$SERVE_SMOKE_NETWORK"
  local db_container="$SERVE_SMOKE_DB"
  local app_container="$SERVE_SMOKE_APP"
  local db_password="serve-smoke-only-not-a-secret"
  local db_user="procurali_test"
  local db_name="procurali_test_serving"
  local port
  port="$(free_port)"

  trap serve_smoke_cleanup EXIT

  echo "build-artifact: serve-smoke: disposable network $network on 127.0.0.1:$port"
  docker network create "$network" >/dev/null
  docker run -d --name "$db_container" --network "$network" --network-alias db \
    -e "POSTGRES_USER=$db_user" -e "POSTGRES_PASSWORD=$db_password" -e "POSTGRES_DB=$db_name" \
    "$ARTIFACT_PG_IMAGE" >/dev/null

  echo "build-artifact: serve-smoke: waiting for the disposable database"
  local ready=0
  for _ in $(seq 1 60); do
    if docker exec "$db_container" pg_isready -U "$db_user" -d "$db_name" >/dev/null 2>&1; then
      ready=1
      break
    fi
    sleep 1
  done
  [ "$ready" -eq 1 ] || { echo "build-artifact: serve-smoke: the disposable database never became ready" >&2; return 1; }

  local sqlx_bin
  sqlx_bin="$(command -v sqlx)" || {
    echo "build-artifact: serve-smoke: pinned sqlx-cli is required to migrate the disposable database" >&2
    return 1
  }
  echo "build-artifact: serve-smoke: applying the tracked migrations"
  docker run --rm --network "$network" \
    -v "$sqlx_bin:/usr/local/bin/sqlx:ro" \
    -v "$REPO_ROOT/backend/migrations:/migrations:ro" \
    -e "DATABASE_URL=postgres://$db_user:$db_password@db:5432/$db_name" \
    --entrypoint /usr/local/bin/sqlx \
    "$ARTIFACT_PG_IMAGE" migrate run --source /migrations >/dev/null || {
    echo "build-artifact: serve-smoke: the migration job failed" >&2
    return 1
  }

  echo "build-artifact: serve-smoke: starting the image"
  docker run -d --name "$app_container" --network "$network" \
    -p "127.0.0.1:$port:8080" \
    -e "DATABASE_URL=postgres://$db_user:$db_password@db:5432/$db_name" \
    -e "PROCURALI_ENV=test" \
    -e "PHONE_PROVIDER=fake" \
    -e "FAKE_VERIFY_CODE=135790" \
    -e "SESSION_KEY=serve-smoke-only-session-key" \
    -e "PHONE_LOOKUP_KEY=serve-smoke-only-lookup-key" \
    -e "PHONE_ENCRYPTION_KEY=serve-smoke-only-encryption-key" \
    "$tag" >/dev/null

  local body
  body="$(mktemp)"
  local code
  for _ in $(seq 1 60); do
    code="$(curl -sS -m 5 -o "$body" -w "%{http_code}" "http://127.0.0.1:$port/health/ready" 2>/dev/null || echo 000)"
    if [ "$code" = "200" ] && grep -q '"ready"' "$body"; then
      break
    fi
    sleep 1
  done
  if [ "$code" != "200" ] || ! grep -q '"ready"' "$body"; then
    echo "build-artifact: serve-smoke: the image never became ready (code $code)" >&2
    docker logs "$app_container" 2>&1 | tail -5 >&2 || true
    rm -f "$body"
    return 1
  fi
  echo "build-artifact: serve-smoke: readiness answered by the image"

  code="$(curl -sS -m 10 -o "$body" -w "%{http_code}" "http://127.0.0.1:$port/health/live")" || {
    echo "build-artifact: serve-smoke: liveness unreachable" >&2
    rm -f "$body"
    return 1
  }
  [ "$code" = "200" ] && grep -q '"alive"' "$body" || {
    echo "build-artifact: serve-smoke: liveness not alive (code $code)" >&2
    rm -f "$body"
    return 1
  }
  echo "build-artifact: serve-smoke: liveness alive"

  code="$(curl -sS -m 10 -o "$body" -w "%{http_code}" "http://127.0.0.1:$port/api/v1/catalogs/cities")" || {
    echo "build-artifact: serve-smoke: public catalog unreachable" >&2
    rm -f "$body"
    return 1
  }
  [ "$code" = "200" ] || {
    echo "build-artifact: serve-smoke: public catalog not public (code $code)" >&2
    rm -f "$body"
    return 1
  }
  if grep -Eq '"phone"|\+?[0-9][0-9 ./-]{7,}[0-9]' "$body"; then
    echo "build-artifact: serve-smoke: public body carries phone-shaped material" >&2
    rm -f "$body"
    return 1
  fi
  echo "build-artifact: serve-smoke: public catalog readable with no phone-shaped material"

  code="$(curl -sS -m 10 -o "$body" -w "%{http_code}" "http://127.0.0.1:$port/api/v1/metrics/operational?purpose=smoke")" || {
    echo "build-artifact: serve-smoke: staff boundary unreachable" >&2
    rm -f "$body"
    return 1
  }
  [ "$code" = "401" ] || {
    echo "build-artifact: serve-smoke: cookie-less staff read not refused (code $code)" >&2
    rm -f "$body"
    return 1
  }
  echo "build-artifact: serve-smoke: cookie-less staff read refused with 401 (no message sent)"

  code="$(curl -sS -m 10 -o "$body" -w "%{http_code}" "http://127.0.0.1:$port/api/v1/not-a-route")" || true
  [ "$code" = "404" ] || {
    echo "build-artifact: serve-smoke: unknown route not refused (code $code)" >&2
    rm -f "$body"
    return 1
  }
  rm -f "$body"

  echo "build-artifact: serve-smoke: waiting for the container health check"
  local healthy=0
  for _ in $(seq 1 30); do
    if [ "$(docker inspect -f '{{.State.Health.Status}}' "$app_container" 2>/dev/null)" = "healthy" ]; then
      healthy=1
      break
    fi
    sleep 2
  done
  [ "$healthy" -eq 1 ] || {
    echo "build-artifact: serve-smoke: the container health check never reported healthy" >&2
    return 1
  }
  echo "build-artifact: serve-smoke: container reports healthy"

  echo "build-artifact: serve-smoke: stopping the image (SIGTERM drain)"
  docker stop --time 20 "$app_container" >/dev/null
  local exit_code
  exit_code="$(docker inspect -f '{{.State.ExitCode}}' "$app_container")"
  [ "$exit_code" = "0" ] || {
    echo "build-artifact: serve-smoke: the image exited with code $exit_code" >&2
    return 1
  }
  echo "build-artifact: serve-smoke: OK (the image served real traffic and drained cleanly)"
}

if [ "$SELF_TEST" -eq 1 ]; then
  echo "self-test: unknown flag fails"
  if ./scripts/build-artifact.sh --bogus >/dev/null 2>&1; then
    echo "self-test: unknown flag wrongly accepted" >&2
    exit 1
  fi
  echo "build-artifact self-test: OK"
  exit 0
fi

if [ "$VERIFY_ONLY" -eq 0 ]; then
  echo "build-artifact: web bundle (pinned toolchain)"
  (cd web && npm ci --no-audit --no-fund)
  (cd web && npm run build)
  echo "build-artifact: image $TAG from revision $REVISION"
  docker build --file infra/Dockerfile \
    --build-arg "GIT_SHA=$REVISION" \
    --build-arg "BUILD_DATE=$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --tag "$TAG" .
fi
verify_image "$TAG"
if [ "$SERVE_SMOKE" -eq 1 ]; then
  serve_smoke "$TAG"
fi
echo "build-artifact: OK ($TAG)"
