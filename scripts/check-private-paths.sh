#!/usr/bin/env bash
# Guard: private `.local/` workspace must never be staged, tracked, or published.
# Usage:
#   scripts/check-private-paths.sh            # guard the current repository
#   scripts/check-private-paths.sh --self-test # fixture in a disposable temp repo only
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

fail() {
  echo "private-path-guard: $*" >&2
  return 1
}

# Check one repository working copy for private-path leaks.
# Prints violations and returns nonzero when any is found.
check_repo() {
  local root="$1"
  local violations=0

  # 1. The ignore rule must cover .local/.
  if ! git -C "$root" check-ignore -q ".local/plans/backend-mvp/README.md" 2>/dev/null; then
    echo "private-path-guard: .gitignore does not ignore .local/ nested files" >&2
    violations=$((violations + 1))
  fi

  # 2. No staged .local/ path (covers `git add -f .local/...` attempts).
  local staged
  staged="$(git -C "$root" diff --cached --name-only --diff-filter=ACMRT 2>/dev/null || true)"
  if printf '%s\n' "$staged" | grep -E '(^|/)\.local(/|$)' >/dev/null 2>&1; then
    echo "private-path-guard: staged .local/ path detected:" >&2
    printf '%s\n' "$staged" | grep -E '(^|/)\.local(/|$)' >&2
    violations=$((violations + 1))
  fi

  # 3. No tracked .local/ path.
  local tracked
  tracked="$(git -C "$root" ls-files 2>/dev/null | grep -E '(^|/)\.local(/|$)' || true)"
  if [ -n "$tracked" ]; then
    echo "private-path-guard: tracked .local/ path detected:" >&2
    printf '%s\n' "$tracked" >&2
    violations=$((violations + 1))
  fi

  if [ "$violations" -ne 0 ]; then
    return 1
  fi
  echo "private-path-guard: OK (no staged/tracked .local/ paths)"
  return 0
}

# Fixture: exercise the guard inside a disposable temporary repository.
# Never touches the real index.
self_test() {
  local tmp
  tmp="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" EXIT
  git -C "$tmp" init -q
  git -C "$tmp" config user.email "guard-fixture@example.invalid"
  git -C "$tmp" config user.name "guard-fixture"
  printf '/.local/\n' > "$tmp/.gitignore"
  mkdir -p "$tmp/.local/plans"
  printf 'private\n' > "$tmp/.local/plans/secret.md"
  printf 'public\n' > "$tmp/README.md"
  git -C "$tmp" add README.md

  local failures=0

  # Case 1: clean tree passes.
  if ! check_repo "$tmp" >/dev/null; then
    echo "self-test: expected clean fixture to pass" >&2
    failures=$((failures + 1))
  fi

  # Case 2: force-added private path is refused.
  git -C "$tmp" add -f .local/plans/secret.md
  if check_repo "$tmp" >/dev/null 2>&1; then
    echo "self-test: expected force-added .local/ path to be refused" >&2
    failures=$((failures + 1))
  fi
  git -C "$tmp" reset -q

  # Case 3: committed private path is refused as tracked.
  git -C "$tmp" add -f .local/plans/secret.md
  git -C "$tmp" -c user.email=fixture@example.invalid -c user.name=fixture commit -qm fixture
  if check_repo "$tmp" >/dev/null 2>&1; then
    echo "self-test: expected tracked .local/ path to be refused" >&2
    failures=$((failures + 1))
  fi

  if [ "$failures" -ne 0 ]; then
    echo "private-path-guard self-test: FAILED ($failures case(s))" >&2
    return 1
  fi
  echo "private-path-guard self-test: OK (clean passes; staged/tracked .local/ refused)"
  return 0
}

case "${1:-}" in
  --self-test) self_test ;;
  "") check_repo "$REPO_ROOT" ;;
  *) echo "usage: $(basename "$0") [--self-test]" >&2; exit 2 ;;
esac
