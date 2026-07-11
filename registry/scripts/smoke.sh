#!/usr/bin/env bash
#
# Smoke test against local `wrangler dev`: the hostname-role split
# (wrangler dev pins every request's Host header to one emulated
# hostname, so two instances share the local state - the registry role
# on the default host, the website role via --host cabinpkg.com),
# /healthz, the uniform 401 with its
# byte-identical WWW-Authenticate challenge, the OAuth and session planes
# on the website origin (redirects, cookie attributes, 401 JSON without a
# session), and - given a token - the three authenticated read routes on
# the registry host plus the full publish / yank write flow on the
# website origin (first publish, idempotent re-publish, immutability
# conflict, yank state transitions, artifact checksum), the verification
# lifecycle (pending -> verify -> resolvable with a verify-scoped token,
# verdict idempotency and conflicts, and the reject -> blob reclaim ->
# quota refund -> republish flow including the shared-blob refcount
# case), the budget breaker (writes 402 while service_mode =
# writes_blocked, reads unaffected), blob replication into the BACKUP
# bucket, and the nightly dump job (triggered via the /__scheduled test
# route against a local mock of the D1 export API serving a real
# `wrangler d1 export --local` dump). Session-authenticated flows (token
# create/revoke) need a real GitHub sign-in and stay out of scope here;
# their logic is unit-tested. Local-only: state lives in .wrangler/,
# never a deployed environment.
#
#   scripts/smoke.sh                                   healthz + 401 only
#   CABIN_REGISTRY_SMOKE_TOKEN=cabin_smoke scripts/smoke.sh   full run
#
# The token is seeded into the local D1 state before the checks, so any
# `cabin_...` value works.

set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/.."

port="${SMOKE_PORT:-8787}"
web_port="${SMOKE_WEB_PORT:-8789}"
mock_port="${SMOKE_MOCK_PORT:-8788}"
base="http://127.0.0.1:${port}"
# The website role: a second wrangler dev instance emulating the
# website origin's hostname (--host cabinpkg.com) over the same local
# D1/R2 state, because each instance pins the Host header the Worker's
# role dispatch reads.
web_base="http://127.0.0.1:${web_port}"
# WEB_ORIGIN from wrangler.jsonc (env dev), which the challenge, the
# config.json api field, and the quota details embed.
web_origin="https://cabinpkg.com"
token="${CABIN_REGISTRY_SMOKE_TOKEN:-}"
body="$(mktemp)"
headers="$(mktemp)"
dev_log="$(mktemp)"
web_log="$(mktemp)"
dev_pid=""
web_pid=""
mock_pid=""
dev_vars=".dev.vars.dev"
dev_vars_backup=""
dev_vars_created=""

cleanup() {
  # Kill the whole process group: $dev_pid is the backgrounded function's
  # wrapper subshell, and killing it alone would orphan npx/wrangler/workerd
  # and leave the port bound.
  [[ -n "$dev_pid" ]] && kill -- "-$dev_pid" 2>/dev/null || true
  [[ -n "$web_pid" ]] && kill -- "-$web_pid" 2>/dev/null || true
  [[ -n "$mock_pid" ]] && kill "$mock_pid" 2>/dev/null || true
  [[ -n "$dev_vars_created" ]] && rm -f "$dev_vars" || true
  [[ -n "$dev_vars_backup" ]] && mv "$dev_vars_backup" "$dev_vars" || true
  rm -f "$body" "$headers" "$dev_log" "$web_log"
}
trap cleanup EXIT

step() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

wrangler() { npx --yes wrangler "$@"; }

# check_at <base> <path> <expected statuses...>; body lands in $body.
check_at() {
  local at="$1" path="$2"
  shift 2
  local status
  # ${arr[@]+...}: empty-array expansion trips `set -u` on macOS bash 3.2.
  status="$(curl -sS -o "$body" -w '%{http_code}' \
    ${curl_args[@]+"${curl_args[@]}"} "$at$path")"
  for expected in "$@"; do
    [[ "$status" == "$expected" ]] && { printf '    %s -> %s\n' "$path" "$status"; return 0; }
  done
  fail "$path returned $status, expected one of: $* (body: $(cat "$body"))"
}

# check hits the registry host; wcheck the website origin.
check() { check_at "$base" "$@"; }
wcheck() { check_at "$web_base" "$@"; }

# request_at <base> <method> <path> <data-file> <expected...>; body in $body.
request_at() {
  local at="$1" method="$2" path="$3" data="$4"
  shift 4
  local status
  status="$(curl -sS -o "$body" -w '%{http_code}' -X "$method" --data-binary "@$data" \
    ${curl_args[@]+"${curl_args[@]}"} "$at$path")"
  for expected in "$@"; do
    [[ "$status" == "$expected" ]] && { printf '    %s %s -> %s\n' "$method" "$path" "$status"; return 0; }
  done
  fail "$method $path returned $status, expected one of: $* (body: $(cat "$body"))"
}

# The mutation routes live on the website origin only, so every write
# goes through wrequest; on the registry host, non-read-plane paths are
# covered by uniform_401 below.
wrequest() { request_at "$web_base" "$@"; }

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

verify_token="${token:+${token}-verify}"
if [[ -n "$token" ]]; then
  step "seeding the smoke tokens into the local dev database"
  hash="$(printf '%s' "$token" | shasum -a 256 | cut -d' ' -f1)"
  verify_hash="$(printf '%s' "$verify_token" | shasum -a 256 | cut -d' ' -f1)"
  # The fixture rows are cleared so re-runs still see a first publish; the
  # content-addressed R2 blob may survive, which the publish path skips.
  wrangler d1 execute DB --env dev --local --command "
    INSERT OR IGNORE INTO users (github_id, login, created_at)
      VALUES (0, 'smoke', '1970-01-01T00:00:00Z');
    INSERT OR REPLACE INTO tokens (id, user_id, name, token_hash, scopes, created_at)
      VALUES ('smoke', 0, 'smoke', '${hash}', 'publish,yank', '1970-01-01T00:00:00Z');
    INSERT OR REPLACE INTO tokens (id, user_id, name, token_hash, scopes, created_at)
      VALUES ('smoke-verify', 0, 'smoke-verify', '${verify_hash}', 'verify', '1970-01-01T00:00:00Z');
    DELETE FROM versions WHERE name = 'withdep';
    DELETE FROM packages WHERE name = 'withdep';
    DELETE FROM meta WHERE key IN ('last_backup_at', 'last_backup_key');
    DELETE FROM backup_replication_failures;"
fi

# The backup-cron leg drives the worker's dump job against a local mock
# of the D1 export API, serving a dump exported from the local database
# right here - so the job's polling, streaming, validation, and
# bookkeeping all run for real without touching Cloudflare.
step "exporting a local dump for the export-API mock"
mock_dir="$(mktemp -d)"
trap 'cleanup; rm -rf "$mock_dir"' EXIT
wrangler d1 export DB --env dev --local --output "$mock_dir/dump.sql"

step "starting the export-API mock on port ${mock_port}"
cat >"$mock_dir/mock.js" <<'MOCK_EOF'
const http = require("http");
const fs = require("fs");
const [port, dumpPath] = process.argv.slice(2);
const exportPath = /^\/accounts\/[0-9a-f]{32}\/d1\/database\/[0-9a-f-]{36}\/export$/;
http.createServer((req, res) => {
  if (req.method === "POST" && exportPath.test(req.url)) {
    res.setHeader("content-type", "application/json");
    res.end(JSON.stringify({ success: true, result: { status: "complete",
      at_bookmark: "smoke",
      result: { signed_url: `http://127.0.0.1:${port}/dump.sql`, filename: "dump.sql" } } }));
  } else if (req.method === "GET" && req.url === "/dump.sql") {
    const dump = fs.readFileSync(dumpPath);
    res.setHeader("content-length", dump.length);
    res.end(dump);
  } else {
    res.statusCode = 404;
    res.end(`unexpected request: ${req.method} ${req.url}`);
  }
}).listen(port, "127.0.0.1");
MOCK_EOF
node "$mock_dir/mock.js" "$mock_port" "$mock_dir/dump.sql" >"$mock_dir/mock.log" 2>&1 &
mock_pid=$!
disown "$mock_pid"
for _ in $(seq 1 20); do
  kill -0 "$mock_pid" 2>/dev/null || { cat "$mock_dir/mock.log" >&2; fail "the export-API mock exited early"; }
  curl -fsS -o /dev/null "http://127.0.0.1:${mock_port}/dump.sql" 2>/dev/null && break
  sleep 0.5
done

# Point the worker's export calls at the mock. Wrangler reads
# .dev.vars.dev for --env dev; an existing file is saved and restored.
if [[ -f "$dev_vars" ]]; then
  dev_vars_backup="$(mktemp)"
  cp "$dev_vars" "$dev_vars_backup"
fi
# SESSION_SECRET is pinned so the session-plane leg below can mint a
# valid session cookie for the seeded user (github id 0) without a
# GitHub round trip; ALLOWED_GITHUB_IDS admits that id plus id 1, whose
# user row deliberately does not exist (the post-wipe ghost-session
# case).
cat >"$dev_vars" <<EOF
CF_API_BASE="http://127.0.0.1:${mock_port}"
D1_EXPORT_API_TOKEN="smoke-placeholder"
SESSION_SECRET="smoke-session-secret"
ALLOWED_GITHUB_IDS="0,1"
EOF
dev_vars_created=1

# A stale server on either port would silently answer the checks below in
# place of the instances started here.
for stale_port in "$port" "$web_port"; do
  if curl -fsS -o /dev/null "http://127.0.0.1:${stale_port}/healthz" 2>/dev/null; then
    fail "something is already serving on port ${stale_port}; kill it first"
  fi
done

step "starting wrangler dev on port ${port} (first build takes a while)"
# Job control (-m) gives each dev-server tree its own process group so
# cleanup can kill all of it at once.
set -m
wrangler dev --env dev --port "$port" --test-scheduled >"$dev_log" 2>&1 &
dev_pid=$!
set +m
disown "$dev_pid"

for _ in $(seq 1 300); do
  kill -0 "$dev_pid" 2>/dev/null || { cat "$dev_log" >&2; fail "wrangler dev exited early"; }
  curl -fsS -o /dev/null "$base/healthz" 2>/dev/null && break
  sleep 1
done

# The website-role instance: same code, same local state, but wrangler
# pins its emulated Host header to cabinpkg.com, which is what flips the
# Worker's role dispatch. Started second so the first instance's build
# is already cached.
step "starting the website-role wrangler dev on port ${web_port}"
set -m
wrangler dev --env dev --port "$web_port" --host cabinpkg.com >"$web_log" 2>&1 &
web_pid=$!
set +m
disown "$web_pid"

# /healthz only exists on the registry role; any HTTP status at all
# (the website role answers it 401/404) proves the instance is up.
for _ in $(seq 1 300); do
  kill -0 "$web_pid" 2>/dev/null || { cat "$web_log" >&2; fail "the website-role wrangler dev exited early"; }
  [[ "$(curl -sS -o /dev/null -w '%{http_code}' "$web_base/healthz" 2>/dev/null)" != "000" ]] && break
  sleep 1
done

curl_args=()
step "healthz is unauthenticated and empty"
check /healthz 200
[[ -s "$body" ]] && fail "/healthz returned a body: $(cat "$body")"

expected_401='{"errors":[{"detail":"authentication required"}]}'
challenge="Cabin login_url=\"${web_origin}/settings/tokens\""

# uniform_401 <base> <path> [extra curl args...]: the response must be
# the exact envelope plus the byte-identical WWW-Authenticate challenge,
# whatever the path, method, or credential. The header value is compared
# byte for byte (a duplicated header or a suffixed value must fail, so
# the comparison is against the exact expected string, not a substring).
uniform_401() {
  local at="$1" path="$2"
  shift 2
  local status got
  status="$(curl -sS -o "$body" -D "$headers" -w '%{http_code}' "$@" "$at$path")"
  [[ "$status" == "401" ]] || fail "$path returned $status, expected the uniform 401"
  [[ "$(cat "$body")" == "$expected_401" ]] || fail "401 body mismatch on $path: $(cat "$body")"
  got="$(grep -i '^www-authenticate:' "$headers" | sed 's/^[^:]*: //' | tr -d '\r')"
  [[ "$got" == "$challenge" ]] \
    || fail "401 on $path challenge mismatch: got '$got', expected '$challenge'"
  printf '    %s -> uniform 401 with the challenge\n' "$path"
}

step "the registry host is a uniform 401 with the challenge off the read plane"
uniform_401 "$base" /config.json
uniform_401 "$base" /packages/smoke.json
# Non-read-plane paths - the whole API and session surface included -
# are indistinguishable from unknown paths, whatever credential comes
# along.
uniform_401 "$base" /api/v1/packages/smoke/0.1.0
uniform_401 "$base" /api/v1/user
uniform_401 "$base" /unknown/path
uniform_401 "$base" /me
uniform_401 "$base" "/api/v1/admin/versions?status=pending" \
  -H "Authorization: Bearer cabin_definitelyNotAToken"
# Write methods too: the mutation surface simply does not exist here.
uniform_401 "$base" /api/v1/packages/smoke/0.1.0 -X PUT --data-binary "x"
uniform_401 "$base" /api/v1/packages/smoke/0.1.0/yank -X PATCH --data-binary "{}"

step "session endpoints answer 401 json without a session (no challenge)"
wcheck /api/v1/user 401
[[ "$(cat "$body")" == "$expected_401" ]] || fail "session 401 body mismatch: $(cat "$body")"
curl -sS -o /dev/null -D "$headers" "$web_base/api/v1/user"
! grep -qi '^www-authenticate:' "$headers" \
  || fail "the session-plane 401 must not carry the bearer challenge"
wcheck /api/v1/user/usage 401
wcheck /api/v1/user/packages 401
wcheck /api/v1/user/tokens 401
wcheck /api/v1/user/logout 401 -X POST

step "the oauth plane lives on the website origin with host-only cookies"
curl -sS -o /dev/null -D "$headers" "$web_base/login"
grep -q '^HTTP/[^ ]* 302' "$headers" || fail "/login did not answer 302: $(head -1 "$headers")"
grep -qi '^location: https://github.com/login/oauth/authorize' "$headers" \
  || fail "/login redirect is not to github: $(cat "$headers")"
state_cookie="$(grep -i '^set-cookie: cabin_oauth_state=' "$headers" || true)"
[[ -n "$state_cookie" ]] || fail "/login set no state cookie: $(cat "$headers")"
case "$state_cookie" in
  *"Path=/callback"*) ;;
  *) fail "state cookie is not scoped to /callback: $state_cookie" ;;
esac
for attribute in HttpOnly Secure "SameSite=Lax"; do
  case "$state_cookie" in
    *"$attribute"*) ;;
    *) fail "state cookie is missing $attribute: $state_cookie" ;;
  esac
done
! printf '%s' "$state_cookie" | grep -qi 'domain=' \
  || fail "the state cookie must be host-only: $state_cookie"

step "a parameterless callback redirects to the denied page"
curl -sS -o /dev/null -D "$headers" "$web_base/callback"
grep -qi '^location: /login/denied' "$headers" \
  || fail "/callback refusal is not /login/denied: $(cat "$headers")"
# /login is absent from the registry host like everything non-read-plane.
uniform_401 "$base" /login

if [[ -z "$token" ]]; then
  step "CABIN_REGISTRY_SMOKE_TOKEN not set; skipping authenticated checks"
  echo "smoke OK"
  exit 0
fi

# The two credentials the checks below switch between: the ordinary
# publish/yank token and the verifier's verify-scoped one.
as_publisher() { curl_args=(-H "Authorization: Bearer $token"); }
as_verifier() { curl_args=(-H "Authorization: Bearer $verify_token"); }

as_publisher
step "authenticated read routes"
check /config.json 200
grep -q '"auth-required":true' "$body" || fail "config.json missing auth-required: $(cat "$body")"
# The api field names the website origin, crates.io-style.
grep -qF "\"api\":\"${web_origin}\"" "$body" \
  || fail "config.json api is not the website origin: $(cat "$body")"
# 200 only with previously published local data; 404 proves auth + routing.
check /packages/smoke.json 200 404
check /artifacts/smoke/smoke-0.1.0.tar.gz 200 404

step "a valid token changes nothing off the read plane on the registry host"
uniform_401 "$base" /api/v1/packages/smoke/0.1.0 -H "Authorization: Bearer $token"
uniform_401 "$base" /api/v1/user -H "Authorization: Bearer $token"

step "the read plane is absent on the website origin"
wcheck /config.json 404
wcheck /packages/smoke.json 404
wcheck /artifacts/smoke/smoke-0.1.0.tar.gz 404
wcheck /healthz 404

# --- The session plane, end to end with a minted session. ---
# The session cookie is `<payload>.<hmac>` keyed by SESSION_SECRET, which
# this run pinned above - so a valid session for the seeded user (github
# id 0, admitted by the pinned ALLOWED_GITHUB_IDS) can be minted without
# a GitHub round trip.
session_payload="0:$(($(date +%s) + 3600))"
session_mac="$(printf 'session:%s' "$session_payload" |
  openssl dgst -sha256 -hmac "smoke-session-secret" | sed 's/^.* //')"
session_cookie="cabin_session=${session_payload}.${session_mac}"

# session_request <method> <path> <expected status> [curl args...];
# body in $body.
session_request() {
  local method="$1" path="$2" expected="$3"
  shift 3
  local status
  status="$(curl -sS -o "$body" -w '%{http_code}' -X "$method" \
    -H "Cookie: $session_cookie" "$@" "$web_base$path")"
  [[ "$status" == "$expected" ]] ||
    fail "$method $path returned $status, expected $expected (body: $(cat "$body"))"
  printf '    %s %s -> %s\n' "$method" "$path" "$status"
}
csrf_headers=(-H "Content-Type: application/json" -H "X-CSRF-Protection: 1")

step "session reads answer the seeded user"
session_request GET /api/v1/user 200
expect_body '"github_id":0'
expect_body '"login":"smoke"'
session_request GET /api/v1/user/usage 200
expect_body '"quotas"'
session_request GET /api/v1/user/packages 200
expect_body '"packages"'

step "a valid session whose user row is gone answers 401 everywhere"
# The post-wipe ghost: allowlisted id 1 has a validly sealed cookie but
# no users row - every endpoint (the token routes included) answers the
# same 401 as no session, never an empty listing or a 500.
ghost_payload="1:$(($(date +%s) + 3600))"
ghost_mac="$(printf 'session:%s' "$ghost_payload" |
  openssl dgst -sha256 -hmac "smoke-session-secret" | sed 's/^.* //')"
real_session_cookie="$session_cookie"
session_cookie="cabin_session=${ghost_payload}.${ghost_mac}"
session_request GET /api/v1/user 401
session_request GET /api/v1/user/tokens 401
session_request POST /api/v1/user/tokens 401 \
  "${csrf_headers[@]}" --data-binary '{"name":"ghost","scopes":[]}'
# Logout is the one exception: a validly sealed cookie is always
# cleared, user row or not.
session_request POST /api/v1/user/logout 200 "${csrf_headers[@]}"
expect_body '"ok":true'
session_cookie="$real_session_cookie"

step "session mutations enforce the csrf header pair"
create_body='{"name":"smoke-session","scopes":["publish"]}'
session_request POST /api/v1/user/tokens 403 \
  -H "Content-Type: application/json" --data-binary "$create_body"
expect_body 'X-CSRF-Protection'
session_request POST /api/v1/user/tokens 403 \
  -H "X-CSRF-Protection: 1" --data-binary "$create_body"

step "token create round-trip: plaintext once, usable, then revoked"
session_request POST /api/v1/user/tokens 201 \
  "${csrf_headers[@]}" --data-binary "$create_body"
expect_body '"name":"smoke-session"'
minted="$(node -e '
  const body = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
  if (!/^cabin_/.test(body.token || "")) process.exit(1);
  console.log(body.token);' "$body")" || fail "create response carries no plaintext token: $(cat "$body")"
minted_id="$(node -e '
  const body = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
  console.log(body.id);' "$body")"
# The minted token works on the bearer plane...
curl_args=(-H "Authorization: Bearer $minted")
check /config.json 200
# ...and the listing shows metadata only - never the plaintext.
session_request GET /api/v1/user/tokens 200
expect_body '"name":"smoke-session"'
! grep -qF "$minted" "$body" || fail "the token listing leaked a plaintext token"
session_request POST "/api/v1/user/tokens/$minted_id/revoke" 200 "${csrf_headers[@]}"
expect_body '"ok":true'
uniform_401 "$base" /config.json -H "Authorization: Bearer $minted"
as_publisher

step "logout requires the csrf pair and clears the session cookie"
session_request POST /api/v1/user/logout 403
expect_body 'X-CSRF-Protection'
curl -sS -o "$body" -D "$headers" -X POST -H "Cookie: $session_cookie" \
  "${csrf_headers[@]}" "$web_base/api/v1/user/logout"
grep -q '"ok":true' "$body" || fail "logout did not answer ok: $(cat "$body")"
logout_cookie="$(grep -i '^set-cookie: cabin_session=' "$headers" || true)"
[[ -n "$logout_cookie" ]] || fail "logout set no clearing cookie: $(cat "$headers")"
case "$logout_cookie" in
  *"Max-Age=0"*) ;;
  *) fail "logout cookie does not clear: $logout_cookie" ;;
esac

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
blob_hash="$(shasum -a 256 "$fixture_archive" | cut -d' ' -f1)"
work="$(mktemp -d)"
trap 'cleanup; rm -rf "$work" "$mock_dir"' EXIT

step "first publish creates the version pending verification"
frame "$fixture_metadata" "$fixture_archive" "$work/publish.bin"
wrequest PUT "$publish_path" "$work/publish.bin" 201
expect_body '"ok":true'
expect_body '"verification":"pending"'

step "pending versions are invisible to ordinary tokens"
check "/packages/$name.json" 404
check "/artifacts/$name/$name-$version.tar.gz" 404
wcheck "/api/v1/admin/versions?status=pending" 403
expect_body 'verify scope'
printf '{"verdict":"verified"}' >"$work/verdict-unbound.json"
wrequest PATCH "/api/v1/admin/versions/$name/$version" "$work/verdict-unbound.json" 403
expect_body 'verify scope'

step "the verify scope lists and downloads pending versions"
as_verifier
wcheck "/api/v1/admin/versions?status=pending" 200
expect_body '"name":"withdep"'
expect_body '"version":"0.2.0"'
expect_body '"published_by":0'
expect_body '"metadata":{'
# A verified verdict must echo the listing's checksum and published_at.
listed_published_at="$(node -e '
  const doc = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
  const v = doc.versions.find((v) => v.name === "withdep" && v.version === "0.2.0");
  if (!v || !v.published_at) process.exit(1);
  console.log(v.published_at);' "$body")" \
  || fail "the admin listing is missing withdep@0.2.0 or its published_at"
printf '{"verdict":"verified","checksum":"%s","published_at":"%s"}' \
  "$blob_hash" "$listed_published_at" >"$work/verdict-verified.json"
wcheck "/api/v1/admin/versions?status=bogus" 400
check "/artifacts/$name/$name-$version.tar.gz" 200

step "a verified verdict must name the listing it inspected"
wrequest PATCH "/api/v1/admin/versions/$name/$version" "$work/verdict-unbound.json" 400
expect_body 'requires the checksum'

step "a verified verdict makes the version resolvable"
wrequest PATCH "/api/v1/admin/versions/$name/$version" "$work/verdict-verified.json" 200
expect_body '"verification":"verified"'
expect_body '"changed":true'
as_publisher
check "/packages/$name.json" 200
expect_body '"0.2.0"'
check "/artifacts/$name/$name-$version.tar.gz" 200

step "verdicts are idempotent for the same value and conflict otherwise"
as_verifier
wrequest PATCH "/api/v1/admin/versions/$name/$version" "$work/verdict-verified.json" 200
expect_body '"changed":false'
printf '{"verdict":"rejected","reason":"smoke rejection"}' >"$work/verdict-rejected.json"
wrequest PATCH "/api/v1/admin/versions/$name/$version" "$work/verdict-rejected.json" 409
expect_body 'immutable'
as_publisher

# await_backup_blob <key> <out-file>: replication runs via waitUntil
# after the response, so poll the BACKUP bucket briefly.
await_backup_blob() {
  local key="$1" out="$2"
  for _ in $(seq 1 20); do
    wrangler r2 object get "cabin-registry-dev-backup/$key" \
      --file "$out" --local >/dev/null 2>&1 && return 0
    sleep 0.5
  done
  fail "blob $key never appeared in the BACKUP bucket"
}

step "the published blob replicates to the BACKUP bucket"
await_backup_blob "blobs/sha256/$blob_hash" "$work/replicated.tar.gz"
cmp -s "$work/replicated.tar.gz" "$fixture_archive" \
  || fail "replicated blob differs from the published archive"

# A retry of a publish whose isolate died before replicating takes the
# idempotent no-op path; it must re-schedule the copy.
step "an idempotent re-publish heals missing primary and backup blobs"
wrangler r2 object delete "cabin-registry-dev-backup/blobs/sha256/$blob_hash" --local >/dev/null
wrangler r2 object delete "cabin-registry-dev-blobs/blobs/sha256/$blob_hash" --local >/dev/null
wrequest PUT "$publish_path" "$work/publish.bin" 200
expect_body '"no_op":true'
expect_body '"verification":"verified"'
check "/artifacts/$name/$name-$version.tar.gz" 200
await_backup_blob "blobs/sha256/$blob_hash" "$work/rehealed.tar.gz"
cmp -s "$work/rehealed.tar.gz" "$fixture_archive" \
  || fail "re-healed blob differs from the published archive"

step "byte-identical re-publish is an idempotent no-op reporting the status"
wrequest PUT "$publish_path" "$work/publish.bin" 200
expect_body '"no_op":true'
expect_body '"verification":"verified"'

step "tampered re-publish hits the immutability wall"
cat "$fixture_archive" >"$work/tampered.tar.gz"
printf 'x' >>"$work/tampered.tar.gz"
old_hash="$(shasum -a 256 "$fixture_archive" | cut -d' ' -f1)"
new_hash="$(shasum -a 256 "$work/tampered.tar.gz" | cut -d' ' -f1)"
sed "s/$old_hash/$new_hash/" "$fixture_metadata" >"$work/tampered.json"
frame "$work/tampered.json" "$work/tampered.tar.gz" "$work/tampered.bin"
wrequest PUT "$publish_path" "$work/tampered.bin" 409
expect_body 'immutable'

step "yank and un-yank walk the state transitions"
printf '{"yanked":true}' >"$work/yank.json"
wrequest PATCH "$publish_path/yank" "$work/yank.json" 200
expect_body '"yanked":true'
expect_body '"changed":true'
check "/packages/$name.json" 200
expect_body '"yanked":true'
# The session packages listing mirrors the row: the seeded user created
# the package, its version is verified by now, and currently yanked.
session_request GET /api/v1/user/packages 200
expect_body '"name":"withdep"'
expect_body '"verification":"verified"'
expect_body '"yanked":true'
printf '{"yanked":false}' >"$work/unyank.json"
wrequest PATCH "$publish_path/yank" "$work/unyank.json" 200
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

# The dev vars pin SERVICE_MODE_TTL_SECS to 0, so the running worker sees
# the flipped mode immediately instead of after the 60 s cache TTL.
step "writes answer 402 while writes_blocked; reads stay open"
wrangler d1 execute DB --env dev --local --command "
  UPDATE meta SET value = 'writes_blocked' WHERE key = 'service_mode';
  UPDATE meta SET value = 'forced by smoke.sh' WHERE key = 'service_mode_reason';"
wrequest PUT "$publish_path" "$work/publish.bin" 402
expect_body 'registry_over_budget'
wrequest PATCH "$publish_path/yank" "$work/unyank.json" 402
expect_body 'registry_over_budget'
check "/packages/$name.json" 200

step "restoring service_mode reopens writes"
wrangler d1 execute DB --env dev --local --command "
  UPDATE meta SET value = 'normal' WHERE key = 'service_mode';
  UPDATE meta SET value = '' WHERE key = 'service_mode_reason';"
wrequest PUT "$publish_path" "$work/publish.bin" 200
expect_body '"no_op":true'

# --- The reject -> blob reclaim -> quota refund -> republish flow. ---
# The PUTs above consumed the publish bucket's full burst; give this leg
# its own by resetting the token's bucket columns.
wrangler d1 execute DB --env dev --local --command "
  UPDATE tokens SET rl_tokens = NULL, rl_updated_at = NULL WHERE id = 'smoke';"

# meta.total_stored_bytes, the exact storage self-accounting.
stored_bytes() {
  wrangler d1 execute DB --env dev --local --json --command \
    "SELECT value FROM meta WHERE key = 'total_stored_bytes'" |
    node -e '
      const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
      console.log(out[0].results[0].value);'
}

# 0.2.1 with the exact archive 0.2.0 published: the shared-blob case.
version2="0.2.1"
publish2_path="/api/v1/packages/$name/$version2"
sed 's/0\.2\.0/0.2.1/g' "$fixture_metadata" >"$work/withdep-0.2.1.json"
frame "$work/withdep-0.2.1.json" "$fixture_archive" "$work/publish2.bin"

step "publishing a second version with identical content shares the blob"
before_bytes="$(stored_bytes)"
wrequest PUT "$publish2_path" "$work/publish2.bin" 201
expect_body '"verification":"pending"'
[[ "$(stored_bytes)" == "$before_bytes" ]] \
  || fail "a shared blob was double-counted: $(stored_bytes) (was $before_bytes)"

step "a verdict bound to a stale listing conflicts"
as_verifier
printf '{"verdict":"verified","checksum":"%s","published_at":"1970-01-01T00:00:00.000Z"}' \
  "$(printf 'stale' | shasum -a 256 | cut -d' ' -f1)" >"$work/verdict-stale.json"
wrequest PATCH "/api/v1/admin/versions/$name/$version2" "$work/verdict-stale.json" 409
expect_body 'changed since it was listed'

step "rejecting a version sharing its blob keeps the blob and the accounting"
# Bound to the listed checksum: the verdict applies only to these bytes.
printf '{"verdict":"rejected","reason":"smoke rejection","checksum":"%s"}' "$blob_hash" \
  >"$work/verdict-rejected-bound.json"
wrequest PATCH "/api/v1/admin/versions/$name/$version2" "$work/verdict-rejected-bound.json" 200
expect_body '"verification":"rejected"'
expect_body '"changed":true'
check "/artifacts/$name/$name-$version2.tar.gz" 404
as_publisher
[[ "$(stored_bytes)" == "$before_bytes" ]] \
  || fail "rejecting a shared blob changed the accounting: $(stored_bytes) (was $before_bytes)"
check "/artifacts/$name/$name-$version.tar.gz" 200
check "/packages/$name.json" 200
! grep -qF '"0.2.1"' "$body" \
  || fail "a rejected version leaked into the package document: $(cat "$body")"
wrequest PATCH "$publish2_path/yank" "$work/yank.json" 404

step "republishing over a rejected version replaces it as pending"
cat "$fixture_archive" >"$work/replacement.tar.gz"
printf 'y' >>"$work/replacement.tar.gz"
replacement_hash="$(shasum -a 256 "$work/replacement.tar.gz" | cut -d' ' -f1)"
sed "s/$blob_hash/$replacement_hash/" "$work/withdep-0.2.1.json" >"$work/replacement.json"
frame "$work/replacement.json" "$work/replacement.tar.gz" "$work/replacement.bin"
wrequest PUT "$publish2_path" "$work/replacement.bin" 201
expect_body '"verification":"pending"'
replacement_size="$(wc -c <"$work/replacement.tar.gz" | tr -d ' ')"
[[ "$(stored_bytes)" == "$((before_bytes + replacement_size))" ]] \
  || fail "the replacement archive was not counted: $(stored_bytes)"
as_verifier
wcheck "/api/v1/admin/versions?status=pending" 200
expect_body '"version":"0.2.1"'
expect_body "$replacement_hash"

step "rejecting an unshared blob reclaims it and refunds the bytes"
wrequest PATCH "/api/v1/admin/versions/$name/$version2" "$work/verdict-rejected.json" 200
as_publisher
[[ "$(stored_bytes)" == "$before_bytes" ]] \
  || fail "the rejection did not refund the replacement bytes: $(stored_bytes)"
if wrangler r2 object get "cabin-registry-dev-blobs/blobs/sha256/$replacement_hash" \
  --file "$work/reclaimed.tar.gz" --local >/dev/null 2>&1; then
  fail "the rejected version's unshared blob was not reclaimed"
fi

step "republishing identical bytes over a rejected version restarts verification"
wrequest PUT "$publish2_path" "$work/replacement.bin" 201
expect_body '"verification":"pending"'
[[ "$(stored_bytes)" == "$((before_bytes + replacement_size))" ]] \
  || fail "the re-uploaded blob was not re-counted: $(stored_bytes)"
as_verifier
check "/artifacts/$name/$name-$version2.tar.gz" 200
as_publisher

# The /__scheduled test route (wrangler dev --test-scheduled) invokes
# the cron handler; any non-breaker expression routes to the dump job,
# which talks to the export-API mock started above.
step "the backup cron stores a validated dump in the BACKUP bucket"
today="$(date -u +%F)"
dump_key="d1/$today.sql"
check "/__scheduled?cron=0+3+*+*+*" 200
stored=""
for _ in $(seq 1 20); do
  if wrangler r2 object get "cabin-registry-dev-backup/$dump_key" \
    --file "$work/stored-dump.sql" --local >/dev/null 2>&1; then
    stored=1
    break
  fi
  sleep 0.5
done
[[ -n "$stored" ]] || {
  tail -40 "$dev_log" >&2
  fail "dump $dump_key never appeared in the BACKUP bucket"
}
cmp -s "$work/stored-dump.sql" "$mock_dir/dump.sql" \
  || fail "stored dump differs from the mock's exported dump"

step "the dump's sha256 sidecar verifies with shasum -c"
wrangler r2 object get "cabin-registry-dev-backup/$dump_key.sha256" \
  --file "$work/$today.sql.sha256" --local >/dev/null 2>&1 \
  || fail "sidecar $dump_key.sha256 is missing"
cp "$work/stored-dump.sql" "$work/$today.sql"
(cd "$work" && shasum -a 256 -c "$today.sql.sha256" >/dev/null) \
  || fail "shasum -c rejected the sidecar: $(cat "$work/$today.sql.sha256")"

step "meta records the backup"
last_backup_at="$(wrangler d1 execute DB --env dev --local --json --command \
  "SELECT key, value FROM meta WHERE key IN ('last_backup_at', 'last_backup_key')" |
  node -e '
    const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
    const rows = Object.fromEntries(out[0].results.map((r) => [r.key, r.value]));
    if (rows.last_backup_key !== process.argv[1]) process.exit(1);
    if (!/^\d{4}-\d{2}-\d{2}T/.test(rows.last_backup_at || "")) process.exit(1);
    console.log(rows.last_backup_at);
  ' "$dump_key")" || fail "meta.last_backup_at / last_backup_key not recorded"
printf '    last_backup_at = %s\n' "$last_backup_at"

# One validated dump per date: a same-day re-run must skip instead of
# re-exporting (a failed re-export would overwrite the verified copy),
# so last_backup_at must not move.
step "a same-day re-run of the dump job is a no-op"
check "/__scheduled?cron=0+3+*+*+*" 200
rerun_at="$(wrangler d1 execute DB --env dev --local --json --command \
  "SELECT value FROM meta WHERE key = 'last_backup_at'" |
  node -e '
    const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
    console.log(out[0].results[0].value);
  ')"
[[ "$rerun_at" == "$last_backup_at" ]] \
  || fail "same-day re-run rewrote last_backup_at: $rerun_at (was $last_backup_at)"

echo "smoke OK"
