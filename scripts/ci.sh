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

echo "ci.sh: private-path guard"
"$REPO_ROOT/scripts/check-private-paths.sh"

echo "ci.sh: OK (no artifacts uploaded; logs contain no secrets by construction)"
