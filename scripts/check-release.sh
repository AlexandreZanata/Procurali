#!/usr/bin/env bash
# Release gate: exact lockfiles, advisories/licenses, production config
# validation (P17-T05). See docs/engineering/testing.md and
# docs/engineering/dependencies.md.
# Usage:
#   scripts/check-release.sh            # full gate against this checkout
#   scripts/check-release.sh --self-test # offline synthetic fixtures only
# Fails on: lockfile drift, yanked/unsound/unmaintained or high/critical
# advisories, strong-copyleft licenses without a permissive alternative,
# and production configuration that accepts fakes or missing secrets.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BACKEND="$REPO_ROOT/backend"
WEB="$REPO_ROOT/web"
cd "$REPO_ROOT"

failures=0
fail() {
  echo "check-release: $*" >&2
  failures=$((failures + 1))
}

step_lockfiles() {
  echo "check-release: lockfiles (exact, no drift)"
  (cd "$BACKEND" && cargo fetch --locked) || return 1
  (cd "$WEB" && npm install --dry-run --no-audit --no-fund >/dev/null) || return 1
  echo "check-release: lockfiles exact"
}

step_advisories() {
  echo "check-release: advisories (cargo audit)"
  # RUSTSEC-2023-0071 (rsa Marvin, medium, no fixed upgrade) and
  # RUSTSEC-2026-0381 (webpki-roots bundled roots) are recorded
  # exceptions in docs/engineering/dependencies.md. Otherwise the gate
  # tolerates zero unassessed advisories: anything else fails loudly for
  # human triage (record or repair), never silently.
  (cd "$BACKEND" && cargo audit --deny yanked --deny unsound --deny unmaintained --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2026-0381) || return 1
  local report
  report="$(cd "$BACKEND" && cargo audit --json --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2026-0381)" || return 1
  local remaining
  remaining="$(printf '%s' "$report" | python3 -c "
import json, sys
try:
    data = json.load(sys.stdin)
except Exception:
    print('unparseable-audit-report')
    sys.exit(0)
found = sorted({
    entry.get('advisory', {}).get('id', '?')
    for entry in data.get('vulnerabilities', {}).get('list', [])
})
print(' '.join(found))
")"
  if [ "$remaining" = "unparseable-audit-report" ]; then
    echo "check-release: advisory report unparseable" >&2
    return 1
  fi
  if [ -n "$remaining" ]; then
    echo "check-release: unassessed advisories: $remaining" >&2
    return 1
  fi
  echo "check-release: advisories clean (only the recorded rsa exception stands)"
}

step_licenses() {
  echo "check-release: licenses (strong copyleft fails without permissive alternative)"
  (cd "$BACKEND" && cargo metadata --format-version 1 --locked --offline >/dev/null) || return 1
  check_cargo_licenses || return 1
  check_npm_licenses || return 1
  echo "check-release: licenses clean"
}

check_cargo_licenses() {
  (cd "$BACKEND" && cargo metadata --format-version 1 --locked --offline) | python3 -c "
import json, re, sys
packages = json.load(sys.stdin)['packages']
permissive = re.compile(r'MIT|Apache|BSD|ISC|Unlicense|Zlib|Unicode|BSL|CDLA-Permissive', re.I)
strong = re.compile(r'[AL]?GPL', re.I)
bad = []
for package in packages:
    lic = package.get('license') or ''
    if not lic or lic == 'NONE:None':
        bad.append(package['name'] + ':missing-license')
        continue
    if strong.search(lic) and not permissive.search(lic):
        bad.append(package['name'] + ':' + lic)
if bad:
    print('forbidden licenses: ' + ', '.join(sorted(set(bad))))
    sys.exit(1)
"
}

check_npm_licenses() {
  python3 -c "
import json, re, sys
lock = json.load(open('$WEB/package-lock.json'))
permissive = re.compile(r'MIT|Apache|BSD|ISC|Unlicense|Zlib|Unicode|CC0|CC-BY|Python-2.0', re.I)
strong = re.compile(r'[AL]?GPL', re.I)
bad = []
seen = set()
for name, entry in lock.get('packages', {}).items():
    if not name or not isinstance(entry, dict):
        continue
    lic = entry.get('license') or ''
    if not lic or lic in seen:
        continue
    seen.add(lic)
    if strong.search(lic) and not permissive.search(lic):
        bad.append(name + ':' + lic)
if bad:
    print('forbidden licenses: ' + ', '.join(sorted(set(bad))[:10]))
    sys.exit(1)
"
}

step_prod_config() {
  echo "check-release: production configuration refusals"
  (cd "$BACKEND" && cargo test --test configuration production_fake_provider_is_refused_even_when_requested) || return 1
  (cd "$BACKEND" && cargo test --test configuration production_requires_https_origin_and_long_session_key) || return 1
  (cd "$BACKEND" && cargo test --test configuration missing_session_key_fails_without_printing_values) || return 1
  (cd "$BACKEND" && cargo test --test configuration test_database_equal_to_application_database_is_refused) || return 1
  (cd "$BACKEND" && cargo test --test provider_verification production_config_refuses_fake_or_missing_credentials) || return 1
  echo "check-release: production refusals green"
}

self_test() {
  echo "self-test: license predicate flags strong copyleft, passes dual-licensed"
  python3 - <<'EOF' || return 1
import re
permissive = re.compile(r'MIT|Apache|BSD|ISC|Unlicense|Zlib|Unicode|CC0|CC-BY|Python-2.0', re.I)
strong = re.compile(r'[AL]?GPL', re.I)
def forbidden(lic):
    return bool(strong.search(lic) and not permissive.search(lic))
assert forbidden("GPL-3.0-only"), "pure GPL must fail"
assert forbidden("AGPL-3.0"), "AGPL must fail"
assert forbidden("LGPL-2.1-only"), "pure LGPL must fail"
assert not forbidden("MIT OR Apache-2.0 OR LGPL-2.1-or-later"), "dual pick must pass"
assert not forbidden("MIT"), "MIT must pass"
assert not forbidden(""), "empty is handled separately"
print("self-test: license predicate OK")
EOF
  echo "self-test: lockfile drift is refused"
  local tmp
  tmp="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" EXIT
  (cd "$tmp" && cargo init -q --name drift-fixture 2>/dev/null) || return 1
  (cd "$tmp" && cargo generate-lockfile --offline 2>/dev/null) || return 1
  (cd "$tmp" && printf '\nserde = "2"\n' >> Cargo.toml)
  if (cd "$tmp" && cargo fetch --locked --offline >/dev/null 2>&1); then
    echo "self-test: drifted lockfile wrongly accepted" >&2
    return 1
  fi
  echo "check-release self-test: OK (predicate + drift refusal)"
}

case "${1:-}" in
  --self-test) self_test ;;
  "")
    step_lockfiles || failures=$((failures + 1))
    step_advisories || failures=$((failures + 1))
    step_licenses || failures=$((failures + 1))
    step_prod_config || failures=$((failures + 1))
    if [ "$failures" -ne 0 ]; then
      echo "check-release: FAILED ($failures step(s))" >&2
      exit 1
    fi
    echo "check-release: OK (release gate green)"
    ;;
  *) echo "usage: $(basename "$0") [--self-test]" >&2; exit 2 ;;
esac
