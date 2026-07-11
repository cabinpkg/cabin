# Registry Service Architecture

The service implements the server side of
[`../../docs/remote-registry.md`](../../docs/remote-registry.md) (the
authoritative protocol contract). This page covers only the decisions local to
the service.

## Storage

- **D1 is canonical.** Users, tokens, packages, versions, and the `meta`
  key-value table all live in one D1 database (`migrations/`).
  Everything the read routes serve is composed from D1 rows; in particular
  each version's canonical index entry is stored verbatim at publish time in
  `versions.metadata_json`, and only its `yanked` field is overwritten from
  the row on the way out, so yank state has exactly one home.
- **R2 holds immutable, content-addressed blobs.** Archive bytes live at
  `blobs/sha256/<checksum-hex>` (the lowercase hex in `versions.checksum`).
  Blobs are never mutated; the one deletion path is the verification
  lifecycle's reclaim of a **rejected** version's blob when no live
  (non-rejected) row references its checksum. Yanking is a D1 row
  update, and the artifact route deliberately keeps serving yanked
  versions so locked-in consumers keep building.
- **Verified versions are immutable.** Every `versions` row carries a
  verification status (`pending` | `verified` | `rejected`, migration
  `0004`; see "The verification lifecycle"). Re-publishing
  byte-identical metadata (which embeds the archive checksum, so
  identical metadata means an identical archive) over a pending or
  verified row is an idempotent
  `200 {"ok":true,"no_op":true,"verification":"<status>"}` that touches
  neither store; different bytes are
  `409 published versions are immutable`. A rejected row is the one
  exception: it never became part of the registry, so any bytes replace
  it and return it to `pending`. There is no unpublish or delete.
- **No KV.** The data is relational and small; a second store would only add
  consistency questions. Response caching can come later at the edge if read
  volume ever warrants it.

## Origins and roles

One Worker serves two hostnames, one role per hostname, dispatched on the
Host header (`src/routes.rs` `role_for_host`; any host that is not the
`WEB_ORIGIN` host gets the registry role, deny by default). The matrix -
which routes and which credential exist where:

| | Registry custom domain (`dev-registry.cabinpkg.com` / `registry.cabinpkg.com`) | Website origin (`cabinpkg.com`) |
| --- | --- | --- |
| `/healthz` | 200, unauthenticated | - |
| `/config.json`, `/packages/*`, `/artifacts/*` | Bearer (the read plane) | - |
| `/login`, `/callback` | - | OAuth browser flow, no credential in / session cookie out |
| `/api/v1/user`, `/api/v1/user/{usage,packages,logout}`, `/api/v1/user/tokens[...]` | - | Session cookie **only** |
| `/api/v1/packages/*`, `/api/v1/admin/*` | - | Bearer **only** |
| everything else | uniform 401 + challenge | uniform 401 + challenge (unauthenticated) / authenticated 404 |

A dash means the path does not exist on that hostname: on the registry
domain every non-read-plane path answers the uniform 401 **without
consulting the `Authorization` header**, indistinguishable from any unknown
path; on the website origin nothing ever matches a read route, so package
data is never served there. Every Bearer-plane 401 carries the
byte-identical `WWW-Authenticate: Cabin login_url="<WEB_ORIGIN>/settings/tokens"`
challenge (`docs/remote-registry.md`, "The login-URL challenge"); session
401s deliberately do not, keeping the planes distinguishable. In production
the website origin reaches this Worker through zone routes
(`cabinpkg.com/api/*`, `/login`, `/callback*` - see `wrangler.jsonc` and
[`runbook.md`](runbook.md), "Integrated topology and route
management"). The frontend consuming
the session plane - `/dashboard`, `/settings/*`, and `/login/denied`, all
static pages - lives in the repository's `website/` project ("Account
pages" in its README).

## Two credential planes

Authentication is split into two planes that never accept each other's
credential, separated by route on top of the hostname split: the
`/api/v1/user` subtree is session-only, everything else under `/api/` is
Bearer-only.

**The data plane is Bearer-only and deny-by-default.** Every data route -
including `/config.json` - requires `Authorization: Bearer cabin_<base62>`.
The uniform `401 {"errors":[{"detail":"authentication required"}]}` (plus
the challenge header) is emitted before any route matching or D1/R2 data
lookup, so unauthenticated callers cannot distinguish existing from
non-existing packages. `/healthz` is the only route outside both planes.
Cookies are never read here.

Tokens are stored as the SHA-256 hex of the full token string; the plaintext
exists only in the client's hands (it is rendered exactly once, in the
create-token response). Any valid, unrevoked token grants read access;
`scopes` (a subset of `publish,yank,verify`) gates the mutation routes and
the verifier's admin plane.
`last_used_at` is updated best-effort off the response path, and log lines
carry the token row id - never the token or its hash.

**The session plane is cookie-only JSON.** `/login` and `/callback` run the
GitHub OAuth sign-in (web application flow, no extra scopes, explicit
`redirect_uri` of `<WEB_ORIGIN>/callback`); the `/api/v1/user` subtree is
the JSON user API the website frontend consumes:

- `GET /api/v1/user` -> `{"github_id":..,"login":..,"plan":..}`;
- `GET /api/v1/user/usage` -> plan, package count, stored bytes (rejected
  versions excluded - their bytes were refunded), today's publishes,
  per-status version counts, and the plan's quotas;
- `GET /api/v1/user/packages` -> the packages the user created, each
  version carrying its verification state and yanked flag (the
  dashboard's package list);
- `GET /api/v1/user/tokens` -> token metadata (never hashes);
- `POST /api/v1/user/tokens` (`{"name":..,"scopes":[..]}`, unknown or
  repeated scopes refused) -> `201` with the plaintext token, exactly once;
- `POST /api/v1/user/tokens/<id>/revoke` -> idempotent `{"ok":true}`,
  scoped to the session's own tokens (a foreign or unknown id is a no-op);
- `POST /api/v1/user/logout` -> `{"ok":true}` with a `Set-Cookie` that
  clears the session cookie (it is HttpOnly, so only the server can).
  Sessions are stateless HMAC values: the sealed value stays verifiable
  until its 8-hour expiry, so clearing the cookie is the sign-out and
  removing the id from `ALLOWED_GITHUB_IDS` is the hard revocation.

The exact response shapes live in `src/user_api.rs` (host-tested). The
`Authorization` header is never read on this plane, and unauthenticated
requests get a plain 401 envelope - never a redirect (redirecting is the
frontend's job) and never the Bearer challenge. `/callback` redirects to
the website's `/dashboard` on success and `/login/denied` on every refusal;
both targets are fixed relative paths, never derived from request input
(the open-redirect guard).

- Identity is the **numeric GitHub id**, never the login name (logins can
  be renamed and reassigned); sign-in is allowed iff the id is listed in
  `ALLOWED_GITHUB_IDS`. Adding a user later = adding their numeric id there
  and redeploying; a malformed entry panics at parse time instead of
  guessing. The allowlist is re-checked on every session request, so
  removing an id locks it out immediately. Per-package ownership is
  intentionally out of scope for now: every allowlisted user can publish
  and yank any package.
- The GitHub access token is used for one `/user` call and never stored.
- Cookies (the short-lived OAuth `state` and the 8-hour session) are
  HMAC-signed values keyed by `SESSION_SECRET` with per-purpose domain
  separation (`src/session.rs`); `HttpOnly; Secure; SameSite=Lax`, and
  **host-only** - no `Domain` attribute, so registry subdomains can never
  receive the website origin's cookies. Paths are narrowed to where each
  cookie is read (`Path=/api/v1/user` for the session, `Path=/callback`
  for the OAuth state), so ordinary website page loads never carry them.
- Session-plane mutations enforce a stateless CSRF discipline suited to a
  JSON API: `Content-Type: application/json` **and** `X-CSRF-Protection: 1`
  are required (`session::csrf_headers_ok`, checked before the body is
  read). Neither header can ride on an HTML form or any other request a
  hostile origin can send without a CORS preflight - which the Worker
  never answers - so with `SameSite=Lax` host-only cookies no server-side
  token state is needed.
- Every session-plane response carries `Content-Security-Policy:
  default-src 'none'; style-src 'unsafe-inline'`,
  `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, and
  `Cache-Control: no-store` (in particular the one response holding a
  plaintext token).
- Sessions, GitHub access tokens, and issued registry tokens are never
  logged.

## The write path

`PUT /api/v1/packages/<name>/<version>` (publish, `publish` scope) and
`PATCH /api/v1/packages/<name>/<version>/yank` (yank, `yank` scope) implement
the mutation half of the protocol contract. Publish validates in a fixed
order, stopping at the first failure:

1. scope (`403`);
2. body size (64 MiB cap) and the length-prefixed framing, which must
   account for the body exactly (`400`);
3. the metadata parses as the canonical `cabin package` document under
   `deny_unknown_fields` - client drift is rejected, and the `400` details
   are fixed strings that never echo request bytes;
4. the URL's `<name>` / `<version>` equal the document's `name` / `version`
   and the archive path its `source` block implies (`400`);
5. the name matches `^[a-z0-9][a-z0-9_-]*$` and the version is valid SemVer
   (`400`);
6. `yanked` is `false` (`400`);
7. the metadata's `checksum` equals `sha256:` + the digest the server itself
   computes from the uploaded archive bytes via SubtleCrypto (`400`).

Only then storage is consulted: an existing pending or verified row answers
with the idempotent `200` no-op (reporting its verification status) or the
`409` immutability conflict, a rejected row is replaced in place (any
bytes; back to `pending`), and a new version writes the R2 blob first
(skipped when the content-addressed key already exists), then one atomic
D1 batch for the `packages` and `versions` rows. New and replaced rows
start `pending`. A crash between the two writes can only leave an
unreferenced blob - see [`runbook.md`](runbook.md).

Yank is a single-column `UPDATE` on the `versions` row (`404` when the pair
is unknown **or not verified** - a version that never became resolvable has
nothing to retract), idempotent, reporting the resulting state and whether
the request changed it. The read path overrides the stored entry's `yanked`
field from the column, so the verbatim `metadata_json` never goes stale on
the one field that mutates.

## The verification lifecycle

Publish stores content; an external verifier (a later step; it runs in
GitHub Actions) decides what becomes part of the registry. The status
lives in `versions.verification` with `verification_reason` /
`verified_at` alongside; the pure transition rules, the artifact read
gate, and the verdict body live in `src/verify.rs`.

- **Reads are gated on `verified`.** `/packages/<name>.json` composes
  verified versions only (the filter sits in the SQL query, so a package
  with none is an ordinary 404), and the artifact route serves verified
  versions to ordinary tokens. The `verify` scope may additionally list
  pending versions (`GET /api/v1/admin/versions?status=...`) and
  download their artifacts - the verifier has to fetch what it inspects.
  Rejected versions are served to no one.
- **Verdicts** (`PATCH /api/v1/admin/versions/<name>/<version>`, scope
  `verify`, budget-gated like every write): `verified` stamps
  `verified_at`; `rejected` records the reason, refunds the archive's
  bytes from `meta.total_stored_bytes` when the row was the checksum's
  sole live reference (decided inside the same transaction that flips
  the row, so a duplicate concurrent verdict cannot refund twice), and
  reclaims the blob best-effort - a failed delete leaves a harmless
  orphan that the replacement path retries, and publishes re-check
  their blob after their batch commits, so a reclaim racing a
  deduplicating publish of the same bytes is self-healed. The body's
  `checksum` and `published_at` (required to verify, optional to
  reject - exposure must name the inspected row generation, and
  `published_at` catches even a same-bytes replacement with new
  metadata) bind the verdict to what the verifier listed,
  and the applying updates are guarded on the row still being pending
  with the bytes the request read, so a verdict racing a conflicting
  verdict or a replacement answers 409 instead of applying - the
  verified arm must never resurrect a row a concurrent rejection just
  reclaimed. Repeating a verified version's verdict is the idempotent
  200; a conflicting verdict on a verified version and any verdict on
  a rejected one are 409 (republish is the recovery path, and a late
  duplicate verdict must never race the replacement back to pending).
- **Trust model.** The `verify` scope is mintable through the session
  token API like `publish` and `yank`: every allowlisted user is
  currently an operator, matching the registry-wide "no per-package
  ownership yet" stance above. A dedicated verifier-only issuance path
  is deliberate future work for when sign-up opens beyond the
  allowlist.
- **Fail-safe direction.** Nothing becomes resolvable unless its status
  is exactly `verified`: a verifier that never runs, an unreadable
  status value, or a broken admin plane can only keep content
  unexposed, never expose it. The breaker cron counts versions pending
  for over an hour and alerts (log + webhook) on every pass while any
  exist, so a stuck verifier is noticed instead of silently blocking
  all publishes from resolving.
- **Accounting.** The storage self-accounting counts a blob's bytes
  while some live (non-rejected) row references its checksum: the
  publish batch counts the sole-live-reference insert, rejection
  refunds it when the last live reference flips, and a replacement
  re-counts a re-uploaded blob. Per-user storage quotas and the usage
  endpoint's stored sum exclude rejected rows the same way. Existing rows were
  backfilled `verified` by migration `0004` (published by the sole
  operator before the pipeline existed).

Conformance is enforced from the monorepo: `scripts/gen-fixtures.sh` builds
the in-tree `cabin` binary and packages real fixture pairs, which the
`conformance` CI job (and a frozen pair under `tests/fixtures/`) feeds
through the full server-side validation path, so the client's canonical
output and the server's schema cannot silently drift.

## Billing model and the budget breaker

The service runs on the Workers **free** plan on purpose. Workers requests
and D1 fail closed on their own when the free limits are hit - without
billing attached they cannot produce a bill. The only real billing exposure
is R2 overage: storage and Class A (write/list) operations. The service
therefore blocks itself gracefully *before* any Cloudflare limit or R2
overage is reached, in two layers.

**Per-user quotas** stop any single user from exhausting the shared free
budget. `users.plan` (default `'free'`) selects a quota set from the map in
`src/quota.rs` - per-archive bytes, total stored bytes per user, new
packages per day, total packages, versions per package per day, and a
publish token bucket (burst plus per-minute refill, state on the token row
in `tokens.rl_tokens` / `tokens.rl_updated_at`). Daily windows are UTC
calendar days. Publish enforces, in order: the budget gate (`402`), scope
(`403`), the rate limit (`429`, `Retry-After`, charged per attempt),
framing (`400`), metadata and checksum (`400`), the idempotent no-op /
immutability wall (`200`/`409`), then - for genuinely new versions only -
the archive-size cap (`413`) and the storage, package, and version quotas
(`403` with per-quota envelope codes) - so a byte-identical re-publish,
including one grandfathered above a later cap, never consumes quota. Attribution rides on
`versions.published_by`, `versions.archive_size`, and `packages.created_by`
(migration `0002`; `scripts/backfill-0002.sh` fills them for pre-migration
rows). The bucket take is persisted as a compare-and-swap on the token
row (retried up to a burst's worth of lost races), so concurrent requests
cannot spend one snapshot twice, and the storage self-accounting is
decided inside the write batch itself - the meta bump counts an archive
only when the just-inserted row is the checksum's sole reference, so
concurrent duplicate archives cannot double-count. The count quotas stay
a preflight on purpose - concurrent publishes can overshoot a near-limit
quota by at most the in-flight request count, which the bucket burst
bounds per token (an allowlisted user holding several tokens scales that
by their token count) and the budget headroom absorbs.

**The service-wide breaker** compares usage against budgets set comfortably
below the free limits (`src/breaker.rs`; the `BUDGET_*` env vars override
the in-code defaults). Storage usage is **exact self-accounting**: the
publish batch adds a blob's size to `meta.total_stored_bytes` the first
time a version row references its checksum (so a retry after a crash
between the R2 and D1 writes still counts the blob, and deduplicated
re-use under a second name never double-counts it), so the one metric
with direct billing exposure never depends on analytics. A missing or
corrupt counter reads as unavailable data - never as zero - so it can
keep or escalate the persisted mode but never unblock writes. Orphaned
blobs (a crash or lost publish race that never commits the D1 rows) sit
outside the counter on purpose; the budget headroom absorbs that bounded
drift, and the runbook's backfill script doubles as the reconciliation
tool. The other metrics (Workers requests/day, R2
Class A operations/month, D1 rows read/day) come from the Cloudflare
GraphQL Analytics API, queried by a cron pass every 15 minutes
(`src/analytics.rs` holds the dataset names; a rejected dataset degrades to
"metric unavailable", and partial data can escalate the mode but never
de-escalate it - missing analytics never unblocks writes).

Degradation order: `normal` -> `warn` (any metric at 80% of budget) ->
`writes_blocked` (any metric at budget). The mode and a human-readable
reason live in `meta.service_mode` / `meta.service_mode_reason`; mode
changes are logged and optionally POSTed to `NOTIFY_WEBHOOK_URL`. On the
request path, publish and yank read the mode through an isolate-memory
cache (~60 s TTL, one D1 point read on expiry; dev pins
`SERVICE_MODE_TTL_SECS` to 0 for the smoke test) and answer
`402 registry_over_budget` with `Retry-After` while blocked. Writes fail
closed - an unreadable or unknown mode blocks them - while reads never
consult the mode at all, so they fail open and yanked-state and downloads
keep working throughout an outage of the breaker itself.

## Backups

Backups are a data-plane concern and run entirely inside Cloudflare:
R2/D1 bindings need no stored credentials, unlike an external pipeline,
which would spread powerful tokens to a second vendor. The one secret
involved (`D1_EXPORT_API_TOKEN`) is scoped to D1 alone. Three pieces,
all operationally documented in [`runbook.md`](runbook.md) ("Disaster
recovery"):

- **Blob replication (RPO ~0).** After a publish's primary R2 put and
  D1 batch succeed, the archive blob is copied to the per-environment
  `BACKUP` bucket under the same content-addressed key, best-effort via
  `waitUntil`; an idempotent re-publish re-schedules the copy (a retry
  of a publish whose isolate died before replicating heals the gap),
  and failures land in the `backup_replication_failures` table
  for `scripts/backup-backfill.sh` to re-run. No code path deletes from
  the backup bucket, so it is append-only: a deletion in the primary -
  malicious or accidental - cannot propagate.
- **Nightly D1 dump (RPO <= 24 h).** A second cron schedule drives the
  D1 REST export endpoint from the Worker itself and streams the
  official `.sql` dump into `BACKUP` at `d1/<date>.sql` plus a `.sha256`
  sidecar, hashing and validating (expected `CREATE TABLE` statements)
  on the way through, then verifying the re-read object against the
  checksum before recording `meta.last_backup_at` /
  `meta.last_backup_key` and pruning beyond retention (30 dailies + 12
  monthly firsts). An invalid result is deleted from the dump key
  again, the sidecar exists only for validated dumps, and a date whose
  dump is already recorded is never re-exported - so a failed attempt
  can neither pose as nor replace a good dump. Two same-date runs can
  overlap only when an operator adds overlapping rehearsal schedules;
  the writes are deliberately not serialized for that case, because
  every interleaving ends either with a correct recorded dump or in a
  state the machinery detects loudly (a sidecar mismatch, a missing
  object, or the freshness alert) - never in silent loss. A D1 lock
  around the dump job is the named upgrade if simultaneous schedules
  ever become a real operational pattern. The scheduled handler routes on the cron expression:
  the breaker's `*/15 * * * *` exactly; any other schedule runs the
  dump job, so rehearsals need no recompile.
- **Freshness alerting.** Every breaker pass evaluates backup health
  (`src/backup.rs`; > 36 h without a successful dump, or a non-empty
  replication failure log) and alerts via log + webhook on every pass
  while unhealthy - a backup system that stops must not stop silently.

First-line recovery is D1 Time Travel (always on; 7-day retention on
the free plan), then the exported dumps, then the backup bucket's blobs
as the artifact store of last resort; `scripts/restore-drill.sh`
rehearses the dump-import path against a scratch database. The backup
bucket doubles stored blob bytes account-wide, which is why the default
storage budget above sits under half the free limit.

## Code layout

Domain logic - token hashing, formatting, and scopes (`src/auth.rs`),
hostname roles, route matching, and path-component validation
(`src/routes.rs`), document composition (`src/documents.rs`), the error
envelope and the challenge header (`src/error.rs`), cookie signing, the
cookie shape, and the CSRF header rule (`src/session.rs`), the session
API's JSON shapes and body validation (`src/user_api.rs`), the sign-in
allowlist (`src/allowlist.rs`), the
quota engine (`src/quota.rs`), the budget breaker (`src/breaker.rs`), the
analytics query shapes (`src/analytics.rs`), the verification lifecycle's
statuses, verdict rules, and read gate (`src/verify.rs`), and the backup
logic - retention, dump validation, freshness (`src/backup.rs`) - compiles
and unit-tests on the host target. The Cloudflare glue
(`src/glue.rs` for the role dispatch and the Bearer planes,
`src/web_glue.rs` for the OAuth and session planes,
`src/backup_glue.rs` for the nightly dump job, wasm32 only) is thin
binding-and-I/O wiring covered by
`scripts/smoke.sh`. Path
components are validated before any lookup: names are `[a-z0-9_-]+`, versions
must look like SemVer, and anything else 404s without touching storage.

Every authenticated response carries the debug header
`x-cabin-registry-generation` from `meta.registry_generation`, so a client
talking to a freshly wiped dev environment is immediately visible (see
[`runbook.md`](runbook.md)).

## Why a standalone workspace

`registry/` is its own Cargo workspace, listed in the root workspace's
`exclude`. The root workspace builds host-native binaries with a large,
carefully audited dependency tree and lockfile; this crate targets
`wasm32-unknown-unknown` through `worker-build` and pulls in the `worker`
ecosystem. Excluding it keeps `cargo build`/`cargo test` at the repository
root byte-identical to before the service existed, keeps the two lockfiles
independent, and mirrors how `website/` coexists in the repository with its
own toolchain and workflow (`.github/workflows/registry.yml`).
