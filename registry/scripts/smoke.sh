#!/usr/bin/env bash
#
# Smoke test against local `wrangler dev`: the hostname-role split
# (wrangler dev pins every request's Host header to one emulated
# hostname, so two instances share the local state - the registry role
# on the default host, the website role via --host cabinpkg.com),
# /healthz, the uniform 401 with its
# byte-identical WWW-Authenticate challenge, the OAuth and session planes
# on the website origin (redirects, cookie attributes, 401 JSON without a
# session), the launch guard (scripts/wipe.sh refuses while
# meta.launched is 'true'), and - given a token - the three
# authenticated read routes on
# the registry host plus claim -> publish -> fetch end to end: the
# scope-claim flow against a local GitHub mock (self-claim, org claim,
# refused re-claims and non-admin org claims, state-cookie discipline),
# membership management through the session API (list/add/remove, the
# uniform owner 403, the last-owner 409), then the full publish / yank
# write flow on the website origin under the just-claimed scope (first
# publish, the reserved-name and -/_ twin 400s,
# idempotent re-publish, immutability conflict, yank state
# transitions, artifact checksum, and the write plane's uniform 403 for
# unclaimed and foreign scopes - 'foreign' stays a seeded fixture
# because it must belong to somebody else), the verification
# lifecycle (pending -> verify -> resolvable with a verify-scoped token,
# verdict idempotency and conflicts, and the reject -> blob reclaim ->
# quota refund -> republish flow including the shared-blob refcount
# case), the source viewer's session-ranged reads (the range policy's
# 400/416 matrix, exact bytes and headers, verified-only with yanked
# browsable, and the counter never moving), the budget breaker (writes
# 503 while service_mode =
# writes_blocked, reads unaffected), verified-only blob replication
# into the BACKUP bucket through the durable queue, the nightly dump
# job (triggered via the /__scheduled test
# route against a local mock of the D1 export API serving a real
# `wrangler d1 export --local` dump), the governor's reconciliation
# cron pass, and - after a restart onto tiny governor pools - the hard
# refusal paths: cached downloads keep serving, uncached ones answer
# the coded 503, the verifier pool stays isolated, a fresh publish
# refuses before any R2 write, and the source viewer fails closed. Ordinary sign-in still needs a
# real GitHub roundtrip and stays out of scope; the session cookie is
# minted directly under the pinned SESSION_SECRET instead. Local-only:
# state lives in .wrangler/, never a deployed environment.
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
github_port="${SMOKE_GITHUB_PORT:-8790}"
base="http://127.0.0.1:${port}"
# The website role: a second wrangler dev instance emulating the
# website origin's hostname (--host cabinpkg.com) over the same local
# D1/R2 state, because each instance pins the Host header the Worker's
# role dispatch reads.
web_base="http://127.0.0.1:${web_port}"
# WEB_ORIGIN from wrangler.jsonc, which the challenge, the
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
github_pid=""
dev_vars=".dev.vars"
dev_vars_backup=""
dev_vars_created=""

cleanup() {
  # A failure inside a breaker leg would otherwise leave the pinned mode
  # behind in the local D1 state, blocking unrelated local work until
  # the next run's seeding normalizes it.
  npx --yes wrangler@4.112.0 d1 execute DB --local --command \
    "UPDATE meta SET value = 'normal' WHERE key = 'service_mode';
     UPDATE meta SET value = '' WHERE key = 'service_mode_reason';" \
    >/dev/null 2>&1 || true
  # Kill the whole process group: $dev_pid is the backgrounded function's
  # wrapper subshell, and killing it alone would orphan npx/wrangler/workerd
  # and leave the port bound.
  [[ -n "$dev_pid" ]] && kill -- "-$dev_pid" 2>/dev/null || true
  [[ -n "$web_pid" ]] && kill -- "-$web_pid" 2>/dev/null || true
  [[ -n "$mock_pid" ]] && kill "$mock_pid" 2>/dev/null || true
  [[ -n "$github_pid" ]] && kill "$github_pid" 2>/dev/null || true
  [[ -n "$dev_vars_created" ]] && rm -f "$dev_vars" || true
  [[ -n "$dev_vars_backup" ]] && mv "$dev_vars_backup" "$dev_vars" || true
  rm -f "$body" "$headers" "$dev_log" "$web_log"
}
trap cleanup EXIT

step() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

wrangler() { npx --yes wrangler@4.112.0 "$@"; }

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

# tamper_zip <src> <dst> <seed>: copy the zip at <src> to <dst> with one
# interior byte flipped, so the bytes (and thus the checksum) change while
# the container stays well formed. The worker's fixed-offset sanity check
# reads only the four-byte local-header prefix and the trailing EOCD, so
# touching a byte in the middle keeps the request on the immutability /
# verification path instead of the container gate (a byte appended past the
# EOCD would move it off `len - 22` and fail that gate first). A distinct
# <seed> yields distinct bytes.
tamper_zip() {
  python3 - "$1" "$2" "$3" <<'PY'
import sys
src, dst, seed = sys.argv[1], sys.argv[2], int(sys.argv[3])
data = bytearray(open(src, "rb").read())
data[len(data) // 2] ^= (seed & 0xFF) or 1
open(dst, "wb").write(data)
PY
}

step "applying migrations to the local database"
wrangler d1 migrations apply DB --local

verify_token="${token:+${token}-verify}"
if [[ -n "$token" ]]; then
  step "seeding the smoke tokens and fixtures into the local database"
  hash="$(printf '%s' "$token" | shasum -a 256 | cut -d' ' -f1)"
  verify_hash="$(printf '%s' "$verify_token" | shasum -a 256 | cut -d' ' -f1)"
  # The fixture rows are cleared so re-runs still see a first publish
  # and a first claim; the content-addressed R2 blob may survive, which
  # the publish path skips. The seeded identities mirror first sign-ins:
  # GitHub account 0 is the claiming user (registry user 1), account 2
  # ('friend', registry user 2) exists so membership management has an
  # account to add. The scopes user 1 works with ('smoke', 'smokeorg',
  # 'denyorg') are claimed through the real flow against the GitHub mock
  # below - only 'foreign' stays a seeded fixture, because it must
  # belong to somebody else (user 2): publishing there must be exactly
  # as forbidden as the unclaimed 'ghost'.
  wrangler d1 execute DB --local --command "
    INSERT OR IGNORE INTO users (id, created_at)
      VALUES (1, '1970-01-01T00:00:00Z');
    INSERT OR IGNORE INTO users (id, created_at)
      VALUES (2, '1970-01-01T00:00:00Z');
    INSERT OR IGNORE INTO identities (provider, provider_account_id, login_snapshot, user_id)
      VALUES ('github', '0', 'smoke', 1);
    INSERT OR IGNORE INTO identities (provider, provider_account_id, login_snapshot, user_id)
      VALUES ('github', '2', 'friend', 2);
    INSERT OR IGNORE INTO scopes (name, proof_provider, proof_account_id, claimed_at)
      VALUES ('foreign', 'github', '2', '1970-01-01T00:00:00Z');
    INSERT OR IGNORE INTO scope_members (scope_name, user_id, role)
      VALUES ('foreign', 2, 'owner');
    INSERT OR REPLACE INTO tokens (id, user_id, name, token_hash, scopes, created_at)
      VALUES ('smoke', 1, 'smoke', '${hash}', 'publish,yank', '1970-01-01T00:00:00Z');
    INSERT OR REPLACE INTO tokens (id, user_id, name, token_hash, scopes, created_at)
      VALUES ('smoke-verify', 1, 'smoke-verify', '${verify_hash}', 'verify', '1970-01-01T00:00:00Z');
    DELETE FROM versions WHERE scope = 'smoke';
    DELETE FROM packages WHERE scope = 'smoke';
    DELETE FROM scope_members WHERE scope_name IN
      ('smoke', 'smokeorg', 'denyorg', 'imposterorg', 'swaporg', 'statedrift',
       'core', 'sm0keorg');
    DELETE FROM scopes WHERE name IN
      ('smoke', 'smokeorg', 'denyorg', 'imposterorg', 'swaporg', 'statedrift',
       'core', 'sm0keorg');
    DELETE FROM meta WHERE key IN ('last_backup_at', 'last_backup_key');
    DELETE FROM backup_pending;
    -- A prior run that failed inside a breaker leg leaves its pinned
    -- mode behind; normalize so re-runs never start blocked.
    UPDATE meta SET value = 'normal' WHERE key = 'service_mode';"
fi

# The backup-cron leg drives the worker's dump job against a local mock
# of the D1 export API, serving a dump exported from the local database
# right here - so the job's polling, streaming, validation, and
# bookkeeping all run for real without touching Cloudflare.
step "exporting a local dump for the export-API mock"
mock_dir="$(mktemp -d)"
trap 'cleanup; rm -rf "$mock_dir"' EXIT
wrangler d1 export DB --local --output "$mock_dir/dump.sql"

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

# The GitHub mock the claim flow's server-side calls run against
# (GITHUB_OAUTH_BASE / GITHUB_API_BASE below): the token exchange
# (which requires the claim callback's exact redirect_uri, like
# GitHub), the authenticated /user read (id 0, login 'Smoke' - the
# uppercase spelling exercises the lowercased self-claim comparison),
# and the org-claim reads. 'smokeorg' grants (active admin); 'denyorg'
# refuses (plain member); 'imposterorg' and 'swaporg' refuse on the
# numeric-id bindings (the membership's user is not the authenticated
# claimant / the membership's organization is not the account /users
# resolves); 'statedrift' is deliberately grantable so its leg's
# refusal can only be the state check; the /__drift toggle turns
# /users/smoke into a different account than /user for the self-claim
# binding leg. API reads without a bearer token answer 401 like GitHub.
step "starting the GitHub mock on port ${github_port}"
cat >"$mock_dir/github-mock.js" <<'MOCK_EOF'
const http = require("http");
const port = process.argv[2];
const api = {
  "/user": { id: 0, login: "Smoke" },
  "/users/smoke": { id: 0, login: "smoke", type: "User" },
  "/users/smokeorg": { id: 7280970, login: "smokeorg", type: "Organization" },
  "/users/denyorg": { id: 555, login: "denyorg", type: "Organization" },
  "/users/imposterorg": { id: 666, login: "imposterorg", type: "Organization" },
  "/users/swaporg": { id: 777, login: "swaporg", type: "Organization" },
  "/orgs/smokeorg/memberships/Smoke": {
    state: "active", role: "admin",
    user: { id: 0, login: "Smoke" },
    organization: { id: 7280970, login: "smokeorg" },
  },
  "/orgs/denyorg/memberships/Smoke": {
    state: "active", role: "member",
    user: { id: 0, login: "Smoke" },
    organization: { id: 555, login: "denyorg" },
  },
  "/orgs/imposterorg/memberships/Smoke": {
    state: "active", role: "admin",
    user: { id: 999, login: "Smoke" },
    organization: { id: 666, login: "imposterorg" },
  },
  "/orgs/swaporg/memberships/Smoke": {
    state: "active", role: "admin",
    user: { id: 0, login: "Smoke" },
    organization: { id: 778, login: "swaporg" },
  },
  "/users/statedrift": { id: 888, login: "statedrift", type: "Organization" },
  "/orgs/statedrift/memberships/Smoke": {
    state: "active", role: "admin",
    user: { id: 0, login: "Smoke" },
    organization: { id: 888, login: "statedrift" },
  },
  // Fully grantable like statedrift, so their refusals can only be
  // the name-fidelity checks: 'core' is reserved vocabulary, and
  // 'sm0keorg' skeleton-folds to the claimed 'smokeorg'.
  "/users/core": { id: 900, login: "core", type: "Organization" },
  "/orgs/core/memberships/Smoke": {
    state: "active", role: "admin",
    user: { id: 0, login: "Smoke" },
    organization: { id: 900, login: "core" },
  },
  "/users/sm0keorg": { id: 901, login: "sm0keorg", type: "Organization" },
  "/orgs/sm0keorg/memberships/Smoke": {
    state: "active", role: "admin",
    user: { id: 0, login: "Smoke" },
    organization: { id: 901, login: "sm0keorg" },
  },
};
// POST /__drift/on makes /users/smoke name a different account than
// /user, so the self-claim's id-equality refusal can be exercised and
// then reverted within one run.
let drift = false;
http.createServer((req, res) => {
  res.setHeader("content-type", "application/json");
  if (req.method === "POST" && (req.url === "/__drift/on" || req.url === "/__drift/off")) {
    drift = req.url === "/__drift/on";
    res.end("{}");
  } else if (req.method === "POST" && req.url === "/login/oauth/access_token") {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
      const redirect = new URLSearchParams(body).get("redirect_uri");
      if (redirect !== "https://cabinpkg.com/callback/claim") {
        res.statusCode = 400;
        res.end(JSON.stringify({ error: "redirect_uri_mismatch" }));
        return;
      }
      res.end(JSON.stringify({ access_token: "gho_smoke", token_type: "bearer" }));
    });
  } else if (req.method === "GET" && api[req.url]) {
    if (!/^Bearer gho_smoke$/.test(req.headers.authorization || "")) {
      res.statusCode = 401;
      res.end(JSON.stringify({ message: "Requires authentication" }));
      return;
    }
    if (req.url === "/users/smoke" && drift) {
      res.end(JSON.stringify({ id: 999, login: "smoke", type: "User" }));
      return;
    }
    res.end(JSON.stringify(api[req.url]));
  } else {
    res.statusCode = 404;
    res.end(JSON.stringify({ message: "Not Found" }));
  }
}).listen(port, "127.0.0.1");
MOCK_EOF
node "$mock_dir/github-mock.js" "$github_port" >"$mock_dir/github-mock.log" 2>&1 &
github_pid=$!
disown "$github_pid"
for _ in $(seq 1 20); do
  kill -0 "$github_pid" 2>/dev/null || { cat "$mock_dir/github-mock.log" >&2; fail "the GitHub mock exited early"; }
  [[ "$(curl -sS -o /dev/null -w '%{http_code}' "http://127.0.0.1:${github_port}/user" 2>/dev/null)" == "401" ]] && break
  sleep 0.5
done

# Point the worker's export calls at the mock. Wrangler reads
# .dev.vars for `wrangler dev`; an existing file is saved and restored.
if [[ -f "$dev_vars" ]]; then
  dev_vars_backup="$(mktemp)"
  cp "$dev_vars" "$dev_vars_backup"
fi
# SESSION_SECRET is pinned so the session-plane leg below can mint a
# valid session cookie for the seeded user (github id 0) without a
# GitHub round trip; ALLOWED_GITHUB_IDS admits that id plus id 1, whose
# identity row deliberately does not exist (the post-wipe ghost-session
# case). SERVICE_MODE_TTL_SECS=0 disables the service-mode cache so the
# breaker leg below observes a flipped mode immediately (the deployed
# worker uses the in-code 60 s TTL), and STATS_CACHE_TTL_SECS=0
# disables the stats edge cache so the download-count leg observes a
# fresh count (deployed: 300 s), with DOWNLOAD_FLUSH_INTERVAL_MS=0
# flushing every buffered download count immediately for the same
# reason (deployed: 30 s batches). The GITHUB_* entries point the
# claim flow's server-side calls at the GitHub mock above (the client
# secret only has to exist for the mock exchange).
cat >"$dev_vars" <<EOF
CF_API_BASE="http://127.0.0.1:${mock_port}"
D1_EXPORT_API_TOKEN="smoke-placeholder"
SESSION_SECRET="smoke-session-secret-not-for-production"
ALLOWED_GITHUB_IDS="0,1"
SERVICE_MODE_TTL_SECS="0"
STATS_CACHE_TTL_SECS="0"
DOWNLOAD_FLUSH_INTERVAL_MS="0"
GITHUB_OAUTH_BASE="http://127.0.0.1:${github_port}"
GITHUB_API_BASE="http://127.0.0.1:${github_port}"
GITHUB_CLIENT_SECRET="smoke-client-secret"
EOF
dev_vars_created=1

# A stale server on either port would silently answer the checks below in
# place of the instances started here.
for stale_port in "$port" "$web_port"; do
  if curl -fsS -o /dev/null "http://127.0.0.1:${stale_port}/healthz" 2>/dev/null; then
    fail "something is already serving on port ${stale_port}; kill it first"
  fi
done

# Job control (-m) gives each dev-server tree its own process group so
# cleanup can kill all of it at once.
start_registry_dev() {
  set -m
  wrangler dev --port "$port" --test-scheduled >"$dev_log" 2>&1 &
  dev_pid=$!
  set +m
  disown "$dev_pid"
  for _ in $(seq 1 300); do
    kill -0 "$dev_pid" 2>/dev/null || { cat "$dev_log" >&2; fail "wrangler dev exited early"; }
    curl -fsS -o /dev/null "$base/healthz" 2>/dev/null && return 0
    sleep 1
  done
  fail "wrangler dev never answered /healthz"
}

# The website-role instance: same code, same local state, but wrangler
# pins its emulated Host header to cabinpkg.com, which is what flips the
# Worker's role dispatch. /healthz only exists on the registry role;
# any HTTP status at all (the website role answers it 401/404) proves
# the instance is up.
start_web_dev() {
  set -m
  wrangler dev --port "$web_port" --host cabinpkg.com >"$web_log" 2>&1 &
  web_pid=$!
  set +m
  disown "$web_pid"
  for _ in $(seq 1 300); do
    kill -0 "$web_pid" 2>/dev/null || { cat "$web_log" >&2; fail "the website-role wrangler dev exited early"; }
    [[ "$(curl -sS -o /dev/null -w '%{http_code}' "$web_base/healthz" 2>/dev/null)" != "000" ]] && return 0
    sleep 1
  done
  fail "the website-role wrangler dev never answered"
}

step "starting wrangler dev on port ${port} (first build takes a while)"
start_registry_dev

# Started second so the first instance's build is already cached.
step "starting the website-role wrangler dev on port ${web_port}"
start_web_dev

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
uniform_401 "$base" /packages/smoke/withdep.json
# Non-read-plane paths - the whole API and session surface included -
# are indistinguishable from unknown paths, whatever credential comes
# along.
uniform_401 "$base" /api/v1/packages/smoke/withdep/0.1.0
uniform_401 "$base" /api/v1/user
uniform_401 "$base" "/api/v1/user/search?q=withdep"
uniform_401 "$base" /api/v1/user/package/smoke/withdep
uniform_401 "$base" /unknown/path
uniform_401 "$base" /me
uniform_401 "$base" "/api/v1/admin/versions?status=pending" \
  -H "Authorization: Bearer cabin_definitelyNotAToken"
# Write methods too: the mutation surface simply does not exist here.
uniform_401 "$base" /api/v1/packages/smoke/withdep/0.1.0 -X PUT --data-binary "x"
uniform_401 "$base" /api/v1/packages/smoke/withdep/0.1.0/yank -X PATCH --data-binary "{}"

step "session endpoints answer 401 json without a session (no challenge)"
wcheck /api/v1/user 401
[[ "$(cat "$body")" == "$expected_401" ]] || fail "session 401 body mismatch: $(cat "$body")"
curl -sS -o /dev/null -D "$headers" "$web_base/api/v1/user"
! grep -qi '^www-authenticate:' "$headers" \
  || fail "the session-plane 401 must not carry the bearer challenge"
wcheck /api/v1/user/usage 401
wcheck /api/v1/user/packages 401
wcheck "/api/v1/user/search?q=withdep" 401
wcheck /api/v1/user/package/smoke/withdep 401
wcheck /api/v1/user/package/smoke/withdep/reverse-dependencies 401
wcheck /api/v1/user/tokens 401
wcheck /api/v1/user/logout 401 -X POST

step "the public stats endpoint is unauthenticated json on the website origin"
wcheck /api/v1/stats 200
expect_body '"packages":'
expect_body '"versions":'
expect_body '"downloads":'
# The subtree is its own plane: unknown paths under it are public 404s,
# non-GET is 405, and on the registry host the surface does not exist.
wcheck /api/v1/stats/anything 404
stats_post_status="$(curl -sS -o "$body" -w '%{http_code}' -X POST "$web_base/api/v1/stats")"
[[ "$stats_post_status" == "405" ]] \
  || fail "POST /api/v1/stats returned $stats_post_status, expected 405"
uniform_401 "$base" /api/v1/stats

step "the oauth plane lives on the website origin with host-only cookies"
curl -sS -o /dev/null -D "$headers" "$web_base/login"
grep -q '^HTTP/[^ ]* 302' "$headers" || fail "/login did not answer 302: $(head -1 "$headers")"
# The authorize base is the GitHub mock (GITHUB_OAUTH_BASE above);
# deployed environments use the real https://github.com default.
grep -qi "^location: http://127.0.0.1:${github_port}/login/oauth/authorize" "$headers" \
  || fail "/login redirect is not the authorize page: $(cat "$headers")"
# Ordinary sign-in requests no OAuth scopes; only the claim flow does.
! grep -i '^location: ' "$headers" | grep -q 'scope=' \
  || fail "/login must not request an oauth scope: $(cat "$headers")"
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

step "the wipe script refuses while meta.launched is 'true'"
# The launch guard end to end (docs/runbook.md, "Data policy"): flip the
# local flag, expect scripts/wipe.sh to refuse with the guard's message
# and to leave the state untouched, then flip it back.
wrangler d1 execute DB --local --command \
  "UPDATE meta SET value = 'true' WHERE key = 'launched'" >/dev/null
if wipe_err="$(scripts/wipe.sh --local 2>&1)"; then
  fail "wipe.sh --local ran against a launched registry"
fi
grep -qF "meta.launched = 'true'" <<<"$wipe_err" \
  || fail "wipe.sh refusal is missing the guard's message: $wipe_err"
# Sentinel: the database survived the refusal (and the servers with it).
generation_rows="$(wrangler d1 execute DB --local --json --command \
  "SELECT value FROM meta WHERE key = 'registry_generation'" |
  node -e '
    const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
    console.log(out[0].results.length);
  ')"
[[ "$generation_rows" == "1" ]] \
  || fail "the refused wipe still touched the database"
check /healthz 200
wrangler d1 execute DB --local --command \
  "UPDATE meta SET value = 'false' WHERE key = 'launched'" >/dev/null

if [[ -z "$token" ]]; then
  step "CABIN_REGISTRY_SMOKE_TOKEN not set; skipping authenticated checks"
  echo "smoke OK"
  exit 0
fi

# The two credentials the checks below switch between: the ordinary
# publish/yank token and the verifier's verify-scoped one.
as_publisher() { curl_args=(-H "Authorization: Bearer $token"); }
as_verifier() { curl_args=(-H "Authorization: Bearer $verify_token"); }

# The verification legs below run the same binary the GitHub Actions
# verifier does, so the pending -> verified / rejected transitions exercise
# the real strict-zip profile parser rather than a hand-written verdict.
# Debug is enough: the caps make an optimized build irrelevant here.
step "building the registry verifier (debug)"
if ! verifier_build="$(cd .. && cargo build -p cabinpkg-registry-verify 2>&1)"; then
  printf '%s\n' "$verifier_build" >&2
  fail "failed to build cabinpkg-registry-verify"
fi
verifier_bin="../target/debug/cabin-registry-verify"

# listing_entry <pending-listing> <name> <version> <out>: extract the one
# admin-listing element the verifier binary consumes (the PendingVersion
# shape: name, version, checksum, published_at, metadata).
listing_entry() {
  node -e '
    const fs = require("fs");
    const doc = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
    const v = doc.versions.find((e) => e.name === process.argv[2] && e.version === process.argv[3]);
    if (!v) process.exit(1);
    fs.writeFileSync(process.argv[4], JSON.stringify({
      name: v.name, version: v.version, checksum: v.checksum,
      published_at: v.published_at, metadata: v.metadata,
    }));
  ' "$1" "$2" "$3" "$4" || fail "the pending listing has no $2@$3"
}

# run_verifier <archive> <listing-entry> <out>: run the built binary,
# writing its JSON verdict to <out>. Exit 2 is an operational failure with
# no verdict, which must abort the smoke run rather than pass silently.
run_verifier() {
  "$verifier_bin" "$1" "$2" >"$3" \
    || fail "the verifier binary failed operationally on $1: $(cat "$3")"
}

as_publisher
step "authenticated read routes"
check /config.json 200
grep -q '"auth-required":true' "$body" || fail "config.json missing auth-required: $(cat "$body")"
# The api field names the website origin, crates.io-style.
grep -qF "\"api\":\"${web_origin}\"" "$body" \
  || fail "config.json api is not the website origin: $(cat "$body")"
# 200 only with previously published local data; 404 proves auth + routing.
check /packages/smoke/withdep.json 200 404
check /artifacts/smoke/withdep/smoke-withdep-0.2.0.zip 200 404

step "a valid token changes nothing off the read plane on the registry host"
uniform_401 "$base" /api/v1/packages/smoke/withdep/0.1.0 -H "Authorization: Bearer $token"
uniform_401 "$base" /api/v1/user -H "Authorization: Bearer $token"

step "the read plane is absent on the website origin"
wcheck /config.json 404
wcheck /packages/smoke/withdep.json 404
wcheck /artifacts/smoke/withdep/smoke-withdep-0.2.0.zip 404
wcheck /healthz 404

# --- The session plane, end to end with a minted session. ---
# The session cookie is `<payload>.<hmac>` keyed by SESSION_SECRET, which
# this run pinned above - so a valid session for the seeded user (github
# id 0, admitted by the pinned ALLOWED_GITHUB_IDS) can be minted without
# a GitHub round trip.
session_payload="0:$(($(date +%s) + 3600))"
session_mac="$(printf 'session:%s' "$session_payload" |
  openssl dgst -sha256 -hmac "smoke-session-secret-not-for-production" | sed 's/^.* //')"
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

step "a valid session whose identity row is gone answers 401 everywhere"
# The post-wipe ghost: allowlisted id 1 has a validly sealed cookie but
# no identity row - every endpoint (the token routes included) answers
# the same 401 as no session, never an empty listing or a 500.
ghost_payload="1:$(($(date +%s) + 3600))"
ghost_mac="$(printf 'session:%s' "$ghost_payload" |
  openssl dgst -sha256 -hmac "smoke-session-secret-not-for-production" | sed 's/^.* //')"
real_session_cookie="$session_cookie"
session_cookie="cabin_session=${ghost_payload}.${ghost_mac}"
session_request GET /api/v1/user 401
session_request GET /api/v1/user/tokens 401
session_request POST /api/v1/user/tokens 401 \
  "${csrf_headers[@]}" --data-binary '{"name":"ghost","scopes":[]}'
# Logout is the one exception: a validly sealed cookie is always
# cleared, identity row or not.
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

# --- The claim flow, end to end against the GitHub mock. ---

# claim_scope <scope> <granted|denied>: drive the claim flow - initiate,
# capture the sealed state cookie and the authorize redirect's state,
# then complete the callback (the mock exchanges any code).
claim_scope() {
  local scope="$1" expected="$2"
  curl -sS -o /dev/null -D "$headers" "$web_base/claim/$scope"
  grep -q '^HTTP/[^ ]* 302' "$headers" || fail "/claim/$scope did not answer 302: $(head -1 "$headers")"
  local location claim_cookie state
  location="$(grep -i '^location: ' "$headers" | sed 's/^[^:]*: //' | tr -d '\r')"
  case "$location" in
    *"/login/oauth/authorize?"*) ;;
    *) fail "/claim/$scope redirect is not the authorize page: $location" ;;
  esac
  # The dedicated roundtrip's shape: read:org, and the subdirectory
  # callback GitHub accepts under the registered /callback URL.
  case "$location" in
    *"scope=read%3Aorg"*) ;;
    *) fail "the claim authorize request must ask for read:org: $location" ;;
  esac
  case "$location" in
    *"redirect_uri=https%3A%2F%2Fcabinpkg.com%2Fcallback%2Fclaim"*) ;;
    *) fail "the claim redirect_uri is not /callback/claim: $location" ;;
  esac
  state="$(printf '%s' "$location" | sed -n 's/.*[?&]state=\([0-9a-f]*\).*/\1/p')"
  [[ -n "$state" ]] || fail "no state in the authorize redirect: $location"
  claim_cookie="$(grep -i '^set-cookie: cabin_claim_state=' "$headers" |
    sed 's/^[^:]*: cabin_claim_state=\([^;]*\);.*/\1/' | tr -d '\r')"
  [[ -n "$claim_cookie" ]] || fail "/claim/$scope set no claim-state cookie: $(cat "$headers")"
  curl -sS -o /dev/null -D "$headers" -H "Cookie: cabin_claim_state=$claim_cookie" \
    "$web_base/callback/claim?code=smoke&state=$state"
  location="$(grep -i '^location: ' "$headers" | sed 's/^[^:]*: //' | tr -d '\r')"
  [[ "$location" == "/dashboard?claim=$expected" ]] \
    || fail "/callback/claim for $scope answered '$location', expected claim=$expected"
  # The claim-state cookie is one-shot: cleared on every outcome.
  grep -i '^set-cookie: cabin_claim_state=' "$headers" | grep -q 'Max-Age=0' \
    || fail "the claim callback did not clear the state cookie: $(cat "$headers")"
  printf '    claim %s -> %s\n' "$scope" "$expected"
}

step "the claim-state cookie is scoped to the claim callback"
curl -sS -o /dev/null -D "$headers" "$web_base/claim/smoke"
claim_cookie_line="$(grep -i '^set-cookie: cabin_claim_state=' "$headers" || true)"
[[ -n "$claim_cookie_line" ]] || fail "/claim/smoke set no claim-state cookie: $(cat "$headers")"
for attribute in "Path=/callback/claim" HttpOnly Secure "SameSite=Lax"; do
  case "$claim_cookie_line" in
    *"$attribute"*) ;;
    *) fail "claim-state cookie is missing $attribute: $claim_cookie_line" ;;
  esac
done
! printf '%s' "$claim_cookie_line" | grep -qi 'domain=' \
  || fail "the claim-state cookie must be host-only: $claim_cookie_line"

step "a self-claim is refused when /users/<scope> is another account"
# With the drift toggle on, /users/smoke names account 999 while the
# authenticated /user is account 0; 'smoke' is still unclaimed, so the
# id-equality binding is the only thing refusing here.
curl -fsS -X POST -o /dev/null "http://127.0.0.1:${github_port}/__drift/on"
claim_scope smoke denied
curl -fsS -X POST -o /dev/null "http://127.0.0.1:${github_port}/__drift/off"

step "a self-claim grants the scope, frozen to the account's numeric id"
# The mock /user answers login 'Smoke'; the grant compares it lowercased.
claim_scope smoke granted
proof="$(wrangler d1 execute DB --local --json --command \
  "SELECT proof_provider, proof_account_id FROM scopes WHERE name = 'smoke'" |
  node -e '
    const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
    const row = out[0].results[0];
    console.log(`${row.proof_provider}:${row.proof_account_id}`);
  ')"
[[ "$proof" == "github:0" ]] \
  || fail "the claim did not freeze the numeric proof: $proof"

step "an org claim needs an active admin membership bound by numeric ids"
claim_scope smokeorg granted
# A plain member, a membership naming a different user than the
# authenticated one, and a membership naming a different organization
# than /users/<scope> resolves must all refuse.
claim_scope denyorg denied
claim_scope imposterorg denied
claim_scope swaporg denied

step "reserved and skeleton-confusable scopes refuse uniformly"
# Both are fully grantable in the mock (like statedrift), so nothing
# but the name-fidelity checks can be what refused them: 'core' is
# reserved vocabulary, 'sm0keorg' skeleton-folds to the just-claimed
# 'smokeorg'. The exemption override (CLAIM_SKELETON_EXEMPT_SCOPES)
# would need a wrangler restart with the var set, so it stays covered
# by the host tests only.
claim_scope core denied
claim_scope sm0keorg denied

step "claims are permanent: a re-claim refuses even the owning account"
claim_scope smoke denied

step "a claim callback without a valid matching state is refused"
curl -sS -o /dev/null -D "$headers" "$web_base/callback/claim"
grep -qi '^location: /dashboard?claim=denied' "$headers" \
  || fail "a bare claim callback did not refuse: $(cat "$headers")"
# A sealed cookie with a mismatched state parameter refuses before any
# GitHub call. 'statedrift' is unclaimed AND fully grantable in the
# mock, so nothing but the state comparison can be what refused it.
curl -sS -o /dev/null -D "$headers" "$web_base/claim/statedrift"
drift_cookie="$(grep -i '^set-cookie: cabin_claim_state=' "$headers" |
  sed 's/^[^:]*: cabin_claim_state=\([^;]*\);.*/\1/' | tr -d '\r')"
[[ -n "$drift_cookie" ]] || fail "/claim/statedrift set no claim-state cookie"
curl -sS -o /dev/null -D "$headers" -H "Cookie: cabin_claim_state=$drift_cookie" \
  "$web_base/callback/claim?code=smoke&state=deadbeef"
grep -qi '^location: /dashboard?claim=denied' "$headers" \
  || fail "a mismatched claim state did not refuse: $(cat "$headers")"

# --- Membership management through the session API. ---
step "scope owners list, add, and remove members"
session_request GET /api/v1/user/scopes/smoke/members 200
expect_body '"github_id":0'
expect_body '"role":"owner"'
session_request POST /api/v1/user/scopes/smoke/members 200 \
  "${csrf_headers[@]}" --data-binary '{"github_id":2,"role":"member"}'
expect_body '"changed":true'
session_request GET /api/v1/user/scopes/smoke/members 200
expect_body '"login":"friend"'
# An existing member keeps their role: no role-change endpoint.
session_request POST /api/v1/user/scopes/smoke/members 200 \
  "${csrf_headers[@]}" --data-binary '{"github_id":2,"role":"owner"}'
expect_body '"role":"member"'
expect_body '"changed":false'
session_request POST /api/v1/user/scopes/smoke/members 400 \
  "${csrf_headers[@]}" --data-binary '{"github_id":999,"role":"member"}'
expect_body 'no registry account'
session_request POST /api/v1/user/scopes/smoke/members 403 \
  -H "Content-Type: application/json" --data-binary '{"github_id":2,"role":"member"}'
expect_body 'X-CSRF-Protection'
session_request POST /api/v1/user/scopes/smoke/members/2/remove 200 "${csrf_headers[@]}"
expect_body '"changed":true'
session_request POST /api/v1/user/scopes/smoke/members/2/remove 200 "${csrf_headers[@]}"
expect_body '"changed":false'

step "the last owner cannot be removed"
session_request POST /api/v1/user/scopes/smoke/members/0/remove 409 "${csrf_headers[@]}"
expect_body 'last owner'

step "the owner gate is one uniform 403 for foreign and unclaimed scopes"
session_request GET /api/v1/user/scopes/foreign/members 403
expect_body 'not an owner'
cp "$body" "$mock_dir/owner-403.json"
session_request GET /api/v1/user/scopes/ghost/members 403
cmp -s "$body" "$mock_dir/owner-403.json" \
  || fail "foreign-scope and unclaimed-scope owner 403s differ: $(cat "$body")"

step "authenticated responses carry the generation header"
curl -sS -o /dev/null -D "$body" ${curl_args[@]+"${curl_args[@]}"} "$base/config.json"
grep -qi '^x-cabin-registry-generation:' "$body" \
  || fail "missing x-cabin-registry-generation header"

# The frozen conformance fixture (tests/fixtures/, regenerated by
# scripts/gen-fixtures.sh) doubles as the smoke publish payload verbatim:
# gen-fixtures.sh authors its packages under the `smoke` scope precisely
# so the pair this flow publishes matches the scope claimed above.
scope="smoke"
name="withdep"
version="0.2.0"
fixture_archive="tests/fixtures/$scope-$name-$version.zip"
publish_path="/api/v1/packages/$scope/$name/$version"
package_path="/packages/$scope/$name.json"
artifact_path="/artifacts/$scope/$name/$scope-$name-$version.zip"
blob_hash="$(shasum -a 256 "$fixture_archive" | cut -d' ' -f1)"
work="$(mktemp -d)"
trap 'cleanup; rm -rf "$work" "$mock_dir"' EXIT
fixture_metadata="tests/fixtures/$scope-$name-$version.json"
grep -qF "\"$scope/$name\"" "$fixture_metadata" \
  || fail "the frozen fixture must carry the scoped name $scope/$name"

step "a bare dependency key is a 400 before any write"
# The fixture depends on the scoped smoke/nodep; stripping the scope
# must be refused (the checksum and identity fields are untouched, so
# the dependency-key check is what fires).
sed 's|"smoke/nodep"|"nodep"|' "$fixture_metadata" >"$work/bare-dep.json"
frame "$work/bare-dep.json" "$fixture_archive" "$work/publish-bare.bin"
wrequest PUT "$publish_path" "$work/publish-bare.bin" 400
expect_body 'canonical <scope>/<name> names'
# The refused attempt still charged the publish bucket (the rate limit
# sits before validation); refund it so the downstream legs keep the
# budget they were written against.
wrangler d1 execute DB --local --command "
  UPDATE tokens SET rl_tokens = NULL, rl_updated_at = NULL WHERE id = 'smoke';" >/dev/null

step "first publish creates the version pending verification"
frame "$fixture_metadata" "$fixture_archive" "$work/publish.bin"
wrequest PUT "$publish_path" "$work/publish.bin" 201
expect_body '"ok":true'
expect_body "\"name\":\"$scope/$name\""
expect_body '"verification":"pending"'

step "reserved and -/_ twin package names are 400s"
# The fixture pair renamed to `with-dep` (name and source path; the
# archive bytes and checksum are untouched, and the shared blob is not
# re-counted) creates the twinnable package; its `_` twin must then be
# the deterministic 400 with no second row.
sed 's|withdep|with-dep|g' "$fixture_metadata" >"$work/twin.json"
frame "$work/twin.json" "$fixture_archive" "$work/publish-twin.bin"
wrequest PUT "/api/v1/packages/$scope/with-dep/$version" "$work/publish-twin.bin" 201
sed 's|withdep|with_dep|g' "$fixture_metadata" >"$work/twin-under.json"
frame "$work/twin-under.json" "$fixture_archive" "$work/publish-twin-under.bin"
wrequest PUT "/api/v1/packages/$scope/with_dep/$version" "$work/publish-twin-under.bin" 400
expect_body "differs only in"
# A reserved name answers in the same validation 400 family.
sed 's|withdep|con|g' "$fixture_metadata" >"$work/reserved.json"
frame "$work/reserved.json" "$fixture_archive" "$work/publish-reserved.bin"
wrequest PUT "/api/v1/packages/$scope/con/$version" "$work/publish-reserved.bin" 400
expect_body 'reserved'
# Like the bare-dep leg: refund the extra publish charges so the
# downstream legs keep the budget they were written against.
wrangler d1 execute DB --local --command "
  UPDATE tokens SET rl_tokens = NULL, rl_updated_at = NULL WHERE id = 'smoke';" >/dev/null

step "pending versions are invisible to ordinary tokens"
check "$package_path" 404
check "$artifact_path" 404
# And they have no backup copy: only versions that become verified
# enter the durable backup set (the verdict batch enqueues the work).
sleep 1
if wrangler r2 object get "cabin-registry-backup/blobs/sha256/$blob_hash" \
    --file /dev/null --local >/dev/null 2>&1; then
  fail "a pending version's blob was replicated to the BACKUP bucket"
fi
# The source viewer gates on verified the same way; a valid range makes
# sure the 404 is the gate, not the range policy (checked first).
session_request GET "/api/v1/user/source/$scope/$name/$version" 404 -H "Range: bytes=-22"
# So do search and the package routes: a pending-only package has no
# hits, no detail, and no dependents.
session_request GET "/api/v1/user/search?q=$name" 200
expect_body '"results":[]'
session_request GET "/api/v1/user/package/$scope/$name" 404
session_request GET "/api/v1/user/package/$scope/$name/reverse-dependencies" 404
wcheck "/api/v1/admin/versions?status=pending" 403
expect_body 'verify scope'
wcheck "/api/v1/admin/packages" 403
expect_body 'verify scope'
printf '{"verdict":"verified"}' >"$work/verdict-unbound.json"
wrequest PATCH "/api/v1/admin/versions/$scope/$name/$version" "$work/verdict-unbound.json" 403
expect_body 'verify scope'

step "the verify scope lists and downloads pending versions"
as_verifier
# Content-Length is only an optimization: a chunked request must hit the
# same cap while the stream is read and leave the pending row untouched.
# The body must be a *semantically valid* rejected verdict - padded past
# the cap with whitespace inside the JSON document - so an uncapped
# handler would parse and apply it, failing the pending-row checks below.
printf '{"verdict":"rejected","reason":"oversized"%4097s}' '' \
  >"$work/oversized-verdict.json"
oversized_status="$(curl -sS -o "$body" -w '%{http_code}' -X PATCH \
  -H "Transfer-Encoding: chunked" --data-binary "@$work/oversized-verdict.json" \
  ${curl_args[@]+"${curl_args[@]}"} \
  "$web_base/api/v1/admin/versions/$scope/$name/$version")"
[[ "$oversized_status" == "400" ]] \
  || fail "oversized chunked verdict returned $oversized_status, expected 400 (body: $(cat "$body"))"
expect_body 'the verdict body must be'
wcheck "/api/v1/admin/versions?status=pending" 200
expect_body "\"name\":\"$scope/$name\""
expect_body '"version":"0.2.0"'
expect_body '"published_by":1'
expect_body '"metadata":{'
cp "$body" "$work/pending.json"
wcheck "/api/v1/admin/versions?status=bogus" 400
# The corpus the name advisories read: every package, ordered, with
# its vetted (any-version-verified) bit - both fixtures still pending.
wcheck "/api/v1/admin/packages" 200
expect_body '"packages":['
expect_body "\"scope\":\"$scope\",\"name\":\"with-dep\",\"vetted\":false"
expect_body "\"scope\":\"$scope\",\"name\":\"$name\",\"vetted\":false"
cp "$body" "$work/corpus.json"
# The verifier downloads the pending artifact and inspects it out of band.
curl -sS -o "$work/download.zip" ${curl_args[@]+"${curl_args[@]}"} "$base$artifact_path"
[[ "$(shasum -a 256 "$work/download.zip" | cut -d' ' -f1)" == "$blob_hash" ]] \
  || fail "the pending download differs from the published archive"

step "a verified verdict must name the listing it inspected"
wrequest PATCH "/api/v1/admin/versions/$scope/$name/$version" "$work/verdict-unbound.json" 400
expect_body 'requires the checksum'

step "the advisory gate abstains on the skeleton-equal fixture pair"
# `withdep` and `with-dep` fold to the same skeleton, and neither is
# vetted yet, so the workflow's pre-download gate abstains on the real
# corpus - exercised through the real binary. The bound verdict below
# then plays the operator's manual resolution from the runbook.
listing_entry "$work/pending.json" "$scope/$name" "$version" "$work/entry.json"
"$verifier_bin" --name-advisories "$work/entry.json" "$work/corpus.json" >"$work/advice.json" \
  || fail "the advisory mode failed operationally: $(cat "$work/advice.json")"
grep -qF "\"advice\":\"abstain\"" "$work/advice.json" \
  || fail "the skeleton-equal pair did not abstain: $(cat "$work/advice.json")"
grep -qF "confusable_package ($scope/with-dep)" "$work/advice.json" \
  || fail "the abstain does not name its rule: $(cat "$work/advice.json")"

step "the real verifier verifies the fixture and the verdict makes it resolvable"
run_verifier "$work/download.zip" "$work/entry.json" "$work/verdict-real.json"
grep -qF '"verdict":"verified"' "$work/verdict-real.json" \
  || fail "the verifier did not verify the fixture: $(cat "$work/verdict-real.json")"
# The verdict binds to the checksum and published_at the listing reported.
node -e '
  const fs = require("fs");
  const entry = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  fs.writeFileSync(process.argv[2], JSON.stringify({
    verdict: "verified", checksum: entry.checksum, published_at: entry.published_at,
  }));' "$work/entry.json" "$work/verdict-verified.json"
wrequest PATCH "/api/v1/admin/versions/$scope/$name/$version" "$work/verdict-verified.json" 200
expect_body '"verification":"verified"'
expect_body '"changed":true'
as_publisher
check "$package_path" 200
expect_body "\"name\":\"$scope/$name\""
expect_body '"0.2.0"'
# This is the first verified download (a cache-miss fill), so it also
# proves the outward answer to an authenticated request never licenses
# a shared cache: the `public` freshness header lives only on the
# internal cache copy, and the client sees no-store.
first_download_status="$(curl -sS -o /dev/null -D "$headers" -w '%{http_code}' \
  ${curl_args[@]+"${curl_args[@]}"} "$base$artifact_path")"
[[ "$first_download_status" == "200" ]] \
  || fail "the first verified download returned $first_download_status"
grep -qi '^cache-control: no-store' "$headers" \
  || fail "a cache-miss artifact is missing the outward no-store: $(grep -i cache-control "$headers")"

step "a verified version flips the corpus row to vetted"
as_verifier
wcheck "/api/v1/admin/packages" 200
expect_body "\"scope\":\"$scope\",\"name\":\"$name\",\"vetted\":true"
expect_body "\"scope\":\"$scope\",\"name\":\"with-dep\",\"vetted\":false"
as_publisher

step "search and the package routes see the verified version"
session_request GET "/api/v1/user/search?q=$name" 200
expect_body "\"scope\":\"$scope\",\"name\":\"$name\",\"version\":\"$version\""
# A whitespace-only query is the fixed 400 detail.
session_request GET "/api/v1/user/search?q=%20" 400
expect_body '1 to 64 characters'
session_request GET "/api/v1/user/package/$scope/$name" 200
expect_body "\"newest_version\":\"$version\""
expect_body '"smoke/nodep":"^0.1"'
session_request GET "/api/v1/user/package/$scope/$name/reverse-dependencies" 200
expect_body '"dependents":[]'
# The fixture's dependency itself was never published: an invisible
# target is the authenticated 404, before any dependents walk.
session_request GET /api/v1/user/package/smoke/nodep/reverse-dependencies 404

step "verdicts are idempotent for the same value and conflict otherwise"
as_verifier
wrequest PATCH "/api/v1/admin/versions/$scope/$name/$version" "$work/verdict-verified.json" 200
expect_body '"changed":false'
printf '{"verdict":"rejected","reason":"smoke rejection"}' >"$work/verdict-rejected.json"
wrequest PATCH "/api/v1/admin/versions/$scope/$name/$version" "$work/verdict-rejected.json" 409
expect_body 'immutable'
as_publisher

step "verified downloads count; the verifier's pending fetch never did"
# The counter lands off the response path (waitUntil), so poll the
# version row itself - per-row, so verified packages left in the local
# state by other work never skew the expectation. The row was
# recreated by this run's publish, and exactly one verified download
# has happened, while the verifier fetched the artifact when it was
# still pending - so 1 here also proves the pending fetch never
# counted.
await_row_downloads() {
  local expected="$1" row_downloads=""
  for _ in $(seq 1 20); do
    row_downloads="$(wrangler d1 execute DB --local --json --command \
      "SELECT downloads FROM versions
       WHERE scope = 'smoke' AND name = 'withdep' AND version = '0.2.0'" |
      node -e '
        const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
        console.log(out[0].results[0].downloads);
      ')"
    [[ "$row_downloads" == "$expected" ]] &&
      { printf '    downloads(smoke/withdep@0.2.0) = %s\n' "$expected"; return 0; }
    sleep 0.5
  done
  fail "smoke/withdep@0.2.0 downloads never reached $expected (last: $row_downloads)"
}
await_row_downloads 1
# The public totals reflect served downloads; >= keeps the assertion
# meaningful whatever else the local state holds.
curl -sS -o "$body" "$web_base/api/v1/stats"
node -e '
  const stats = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
  for (const key of ["packages", "versions", "downloads"]) {
    if (!Number.isInteger(stats[key]) || stats[key] < 1) process.exit(1);
  }' "$body" || fail "stats totals do not reflect the verified download: $(cat "$body")"
check "$artifact_path" 200
await_row_downloads 2

# await_backup_blob <key> <out-file>: replication runs via waitUntil
# after the response, so poll the BACKUP bucket briefly.
await_backup_blob() {
  local key="$1" out="$2"
  for _ in $(seq 1 20); do
    wrangler r2 object get "cabin-registry-backup/$key" \
      --file "$out" --local >/dev/null 2>&1 && return 0
    sleep 0.5
  done
  fail "blob $key never appeared in the BACKUP bucket"
}

# The verdict batch enqueued the backup work transactionally with the
# verified transition, and the verdict's waitUntil drain replicated it.
step "the verified blob replicates to the BACKUP bucket and drains its queue row"
await_backup_blob "blobs/sha256/$blob_hash" "$work/replicated.zip"
cmp -s "$work/replicated.zip" "$fixture_archive" \
  || fail "replicated blob differs from the published archive"
for _ in $(seq 1 20); do
  queue_rows="$(wrangler d1 execute DB --local --json --command \
    "SELECT COUNT(*) AS n FROM backup_pending
     WHERE key = 'blobs/sha256/$blob_hash'" |
    node -e '
      const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
      console.log(out[0].results[0].n);
    ')"
  [[ "$queue_rows" == "0" ]] && break
  sleep 0.5
done
[[ "$queue_rows" == "0" ]] || fail "the drained backup queue row was not deleted"

# The backup set is durable through the queue, not through re-publish:
# an idempotent no-op heals a reclaim-raced primary blob (the retry
# holds the bytes), while the append-only backup bucket is never
# rewritten by the publish path - a lost backup object is
# scripts/backup-backfill.sh territory.
step "an idempotent re-publish heals the primary blob only"
wrangler r2 object delete "cabin-registry-backup/blobs/sha256/$blob_hash" --local >/dev/null
wrangler r2 object delete "cabin-registry-blobs/blobs/sha256/$blob_hash" --local >/dev/null
wrequest PUT "$publish_path" "$work/publish.bin" 200
expect_body '"no_op":true'
expect_body '"verification":"verified"'
# The heal runs before the response, so the primary object itself is
# back (the artifact route could otherwise answer from the edge cache).
wrangler r2 object get "cabin-registry-blobs/blobs/sha256/$blob_hash" \
  --file "$work/healed.zip" --local >/dev/null \
  || fail "the idempotent re-publish did not heal the primary blob"
cmp -s "$work/healed.zip" "$fixture_archive" \
  || fail "the healed primary blob differs from the published archive"
check "$artifact_path" 200
sleep 1
if wrangler r2 object get "cabin-registry-backup/blobs/sha256/$blob_hash" \
    --file /dev/null --local >/dev/null 2>&1; then
  fail "a re-publish rewrote the append-only BACKUP bucket"
fi
# Put the copy back so later legs and the local state stay coherent.
wrangler r2 object put "cabin-registry-backup/blobs/sha256/$blob_hash" \
  --file "$fixture_archive" --local >/dev/null

step "byte-identical re-publish is an idempotent no-op reporting the status"
wrequest PUT "$publish_path" "$work/publish.bin" 200
expect_body '"no_op":true'
expect_body '"verification":"verified"'

step "tampered re-publish hits the immutability wall"
tamper_zip "$fixture_archive" "$work/tampered.zip" 1
old_hash="$(shasum -a 256 "$fixture_archive" | cut -d' ' -f1)"
new_hash="$(shasum -a 256 "$work/tampered.zip" | cut -d' ' -f1)"
sed "s/$old_hash/$new_hash/" "$fixture_metadata" >"$work/tampered.json"
frame "$work/tampered.json" "$work/tampered.zip" "$work/tampered.bin"
wrequest PUT "$publish_path" "$work/tampered.bin" 409
expect_body 'immutable'

step "yank and un-yank walk the state transitions"
printf '{"yanked":true}' >"$work/yank.json"
wrequest PATCH "$publish_path/yank" "$work/yank.json" 200
expect_body '"yanked":true'
expect_body '"changed":true'
check "$package_path" 200
expect_body '"yanked":true'
# The session packages listing mirrors the row: the seeded user created
# the package, its version is verified by now, and currently yanked.
session_request GET /api/v1/user/packages 200
expect_body "\"name\":\"$scope/$name\""
expect_body '"verification":"verified"'
expect_body '"yanked":true'
# Yanked stays browsable in the source viewer, like the artifact route.
session_request GET "/api/v1/user/source/$scope/$name/$version" 206 -H "Range: bytes=-22"
printf '{"yanked":false}' >"$work/unyank.json"
wrequest PATCH "$publish_path/yank" "$work/unyank.json" 200
expect_body '"yanked":false'
expect_body '"changed":true'
check "$package_path" 200
expect_body '"yanked":false'

step "published artifact downloads with the published checksum"
curl -sS -o "$work/artifact.zip" ${curl_args[@]+"${curl_args[@]}"} \
  "$base$artifact_path"
got_hash="$(shasum -a 256 "$work/artifact.zip" | cut -d' ' -f1)"
[[ "$got_hash" == "$old_hash" ]] \
  || fail "artifact checksum mismatch: got $got_hash, expected $old_hash"
grep -qF "sha256:$old_hash" "$fixture_metadata" \
  || fail "fixture metadata does not carry sha256:$old_hash"

# --- The source viewer's session-ranged reads. ---
step "the source route serves session-ranged reads of the verified archive"
source_path="/api/v1/user/source/$scope/$name/$version"
archive_size="$(wc -c <"$fixture_archive" | tr -d ' ')"

# The version row's download counter, for the never-counted assertion.
row_downloads() {
  wrangler d1 execute DB --local --json --command \
    "SELECT downloads FROM versions
     WHERE scope = 'smoke' AND name = 'withdep' AND version = '0.2.0'" |
    node -e '
      const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
      console.log(out[0].results[0].downloads);'
}
# The two artifact fetches since the counted-downloads step (the heal
# re-check and the checksum download) land their deferred increments
# here; awaiting the exact count keeps the flat-counter assertion below
# race-free. A new artifact fetch above must bump this number.
await_row_downloads 4

# source_range <range-or-empty> <expected>: a source-route read with the
# minted session; body in $body, headers in $headers.
source_range() {
  local range="$1" expected="$2" got
  local range_args=()
  [[ -n "$range" ]] && range_args=(-H "Range: $range")
  got="$(curl -sS -o "$body" -D "$headers" -w '%{http_code}' \
    -H "Cookie: $session_cookie" ${range_args[@]+"${range_args[@]}"} \
    "$web_base$source_path")"
  [[ "$got" == "$expected" ]] ||
    fail "source ${range:-<no range>} returned $got, expected $expected (body: $(cat "$body"))"
  printf '    source %s -> %s\n' "${range:-<no range>}" "$got"
}
# source_header <pattern>: the last response must carry the header.
source_header() {
  tr -d '\r' <"$headers" | grep -qi "$1" \
    || fail "missing header $1: $(tr -d '\r' <"$headers" | grep -i '^[a-z-]*:' | head -20)"
}

# No credential and a bearer token both answer the session plane's plain
# 401: the route never accepts the machine plane's credential.
got="$(curl -sS -o "$body" -w '%{http_code}' -H "Range: bytes=-22" "$web_base$source_path")"
[[ "$got" == "401" ]] || fail "a session-less source read answered $got"
got="$(curl -sS -o "$body" -w '%{http_code}' -H "Range: bytes=-22" \
  -H "Authorization: Bearer $token" "$web_base$source_path")"
[[ "$got" == "401" ]] || fail "a bearer token opened the source route: $got"

# The range policy: required (400 when absent), single, bounded, capped
# at 4 MiB (416 otherwise).
source_range "" 400
grep -q 'bounded range' "$body" || fail "the 400 does not name the range policy: $(cat "$body")"
for bad in "bytes=0-" "bytes=abc-5" "bytes=0-5,10-20" "bytes=-0" "bytes=0-4194304"; do
  source_range "$bad" 416
done
# A start past the end is the size-relative 416 naming the actual size.
source_range "bytes=$archive_size-$((archive_size + 10))" 416
source_header "^content-range: bytes \*/$archive_size$"

# A suffix read returns the exact EOCD bytes with the exact headers,
# no-store and nosniff like every session-plane response.
source_range "bytes=-22" 206
tail -c 22 "$fixture_archive" >"$work/eocd.expected"
cmp -s "$body" "$work/eocd.expected" || fail "the EOCD suffix read differs from the archive tail"
source_header "^content-range: bytes $((archive_size - 22))-$((archive_size - 1))/$archive_size$"
source_header "^content-length: 22$"
source_header "^cache-control: no-store$"
source_header "^x-content-type-options: nosniff$"
source_header "^accept-ranges: bytes$"
# A bounded read slices the archive's first bytes; an end past the last
# byte is clamped HTTP-style.
source_range "bytes=0-3" 206
head -c 4 "$fixture_archive" >"$work/magic.expected"
cmp -s "$body" "$work/magic.expected" || fail "the bounded read differs from the archive head"
source_range "bytes=$((archive_size - 10))-$((archive_size + 100))" 206
source_header "^content-range: bytes $((archive_size - 10))-$((archive_size - 1))/$archive_size$"

# Unknown versions and unparsable triples answer the plain 404.
session_request GET "/api/v1/user/source/$scope/$name/9.9.9" 404 -H "Range: bytes=-22"
session_request GET "/api/v1/user/source/$scope/$name/notsemver" 404

# Source reads are never downloads: the counter did not move.
sleep 1
[[ "$(row_downloads)" == "4" ]] \
  || fail "source reads moved the download counter: $(row_downloads) (expected 4)"

# The dev vars pin SERVICE_MODE_TTL_SECS to 0, so the running worker sees
# the flipped mode immediately instead of after the 60 s cache TTL.
step "writes answer 503 while writes_blocked; reads stay open"
wrangler d1 execute DB --local --command "
  UPDATE meta SET value = 'writes_blocked' WHERE key = 'service_mode';
  UPDATE meta SET value = 'forced by smoke.sh' WHERE key = 'service_mode_reason';"
wrequest PUT "$publish_path" "$work/publish.bin" 503
expect_body 'registry_over_budget'
wrequest PATCH "$publish_path/yank" "$work/unyank.json" 503
expect_body 'registry_over_budget'
check "$package_path" 200
# Source reads are reads: they never consult the service mode.
session_request GET "$source_path" 206 -H "Range: bytes=-22"
# Verdicts are deliberately exempt from the budget gates: the idempotent
# repeat lands (the queue drains while blocked), and an unknown triple
# is the authenticated 404, never the 503.
as_verifier
wrequest PATCH "/api/v1/admin/versions/$scope/$name/$version" "$work/verdict-verified.json" 200
expect_body '"changed":false'
wrequest PATCH "/api/v1/admin/versions/$scope/$name/9.9.9" "$work/verdict-verified.json" 404
as_publisher

step "reads answer 503 while reads_blocked; the exempt planes stay open"
wrangler d1 execute DB --local --command "
  UPDATE meta SET value = 'reads_blocked' WHERE key = 'service_mode';"
downloads_before="$(row_downloads)"
# The data plane refuses with the read-side envelope and the
# cron-cadence Retry-After; writes stay blocked too (reads_blocked sits
# above writes_blocked on the ladder).
got="$(curl -sS -o "$body" -D "$headers" -w '%{http_code}' \
  ${curl_args[@]+"${curl_args[@]}"} "$base$package_path")"
[[ "$got" == "503" ]] || fail "a read under reads_blocked answered $got"
expect_body 'registry_over_budget'
expect_body 'read budget'
tr -d '\r' <"$headers" | grep -qi '^retry-after: 900$' \
  || fail "the read 503 must carry Retry-After: 900"
check "$artifact_path" 503
check /config.json 503
wrequest PUT "$publish_path" "$work/publish.bin" 503
# Unauthenticated callers cannot observe service state: the uniform 401
# is byte-identical, and /healthz stays up.
uniform_401 "$base" /config.json
check /healthz 200
# The exempt planes: the session plane and the public stats (where
# operators and users see what is happening), the admin plane, and the
# verifier's config and artifact fetches - but not package documents,
# which the verifier never reads.
session_request GET "$source_path" 206 -H "Range: bytes=-22"
wcheck /api/v1/stats 200
as_verifier
wcheck "/api/v1/admin/versions?status=pending" 200
check /config.json 200
check "$artifact_path" 200
check "$package_path" 503
as_publisher
# The exempt fetch was served, but the download counter follows the
# write plane's fail-closed rule and must not have moved.
sleep 1
[[ "$(row_downloads)" == "$downloads_before" ]] \
  || fail "a reads_blocked download moved the counter: $(row_downloads) (expected $downloads_before)"

step "restoring service_mode reopens writes"
wrangler d1 execute DB --local --command "
  UPDATE meta SET value = 'normal' WHERE key = 'service_mode';
  UPDATE meta SET value = '' WHERE key = 'service_mode_reason';"
wrequest PUT "$publish_path" "$work/publish.bin" 200
expect_body '"no_op":true'

# --- The reject -> blob reclaim -> quota refund -> republish flow. ---
# The PUTs above consumed the publish bucket's full burst; give this leg
# its own by resetting the token's bucket columns.
wrangler d1 execute DB --local --command "
  UPDATE tokens SET rl_tokens = NULL, rl_updated_at = NULL WHERE id = 'smoke';"

# meta.total_stored_bytes, the exact storage self-accounting.
stored_bytes() {
  wrangler d1 execute DB --local --json --command \
    "SELECT value FROM meta WHERE key = 'total_stored_bytes'" |
    node -e '
      const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
      console.log(out[0].results[0].value);'
}

# 0.2.1 with the exact archive 0.2.0 published: the shared-blob case.
version2="0.2.1"
publish2_path="/api/v1/packages/$scope/$name/$version2"
artifact2_path="/artifacts/$scope/$name/$scope-$name-$version2.zip"
verdict2_path="/api/v1/admin/versions/$scope/$name/$version2"
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
wrequest PATCH "$verdict2_path" "$work/verdict-stale.json" 409
expect_body 'changed since it was listed'

step "rejecting a version sharing its blob keeps the blob and the accounting"
# Bound to the listed checksum: the verdict applies only to these bytes.
printf '{"verdict":"rejected","reason":"smoke rejection","checksum":"%s"}' "$blob_hash" \
  >"$work/verdict-rejected-bound.json"
wrequest PATCH "$verdict2_path" "$work/verdict-rejected-bound.json" 200
expect_body '"verification":"rejected"'
expect_body '"changed":true'
check "$artifact2_path" 404
# Rejected versions are invisible to the source viewer too.
session_request GET "/api/v1/user/source/$scope/$name/$version2" 404 -H "Range: bytes=-22"
as_publisher
[[ "$(stored_bytes)" == "$before_bytes" ]] \
  || fail "rejecting a shared blob changed the accounting: $(stored_bytes) (was $before_bytes)"
check "$artifact_path" 200
check "$package_path" 200
! grep -qF '"0.2.1"' "$body" \
  || fail "a rejected version leaked into the package document: $(cat "$body")"
wrequest PATCH "$publish2_path/yank" "$work/yank.json" 404

step "republishing over a rejected version replaces it as pending"
tamper_zip "$fixture_archive" "$work/replacement.zip" 2
replacement_hash="$(shasum -a 256 "$work/replacement.zip" | cut -d' ' -f1)"
sed "s/$blob_hash/$replacement_hash/" "$work/withdep-0.2.1.json" >"$work/replacement.json"
frame "$work/replacement.json" "$work/replacement.zip" "$work/replacement.bin"
wrequest PUT "$publish2_path" "$work/replacement.bin" 201
expect_body '"verification":"pending"'
replacement_size="$(wc -c <"$work/replacement.zip" | tr -d ' ')"
[[ "$(stored_bytes)" == "$((before_bytes + replacement_size))" ]] \
  || fail "the replacement archive was not counted: $(stored_bytes)"
as_verifier
wcheck "/api/v1/admin/versions?status=pending" 200
expect_body '"version":"0.2.1"'
expect_body "$replacement_hash"

step "rejecting an unshared blob reclaims it and refunds the bytes"
wrequest PATCH "$verdict2_path" "$work/verdict-rejected.json" 200
as_publisher
[[ "$(stored_bytes)" == "$before_bytes" ]] \
  || fail "the rejection did not refund the replacement bytes: $(stored_bytes)"
if wrangler r2 object get "cabin-registry-blobs/blobs/sha256/$replacement_hash" \
  --file "$work/reclaimed.zip" --local >/dev/null 2>&1; then
  fail "the rejected version's unshared blob was not reclaimed"
fi

step "republishing identical bytes over a rejected version restarts verification"
wrequest PUT "$publish2_path" "$work/replacement.bin" 201
expect_body '"verification":"pending"'
[[ "$(stored_bytes)" == "$((before_bytes + replacement_size))" ]] \
  || fail "the re-uploaded blob was not re-counted: $(stored_bytes)"
as_verifier
check "$artifact2_path" 200
as_publisher

# --- Scope authorization: the write plane's uniform 403. ---
# These attempts charge the publish bucket like any others (the
# membership gate sits after the rate limit), so the leg gets its own.
wrangler d1 execute DB --local --command "
  UPDATE tokens SET rl_tokens = NULL, rl_updated_at = NULL WHERE id = 'smoke';"

step "publishing to an unclaimed or foreign scope is one uniform 403"
# 'ghost' was never claimed; 'foreign' belongs only to the seeded user 2.
# Both must answer the byte-identical refusal, so an authenticated
# publisher cannot probe which scopes exist. The gate fires before the
# body is read, so the well-formed publish body is irrelevant.
wrequest PUT "/api/v1/packages/ghost/$name/$version" "$work/publish.bin" 403
cp "$body" "$work/ghost-403.json"
grep -qF 'not a member' "$work/ghost-403.json" \
  || fail "the refusal is not the membership detail: $(cat "$work/ghost-403.json")"
wrequest PUT "/api/v1/packages/foreign/$name/$version" "$work/publish.bin" 403
cmp -s "$body" "$work/ghost-403.json" \
  || fail "foreign-scope and unclaimed-scope refusals differ: $(cat "$body")"
wrequest PATCH "/api/v1/packages/foreign/$name/$version/yank" "$work/yank.json" 403
cmp -s "$body" "$work/ghost-403.json" \
  || fail "the yank refusal differs from the publish refusal: $(cat "$body")"

# The /__scheduled test route (wrangler dev --test-scheduled) invokes
# the cron handler; any non-breaker expression routes to the dump job,
# which talks to the export-API mock started above.
step "the backup cron stores a validated dump in the BACKUP bucket"
today="$(date -u +%F)"
dump_key="d1/$today.sql"
check "/__scheduled?cron=0+3+*+*+*" 200
stored=""
for _ in $(seq 1 20); do
  if wrangler r2 object get "cabin-registry-backup/$dump_key" \
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
wrangler r2 object get "cabin-registry-backup/$dump_key.sha256" \
  --file "$work/$today.sql.sha256" --local >/dev/null 2>&1 \
  || fail "sidecar $dump_key.sha256 is missing"
cp "$work/stored-dump.sql" "$work/$today.sql"
(cd "$work" && shasum -a 256 -c "$today.sql.sha256" >/dev/null) \
  || fail "shasum -c rejected the sidecar: $(cat "$work/$today.sql.sha256")"

step "meta records the backup"
last_backup_at="$(wrangler d1 execute DB --local --json --command \
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
rerun_at="$(wrangler d1 execute DB --local --json --command \
  "SELECT value FROM meta WHERE key = 'last_backup_at'" |
  node -e '
    const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
    console.log(out[0].results[0].value);
  ')"
[[ "$rerun_at" == "$last_backup_at" ]] \
  || fail "same-day re-run rewrote last_backup_at: $rerun_at (was $last_backup_at)"

# The breaker expression additionally runs the governor reconciliation
# (the usage log line proves the ledger answered) and a backup-queue
# drain; with no analytics token the budget evaluation degrades
# gracefully on the exact storage counter alone.
step "the breaker cron reconciles the governor ledger"
check "/__scheduled?cron=*/15+*+*+*+*" 200
governor_logged=""
for _ in $(seq 1 20); do
  grep -q "governor usage:" "$dev_log" && { governor_logged=1; break; }
  sleep 0.5
done
[[ -n "$governor_logged" ]] \
  || fail "the breaker cron pass never logged the governor usage snapshot"

# --- The strict zip container profile, at publish and at verification. ---
# These publishes charge the publish bucket like any others, so the leg
# gets its own burst.
wrangler d1 execute DB --local --command "
  UPDATE tokens SET rl_tokens = NULL, rl_updated_at = NULL WHERE id = 'smoke';"

profile_version="0.3.0"
profile_publish_path="/api/v1/packages/$scope/$name/$profile_version"
profile_verdict_path="/api/v1/admin/versions/$scope/$name/$profile_version"
profile_artifact_path="/artifacts/$scope/$name/$scope-$name-$profile_version.zip"

step "the publish path fast-fails a non-zip body before hashing it"
# Canonical metadata for a fresh version, but an archive part that is
# plainly not a zip: the fixed-offset container gate rejects it (400)
# ahead of the checksum and immutability checks.
printf 'not a zip archive' >"$work/notzip.bin"
notzip_hash="$(shasum -a 256 "$work/notzip.bin" | cut -d' ' -f1)"
sed "s/0\\.2\\.0/$profile_version/g" "$fixture_metadata" |
  sed "s/$blob_hash/$notzip_hash/" >"$work/notzip.json"
frame "$work/notzip.json" "$work/notzip.bin" "$work/notzip.publish.bin"
wrequest PUT "$profile_publish_path" "$work/notzip.publish.bin" 400
expect_body 'archive is not a zip container'

step "a profile-violating archive publishes pending, then the verifier rejects it"
# A single stored zero-length entry named '../evil': the EOCD arithmetic
# is exact, so it clears the worker's container gate, but the strict
# profile fails it on path traversal. Hand-assembled (the full violation
# matrix - zip64, wrong method, extra fields, GP bits, local/central
# disagreement, case collisions, directory entries - lives in the
# verifier crate's Rust tests).
python3 - "$work/evil.zip" <<'PY'
import struct, sys
name = b"../evil"
lfh = struct.pack("<IHHHHHIIIHH", 0x04034b50, 20, 0, 0, 0, 0, 0, 0, 0, len(name), 0) + name
cd = struct.pack("<IHHHHHHIIIHHHHHII",
                 0x02014b50, 20, 20, 0, 0, 0, 0, 0, 0, 0, len(name), 0, 0, 0, 0, 0, 0) + name
eocd = struct.pack("<IHHHHIIH", 0x06054b50, 0, 0, 1, 1, len(cd), len(lfh), 0)
open(sys.argv[1], "wb").write(lfh + cd + eocd)
PY
evil_hash="$(shasum -a 256 "$work/evil.zip" | cut -d' ' -f1)"
sed "s/0\\.2\\.0/$profile_version/g" "$fixture_metadata" |
  sed "s/$blob_hash/$evil_hash/" >"$work/evil.json"
frame "$work/evil.json" "$work/evil.zip" "$work/evil.publish.bin"
wrequest PUT "$profile_publish_path" "$work/evil.publish.bin" 201
expect_body '"verification":"pending"'

as_verifier
wcheck "/api/v1/admin/versions?status=pending" 200
cp "$body" "$work/pending-profile.json"
curl -sS -o "$work/evil-download.zip" ${curl_args[@]+"${curl_args[@]}"} \
  "$base$profile_artifact_path"
cmp -s "$work/evil-download.zip" "$work/evil.zip" \
  || fail "the pending profile-violation download differs from what was published"
listing_entry "$work/pending-profile.json" "$scope/$name" "$profile_version" "$work/entry-profile.json"
run_verifier "$work/evil-download.zip" "$work/entry-profile.json" "$work/verdict-profile.json"
grep -qF '"verdict":"rejected"' "$work/verdict-profile.json" \
  || fail "the verifier did not reject the traversal archive: $(cat "$work/verdict-profile.json")"
grep -qF 'path_traversal' "$work/verdict-profile.json" \
  || fail "the rejection is not path_traversal: $(cat "$work/verdict-profile.json")"
# PATCH the actual rejected verdict: reason from the binary, bound to the
# checksum and published_at the listing reported.
node -e '
  const fs = require("fs");
  const verdict = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const entry = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
  fs.writeFileSync(process.argv[3], JSON.stringify({
    verdict: "rejected", reason: verdict.reasons[0],
    checksum: entry.checksum, published_at: entry.published_at,
  }));' "$work/verdict-profile.json" "$work/entry-profile.json" "$work/verdict-profile-patch.json"
wrequest PATCH "$profile_verdict_path" "$work/verdict-profile-patch.json" 200
expect_body '"verification":"rejected"'
expect_body '"changed":true'
as_publisher
check "$profile_artifact_path" 404

# --- The governor's hard limits, against tiny pools. ---
# The isolation fixtures publish while pools are still large: a pending
# version (never verified, never downloaded) and a verified one (never
# downloaded), each with distinct content-addressed bytes.
step "seeding isolation fixtures for the governor legs"
wrangler d1 execute DB --local --command "
  UPDATE tokens SET rl_tokens = NULL, rl_updated_at = NULL WHERE id = 'smoke';" >/dev/null
make_min_zip() { # <out-file> <entry-name>: a container-valid zip
  python3 - "$1" "$2" <<'PY'
import struct, sys
name = sys.argv[2].encode()
lfh = struct.pack("<IHHHHHIIIHH", 0x04034b50, 20, 0, 0, 0, 0, 0, 0, 0, len(name), 0) + name
cd = struct.pack("<IHHHHHHIIIHHHHHII",
                 0x02014b50, 20, 20, 0, 0, 0, 0, 0, 0, 0, len(name), 0, 0, 0, 0, 0, 0) + name
eocd = struct.pack("<IHHHHIIH", 0x06054b50, 0, 0, 1, 1, len(cd), len(lfh), 0)
open(sys.argv[1], "wb").write(lfh + cd + eocd)
PY
}
publish_min_version() { # <version> <zip>: sed the fixture metadata onto it
  local v="$1" zip="$2" hash
  hash="$(shasum -a 256 "$zip" | cut -d' ' -f1)"
  sed "s/0\.2\.0/$v/g" "$fixture_metadata" |
    sed "s/$blob_hash/$hash/" >"$work/iso-$v.json"
  frame "$work/iso-$v.json" "$zip" "$work/iso-$v.publish.bin"
  wrequest PUT "/api/v1/packages/$scope/$name/$v" "$work/iso-$v.publish.bin" 201
  expect_body '"verification":"pending"'
}
iso_pending_version="0.4.0"
iso_verified_version="0.5.0"
make_min_zip "$work/iso-pending.zip" "isopending"
make_min_zip "$work/iso-verified.zip" "isoverified"
publish_min_version "$iso_pending_version" "$work/iso-pending.zip"
publish_min_version "$iso_verified_version" "$work/iso-verified.zip"
as_verifier
wcheck "/api/v1/admin/versions?status=pending" 200
cp "$body" "$work/pending-iso.json"
listing_entry "$work/pending-iso.json" "$scope/$name" "$iso_verified_version" "$work/entry-iso.json"
node -e '
  const fs = require("fs");
  const entry = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  fs.writeFileSync(process.argv[2], JSON.stringify({
    verdict: "verified", checksum: entry.checksum, published_at: entry.published_at,
  }));' "$work/entry-iso.json" "$work/verdict-iso.json"
wrequest PATCH "/api/v1/admin/versions/$scope/$name/$iso_verified_version" "$work/verdict-iso.json" 200
expect_body '"verification":"verified"'
as_publisher

# Tiny pools via a restart: the ledger and windows persist in the local
# Durable Object state, so the main run's consumption already exceeds
# these limits and every fresh billable call must refuse - while the
# edge cache, filled before the restart, keeps serving.
step "restarting wrangler dev with tiny governor pools"
kill -- "-$dev_pid" 2>/dev/null || true
kill -- "-$web_pid" 2>/dev/null || true
for _ in $(seq 1 30); do
  curl -fsS -o /dev/null "$base/healthz" 2>/dev/null || break
  sleep 0.5
done
cat >>"$dev_vars" <<EOF
GOVERNOR_R2_CLASS_B_ORDINARY_MONTH="1"
GOVERNOR_STORAGE_PRIMARY_BYTES="1"
GOVERNOR_R2_CLASS_B_SOURCE_MONTH="0"
EOF
start_registry_dev
start_web_dev

iso_verified_artifact="/artifacts/$scope/$name/$scope-$name-$iso_verified_version.zip"
iso_pending_artifact="/artifacts/$scope/$name/$scope-$name-$iso_pending_version.zip"

step "cached verified downloads keep serving under an exhausted read pool"
check "$artifact_path" 200
# A cache HIT (this response cannot have consumed the pool) thaws to
# the same outward no-store; the stored copy's public header never
# escapes.
curl -sS -o /dev/null -D "$headers" ${curl_args[@]+"${curl_args[@]}"} "$base$artifact_path"
grep -qi '^cache-control: no-store' "$headers" \
  || fail "a cache-hit artifact is missing the outward no-store: $(grep -i cache-control "$headers")"

step "an uncached verified download is refused with the budget envelope"
check "$iso_verified_artifact" 503
expect_body 'registry_over_budget'

step "the verifier pool is isolated from the exhausted ordinary pool"
as_verifier
check "$iso_pending_artifact" 200
as_publisher

step "a fresh publish is refused before any r2 write when storage is exhausted"
wrangler d1 execute DB --local --command "
  UPDATE tokens SET rl_tokens = NULL, rl_updated_at = NULL WHERE id = 'smoke';" >/dev/null
make_min_zip "$work/iso-blocked.zip" "isoblocked"
blocked_hash="$(shasum -a 256 "$work/iso-blocked.zip" | cut -d' ' -f1)"
sed "s/0\.2\.0/0.6.0/g" "$fixture_metadata" |
  sed "s/$blob_hash/$blocked_hash/" >"$work/iso-blocked.json"
frame "$work/iso-blocked.json" "$work/iso-blocked.zip" "$work/iso-blocked.publish.bin"
wrequest PUT "/api/v1/packages/$scope/$name/0.6.0" "$work/iso-blocked.publish.bin" 503
expect_body 'registry_over_budget'
if wrangler r2 object get "cabin-registry-blobs/blobs/sha256/$blocked_hash" \
    --file /dev/null --local >/dev/null 2>&1; then
  fail "a refused publish still wrote its blob to R2"
fi

step "an idempotent re-publish stays a 200 no-op under a full storage pool"
wrequest PUT "$publish_path" "$work/publish.bin" 200
expect_body '"no_op":true'

step "source-viewer reads fail closed on an exhausted source pool"
session_request GET "/api/v1/user/source/$scope/$name/$iso_verified_version" 503 -H "Range: bytes=-22"
expect_body 'registry_over_budget'

step "the admin governor endpoint reports usage and takes operator actions"
# Ordinary tokens are refused; the verify scope reads the snapshot.
wcheck "/api/v1/admin/governor" 403
expect_body 'verify scope'
as_verifier
wcheck "/api/v1/admin/governor" 200
expect_body '"storage"'
# An idempotent release of an unknown key answers ok.
printf '{"release":{"pool":"primary","key":"blobs/sha256/none"}}' >"$work/gov-release.json"
wrequest POST "/api/v1/admin/governor" "$work/gov-release.json" 200
expect_body '"ok":true'
# The pre-launch ledger wipe clears the primary rows and the windows,
# while the backup and dump rows survive - the registry wipe never
# touches the BACKUP bucket, so their objects keep billing...
printf '{"wipe":true}' >"$work/gov-wipe.json"
wrequest POST "/api/v1/admin/governor" "$work/gov-wipe.json" 200
expect_body '"ok":true'
wcheck "/api/v1/admin/governor" 200
node -e '
  const s = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
  if (s.storage.some((row) => row.pool === "primary")) process.exit(1);
  if (!s.storage.some((row) => row.pool === "backup")) process.exit(1);
  if (s.ops.length !== 0) process.exit(1);
' "$body" || fail "the governor wipe left the wrong ledger rows: $(cat "$body")"

# ...and reconciliation rebuilds the primary rows from D1's live set:
# the next breaker cron pass commits every live checksum back into the
# ledger (increase-only) and logs how many it recorded; the pass after
# that logs the rebuilt rows in its usage snapshot (each pass logs
# usage before it reconciles).
step "reconciliation rebuilds the wiped primary ledger from d1"
recon_mark="$(($(wc -l <"$dev_log" | tr -d ' ') + 1))"
check "/__scheduled?cron=*/15+*+*+*+*" 200
recon_ok=""
for _ in $(seq 1 20); do
  if tail -n "+$recon_mark" "$dev_log" | grep -q "previously unledgered blob"; then
    recon_ok=1
    break
  fi
  sleep 0.5
done
[[ -n "$recon_ok" ]] \
  || fail "the post-wipe cron pass never re-committed the live primary set"
recon_mark="$(($(wc -l <"$dev_log" | tr -d ' ') + 1))"
check "/__scheduled?cron=*/15+*+*+*+*" 200
recon_ok=""
for _ in $(seq 1 20); do
  if tail -n "+$recon_mark" "$dev_log" | grep -q "primary/committed="; then
    recon_ok=1
    break
  fi
  sleep 0.5
done
[[ -n "$recon_ok" ]] \
  || fail "the rebuilt primary ledger never appeared in the usage snapshot"
# ...and refuses while the registry is launched (the wipe guard).
wrangler d1 execute DB --local --command \
  "UPDATE meta SET value = 'true' WHERE key = 'launched';" >/dev/null
wrequest POST "/api/v1/admin/governor" "$work/gov-wipe.json" 403
expect_body 'launched'
wrangler d1 execute DB --local --command \
  "UPDATE meta SET value = 'false' WHERE key = 'launched';" >/dev/null
as_publisher

echo "smoke OK"
