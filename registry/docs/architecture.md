# Registry Service Architecture

The service implements the server side of
[`../../docs/remote-registry.md`](../../docs/remote-registry.md) (the
authoritative protocol contract). This page covers only the decisions local to
the service.

## Storage

- **D1 is canonical.** Users and their external identities, scopes and
  their members, tokens, packages, versions, and the `meta` key-value
  table all live in one D1 database (`migrations/`).
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

| | Registry custom domain (`registry.cabinpkg.com`) | Website origin (`cabinpkg.com`) |
| --- | --- | --- |
| `/healthz` | 200, unauthenticated | - |
| `/config.json`, `/packages/*`, `/artifacts/*` | Bearer (the read plane) | - |
| `/login`, `/callback` | - | OAuth browser flow, no credential in / session cookie out |
| `/claim/<scope>`, `/callback/claim` | - | the scope-claim flow's dedicated OAuth roundtrip ("Scopes" below) |
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
(`cabinpkg.com/api/*`, `/login`, `/callback*` - which also covers the
claim flow's `/callback/claim` - and `/claim/*`; see `wrangler.jsonc`
and [`runbook.md`](runbook.md), "Integrated topology and route
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
GitHub OAuth sign-in (web application flow, no OAuth scopes requested,
explicit `redirect_uri` of `<WEB_ORIGIN>/callback`); `/claim/<scope>`
and `/callback/claim` run the scope-claim flow's dedicated roundtrip
("Scopes" below), the one flow that requests an OAuth scope
(`read:org`); and the `/api/v1/user` subtree is the JSON user API the
website frontend consumes:

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
- `GET /api/v1/user/scopes/<scope>/members` -> the scope's members
  (GitHub numeric id, display login, role) - like the two mutations
  below it is owner-gated behind one uniform `403` ("the scope does not
  exist or you are not an owner of it"), byte-identical for a scope
  that does not exist and one the user does not own, so the session
  plane is no scope-existence oracle either;
- `POST /api/v1/user/scopes/<scope>/members`
  (`{"github_id":..,"role":"owner"|"member"}`) -> the resulting
  membership; the account is identified by GitHub numeric id and must
  already have a registry account (an `identities` row - it must have
  signed in once; `400` otherwise), and an existing member keeps their
  role (there is no role-change endpoint);
- `POST /api/v1/user/scopes/<scope>/members/<github_id>/remove` ->
  idempotent resulting-state `{"ok":true,"changed":..}`, except that
  removing the scope's last `owner` is a `409` - the rule is enforced
  inside the DELETE itself, so concurrent removals cannot race a scope
  into ownerlessness;
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

- Identity is **registry-native**: a `users` row (registry id, quota
  plan) plus one `identities` row per external account, keyed by
  `(provider, provider_account_id)` - provider-neutral in schema,
  GitHub-only in policy (`provider = 'github'`). The account id is the
  **numeric** GitHub id as text, never the login name (logins can be
  renamed and reassigned; `login_snapshot` is display-only, refreshed
  on each sign-in). Sign-in upserts the identity and creates the user
  row on first sign-in, in one D1 batch. Tokens, quotas, and package
  attribution all key on `users.id`; the provider account id lives
  only in `identities` (and in scope proof records).
- Sign-in is allowed iff the numeric GitHub id is listed in
  `ALLOWED_GITHUB_IDS`. Adding a user later = adding their numeric id
  there and redeploying; a malformed entry panics at parse time instead
  of guessing. The allowlist is re-checked on every session request, so
  removing an id locks it out immediately. Write authorization is per
  scope, not per package: publish and yank require membership in the
  target scope ("Scopes" below), and every member can act on every
  package under it.
- The session cookie names the external identity (the numeric GitHub
  id), resolved through `identities` on every request - deliberately
  not the `users.id`, which a pre-launch wipe would re-issue: a
  still-valid ghost cookie sealed over a row id could bind to whoever
  received that id after the wipe. A session whose identity row is gone
  answers the same 401 as no session.
- GitHub access tokens are transient: sign-in uses one for a single
  `/user` call, a claim for its few verification reads ("Scopes"
  below), and both drop it - never stored, never logged. Sign-in
  requests no OAuth scopes; only the claim roundtrip requests
  `read:org`. (GitHub grants are per app and cumulative, so after a
  user's first claim GitHub may attach the already-granted `read:org`
  to later sign-in tokens too - harmless here precisely because every
  token is transient and sign-in reads only `/user`.)
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

## Scopes

Registry package names are scoped (`<scope>/<name>`, e.g. `fmtlib/fmt`) -
one canonical name everywhere, with no alias or bare-name mechanism -
and a scope is a registry-native entity: it is claimed by proving
control of the same-named GitHub account, and the `scopes` row freezes
the proof - provider plus numeric account id - at claim time, so a
later GitHub account reusing the login can never re-claim the string
(disputes are handled manually; there is no reservation list and no
alias mechanism). `scope_members` holds per-scope membership, where
`owner` is the member role with admin rights - "owner" never means the
name prefix; the prefix entity is always called "scope". Publish/yank
authorization consults only registry state (`scope_members`), never a
live GitHub call: cabin tokens carry no GitHub credentials.

The scope grammar is GitHub-login-compatible on purpose (every
lowercased login fits): `[a-z0-9]([a-z0-9-]*[a-z0-9])?`, at most 39
characters. It is deliberately a small superset of GitHub's own login
rules (which also forbid consecutive hyphens): claimability is proved
by the claim flow's account-control check, never by the charset, and an
unclaimable string can never gain members, so it answers the write
plane's uniform 403 forever. The package part keeps the package grammar
`^[a-z0-9][a-z0-9_-]*$`, and the full name contains exactly one `/`.

**The claim flow.** `GET /claim/<scope>` starts a dedicated GitHub
OAuth roundtrip - sign-in discards its token after one `/user` call, so
a claim cannot ride on it - mirroring `/login`'s sealed-state
discipline with its own cookie (`cabin_claim_state`, `Path=
/callback/claim`, its own HMAC purpose) that also seals the scope being
claimed, and requesting `read:org` (this flow only). On
`/callback/claim`, with the transient token: the claim is granted iff
the scope equals the authenticated user's lowercased login
(self-claim), or `GET /orgs/<scope>/memberships/<login>` shows an
`active` membership with the `admin` role (org claim). The scope
string is frozen to the account's **numeric** id, resolved via
`GET /users/<scope>` and bound by id equality against the claimant
(self) or the membership's organization (org) - logins can be renamed
and reassigned between any two calls; ids cannot. The claimant must be
allowlisted and have a registry account (sign in first), because the
grant writes `scopes` plus the claimant as the first `owner` in
`scope_members`, in one D1 batch - a plain primary-key insert, so the
loser of a claim race rolls back seedless and is refused. Every
refusal is one uniform redirect with no detail. A claim is
**permanent**: an already-claimed scope refuses whoever asks - even an
account that now controls the GitHub name - and there are no transfer
or release endpoints; disputes are handled manually by the operator
(direct D1 surgery; migration `0007` pins the role domain so a
hand-run typo cannot orphan a scope). Because a claim only ever binds
a scope to the account that genuinely controls the same-named GitHub
account, with that account's user as owner, a forced navigation to
`/claim/<scope>` can at worst claim the victim's own name for the
victim - accepted pre-launch griefing, not a takeover vector.

Scope-proof automation is GitHub-only **by policy**, even though the
schema (`proof_provider`, `identities.provider`) is provider-neutral.
Membership management is registry-side only: owners list, add, and
remove members through the session API ("Two credential planes"
above); there is no automatic GitHub org sync (TODO: revisit once
sign-up opens beyond the allowlist), so org membership changes on
GitHub propagate only when an owner edits the member list.

The client's *name model* is scoped: manifests, local file
registries, lockfiles, and the resolver carry `<scope>/<name>`
verbatim, and `cabin publish` rejects bare names outright. Its *wire
protocol* and the external verifier
(`crates/cabin-registry-verify` and its workflow) still speak bare
names, so in the interim the client blocks every remote publish
before any connection (bare names fail the scoped-name requirement;
scoped names cannot be expressed on the bare routes) and rejects
scoped names at the remote fetch boundary. With scopes claimable, a
scoped publish crafted against the registry directly is accepted and
stored but fails the verifier's artifact download or
archive/manifest name check, so it stays `pending` (or is rejected)
instead of ever resolving - the fail-safe direction ("The
verification lifecycle"), made loud by the stale-pending alert
rather than silent. End-to-end publishes on the deployed registry
begin working when the scoped-routes step rewires the client and the
verifier.

## The write path

`PUT /api/v1/packages/<scope>/<name>/<version>` (publish, `publish`
scope) and `PATCH /api/v1/packages/<scope>/<name>/<version>/yank` (yank,
`yank` scope) implement the mutation half of the protocol contract.
Publish validates in a fixed order, stopping at the first failure:

1. token scope (`403`);
2. scope membership: the token's user must be a member of `<scope>`
   (`403`, uniform - below);
3. body size (64 MiB cap) and the length-prefixed framing, which must
   account for the body exactly (`400`);
4. the metadata parses as the canonical `cabin package` document under
   `deny_unknown_fields` - client drift is rejected, and the `400` details
   are fixed strings that never echo request bytes;
5. the URL's segments equal the document's `name` (the full
   `<scope>/<name>`) and `version`, and the archive path its `source`
   block implies -
   `../../artifacts/<scope>/<name>/<scope>-<name>-<version>.tar.gz`, the
   filename embedding the scope like the artifact route (`400`);
6. the scope and name match the grammars in "Scopes" and the version is
   valid SemVer (`400`);
7. `yanked` is `false` (`400`);
8. the metadata's `checksum` equals `sha256:` + the digest the server itself
   computes from the uploaded archive bytes via SubtleCrypto (`400`).

Publishing under a scope the user is a member of is all it takes to
create a package there: the first published version inserts the
`packages` row. The membership refusal is **one uniform `403`**
(`the scope does not exist or the token's user is not a member of it`),
byte-identical for a scope that was never claimed and one the user is
not a member of, so the authenticated write plane is no scope-existence
oracle - the read plane already reveals package existence to any valid
token, but which *scopes* are claimed is nobody's business to probe.
The check sits after the rate limit (probing is throttled like any
publish attempt) and consults only `scope_members`, never a live
provider call.

Only then storage is consulted: an existing pending or verified row answers
with the idempotent `200` no-op (reporting its verification status) or the
`409` immutability conflict, a rejected row is replaced in place (any
bytes; back to `pending`), and a new version writes the R2 blob first
(skipped when the content-addressed key already exists), then one atomic
D1 batch for the `packages` and `versions` rows. New and replaced rows
start `pending`. A crash between the two writes can only leave an
unreferenced blob - see [`runbook.md`](runbook.md).

Yank is a single-column `UPDATE` on the `versions` row, behind the same
uniform membership `403` - answered **before** the version lookup, so a
non-member cannot probe which versions exist under a foreign scope -
then `404` when the triple is unknown **or not verified** (a version
that never became resolvable has nothing to retract), idempotent,
reporting the resulting state and whether the request changed it. The
read path overrides the stored entry's `yanked` field from the column,
so the verbatim `metadata_json` never goes stale on the one field that
mutates.

## The verification lifecycle

Publish stores content; an external verifier (a later step; it runs in
GitHub Actions) decides what becomes part of the registry. The status
lives in `versions.verification` with `verification_reason` /
`verified_at` alongside; the pure transition rules, the artifact read
gate, and the verdict body live in `src/verify.rs`.

- **Reads are gated on `verified`.** `/packages/<scope>/<name>.json`
  composes
  verified versions only (the filter sits in the SQL query, so a package
  with none is an ordinary 404), and the artifact route serves verified
  versions to ordinary tokens. The `verify` scope may additionally list
  pending versions (`GET /api/v1/admin/versions?status=...`, each
  entry's `name` the canonical `<scope>/<name>`) and
  download their artifacts - the verifier has to fetch what it inspects.
  Rejected versions are served to no one.
- **Verdicts** (`PATCH /api/v1/admin/versions/<scope>/<name>/<version>`,
  scope `verify`, budget-gated like every write - the admin plane is
  registry infrastructure, so it needs no scope membership): `verified`
  stamps
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
  currently an operator. A dedicated verifier-only issuance path
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
calendar days. Publish enforces, in order: the budget gate (`402`), the
token scope (`403`), the rate limit (`429`, `Retry-After`, charged per
attempt), scope membership (the uniform `403` - "The write path"),
framing (`400`), metadata and checksum (`400`), the idempotent no-op /
immutability wall (`200`/`409`), then - for genuinely new versions only -
the archive-size cap (`413`) and the storage, package, and version quotas
(`403` with per-quota envelope codes) - so a byte-identical re-publish,
including one grandfathered above a later cap, never consumes quota. The
per-package quota counts key on the full `(scope, name)` pair, so equal
package parts under two scopes never share a bucket. Attribution rides on
`versions.published_by`, `versions.archive_size`, and `packages.created_by`,
keyed by the registry-native `users.id` (never a provider account
id). The bucket take is persisted as a compare-and-swap on the token
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
drift, and the counter can be recomputed from D1 alone if drift ever
needs reconciling - every version row carries `archive_size`, one size
per distinct live checksum (see [`runbook.md`](runbook.md), "Orphaned
R2 blobs"). The other metrics (Workers requests/day, R2
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
cache (~60 s TTL, one D1 point read on expiry; the smoke test pins
`SERVICE_MODE_TTL_SECS` to 0 via `.dev.vars`) and answer
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
  D1 batch succeed, the archive blob is copied to the
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
API's JSON shapes and body validation (`src/user_api.rs`), the
scope-claim grant rules and GitHub-response parsing (`src/claim.rs`),
the sign-in allowlist (`src/allowlist.rs`), the
quota engine (`src/quota.rs`), the budget breaker (`src/breaker.rs`), the
analytics query shapes (`src/analytics.rs`), the verification lifecycle's
statuses, verdict rules, and read gate (`src/verify.rs`), and the backup
logic - retention, dump validation, freshness (`src/backup.rs`) - compiles
and unit-tests on the host target. The Cloudflare glue
(`src/glue.rs` for the role dispatch and the Bearer planes,
`src/web_glue.rs` for the OAuth and session planes,
`src/backup_glue.rs` for the nightly dump job, wasm32 only) is thin
binding-and-I/O wiring covered by
`scripts/smoke.sh`. Every SQL statement the glue executes is a named
const in `src/sql.rs`, schema-validated at test time and guarded in CI
(see "Why no ORM" below). Read-plane path
components are validated before any lookup: scopes and names follow the
grammars in "Scopes", versions must look like SemVer, and anything else
answers without touching storage - the artifact filename must additionally
repeat the `<scope>-<name>-` prefix its directory segments fix, so a
downloaded tarball stays self-identifying and a disagreeing filename
never parses. The API routes only split their segments
(`src/routes.rs` documents why): publish validates them inside its `400`
sequence behind the membership gate, and yank and the admin verdict
answer unknown triples with an authenticated 404 straight from a
parameterized D1 query - no segment ever becomes a path or storage key
by itself.

Every authenticated response carries the debug header
`x-cabin-registry-generation` from `meta.registry_generation`, so a client
talking to a freshly wiped (pre-launch) registry is immediately visible
(see [`runbook.md`](runbook.md)).

## Why no ORM

An ORM was evaluated for the D1 access and rejected: the usual Rust
choices either do not compile for `wasm32-unknown-unknown` or drag in a
driver stack the Workers runtime cannot host, the generated code works
against the script-size limit, and - decisively - D1's only atomicity
primitive is the batch (see "The write path"), which an ORM's
connection-held transaction model fights rather than uses. What an ORM
would actually buy is covered without one:

- **Injection safety** comes from parameterization: every statement the
  Worker executes is prepared, and every runtime value rides a `?N`
  bind (the few fixed queries take no input at all).
- **Atomicity** is D1 batches by design; the multi-statement writes are
  explicit batches with their guards spelled out in SQL.
- **Typo and schema-drift assurance** - what an ORM's typed columns
  would catch at compile time - comes at test time instead: every
  executed statement is a named const in `src/sql.rs`, and
  `tests/sql_validation.rs` prepares each one with `rusqlite` against
  the real schema, freshly migrated from zero (D1 speaks `SQLite`'s
  dialect for everything the service uses). `scripts/check-sql.sh`,
  run by CI, keeps executed SQL from growing outside that module.
- Dynamic query construction does not exist today; if it ever
  genuinely grows, the designated escape hatch is `sea-query` (a
  wasm-safe query builder, not an ORM).

## Why a standalone workspace

`registry/` is its own Cargo workspace, listed in the root workspace's
`exclude`. The root workspace builds host-native binaries with a large,
carefully audited dependency tree and lockfile; this crate targets
`wasm32-unknown-unknown` through `worker-build` and pulls in the `worker`
ecosystem. Excluding it keeps `cargo build`/`cargo test` at the repository
root byte-identical to before the service existed, keeps the two lockfiles
independent, and mirrors how `website/` coexists in the repository with its
own toolchain and workflow (`.github/workflows/registry.yml`).
