#!/usr/bin/env bash
# Fail-loud test tier dispatcher. See docs/engineering/testing.md.
# Usage: scripts/test.sh <tier> [--task TASK-ID]
#   tiers: unit | api | privacy | all
#   Tiers without a registered suite (integration, concurrency, web, e2e, ops,
#   critical-mutations, load) exit nonzero instead of silently passing.
#   --task restricts to the named task's registered targets; unknown tasks or
#   zero matches exit nonzero.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BACKEND="$REPO_ROOT/backend"
BIN_NAME="procurali-backend"
cd "$BACKEND"

usage() {
  echo "usage: scripts/test.sh <unit|api|privacy|all> [--task TASK-ID]" >&2
  exit 2
}

# Registered suites per tier. Extended by later cards as suites land.
tier_targets() {
  case "$1" in
    unit) printf '%s\n' "bin" "configuration" "privacy_logging" "api_scaffold" ;;
    api) printf '%s\n' "api_scaffold" ;;
    privacy) printf '%s\n' "privacy_logging" ;;
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
  echo "test.sh: pending tiers without registered suites: integration concurrency web e2e ops critical-mutations load"
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
    integration | concurrency | web | e2e | ops | critical-mutations | load)
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
