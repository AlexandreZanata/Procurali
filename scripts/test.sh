#!/usr/bin/env bash
# Fail-loud test tier dispatcher. See docs/engineering/testing.md.
# Usage: scripts/test.sh <tier> [--task TASK-ID]
#   tiers: unit | api | privacy | web | e2e | all
#   Tiers without a registered suite (integration, concurrency, ops,
#   critical-mutations, load) exit nonzero instead of silently passing.
#   The e2e tier requires PROCURALI_E2E_BASE_URL plus TEST_DATABASE_URL and
#   fails loudly without them or with zero registered e2e cases.
#   --task restricts to the named task's registered targets; unknown tasks or
#   zero matches exit nonzero.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BACKEND="$REPO_ROOT/backend"
WEB="$REPO_ROOT/web"
BIN_NAME="procurali-backend"

# Web component tier: pinned clean install, typecheck, build, browser specs.
# Any missing tool, type error, build failure, or spec failure is failure.
run_web_tier() {
  cd "$WEB"
  echo "test.sh: web: npm ci (pinned lockfile)"
  npm ci --no-audit --no-fund
  echo "test.sh: web: npm run check (tsc --noEmit)"
  npm run check
  echo "test.sh: web: npm run build"
  npm run build
  echo "test.sh: web: playwright component project"
  npx playwright test --project=component
}

# Full-journey tier: real disposable backend/database through the harness.
# Missing env, unreachable backend, or zero e2e cases fails loudly.
run_e2e_tier() {
  cd "$WEB"
  echo "test.sh: e2e: playwright e2e project (real disposable stack)"
  npx playwright test --project=e2e
}

cd "$BACKEND"

usage() {
  echo "usage: scripts/test.sh <unit|api|privacy|web|e2e|all> [--task TASK-ID]" >&2
  exit 2
}

# Registered suites per tier. Extended by later cards as suites land.
tier_targets() {
  case "$1" in
    unit) printf '%s\n' "bin" "configuration" "privacy_logging" "api_scaffold" ;;
    api) printf '%s\n' "api_scaffold" ;;
    privacy) printf '%s\n' "privacy_logging" ;;
    web) printf '%s\n' "web-component" ;;
    e2e) printf '%s\n' "e2e-suite" ;;
    *) return 1 ;;
  esac
}

# Task registration: which test target proves each implemented card.
task_targets() {
  case "$1" in
    P01-T01) printf '%s\n' "bin" ;;
    P01-T02) printf '%s\n' "configuration" ;;
    P01-T03) printf '%s\n' "privacy_logging" ;;
    P01-T04) printf '%s\n' "api_scaffold" ;;
    *) return 1 ;;
  esac
}

TIER="${1:-}"
if [ -z "$TIER" ]; then
  usage
fi
shift || true

TASK=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --task)
      TASK="${2:-}"
      shift 2 || usage
      ;;
    *) usage ;;
  esac
done

TARGETS=()
if [ "$TIER" = "all" ]; then
  if [ -n "$TASK" ]; then
    echo "test.sh: --task cannot be combined with tier 'all'" >&2
    exit 2
  fi
  for tier in unit api privacy; do
    while IFS= read -r target; do
      TARGETS+=("$tier:$target")
    done < <(tier_targets "$tier")
  done
  echo "test.sh: pending tiers without registered suites: integration concurrency ops critical-mutations load"
  echo "test.sh: tier 'web' runs separately (node toolchain); tier 'e2e' needs a live disposable stack"
  echo "test.sh: running cargo tiers for 'all'; invoking web tier next"
  run_web_tier
elif [ "$TIER" = "web" ]; then
  if [ -n "$TASK" ]; then
    echo "test.sh: --task filtering is not supported for tier 'web' yet (failing loudly)" >&2
    exit 2
  fi
  run_web_tier
  echo "test.sh: web tier passed"
  exit 0
elif [ "$TIER" = "e2e" ]; then
  if [ -n "$TASK" ]; then
    echo "test.sh: --task filtering is not supported for tier 'e2e' yet (failing loudly)" >&2
    exit 2
  fi
  run_e2e_tier
  echo "test.sh: e2e tier passed"
  exit 0
elif tier_targets "$TIER" >/dev/null; then
  if [ -n "$TASK" ]; then
    if ! task_targets "$TASK" >/dev/null; then
      echo "test.sh: unknown task '$TASK' (no registered tests)" >&2
      exit 2
    fi
    while IFS= read -r target; do
      if tier_targets "$TIER" | grep -qx "$target"; then
        TARGETS+=("$TIER:$target")
      fi
    done < <(task_targets "$TASK")
    if [ "${#TARGETS[@]}" -eq 0 ]; then
      echo "test.sh: task '$TASK' has no registered tests in tier '$TIER'" >&2
      exit 2
    fi
  else
    while IFS= read -r target; do
      TARGETS+=("$TIER:$target")
    done < <(tier_targets "$TIER")
  fi
else
  case "$TIER" in
    integration | concurrency | ops | critical-mutations | load)
      echo "test.sh: no suite registered for tier '$TIER' yet (failing loudly)" >&2
      exit 3
      ;;
    *)
      echo "test.sh: unknown tier '$TIER'" >&2
      usage
      ;;
  esac
fi

# Deduplicate targets while preserving order.
UNIQUE=()
for entry in "${TARGETS[@]}"; do
  target="${entry#*:}"
  skip=0
  for seen in "${UNIQUE[@]}"; do
    if [ "$seen" = "$target" ]; then
      skip=1
      break
    fi
  done
  if [ "$skip" -eq 0 ]; then
    UNIQUE+=("$target")
  fi
done

failures=0
for target in "${UNIQUE[@]}"; do
  case "$target" in
    lib) args=(test --lib) ;;
    bin) args=(test --bin "$BIN_NAME") ;;
    *) args=(test --test "$target") ;;
  esac
  echo "test.sh: target '$target' (cargo ${args[*]})"
  discovered="$(cargo "${args[@]}" -- --list 2>/dev/null | grep -c ': test$' || true)"
  if [ "$discovered" -eq 0 ]; then
    echo "test.sh: target '$target' discovered zero tests (failing loudly)" >&2
    failures=$((failures + 1))
    continue
  fi
  echo "test.sh: target '$target' discovered $discovered test(s)"
  # shellcheck disable=SC2086
  if ! cargo "${args[@]}"; then
    echo "test.sh: target '$target' FAILED" >&2
    failures=$((failures + 1))
  fi
done

if [ "$failures" -ne 0 ]; then
  echo "test.sh: $failures target(s) failed" >&2
  exit 1
fi
echo "test.sh: all ${#UNIQUE[@]} target(s) passed"
