#!/usr/bin/env bash
#
# Smoke test against a local `wrangler dev` instance: /healthz, the uniform
# unauthenticated 401, the unauthenticated /me -> /login redirect, and -
# given a token - the three authenticated read routes plus the full
# publish / yank write flow (first publish, idempotent re-publish,
# immutability conflict, yank state transitions, artifact checksum).
# Local-only: state lives in .wrangler/, never a deployed environment.
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

# request <method> <path> <data-file> <expected statuses...>; body in $body.
request() {
  local method="$1" path="$2" data="$3"
  shift 3
  local status
  status="$(curl -sS -o "$body" -w '%{http_code}' -X "$method" --data-binary "@$data" \
    ${curl_args[@]+"${curl_args[@]}"} "$base$path")"
  for expected in "$@"; do
    [[ "$status" == "$expected" ]] && { printf '    %s %s -> %s\n' "$method" "$path" "$status"; return 0; }
  done
  fail "$method $path returned $status, expected one of: $* (body: $(cat "$body"))"
}

# expect_body <fixed string>: the last response body must contain it.
expect_body() {
  grep -qF "$1" "$body" || fail "response body missing $1: $(cat "$body")"
}

# Writes <n> to stdout as a u32 little-endian. Emitted directly (never via
# command substitution): the length bytes are usually NULs.
u32le() {
  local n="$1"
  # shellcheck disable=SC2059
  printf "$(printf '\\%03o\\%03o\\%03o\\%03o' \
    $((n & 255)) $(((n >> 8) & 255)) $(((n >> 16) & 255)) $(((n >> 24) & 255)))"
}

# frame <metadata-file> <archive-file> <out>: the publish body framing,
# [u32 LE metadata_len][metadata][u32 LE archive_len][archive].
frame() {
  local metadata="$1" archive="$2" out="$3"
  {
    u32le "$(wc -c <"$metadata")"
    cat "$metadata"
    u32le "$(wc -c <"$archive")"
    cat "$archive"
  } >"$out"
}

step "applying migrations to the local dev database"
wrangler d1 migrations apply DB --env dev --local

if [[ -n "$token" ]]; then
  step "seeding the smoke token into the local dev database"
  hash="$(printf '%s' "$token" | shasum -a 256 | cut -d' ' -f1)"
  # The fixture rows are cleared so re-runs still see a first publish; the
  # content-addressed R2 blob may survive, which the publish path skips.
  wrangler d1 execute DB --env dev --local --command "
    INSERT OR IGNORE INTO users (github_id, login, created_at)
      VALUES (0, 'smoke', '1970-01-01T00:00:00Z');
    INSERT OR REPLACE INTO tokens (id, user_id, name, token_hash, scopes, created_at)
      VALUES ('smoke', 0, 'smoke', '${hash}', 'publish,yank', '1970-01-01T00:00:00Z');
    DELETE FROM versions WHERE name = 'withdep';
    DELETE FROM packages WHERE name = 'withdep';"
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

step "unauthenticated /me redirects to the sign-in page"
curl -sS -o /dev/null -D "$body" "$base/me"
grep -q '^HTTP/[^ ]* 302' "$body" || fail "/me did not answer 302: $(head -1 "$body")"
grep -qi '^location: /login' "$body" || fail "/me redirect is not to /login: $(cat "$body")"

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

# The frozen conformance fixture (tests/fixtures/, regenerated by
# scripts/gen-fixtures.sh) doubles as the smoke publish payload.
name="withdep"
version="0.2.0"
fixture_metadata="tests/fixtures/$name-$version.json"
fixture_archive="tests/fixtures/$name-$version.tar.gz"
publish_path="/api/v1/packages/$name/$version"
work="$(mktemp -d)"
trap 'cleanup; rm -rf "$work"' EXIT

step "first publish creates the version"
frame "$fixture_metadata" "$fixture_archive" "$work/publish.bin"
request PUT "$publish_path" "$work/publish.bin" 201
expect_body '"ok":true'

step "byte-identical re-publish is an idempotent no-op"
request PUT "$publish_path" "$work/publish.bin" 200
expect_body '"no_op":true'

step "tampered re-publish hits the immutability wall"
cat "$fixture_archive" >"$work/tampered.tar.gz"
printf 'x' >>"$work/tampered.tar.gz"
old_hash="$(shasum -a 256 "$fixture_archive" | cut -d' ' -f1)"
new_hash="$(shasum -a 256 "$work/tampered.tar.gz" | cut -d' ' -f1)"
sed "s/$old_hash/$new_hash/" "$fixture_metadata" >"$work/tampered.json"
frame "$work/tampered.json" "$work/tampered.tar.gz" "$work/tampered.bin"
request PUT "$publish_path" "$work/tampered.bin" 409
expect_body 'immutable'

step "yank and un-yank walk the state transitions"
printf '{"yanked":true}' >"$work/yank.json"
request PATCH "$publish_path/yank" "$work/yank.json" 200
expect_body '"yanked":true'
expect_body '"changed":true'
check "/packages/$name.json" 200
expect_body '"yanked":true'
printf '{"yanked":false}' >"$work/unyank.json"
request PATCH "$publish_path/yank" "$work/unyank.json" 200
expect_body '"yanked":false'
expect_body '"changed":true'
check "/packages/$name.json" 200
expect_body '"yanked":false'

step "published artifact downloads with the published checksum"
curl -sS -o "$work/artifact.tar.gz" ${curl_args[@]+"${curl_args[@]}"} \
  "$base/artifacts/$name/$name-$version.tar.gz"
got_hash="$(shasum -a 256 "$work/artifact.tar.gz" | cut -d' ' -f1)"
[[ "$got_hash" == "$old_hash" ]] \
  || fail "artifact checksum mismatch: got $got_hash, expected $old_hash"
grep -qF "sha256:$old_hash" "$fixture_metadata" \
  || fail "fixture metadata does not carry sha256:$old_hash"

echo "smoke OK"
