#!/usr/bin/env bash
# Sensitivity probes for core guards (P17-T04). See docs/engineering/testing.md.
# Usage: scripts/critical-mutations.sh [--list] [GUARD-ID...]
#   Each guard from contracts/critical-guards.json is applied to a disposable
#   `git worktree` copy of HEAD; the named suites must FAIL there (mutant
#   killed) and the same suites must PASS after revert (copy sane). The real
#   working tree is never written: any modification there aborts the run.
#   A surviving mutant fails the gate and names its new-test obligation.
#   Requires TEST_DATABASE_URL pointing at a disposable procurali_test_*
#   database (suites allocate their own databases from it).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUARDS="$REPO_ROOT/contracts/critical-guards.json"
cd "$REPO_ROOT"

WANT=()
LIST_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --list)
      LIST_ONLY=1
      ;;
    *)
      WANT+=("$arg")
      ;;
  esac
done

mapfile -t IDS < <(python3 -c "
import json
for guard in json.load(open('$GUARDS'))['guards']:
    print(guard['id'])
")
if [ "$LIST_ONLY" -eq 1 ]; then
  printf '%s\n' "${IDS[@]}"
  exit 0
fi

# Tracked production modifications would leak into the HEAD worktree
# copy below (mutants patch backend/src there). Untracked new files and
# docs/scripts prose are fine: the guard contract and the script itself
# are read from this tree, and cargo tests never read them.
if [ -n "$(git diff --name-only -- backend/)" ]; then
  echo "critical-mutations: working tree has uncommitted tracked changes under backend/;" >&2
  echo "critical-mutations: commit or stash first so probes cannot hide behind them" >&2
  exit 2
fi
if [ "${#WANT[@]}" -eq 0 ]; then
  WANT=("${IDS[@]}")
fi
for want in "${WANT[@]}"; do
  if ! printf '%s\n' "${IDS[@]}" | grep -qx "$want"; then
    echo "critical-mutations: unknown guard '$want'" >&2
    exit 2
  fi
done

WORKTREE="/tmp/procurali-mutations"
trap 'git worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true' EXIT
rm -rf "$WORKTREE"
git worktree add --detach "$WORKTREE" HEAD >/dev/null 2>&1

survivors=()
for id in "${WANT[@]}"; do
  echo "critical-mutations: guard '$id'"
  python3 - "$GUARDS" "$id" "$WORKTREE" <<'EOF'
import json, sys
guards_path, want, tree = sys.argv[1], sys.argv[2], sys.argv[3]
guard = next(g for g in json.load(open(guards_path))["guards"] if g["id"] == want)
patches = guard.get("patches") or [
    {"search": guard["search"], "replace": guard["replace"]}
]
path = tree + "/" + guard["file"]
with open(path) as handle:
    content = handle.read()
for patch in patches:
    assert content.count(patch["search"]) == 1, (
        f"mutant anchor must match exactly once in {guard['file']}: {patch['search'][:60]}")
    content = content.replace(patch["search"], patch["replace"])
with open(path, "w") as handle:
    handle.write(content)
print(f"critical-mutations: mutant applied to {guard['file']} ({len(patches)} patch(es))")
EOF
  mapfile -t SUITES < <(python3 -c "
import json, sys
guard = next(g for g in json.load(open('$GUARDS'))['guards'] if g['id'] == '$id')
print('\n'.join(guard['suites']))
")
  killed=0
  for suite in "${SUITES[@]}"; do
    echo "critical-mutations: expecting failure: cargo test --test $suite"
    if (cd "$WORKTREE/backend" && cargo test --test "$suite" >/tmp/mutant-out.txt 2>&1); then
      echo "critical-mutations: SURVIVED by suite '$suite' (mutant '$id' undetected)" >&2
    else
      echo "critical-mutations: killed by suite '$suite'"
      killed=1
    fi
  done
  # Revert and prove the copy sane: same suites pass unmodified.
  # Only tracked files matter here (build outputs are untracked by design).
  (cd "$WORKTREE" && git checkout -- .) >/dev/null
  if [ -n "$(git -C "$WORKTREE" status --porcelain --untracked-files=no)" ]; then
    echo "critical-mutations: worktree not clean after revert (mutant '$id')" >&2
    exit 1
  fi
  for suite in "${SUITES[@]}"; do
    echo "critical-mutations: confirming pass unmodified: $suite"
    if ! (cd "$WORKTREE/backend" && cargo test --test "$suite" >/tmp/mutant-out.txt 2>&1); then
      echo "critical-mutations: suite '$suite' fails WITHOUT the mutant (broken baseline)" >&2
      tail -n 20 /tmp/mutant-out.txt >&2
      exit 1
    fi
  done
  if [ "$killed" -eq 0 ]; then
    survivors+=("$id")
  fi
done

if [ "${#survivors[@]}" -ne 0 ]; then
  echo "critical-mutations: SURVIVING mutants fail the gate: ${survivors[*]}" >&2
  echo "critical-mutations: each survivor needs a new focused test obligation" >&2
  exit 1
fi
echo "critical-mutations: all ${#WANT[@]} guard(s) killed; working tree untouched"
