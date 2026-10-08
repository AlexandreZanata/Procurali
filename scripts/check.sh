#!/usr/bin/env bash
# Quick verification gate. See docs/engineering/testing.md.
# Usage: scripts/check.sh fast
# Runs formatting, lints-as-errors, the unit tier, and the private-path guard,
# preserving the first failing exit code. Unknown commands fail (never no-op).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BACKEND="$REPO_ROOT/backend"

usage() {
  echo "usage: scripts/check.sh fast" >&2
  exit 2
}

COMMAND="${1:-}"
if [ -z "$COMMAND" ]; then
  usage
fi
shift || true
if [ "$#" -gt 0 ]; then
  usage
fi

case "$COMMAND" in
  fast) ;;
  *)
    echo "check.sh: unknown command '$COMMAND'" >&2
    usage
    ;;
esac

echo "check.sh fast: cargo fmt --check"
cargo fmt --check --manifest-path "$BACKEND/Cargo.toml"

echo "check.sh fast: cargo clippy --all-targets -- -D warnings"
cargo clippy --all-targets --manifest-path "$BACKEND/Cargo.toml" -- -D warnings

echo "check.sh fast: scripts/test.sh unit"
"$REPO_ROOT/scripts/test.sh" unit

echo "check.sh fast: scripts/check-private-paths.sh"
"$REPO_ROOT/scripts/check-private-paths.sh"

echo "check.sh fast: OK"
