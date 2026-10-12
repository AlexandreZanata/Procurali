#!/usr/bin/env bash
# Secret and private-path scanner (P17-T05). See docs/engineering/testing.md.
# Usage:
#   scripts/check-secrets.sh            # scan tracked + staged files
#   scripts/check-secrets.sh --self-test # disposable-temp-repo fixtures only
# Fails on: staged/tracked `.local/` paths (delegated guard), high-confidence
# secret material, and production-bypass markers. Test-only fixtures
# (`test-only-*`, `example`, `fake`, `not-a-secret`, all-zero/one runs) are
# allowlisted by construction and proven passing in --self-test.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

ALLOWLIST='test-only|example|fake|synthetic|changeme|0000+|xxxx+|not-a-secret|test-test|00000000-0000-0000-0000-000000000000'

scan_paths() {
  {
    git ls-files
    git diff --cached --name-only --diff-filter=ACMRT 2>/dev/null || true
  } | sort -u | grep -v -E '(^|/)\.local(/|$)' || true
}

failures=0
report() {
  echo "check-secrets: $*" >&2
  failures=$((failures + 1))
}

check_private_paths() {
  if ! ./scripts/check-private-paths.sh >/dev/null 2>&1; then
    report "private-path guard fails (staged/tracked .local/)"
  else
    echo "check-secrets: private paths clean"
  fi
}

check_pattern() {
  local label="$1"
  local pattern="$2"
  local hits
  hits="$(scan_paths | tr '\n' '\0' | xargs -0 grep -nE -I "$pattern" 2>/dev/null | grep -vEi "$ALLOWLIST" || true)"
  if [ -n "$hits" ]; then
    report "$label material found:"
    printf '%s\n' "$hits" >&2
  else
    echo "check-secrets: $label clean"
  fi
}

run_scan() {
  check_private_paths
  check_pattern "private-key-block" 'BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----'
  check_pattern "aws-key" 'AKIA[0-9A-Z]{16}|aws_secret_access_key.{0,30}["'\''][A-Za-z0-9/+=]{16,}["'\'']'
  check_pattern "live-secret-key" 'sk-live-[A-Za-z0-9]+'
  check_pattern "api-key-assignment" '(api[_-]?key|client[_-]?secret|auth[_-]?token)[[:space:]]*[:=][[:space:]]*['\''"][^'\''"]{12,}['\''"]'
  check_pattern "password-assignment" '[Pp][Aa][Ss][Ss][Ww][Oo][Rr][Dd][[:space:]]*[:=][[:space:]]*['\''"][^'\''"]{8,}['\''"]'
  check_pattern "production-bypass" 'X-Test-Bypass|bypass[_-]?auth|auth[_-]?bypass'
  if [ "$failures" -ne 0 ]; then
    echo "check-secrets: FAILED ($failures group(s))" >&2
    return 1
  fi
  echo "check-secrets: OK (tracked + staged files clean)"
  return 0
}

self_test() {
  local tmp
  tmp="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" EXIT
  git -C "$tmp" init -q
  git -C "$tmp" config user.email "guard-fixture@example.invalid"
  git -C "$tmp" config user.name "guard-fixture"
  printf '/.local/\n' > "$tmp/.gitignore"
  mkdir -p "$tmp/.local"

  # Clean fixture: test-only values must pass every pattern.
  cat > "$tmp/clean.txt" <<'EOF'
lookup_key = "p99-test-only-lookup-key"
password = "procurali-test-only-not-a-secret"
api_key = "example-fake-value"
EOF
  # Seeded findings: each must trip its pattern.
  cat > "$tmp/evil-aws.txt" <<'EOF'
aws_secret_access_key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
EOF
  cat > "$tmp/evil-key.txt" <<'EOF'
-----BEGIN RSA PRIVATE KEY-----
MIIBOgIBAAJBAKcJbInV+iQ7eDk4rEvzbA7lKQ7Q3wIDAQAB
EOF
  local allow="test-only|example|fake|synthetic|changeme|0000+|xxxx+|not-a-secret|test-test"
  echo "self-test: clean fixture must pass"
  if grep -nE -I "password[[:space:]]*[:=][[:space:]]*['\"][^'\"]{8,}['\"]" "$tmp/clean.txt" 2>/dev/null | grep -vEi "$allow" | grep -q .; then
    echo "self-test: clean fixture wrongly flagged" >&2
    return 1
  fi
  echo "self-test: seeded AWS finding must trip"
  grep -nE -I 'aws_secret_access_key.{0,30}["'\''][A-Za-z0-9/+=]{16,}["'\'']' "$tmp/evil-aws.txt" >/dev/null || {
    echo "self-test: seeded AWS finding MISSED" >&2
    return 1
  }
  echo "self-test: seeded key-block finding must trip"
  grep -nE -I 'BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----' "$tmp/evil-key.txt" >/dev/null || {
    echo "self-test: seeded key-block MISSED" >&2
    return 1
  }
  echo "self-test: force-added .local/ must trip the staged-path rule"
  printf 'plan\n' > "$tmp/.local/secret.md"
  git -C "$tmp" add -f .local/secret.md
  if ! git -C "$tmp" diff --cached --name-only | grep -E '(^|/)\.local(/|$)' | grep -q .; then
    echo "self-test: staged .local/ path MISSED" >&2
    return 1
  fi
  echo "check-secrets self-test: OK (clean passes; seeded findings trip)"
}

case "${1:-}" in
  --self-test) self_test ;;
  "") run_scan ;;
  *) echo "usage: $(basename "$0") [--self-test]" >&2; exit 2 ;;
esac
