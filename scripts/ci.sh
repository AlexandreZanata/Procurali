#!/usr/bin/env bash
# Continuous-integration entry point: every check from tracked files only.
# Usage: scripts/ci.sh
# No step reads `.local/`; no artifact is uploaded. Exit codes propagate
# directly (no masking pipelines); the first failure stops the run.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BACKEND="$REPO_ROOT/backend"
cd "$BACKEND"

echo "ci.sh: toolchain versions"
rustc --version
cargo --version

echo "ci.sh: locked dependency fetch"
cargo fetch --locked

echo "ci.sh: locked build"
cargo build --locked

echo "ci.sh: formatting"
cargo fmt --check

echo "ci.sh: lints as errors"
cargo clippy --all-targets -- -D warnings

echo "ci.sh: unit tier"
"$REPO_ROOT/scripts/test.sh" unit

echo "ci.sh: api tier"
"$REPO_ROOT/scripts/test.sh" api

echo "ci.sh: privacy tier"
"$REPO_ROOT/scripts/test.sh" privacy

echo "ci.sh: integration tier (real PostgreSQL, disposable database)"
if [ -z "${TEST_DATABASE_URL:-}" ]; then
  echo "ci.sh: TEST_DATABASE_URL is required for the integration tier (no silent skip)" >&2
  exit 1
fi
"$REPO_ROOT/scripts/test.sh" integration

echo "ci.sh: concurrency tier (real PostgreSQL, disposable database)"
"$REPO_ROOT/scripts/test.sh" concurrency

echo "ci.sh: web tier (pinned toolchain, no backend)"
"$REPO_ROOT/scripts/test.sh" web

echo "ci.sh: e2e tier (the served application over a disposable stack)"
"$REPO_ROOT/scripts/test.sh" e2e

echo "ci.sh: critical-mutations tier (isolated worktree probes)"
"$REPO_ROOT/scripts/test.sh" critical-mutations

echo "ci.sh: release gate (lockfiles, advisories, licenses, secrets, prod config)"
"$REPO_ROOT/scripts/check-release.sh"
"$REPO_ROOT/scripts/check-secrets.sh"

echo "ci.sh: private-path guard"
"$REPO_ROOT/scripts/check-private-paths.sh"

echo "ci.sh: OK (no artifacts uploaded; logs contain no secrets by construction)"
