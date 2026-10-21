#!/usr/bin/env bash
# Operational smoke probe (P18-T06): current schema plus minimal
# authenticated/public flow, with no uncontrolled messages.
# Usage:
#   scripts/smoke.sh --database-url URL [--base-url URL]
#   scripts/smoke.sh --self-test
# --database-url must be disposable (`procurali_test_*`) on loopback; the
# schema check compares applied `_sqlx_migrations` versions against
# `backend/migrations/*.sql` exactly. --base-url (optional, loopback only)
# adds live HTTP probes: liveness, readiness, one public catalog read, and
# one cookie-less staff-metrics read that must answer 401. Only GET is ever
# sent: no verification, contact, or WhatsApp message can fire.
# Without --base-url the HTTP flow reports PENDING (exit 2) instead of
# inventing success: the caller has to name the served target. Credentials
# never appear in output.
set -euo pipefail

SMOKE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

refuse() { echo "smoke: $1" >&2; return 2; }

smoke_check_database_url() {
  case "$1" in
    postgres://*@127.0.0.1:*/*procurali_test_* | postgres://*@localhost:*/*procurali_test_*) ;;
    *) refuse "refusing non-disposable or non-loopback database URL" ;;
  esac
}

smoke_check_base_url() {
  case "$1" in
    http://127.0.0.1* | http://localhost* | https://127.0.0.1* | https://localhost*) ;;
    *) refuse "refusing non-loopback --base-url" ;;
  esac
}

# Live HTTP probes against one loopback base URL. GET only. Fails loudly on
# any unreachable endpoint, wrong status, ready-absence, phone-shaped public
# material, or a staff boundary that does not refuse cookie-less reads.
smoke_probe_http() {
  local base_url="$1"
  local body
  body="$(mktemp)"
  local code
  code="$(curl -sS -m 10 -o "$body" -w "%{http_code}" "$base_url/health/live")" || {
    echo "smoke: liveness unreachable (failed check, not success)" >&2
    rm -f "$body"
    return 1
  }
  if [ "$code" != "200" ] || ! grep -q '"alive"' "$body"; then
    echo "smoke: liveness not alive (code $code; failed check, not success)" >&2
    rm -f "$body"
    return 1
  fi
  echo "smoke: liveness alive"
  code="$(curl -sS -m 10 -o "$body" -w "%{http_code}" "$base_url/health/ready")" || {
    echo "smoke: readiness unreachable (failed check, not success)" >&2
    rm -f "$body"
    return 1
  }
  if [ "$code" != "200" ] || ! grep -q '"ready"' "$body"; then
    echo "smoke: server not ready (code $code; failed check, not success)" >&2
    rm -f "$body"
    return 1
  fi
  echo "smoke: readiness ready"
  code="$(curl -sS -m 10 -o "$body" -w "%{http_code}" "$base_url/api/v1/catalogs/cities")" || {
    echo "smoke: public catalog unreachable (failed check, not success)" >&2
    rm -f "$body"
    return 1
  }
  if [ "$code" != "200" ]; then
    echo "smoke: public catalog not public (code $code; failed check, not success)" >&2
    rm -f "$body"
    return 1
  fi
  if grep -Eq '"phone"|\+?[0-9][0-9 ./-]{7,}[0-9]' "$body"; then
    echo "smoke: public body carries phone-shaped material (failed check, not success)" >&2
    rm -f "$body"
    return 1
  fi
  echo "smoke: public catalog readable with no phone-shaped material"
  code="$(curl -sS -m 10 -o "$body" -w "%{http_code}" "$base_url/api/v1/metrics/operational?purpose=smoke")" || {
    echo "smoke: staff boundary unreachable (failed check, not success)" >&2
    rm -f "$body"
    return 1
  }
  if [ "$code" != "401" ]; then
    echo "smoke: cookie-less staff read not refused (code $code; failed check, not success)" >&2
    rm -f "$body"
    return 1
  fi
  echo "smoke: cookie-less staff read refused with 401 (no message sent)"
  rm -f "$body"
}

# Pinned image for database probes (same pin as the Compose stacks).
SMOKE_PG_IMAGE="postgres:18.6@sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336"

# Schema agreement on one disposable database URL: applied versions must equal
# the tracked migration files exactly. Prints host:port/database only.
smoke_check_schema() {
  local url="$1"
  local user pass hostport db
  user="$(printf '%s' "$url" | sed -E 's|.*://([^:]+):.*|\1|')"
  pass="$(printf '%s' "$url" | sed -E 's|.*://[^:]+:([^@]+)@.*|\1|')"
  hostport="$(printf '%s' "$url" | sed -E 's|.*@([^/]+)/.*|\1|')"
  db="$(printf '%s' "$url" | sed 's|.*/||; s|?.*||')"
  echo "smoke: database ${hostport}/${db} (credentials never printed)"
  local applied
  applied="$(docker run --rm --network host -e "PGPASSWORD=$pass" "$SMOKE_PG_IMAGE" \
    psql -h "${hostport%%:*}" -p "${hostport##*:}" -U "$user" -d "$db" -tAc \
    "SELECT version FROM _sqlx_migrations ORDER BY version;" 2>/dev/null)" || {
    echo "smoke: database unreachable or unmigrated (failed check, not success)" >&2
    return 1
  }
  local tracked
  tracked="$(for f in "$SMOKE_ROOT"/backend/migrations/*.sql; do basename "$f" .sql | cut -d_ -f1 | sed 's/^0*//'; done | paste -sd, -)"
  local applied_csv
  applied_csv="$(printf '%s' "$applied" | sed 's/^0*//' | paste -sd, -)"
  if [ "$applied_csv" != "$tracked" ]; then
    echo "smoke: migration mismatch (applied vs tracked differ; failed check, not success)" >&2
    return 1
  fi
  echo "smoke: schema agrees ($(printf '%s' "$applied" | grep -c . || true) applied migrations match tracked files)"
}

smoke_self_test() {
  echo "self-test: missing database URL fails instead of succeeding"
  SMOKE_SOURCED=1 bash -c 'source "'"$SMOKE_ROOT"'/scripts/smoke.sh"; smoke_main' 2>/dev/null && {
    echo "self-test: missing url wrongly accepted" >&2
    return 1
  }
  echo "self-test: non-disposable database is refused"
  smoke_check_database_url "postgres://u@127.0.0.1:5439/procurali_prod" 2>/dev/null && {
    echo "self-test: production database wrongly accepted" >&2
    return 1
  }
  echo "self-test: non-loopback host is refused"
  smoke_check_database_url "postgres://u@db.example.com:5439/procurali_test_x" 2>/dev/null && {
    echo "self-test: remote host wrongly accepted" >&2
    return 1
  }
  echo "self-test: non-loopback base URL is refused"
  smoke_check_base_url "https://staging.example.com" 2>/dev/null && {
    echo "self-test: remote base URL wrongly accepted" >&2
    return 1
  }
  echo "self-test: probe logic against a stub (script behavior only, not product evidence)"
  local stub
  stub="$(mktemp).py"
  cat > "$stub" <<'PYEOF'
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/health/live":
            body, code = {"status": "alive", "service": "stub"}, 200
        elif self.path == "/health/ready":
            body, code = {"status": "ready", "unavailable": []}, 200
        elif self.path == "/api/v1/catalogs/cities":
            body, code = {"cities": []}, 200
        elif self.path.startswith("/api/v1/metrics/operational"):
            body, code = {"code": "unauthenticated"}, 401
        else:
            body, code = {"code": "not_found"}, 404
        payload = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)
    def log_message(self, *args):
        pass
HTTPServer(("127.0.0.1", 18091), Stub).serve_forever()
PYEOF
  python3 "$stub" & local stub_pid=$!
  sleep 1
  smoke_probe_http "http://127.0.0.1:18091" || {
    echo "self-test: healthy stub wrongly rejected" >&2
    kill "$stub_pid" 2>/dev/null || true
    rm -f "$stub"
    return 1
  }
  kill "$stub_pid" 2>/dev/null || true
  rm -f "$stub"
  echo "self-test: unreachable server fails instead of succeeding"
  smoke_probe_http "http://127.0.0.1:18092" 2>/dev/null && {
    echo "self-test: unreachable server wrongly accepted" >&2
    return 1
  }
  echo "smoke self-test: OK"
}

smoke_main() {
  local database_url="" base_url="" self_test=0
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --database-url) database_url="${2:-}"; shift 2 || return 2 ;;
      --base-url) base_url="${2:-}"; shift 2 || return 2 ;;
      --self-test) self_test=1; shift ;;
      *) echo "usage: $(basename "$0") --database-url URL [--base-url URL] | --self-test" >&2; return 2 ;;
    esac
  done
  if [ "$self_test" -eq 1 ]; then
    smoke_self_test
    return "$?"
  fi
  [ -n "$database_url" ] || { refuse "--database-url is required (explicit disposable naming)"; return 2; }
  smoke_check_database_url "$database_url" || return 2
  echo "smoke: (1/2) schema agreement (applied migrations vs tracked files)"
  smoke_check_schema "$database_url" || return 1
  if [ -z "$base_url" ]; then
    echo "smoke: (2/2) HTTP flow: PENDING (no --base-url; the served target must be named)"
    return 2
  fi
  smoke_check_base_url "$base_url" || return 2
  echo "smoke: (2/2) live HTTP probes (GET only; no message can fire)"
  smoke_probe_http "$base_url" || return 1
  echo "smoke: OK (schema agrees; minimal public/authenticated flow validated)"
}

if [ "${SMOKE_SOURCED:-0}" != "1" ] && [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  cd "$SMOKE_ROOT"
  smoke_main "$@"
fi
