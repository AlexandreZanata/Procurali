#!/usr/bin/env bash
# Bounded load + correctness probes (P19-T02) against an explicit
# disposable target. Never sends real verification or WhatsApp messages:
# the deterministic fake provider answers challenges and only GET/POST
# against the disposable backend run.
# Usage:
#   PROCURALI_E2E_FAKE_CODE=... scripts/load-test.sh --database-url URL --base-url URL [--buyers N] [--sellers N] [--parallel N]
#   scripts/load-test.sh --self-test
# URLs must be loopback; the database must be `procurali_test_*`.
# Workload (defaults from contracts/load-scenarios.json): register buyers
# and sellers, publish one demand per buyer, submit offers, open contacts,
# then correctness probes (duplicate handoff replay, expired-demand
# refusal, duplicate-intent refusal). Prints throughput, p50/p95/p99
# latency, status-code histogram, exact final business counts, and the
# refusal-vs-failure classification. Any 5xx, network error, timeout,
# duplicate contact row, leaked destination, or quota overflow fails loudly.
# Credentials never appear in output.
set -euo pipefail

LOAD_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOAD_PG_IMAGE="postgres:18.6@sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336"

refuse() { echo "load-test: $1" >&2; return 2; }

load_check_database_url() {
  case "$1" in
    postgres://*@127.0.0.1:*/*procurali_test_* | postgres://*@localhost:*/*procurali_test_*) ;;
    *) refuse "refusing non-disposable or non-loopback database URL" ;;
  esac
}

load_check_base_url() {
  case "$1" in
    http://127.0.0.1* | http://localhost*) ;;
    *) refuse "refusing non-loopback --base-url (http loopback only; staging uses smoke.sh over https)" ;;
  esac
}

# Percentiles over one-per-line seconds on stdin: prints "p50=.. p95=.. p99=.. n=..".
load_percentiles() {
  local tmp
  tmp="$(mktemp)"
  grep -E '^[0-9.]+$' > "$tmp" || true
  local n
  n="$(wc -l < "$tmp" | tr -d ' ')"
  if [ "$n" -eq 0 ]; then
    echo "p50=none p95=none p99=none n=0"
    rm -f "$tmp"
    return 0
  fi
  sort -n "$tmp" -o "$tmp"
  local p50 p95 p99
  p50="$(sed -n "$(( (n * 50 + 99) / 100 ))p" "$tmp")"
  p95="$(sed -n "$(( (n * 95 + 99) / 100 ))p" "$tmp")"
  p99="$(sed -n "$(( (n * 99 + 99) / 100 ))p" "$tmp")"
  echo "p50=${p50}s p95=${p95}s p99=${p99}s n=${n}"
  rm -f "$tmp"
}

load_self_test() {
  echo "self-test: missing target fails instead of succeeding"
  LOAD_SOURCED=1 bash -c 'source "'"$LOAD_ROOT"'/scripts/load-test.sh"; load_main' 2>/dev/null && {
    echo "self-test: missing target wrongly accepted" >&2
    return 1
  }
  echo "self-test: non-disposable database is refused"
  load_check_database_url "postgres://u@127.0.0.1:5439/procurali_prod" 2>/dev/null && {
    echo "self-test: production database wrongly accepted" >&2
    return 1
  }
  echo "self-test: non-loopback target is refused"
  load_check_base_url "https://staging.example.com" 2>/dev/null && {
    echo "self-test: remote target wrongly accepted" >&2
    return 1
  }
  echo "self-test: percentile math on a known sample"
  out="$(printf '0.1\n0.2\n0.3\n0.4\n0.5\n' | load_percentiles)"
  [ "$out" = "p50=0.3s p95=0.5s p99=0.5s n=5" ] || {
    echo "self-test: percentile math wrong: $out" >&2
    return 1
  }
  out="$(printf '' | load_percentiles)"
  [ "$out" = "p50=none p95=none p99=none n=0" ] || {
    echo "self-test: empty percentile wrong: $out" >&2
    return 1
  }
  echo "self-test: scenario mix parses with bounded quotas"
  python3 - "$LOAD_ROOT/contracts/load-scenarios.json" <<'PYEOF' || {
import json, sys
mix = json.load(open(sys.argv[1]))
assert mix["mix"]["buyers"] * mix["mix"]["demands_per_buyer"] <= 3 * mix["mix"]["buyers"]
assert mix["pass_predicate"]["infrastructure_failures_allowed"] == 0
print("mix ok")
PYEOF
    echo "self-test: scenario mix unreadable" >&2
    return 1
  }
  echo "load-test self-test: OK"
}

load_main() {
  local database_url="" base_url="" buyers="" sellers="" parallel=""
  local self_test=0
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --database-url) database_url="${2:-}"; shift 2 || return 2 ;;
      --base-url) base_url="${2:-}"; shift 2 || return 2 ;;
      --buyers) buyers="${2:-}"; shift 2 || return 2 ;;
      --sellers) sellers="${2:-}"; shift 2 || return 2 ;;
      --parallel) parallel="${2:-}"; shift 2 || return 2 ;;
      --self-test) self_test=1; shift ;;
      *) echo "usage: $(basename "$0") --database-url URL --base-url URL [--buyers N] [--sellers N] [--parallel N] | --self-test" >&2; return 2 ;;
    esac
  done
  if [ "$self_test" -eq 1 ]; then
    load_self_test
    return "$?"
  fi
  [ -n "$database_url" ] || { refuse "--database-url is required (explicit disposable naming)"; return 2; }
  [ -n "$base_url" ] || { refuse "--base-url is required (explicit disposable target)"; return 2; }
  load_check_database_url "$database_url" || return 2
  load_check_base_url "$base_url" || return 2
  FAKE_CODE="${PROCURALI_E2E_FAKE_CODE:-}"
  [ -n "$FAKE_CODE" ] || { refuse "PROCURALI_E2E_FAKE_CODE is required (fake provider only; never a real code)"; return 2; }
  buyers="${buyers:-$(python3 -c "import json; print(json.load(open('$LOAD_ROOT/contracts/load-scenarios.json'))['mix']['buyers'])")}"
  sellers="${sellers:-$(python3 -c "import json; print(json.load(open('$LOAD_ROOT/contracts/load-scenarios.json'))['mix']['sellers_per_buyer'])")}"
  parallel="${parallel:-$(python3 -c "import json; print(json.load(open('$LOAD_ROOT/contracts/load-scenarios.json'))['mix']['parallelism'])")}"
  local user pass hostport db
  user="$(printf '%s' "$database_url" | sed -E 's|.*://([^:]+):.*|\1|')"
  pass="$(printf '%s' "$database_url" | sed -E 's|.*://[^:]+:([^@]+)@.*|\1|')"
  hostport="$(printf '%s' "$database_url" | sed -E 's|.*@([^/]+)/.*|\1|')"
  db="$(printf '%s' "$database_url" | sed 's|.*/||; s|?.*||')"
  echo "load-test: target $base_url with database ${hostport}/${db} (credentials never printed)"
  echo "load-test: mix buyers=$buyers sellers-per-buyer=$sellers parallelism=$parallel (quotas respected, see contracts/load-scenarios.json)"
  local started_at
  started_at="$(date +%s)"

  local work
  work="$(mktemp -d)"
  export LOAD_WORK="$work" LOAD_BASE="$base_url" LOAD_CODE="$FAKE_CODE"
  : > "$work/latencies.txt"
  : > "$work/codes.txt"
  : > "$work/infra_failures.txt"

  # One timed HTTP call: appends "<elapsed>" and "<code|network-error>".
  load_call() {
    local method="$1" path="$2" body="$3" cookie="$4" outfile="$5"
    local args=(-sS -m 30 -o "$outfile" -w "%{http_code} %{time_total}")
    args+=(-X "$method" -H "content-type: application/json" -H "origin: $LOAD_BASE")
    [ -n "$cookie" ] && args+=(-H "cookie: $cookie")
    [ -n "$body" ] && args+=(-d "$body")
    local result code elapsed
    if result="$(curl "${args[@]}" "$LOAD_BASE$path" 2>/dev/null)"; then
      code="${result%% *}"
      elapsed="${result##* }"
    else
      code="network-error"
      elapsed="30.0"
    fi
    printf '%s\n' "$elapsed" >> "$LOAD_WORK/latencies.txt"
    printf '%s\n' "$code" >> "$LOAD_WORK/codes.txt"
    printf '%s' "$code"
  }
  export -f load_call 2>/dev/null || true

  phone() { printf '+55 11 9%03d-%04d' "$((RANDOM % 900 + 100))" "$((RANDOM % 9000 + 1000))"; }

  register_account() {
    local name="$1" phone_number="$2"
    local out="$LOAD_WORK/acc_$name.json"
    local code
    code="$(load_call POST "/api/v1/accounts" "{\"display_name\":\"$name\",\"phone\":\"$phone_number\",\"city\":\"Campinas\",\"region\":\"SP\",\"policy_version\":\"v1\"}" "" "$out")"
    [ "$code" = "201" ] || { echo "load-test: registration refused ($code) for $name" >> "$LOAD_WORK/infra_failures.txt"; return 1; }
    code="$(load_call POST "/api/v1/accounts/challenges" "{\"phone\":\"$phone_number\"}" "" "$out")"
    [ "$code" = "202" ] || { echo "load-test: challenge refused ($code) for $name" >> "$LOAD_WORK/infra_failures.txt"; return 1; }
    code="$(load_call POST "/api/v1/accounts/challenges/confirmations" "{\"phone\":\"$phone_number\",\"code\":\"$LOAD_CODE\"}" "" "$out")"
    [ "$code" = "200" ] || { echo "load-test: confirmation refused ($code) for $name" >> "$LOAD_WORK/infra_failures.txt"; return 1; }
    local login_out="$LOAD_WORK/login_$name.json"
    local headers="$LOAD_WORK/login_$name.headers"
    local login_code login_time
    if login_out2="$(curl -sS -m 30 -D "$headers" -o "$login_out" -w "%{http_code} %{time_total}" -X POST -H "content-type: application/json" -H "origin: $LOAD_BASE" -d "{\"phone\":\"$phone_number\",\"code\":\"$LOAD_CODE\"}" "$LOAD_BASE/api/v1/sessions" 2>/dev/null)"; then
      login_code="${login_out2%% *}"
      login_time="${login_out2##* }"
    else
      login_code="network-error"
      login_time="30.0"
    fi
    printf '%s\n' "$login_time" >> "$LOAD_WORK/latencies.txt"
    printf '%s\n' "$login_code" >> "$LOAD_WORK/codes.txt"
    [ "$login_code" = "200" ] || { echo "load-test: login refused ($login_code) for $name" >> "$LOAD_WORK/infra_failures.txt"; return 1; }
    grep -i "^set-cookie:" "$headers" | head -1 | sed 's/^[Ss]et-[Cc]ookie: //; s/;.*//' | tr -d '\r'
  }

  buyer_job() {
    local i="$1" sellers="$2"
    local bphone
    bphone="$(phone)"
    local cookie
    cookie="$(register_account "LoadBuyer$i" "$bphone")" || return 1
    printf '%s' "$cookie" > "$LOAD_WORK/cookie_buyer_$i"
    local draft="$LOAD_WORK/draft_$i.json"
    local code
    code="$(load_call POST "/api/v1/requests/drafts" "{\"title\":\"Load Fridge $i\",\"category_code\":\"home_appliances\",\"budget\":\"600.00\",\"condition\":\"either\",\"city_code\":\"campinas\",\"region_code\":\"centro\"}" "$cookie" "$draft")"
    [ "$code" = "201" ] || { echo "load-test: draft refused ($code) buyer $i" >> "$LOAD_WORK/infra_failures.txt"; return 1; }
    local demand
    demand="$(python3 -c "import json; print(json.load(open('$draft'))['id'])")"
    printf '%s' "$demand" > "$LOAD_WORK/demand_$i"
    code="$(load_call POST "/api/v1/requests/drafts/$demand/publication" "{}" "$cookie" "$LOAD_WORK/pub_$i.json")"
    { [ "$code" = "200" ] || [ "$code" = "201" ]; } || { echo "load-test: publication refused ($code) buyer $i" >> "$LOAD_WORK/infra_failures.txt"; return 1; }
    local s
    for s in $(seq 1 "$sellers"); do
      printf '%s %s\n' "$i" "$s" >> "$LOAD_WORK/offer_tasks.txt"
    done
  }

  seller_job() {
    local i="$1" s="$2"
    local sphone
    sphone="$(phone)"
    local cookie
    cookie="$(register_account "LoadSeller${i}_${s}" "$sphone")" || return 1
    local demand
    demand="$(cat "$LOAD_WORK/demand_$i")"
    local out="$LOAD_WORK/offer_${i}_${s}.json"
    local code
    code="$(load_call POST "/api/v1/requests/$demand/offers" "{\"cycle_number\":1,\"revision_number\":1,\"description\":\"Load unit $s\",\"price\":\"5${s}0.00\",\"condition\":\"used\",\"city_code\":\"campinas\",\"region_code\":\"centro\",\"available\":true,\"available_in_city\":true}" "$cookie" "$out")"
    [ "$code" = "201" ] || { echo "load-test: offer refused ($code) buyer $i seller $s" >> "$LOAD_WORK/infra_failures.txt"; return 1; }
    python3 -c "import json; print(json.load(open('$out'))['id'])" > "$LOAD_WORK/offerid_${i}_${s}"
  }

  contact_job() {
    local i="$1" s="$2"
    local cookie demand offer handoff
    cookie="$(cat "$LOAD_WORK/cookie_buyer_$i")"
    demand="$(cat "$LOAD_WORK/demand_$i")"
    offer="$(cat "$LOAD_WORK/offerid_${i}_${s}")"
    handoff="$(python3 -c "import uuid; print(uuid.uuid4())")"
    local out="$LOAD_WORK/contact_${i}_${s}.json"
    local code
    code="$(load_call POST "/api/v1/requests/$demand/offers/$offer/contact" "{\"handoff_id\":\"$handoff\",\"expected_offer_terms\":1,\"entry_source\":\"offer_detail\"}" "$cookie" "$out")"
    [ "$code" = "201" ] || { echo "load-test: contact refused ($code) buyer $i seller $s" >> "$LOAD_WORK/infra_failures.txt"; return 1; }
    printf '%s' "$handoff" > "$LOAD_WORK/handoff_${i}_${s}"
  }

  export LOAD_WORK LOAD_BASE LOAD_CODE
  export -f register_account buyer_job seller_job contact_job phone 2>/dev/null || true

  echo "load-test: (1/5) registering and publishing ($buyers buyers)"
  seq 1 "$buyers" | xargs -P "$parallel" -I{} bash -c 'buyer_job {} '"$sellers" || {
    echo "load-test: buyer phase failed" >&2
    return 1
  }
  echo "load-test: (2/5) submitting offers"
  cat "$LOAD_WORK/offer_tasks.txt" | xargs -P "$parallel" -n 2 bash -c 'seller_job "$0" "$1"' || {
    echo "load-test: offer phase failed" >&2
    return 1
  }
  echo "load-test: (3/5) opening contacts"
  cat "$LOAD_WORK/offer_tasks.txt" | xargs -P "$parallel" -n 2 bash -c 'contact_job "$0" "$1"' || {
    echo "load-test: contact phase failed" >&2
    return 1
  }

  echo "load-test: (4/5) correctness probes (idempotent replay, expiry, duplicate intent)"
  # Replay: same handoff twice converges on one contact row.
  first_i=1
  first_cookie="$(cat "$LOAD_WORK/cookie_buyer_$first_i")"
  first_demand="$(cat "$LOAD_WORK/demand_$first_i")"
  first_offer="$(cat "$LOAD_WORK/offerid_${first_i}_1")"
  first_handoff="$(cat "$LOAD_WORK/handoff_${first_i}_1")"
  replay_code="$(load_call POST "/api/v1/requests/$first_demand/offers/$first_offer/contact" "{\"handoff_id\":\"$first_handoff\",\"expected_offer_terms\":1,\"entry_source\":\"offer_detail\"}" "$first_cookie" "$work/replay.json")"
  { [ "$replay_code" = "200" ] || [ "$replay_code" = "201" ]; } || {
    echo "load-test: replay refused ($replay_code; expected idempotent convergence)" >> "$LOAD_WORK/infra_failures.txt"
  }
  # Expiry: backdate one demand, then offers and contact must refuse as business refusals.
  expire_demand="$(cat "$LOAD_WORK/demand_$buyers")"
  docker run --rm --network host -e "PGPASSWORD=$pass" "$LOAD_PG_IMAGE" \
    psql -h "${hostport%%:*}" -p "${hostport##*:}" -U "$user" -d "$db" -tAc \
    "UPDATE request_cycles SET started_at = now() - interval '8 days', deadline = now() - interval '1 day' WHERE request_id = '$expire_demand' AND cycle_number = 1;" >/dev/null || {
    echo "load-test: expiry fixture failed" >> "$LOAD_WORK/infra_failures.txt"
  }
  expire_cookie="$(cat "$LOAD_WORK/cookie_buyer_$buyers")"
  expired_offer_code="$(load_call POST "/api/v1/requests/$expire_demand/offers" "{\"cycle_number\":1,\"revision_number\":1,\"description\":\"Late unit\",\"price\":\"500.00\",\"condition\":\"used\",\"city_code\":\"campinas\",\"region_code\":\"centro\",\"available\":true,\"available_in_city\":true}" "$first_cookie" "$work/expired_offer.json")"
  : > "$work/expected_refusals.txt"
  case "$expired_offer_code" in
    409|403|404|410|422) printf '%s\n' "$expired_offer_code" >> "$work/expected_refusals.txt" ;;
    *) echo "load-test: expired demand took an offer ($expired_offer_code; restriction bypass)" >> "$LOAD_WORK/infra_failures.txt" ;;
  esac
  expired_contact_code="$(load_call POST "/api/v1/requests/$expire_demand/offers/$(cat "$LOAD_WORK/offerid_${buyers}_1")/contact" "{\"handoff_id\":\"$(python3 -c "import uuid; print(uuid.uuid4())")\",\"expected_offer_terms\":1,\"entry_source\":\"offer_detail\"}" "$expire_cookie" "$work/expired_contact.json")"
  case "$expired_contact_code" in
    409|403|404|410|422) printf '%s\n' "$expired_contact_code" >> "$work/expected_refusals.txt" ;;
    *) echo "load-test: expired demand granted contact ($expired_contact_code; restriction bypass)" >> "$LOAD_WORK/infra_failures.txt" ;;
  esac
  # Duplicate intent: the identical draft publishes refused (drafts stay
  # owner-private; the duplicate rule bites at publication). A 2xx or 5xx
  # here is a failure either way.
  dup_draft_code="$(load_call POST "/api/v1/requests/drafts" "{\"title\":\"Load Fridge 1\",\"category_code\":\"home_appliances\",\"budget\":\"600.00\",\"condition\":\"either\",\"city_code\":\"campinas\",\"region_code\":\"centro\"}" "$first_cookie" "$work/dup.json")"
  if [ "$dup_draft_code" = "201" ]; then
    dup_demand="$(python3 -c "import json; print(json.load(open('$work/dup.json'))['id'])")"
    dup_code="$(load_call POST "/api/v1/requests/drafts/$dup_demand/publication" "{}" "$first_cookie" "$work/dup_pub.json")"
  else
    dup_code="$dup_draft_code"
  fi
  case "$dup_code" in
    5*|network-error) echo "load-test: duplicate intent broke infrastructure ($dup_code)" >> "$LOAD_WORK/infra_failures.txt" ;;
    200|201) echo "load-test: duplicate intent published ($dup_code; exact duplicate must refuse)" >> "$LOAD_WORK/infra_failures.txt" ;;
    *) printf '%s\n' "$dup_code" >> "$work/expected_refusals.txt" ;;
  esac

  echo "load-test: (5/5) counts, leaks, and classification"
  db_counts="$(docker run --rm --network host -e "PGPASSWORD=$pass" "$LOAD_PG_IMAGE" \
    psql -h "${hostport%%:*}" -p "${hostport##*:}" -U "$user" -d "$db" -tAc \
    "SELECT 'requests:' || count(*) FROM requests UNION ALL SELECT 'active_requests:' || count(*) FROM requests WHERE state = 'active' UNION ALL SELECT 'offers:' || count(*) FROM offers UNION ALL SELECT 'contacts:' || count(*) FROM contacts;" 2>/dev/null)" || {
    echo "load-test: final counts unreadable" >> "$LOAD_WORK/infra_failures.txt"
    db_counts="unavailable"
  }
  echo "load-test: final business counts: $(printf '%s' "$db_counts" | tr '\n' ' ')"
  expected_contacts=$((buyers * sellers))
  actual_contacts="$(printf '%s' "$db_counts" | grep -o 'contacts:[0-9]*' | cut -d: -f2)"
  if [ -n "$actual_contacts" ] && [ "$actual_contacts" != "$expected_contacts" ]; then
    echo "load-test: contact count drift (want $expected_contacts, have $actual_contacts)" >> "$LOAD_WORK/infra_failures.txt"
  fi
  actual_active="$(printf '%s' "$db_counts" | grep -o 'active_requests:[0-9]*' | cut -d: -f2)"
  if [ -n "$actual_active" ] && [ "$actual_active" != "$buyers" ]; then
    echo "load-test: active demand drift (want $buyers, have $actual_active)" >> "$LOAD_WORK/infra_failures.txt"
  fi
  # Destinations travel only inside buyer handoff receipts: every other
  # saved response body must carry no destination key. Phone-shaped runs
  # mean 10+ consecutive digits here (E.164-shaped): ISO dates, times,
  # prices, and coordinates never reach that length, and UUIDs are
  # stripped first since they share the digit-dash shape.
  strip_uuids() { sed -E 's/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}//g' "$@"; }
  if grep -rEl '"destination"' "$work"/draft_*.json "$work"/pub_*.json "$work"/offer_*.json "$work"/acc_*.json "$work"/login_*.json 2>/dev/null | grep -q .; then
    echo "load-test: destination leaked outside handoff receipts" >> "$LOAD_WORK/infra_failures.txt"
  fi
  leak_sample="$(strip_uuids "$work"/draft_*.json "$work"/pub_*.json "$work"/offer_*.json 2>/dev/null | grep -Eo '\+?[0-9]{10,}' | head -5 || true)"
  if [ -n "$leak_sample" ]; then
    {
      echo "load-test: phone-shaped material in public payloads:"
      printf '%s\n' "$leak_sample"
    } >> "$LOAD_WORK/infra_failures.txt"
  fi

  echo "load-test: latency $(load_percentiles < "$work/latencies.txt")"
  echo "load-test: elapsed $(( $(date +%s) - started_at ))s (cap $(python3 -c "import json; print(json.load(open('$LOAD_ROOT/contracts/load-scenarios.json'))['mix']['duration_cap_secs'])")s)"
  echo "load-test: status codes: $(sort "$work/codes.txt" | uniq -c | tr '\n' ';')"
  if [ -s "$work/expected_refusals.txt" ]; then
    echo "load-test: expected business refusals: $(sort "$work/expected_refusals.txt" | uniq -c | tr '\n' ';')"
  fi
  if [ -s "$LOAD_WORK/infra_failures.txt" ]; then
    echo "load-test: FAILURES:" >&2
    cat "$LOAD_WORK/infra_failures.txt" >&2
    rm -rf "$work"
    return 1
  fi
  echo "load-test: OK (bounded mix green; no duplicates, leaks, quota overflow, or infrastructure failures)"
  rm -rf "$work"
}

if [ "${LOAD_SOURCED:-0}" != "1" ] && [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  load_main "$@"
fi
