#!/usr/bin/env bash
#
# Smoke test against a local `wrangler dev` instance: /healthz, the uniform
# unauthenticated 401, and - given a token - the three authenticated read
# routes. Local-only: state lives in .wrangler/, never a deployed environment.
#
#   scripts/smoke.sh                                   healthz + 401 only
#   CABIN_REGISTRY_SMOKE_TOKEN=cabin_smoke scripts/smoke.sh   full run
#
# The token is seeded into the local D1 state before the checks, so any
# `cabin_...` value works.

set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/.."

port="${SMOKE_PORT:-8787}"
base="http://127.0.0.1:${port}"
token="${CABIN_REGISTRY_SMOKE_TOKEN:-}"
body="$(mktemp)"
dev_log="$(mktemp)"
dev_pid=""

cleanup() {
  # Kill the whole process group: $dev_pid is the backgrounded function's
  # wrapper subshell, and killing it alone would orphan npx/wrangler/workerd
  # and leave the port bound.
  [[ -n "$dev_pid" ]] && kill -- "-$dev_pid" 2>/dev/null || true
  rm -f "$body" "$dev_log"
}
trap cleanup EXIT

step() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

wrangler() { npx --yes wrangler "$@"; }

# check <path> <expected statuses...>; response body lands in $body.
check() {
  local path="$1"
  shift
  local status
  # ${arr[@]+...}: empty-array expansion trips `set -u` on macOS bash 3.2.
  status="$(curl -sS -o "$body" -w '%{http_code}' ${curl_args[@]+"${curl_args[@]}"} "$base$path")"
  for expected in "$@"; do
    [[ "$status" == "$expected" ]] && { printf '    %s -> %s\n' "$path" "$status"; return 0; }
  done
  fail "$path returned $status, expected one of: $* (body: $(cat "$body"))"
}

step "applying migrations to the local dev database"
wrangler d1 migrations apply DB --env dev --local

if [[ -n "$token" ]]; then
  step "seeding the smoke token into the local dev database"
  hash="$(printf '%s' "$token" | shasum -a 256 | cut -d' ' -f1)"
  wrangler d1 execute DB --env dev --local --command "
    INSERT OR IGNORE INTO users (github_id, login, created_at)
      VALUES (0, 'smoke', '1970-01-01T00:00:00Z');
    INSERT OR REPLACE INTO tokens (id, user_id, name, token_hash, scopes, created_at)
      VALUES ('smoke', 0, 'smoke', '${hash}', '', '1970-01-01T00:00:00Z');"
fi

# A stale server on the port would silently answer the checks below in place
# of the instance started here.
if curl -fsS -o /dev/null "$base/healthz" 2>/dev/null; then
  fail "something is already serving on port ${port}; kill it first"
fi

step "starting wrangler dev on port ${port} (first build takes a while)"
# Job control (-m) gives the dev-server tree its own process group so
# cleanup can kill all of it at once.
set -m
wrangler dev --env dev --port "$port" >"$dev_log" 2>&1 &
dev_pid=$!
set +m
disown "$dev_pid"

for _ in $(seq 1 300); do
  kill -0 "$dev_pid" 2>/dev/null || { cat "$dev_log" >&2; fail "wrangler dev exited early"; }
  curl -fsS -o /dev/null "$base/healthz" 2>/dev/null && break
  sleep 1
done

curl_args=()
step "healthz is unauthenticated and empty"
check /healthz 200
[[ -s "$body" ]] && fail "/healthz returned a body: $(cat "$body")"

step "data routes are a uniform 401 without a token"
check /config.json 401
expected_401='{"errors":[{"detail":"authentication required"}]}'
[[ "$(cat "$body")" == "$expected_401" ]] || fail "401 body mismatch: $(cat "$body")"
check /packages/smoke.json 401
[[ "$(cat "$body")" == "$expected_401" ]] || fail "401 body mismatch: $(cat "$body")"

if [[ -z "$token" ]]; then
  step "CABIN_REGISTRY_SMOKE_TOKEN not set; skipping authenticated checks"
  echo "smoke OK"
  exit 0
fi

curl_args=(-H "Authorization: Bearer $token")
step "authenticated read routes"
check /config.json 200
grep -q '"auth-required":true' "$body" || fail "config.json missing auth-required: $(cat "$body")"
# 200 only with previously published local data; 404 proves auth + routing.
check /packages/smoke.json 200 404
check /artifacts/smoke/smoke-0.1.0.tar.gz 200 404

step "authenticated responses carry the generation header"
curl -sS -o /dev/null -D "$body" ${curl_args[@]+"${curl_args[@]}"} "$base/config.json"
grep -qi '^x-cabin-registry-generation:' "$body" \
  || fail "missing x-cabin-registry-generation header"

echo "smoke OK"
