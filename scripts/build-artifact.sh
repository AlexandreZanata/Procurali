#!/usr/bin/env bash
# Reproducible application image (P18-T01). See docs/engineering/testing.md.
# Usage:
#   scripts/build-artifact.sh [--tag TAG] [--revision SHA]
#   scripts/build-artifact.sh --verify-only [--tag TAG]
#   scripts/build-artifact.sh --serve-smoke [--tag TAG]
#   scripts/build-artifact.sh --self-test
# Builds the compiled web bundle, then the pinned multi-stage image with
# revision labels, then verifies: metadata (non-root user, exposed port,
# labels), absence of private/secret/test paths and secret material in
# the exported filesystem, and per-file content hashes for rebuild
# comparison. --serve-smoke fails loudly: no serving binary is merged
# yet (recorded follow-up), so runtime traffic cannot be proven here.
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

if [ "$SELF_TEST" -eq 1 ]; then
  echo "self-test: unknown flag fails"
  if ./scripts/build-artifact.sh --bogus >/dev/null 2>&1; then
    echo "self-test: unknown flag wrongly accepted" >&2
    exit 1
  fi
  echo "self-test: serve-smoke fails loudly without a serving binary"
  if ./scripts/build-artifact.sh --serve-smoke --tag "$TAG" >/dev/null 2>&1; then
    echo "self-test: serve-smoke wrongly passed" >&2
    exit 1
  fi
  echo "build-artifact self-test: OK"
  exit 0
fi

if [ "$SERVE_SMOKE" -eq 1 ]; then
  echo "build-artifact: serve-smoke BLOCKED: no serving binary is merged yet;" >&2
  echo "build-artifact: the image entrypoint is the startup scaffold (prints and exits);" >&2
  echo "build-artifact: runtime health/asset traffic belongs to the server-merge follow-up" >&2
  exit 3
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
echo "build-artifact: OK ($TAG)"
