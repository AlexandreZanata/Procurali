#!/usr/bin/env bash
# Shared guards for local Docker stacks. Sourced by scripts/infra.sh only.
# Every refusal names the offending setting without printing secret values.
# URLs are validated by shape (scheme, loopback host, disposable database name);
# credentials never appear in messages — only host:port/database labels.

# Refuse non-local environments for local-stack commands.
# Usage: procurali_require_local_env <command>
procurali_require_local_env() {
  local command="$1"
  local env="${PROCURALI_ENV:-development}"
  case "$env" in
    development | test) return 0 ;;
    *)
      echo "infra: refusing '$command' with PROCURALI_ENV='$env' (local stacks only)" >&2
      return 1
      ;;
  esac
}

# Split a postgres URL into user, host, port, dbname globals (no secret printed).
# Usage: procurali_split_url <url>; reads PROCURALI_URL_USER/HOST/PORT/DB on success.
procurali_split_url() {
  local url="$1"
  case "$url" in
    postgres://* | postgresql://*) ;;
    *) return 1 ;;
  esac
  local rest="${url#*://}"
  local userinfo="${rest%%@*}"
  PROCURALI_URL_USER="${userinfo%%:*}"
  local hostpart="${rest#*@}"
  [ "$hostpart" != "$rest" ] || return 1
  local hostport="${hostpart%%/*}"
  local dbname="${hostpart#*/}"
  dbname="${dbname%%\?*}"
  [ -n "$dbname" ] || return 1
  PROCURALI_URL_HOST="${hostport%%:*}"
  PROCURALI_URL_PORT="${hostport#*:}"
  if [ "$PROCURALI_URL_PORT" = "$hostport" ]; then
    PROCURALI_URL_PORT="5432"
  fi
  PROCURALI_URL_DB="$dbname"
  [ -n "$PROCURALI_URL_USER" ] && [ -n "$PROCURALI_URL_HOST" ] || return 1
  return 0
}

# Redacted label for messages: host:port/database (never user, password, or URL).
# Usage: procurali_db_label <url>
procurali_db_label() {
  procurali_split_url "$1" || return 1
  printf '%s:%s/%s' "$PROCURALI_URL_HOST" "$PROCURALI_URL_PORT" "$PROCURALI_URL_DB"
}

# Require an explicit disposable TEST_DATABASE_URL that matches the test service
# identity (user/port) and differs from the application DATABASE_URL.
# Usage: procurali_require_disposable_url <expected_user> <expected_port>
procurali_require_disposable_url() {
  local expected_user="$1"
  local expected_port="$2"
  local url="${TEST_DATABASE_URL:-}"
  if [ -z "$url" ]; then
    echo "infra: TEST_DATABASE_URL is required (explicit disposable naming)" >&2
    return 1
  fi
  procurali_split_url "$url" || {
    echo "infra: TEST_DATABASE_URL must use a postgres:// URL with user@host/database" >&2
    return 1
  }
  case "$PROCURALI_URL_DB" in
    procurali_test_*) ;;
    *)
      echo "infra: refusing non-disposable database '$PROCURALI_URL_DB' (must start with procurali_test_)" >&2
      return 1
      ;;
  esac
  case "$PROCURALI_URL_HOST" in
    127.0.0.1 | localhost)
      ;;
    *)
      echo "infra: refusing non-loopback test host '$PROCURALI_URL_HOST'" >&2
      return 1
      ;;
  esac
  if [ "$PROCURALI_URL_USER" != "$expected_user" ] || [ "$PROCURALI_URL_PORT" != "$expected_port" ]; then
    echo "infra: TEST_DATABASE_URL does not match the test service identity ($expected_user@127.0.0.1:$expected_port/<procurali_test_* database>)" >&2
    return 1
  fi
  if [ -n "${DATABASE_URL:-}" ] && [ "$url" = "$DATABASE_URL" ]; then
    echo "infra: TEST_DATABASE_URL must differ from DATABASE_URL" >&2
    return 1
  fi
  return 0
}

# Validate a test project suffix: explicit, safe charset, never dev/prod-like.
# Usage: procurali_require_suffix <suffix>
procurali_require_suffix() {
  local suffix="$1"
  if [ -z "$suffix" ]; then
    echo "infra: an explicit --suffix is required (no default destructive target)" >&2
    return 1
  fi
  case "$suffix" in
    *[!A-Za-z0-9_-]*)
      echo "infra: refusing suffix '$suffix' (allowed: letters, digits, -, _)" >&2
      return 1
      ;;
  esac
  case "$suffix" in
    dev | development | prod | production | staging | main | test)
      echo "infra: refusing environment-like suffix '$suffix'" >&2
      return 1
      ;;
  esac
  return 0
}

# Bounded readiness probe against a Compose project database.
# Usage: procurali_wait_postgres <project> <user> <db> <timeout_secs>
# Prints only a redacted host label on failure.
procurali_wait_postgres() {
  local project="$1"
  local user="$2"
  local db="$3"
  local timeout_secs="$4"
  local waited=0
  while [ "$waited" -lt "$timeout_secs" ]; do
    if docker compose -p "$project" exec -T db pg_isready -U "$user" -d "$db" >/dev/null 2>&1; then
      echo "infra: database ready in project '$project'"
      return 0
    fi
    sleep 2
    waited=$((waited + 2))
  done
  echo "infra: database unreachable in project '$project' after ${timeout_secs}s (user/db withheld)" >&2
  return 1
}
