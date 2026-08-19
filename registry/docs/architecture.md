# Registry Service Architecture

The service implements the server side of
[`../../docs/remote-registry.md`](../../docs/remote-registry.md) (the
authoritative protocol contract). This page covers only the decisions local to
the service.

## Storage

- **D1 is canonical.** Users and their external identities, scopes and
  their members, tokens, packages, versions, revisions, the
  `trustpub_configs` table (the per-scope registry of GitHub
  repositories and workflows allowed to exchange an Actions OIDC token
  for a short-lived `trustpub` token, bound by immutable numeric ids),
  `oidc_used_jtis` (the once-only jti replay guard shared by every
  GitHub OIDC audience the registry accepts), the
  `backup_pending` queue, and the `meta` key-value table all live in one
  D1 database (`migrations/`).
  Everything the read routes serve is composed from D1 rows. The
  resolution-level unit is a `versions` row (`yanked`, `downloads`); the
  immutable byte-level unit is a `revisions` row, whose `checksum` holds
  the canonical `sha256:<64 lowercase hex>` value and whose `revision` id
  is the leading 16 characters of that value's hex tail. Each revision's
  canonical index entry is stored verbatim at publish time in
  `revisions.metadata_json`; composition strips its `schema`, `name`,
  `version`, `checksum`, and `source` fields and injects `yanked` from the
  version row plus the `revision` pointer and `revisions` map, so yank state
  has exactly one home and revision identity is never duplicated.
- **R2 holds immutable, content-addressed blobs.** Archive bytes live at
  `blobs/sha256/<hex>` (the bare hex tail of `revisions.checksum`; key
  derivation strips the `sha256:` prefix so the OCI-style layout is
  stable).
  Blobs are never mutated; the one deletion path is the verification
  lifecycle's reclaim of a **rejected** revision's blob when no live
  (non-rejected) row references its checksum. Yanking is a D1 row
  update on `versions`, and the artifact route deliberately keeps serving
  yanked versions so locked-in consumers keep building.
- **Verified revisions are immutable.** Every `revisions` row carries a
  verification status (`pending` | `verified` | `rejected`; see "The
  verification lifecycle"). Re-publishing
  byte-identical metadata (which embeds the archive checksum, so
  identical metadata means an identical archive) over a pending or
  verified row is an idempotent
  `200 {"ok":true,"no_op":true,"revision":"<16 hex>","verification":"<status>"}`
  that touches neither store. Different bytes for a version that has a live
  revision need the `?new-revision=true` opt-in on the publish PUT: without it
  they are `409 the version is already published with different bytes;
  published revisions are immutable - pass `--new-revision` ...`, with
  it they create a new `pending` revision beside the existing ones. A rejected
  revision never became part of the registry, so identical bytes revive it in
  place back to `pending`. There is no unpublish or delete.
- **No KV.** The data is relational and small; a second store would only add
  consistency questions. Three Cache API surfaces exist: the public stats
  summary's fixed-key entry ("Download counts"), the immutable
  verified-artifact bodies, keyed by content checksum behind
  authentication ("The cost governor" - a cache hit costs no R2
  operation), and the GitHub Actions OIDC JWKS copy under one fixed
  synthetic same-zone key (`src/trustpub.rs`; ~600s TTL, refreshed by
  every origin fetch, with one cache-bypass refetch on an unknown key
  id).
- **The governor's ledger lives in one SQLite-backed Durable Object.**
  Every billable R2 resource Cabin can initiate - stored bytes and
  Class A/B operations - passes through its serialized admission
  control before the R2 call ("The cost governor"). D1 stays canonical
  for registry *content*; the ledger is accounting state whose
  **primary** pool is reconstructible conservatively from D1's
  live-checksum view, while the backup pool re-ledgers through the
  backfill script's queue rows and dump entries regrow with the
  nightly job ([`runbook.md`](runbook.md), "The cost governor").

## Origins and roles

One Worker serves two hostnames, one role per hostname, dispatched on the
Host header (`src/routes.rs` `role_for_host`; any host that is not the
`WEB_ORIGIN` host gets the registry role, deny by default). The matrix -
which routes and which credential exist where:

| | Registry custom domain (`registry.cabinpkg.com`) | Website origin (`cabinpkg.com`) |
| --- | --- | --- |
| `/healthz` | 200, unauthenticated | - |
| `/config.json`, `/packages/*`, `/artifacts/*` | public GET, verified content only (the read plane; a Bearer credential is optional and still honored) | - |
| `/login`, `/callback` | - | OAuth browser flow, no credential in / session cookie out |
| `/claim/<scope>`, `/callback/claim` | - | the scope-claim flow's dedicated OAuth roundtrip ("Scopes" below) |
| `/api/v1/user`, `/api/v1/user/{usage,packages,logout}`, `/api/v1/user/tokens[...]` | - | Session cookie **only** |
| `/api/v1/user/source/<scope>/<name>/<version>` | - | Session cookie **only**, ranged read ("The source viewer's ranged reads" below) |
| `/api/v1/user/search`, `/api/v1/user/package/<scope>/<name>`, `/api/v1/user/package/<scope>/<name>/reverse-dependencies` | - | Session cookie **only** ("Search and reverse dependencies" below) |
| `/api/v1/stats` | - | public GET, no credential ("Download counts") |
| `/api/v1/packages/*`, `/api/v1/admin/*` | - | Bearer **only** |
| `/api/v1/trusted_publishing/tokens` | - | `PUT`: OIDC JWT in the body (the one token-auth-exempt Bearer-plane route); `DELETE`: the exchanged token itself |
| everything else | uniform 401 + challenge | uniform 401 + challenge (unauthenticated) / authenticated 404 |

A dash means the path does not exist on that hostname: on the registry
domain every non-read-plane path answers the uniform 401 **without
consulting the `Authorization` header**, indistinguishable from any unknown
path; on the website origin nothing ever matches a machine read route, so
the read plane does not exist there - the one package-data surface
on this origin is the source viewer's session-authenticated ranged read
(below). Every Bearer-plane 401 carries the
byte-identical `WWW-Authenticate: Cabin login_url="<WEB_ORIGIN>/settings/tokens"`
challenge (`docs/remote-registry.md`, "The login-URL challenge"); session
401s deliberately do not, keeping the planes distinguishable.

**Public verified reads (recorded decision change, 2026-07-31).** The
machine read plane originally required a Bearer token like everything
else, under a uniform-401-before-route-matching discipline that covered
the whole data plane. That discipline is now **narrowed to the mutation
surface**: unauthenticated `GET`s of `/config.json`, package documents,
and artifact downloads serve **verified** content to everyone - a
public C/C++ package ecosystem cannot ask every consumer to
authenticate before downloading. What publicness does and does not
change:

- Pending, rejected, and unknown versions stay indistinguishable from
  missing to everyone without the `verify` scope; yanked keeps its
  download-path semantics. Package **existence** (of verified content)
  is therefore public - that is the point - while scope claims and
  lifecycle states stay non-oracles.
- Every mutation surface keeps its plane: publish, yank, token, admin,
  and verify stay authenticated, still answering the byte-identical
  uniform 401 with the challenge to unauthenticated callers
  (`cargo registry-smoke` asserts the bytes), and the session plane is
  unchanged.
- A presented credential is still a claim: a read-plane request whose
  Bearer token fails to validate answers the uniform 401 rather than
  silently degrading to anonymous, so the verifier's pending fetches
  fail loudly on a rotated token instead of reading as missing rows.
  A request with no `Authorization` header at all is an anonymous
  reader. The read plane is `GET`-only for everyone (405 otherwise).
- Under `reads_blocked`, anonymous readers receive the same
  `503 registry_over_budget` refusal as tokened ones ("Billing model:
  the governor and the breaker"). A public over-budget refusal
  necessarily reveals service state; that is inherent to public reads
  and recorded here alongside the uniform-401 revision.
- Pre-launch gating is unchanged and does not touch the read plane:
  `ALLOWED_GITHUB_IDS` gates sign-in (and thus writes), and
  `meta.launched` gates the destructive operator scripts. Public
  verified reads therefore apply pre-launch too, over data the
  data-policy section of [`runbook.md`](runbook.md) declares
  disposable; the launched end state is the same public read plane. In production
the website origin reaches this Worker through zone routes
(`cabinpkg.com/api/*`, `/login`, `/callback*` - which also covers the
claim flow's `/callback/claim` - and `/claim/*`; see `wrangler.jsonc`
and [`runbook.md`](runbook.md), "Integrated topology and route
management"). The frontend consuming
the session plane - `/dashboard`, `/dashboard/source`,
`/dashboard/package`, `/settings/*`, and `/login/denied`, all
static pages - lives in the repository's `website/` project ("Account
pages" in its README).

**The source viewer's ranged reads.** The matrix served no package data
on the website origin at all until the source viewer (the website's
`/dashboard/source` page) needed to read published archives from the
browser. The recorded decision: this origin gets exactly one
package-data route - `GET /api/v1/user/source/<scope>/<name>/<version>`
- and it is read-only, session-plane, verified-only, and range-limited.
It lives inside the `/api/v1/user` subtree because that is the session
cookie's `Path` - the cookie travels nowhere else - and deliberately
not under `/api/v1/user/packages`, the created-packages listing, where
it would read as ownership-scoped: any **verified** version is
readable, exactly like the artifact route with an ordinary token
(yanked stays viewable; pending, rejected, and corrupt-status rows are
the plain 404 by construction - the verified filter sits in the SQL).
The server stays a byte proxy: authenticate the session, resolve the
version row, forward one bounded R2 ranged read of the immutable blob -
no server-side unzipping, listing, or derived artifacts, which is the
design the strict zip profile exists to enable ("Why a strict zip
profile"; the browser parses the container itself). The `Range` header
is **required** (`400` without one) and must be a single
`bytes=<start>-<end>` or `bytes=-<suffix>` form of at most 4 MiB
(`src/source.rs`; deliberately stricter than RFC 9110's
ignore-and-serve-200); anything else - multi-range, open-ended,
oversized - answers `416`. The cap is a per-request resource bound,
nothing more: sequential requests can walk the whole archive - the
viewer needs exactly that, and a signed-in user could mint a token and
download the artifact anyway - but no single request streams a 16 MiB
blob. Responses carry the session plane's `Cache-Control: no-store`
like every authenticated response. The route never consults the
service mode - the session plane is exempt from the read-side budget
gate ("Billing model: the governor and the breaker") - but every
ranged read is a billable R2 operation, so it **is** governed: the
viewer draws from its own `b_source` pool with a per-user daily
fairness cap, and fails closed before R2 is consulted ("The cost
governor"). A source read is never a download ("Download counts").

**Search and reverse dependencies.** The dashboard's package search
(`GET /api/v1/user/search?q=<term>`) and the package resource
(`GET /api/v1/user/package/<scope>/<name>`, with
`/reverse-dependencies` nested under it) are **session-only by
recorded decision**. The original rationale - a public search endpoint
would let unauthenticated callers enumerate packages - narrowed when
verified reads became public ("Origins and roles"): verified package
existence is now public by design. What remains deliberate is that
these routes stay the dashboard's, serving its frontend on the session
cookie, so `/api/v1/stats` remains the only public JSON route on this
origin and no new unauthenticated, high-cardinality query surface is
opened as a side effect of public reads. All three live under
`/api/v1/user` because that is the session cookie's `Path`, nothing
more; `package` (singular - any one visible package) deliberately does
not nest under `/api/v1/user/packages`, the created-packages listing,
for the source viewer's reason. Visibility is the read plane's:
only packages with at least one **verified** version exist on these
routes (the gate is one shared helper, so the package and
reverse-dependencies 404s are identical by construction), pending and
rejected versions are invisible on these routes as on the whole read
plane (publishers still see their own lifecycle states in
`/api/v1/user/packages`, and the admin plane's `verify`-scoped
listing sees pending), and yanked versions stay listed and counted -
they stay resolvable for existing lockfiles.
Search matches the term (1-64 chars, trimmed, ASCII-lowercased -
names are lowercase by grammar, and the SQL comparison is byte-exact)
as a literal substring of the canonical `<scope>/<name>` name via
`instr` - deliberately not a `LIKE` pattern, which D1 caps at 50
bytes, under the 64-character term contract - ranking exact, then
prefix, then substring, ties by total downloads then name, truncated
to a hard 20.
The reverse-dependency contract is deliberately simple and **defined
over registry-resolvable references**: the distinct packages with at
least one verified version whose runtime `dependencies` map contains
the target's canonical `<scope>/<name>` key - matched exactly; dev-
and system-dependencies are never consulted, and a bare (unscoped)
key contributes no edge, because no bare name can denote a hosted
package (the read plane has no bare-name route; publish rejects bare
dependency keys outright - "The write path" - so the no-edge arm is
defense in depth, not a live case) - each with the count
of such versions and the newest matching version string. Both search
and the dependents walk scan the verified corpus per call
(`json_each` over `metadata_json` for the latter): accepted at
current scale, and watched rather than pre-optimized - the breaker's
`d1_rows_read_day` budget is the tripwire, and the upgrade path (a
publish-maintained dependents table, the crates.io approach) is to be
taken only if that metric warns. The dependents walk is the one
expensive query, which is why the package route does not fold it in:
the package resource stays a cheap two-point read, fit for the source
viewer to reuse as its version-picker data source.

## Two credential planes

Authentication is split into two planes that never accept each other's
credential, separated by route on top of the hostname split: the
`/api/v1/user` subtree is session-only, and everything else under
`/api/` is Bearer-only except the read-only public `/api/v1/stats`
subtree ("Download counts"), which takes no credential at all.

**The mutation surface is Bearer-only and deny-by-default.** Publish,
yank, and the admin plane require `Authorization: Bearer
cabin_<base62>` (or an exchanged `cabin_tp_<base64url>` token - same
header, same hash lookup). The uniform
`401 {"errors":[{"detail":"authentication required"}]}` (plus the
challenge header) is emitted before any route matching or D1/R2 data
lookup, so unauthenticated callers cannot probe the mutation routes -
and on the registry host every non-read-plane path answers the same
bytes. One route is exempt from the token check: the trusted-publishing
exchange (`PUT /api/v1/trusted_publishing/tokens`,
`docs/remote-registry.md` "Trusted publishing"), whose credential is
the GitHub Actions OIDC JWT in its body. It dispatches before
`authenticate` but never before the breaker's write gate, and every
refusal it emits - malformed body, failed verification, no matching
config, an unclaimed scope, a replayed jti - is that same uniform 401,
with the real reason logged for the operator only. The exchange mints
against the matched config: a 30-minute multi-use token backed by the
scope's oldest owner (publish requires scope membership and stamps
`published_by`, so an unclaimed scope refuses before the jti is
consumed), scope-limited to the config's scope, `publish`-only, and
carrying the config's quota class on the token row itself
(`tokens.quota_class`; the auth lookup coalesces token-first, so the
granted tier expires with the token instead of upgrading the backing
user). Each successful exchange lazily prunes expired
`oidc_used_jtis` rows and expired `trustpub` token rows -
deliberately no cron. `DELETE` on the same path revokes the presented
token iff it is a `trustpub` one (deliberately not behind the write
gate: blocking revocation would keep a live credential alive), and
answers everything else with the uniform 401. The read plane itself is public for verified content ("Origins
and roles"); it honors an optional Bearer credential, and a presented
credential that fails to validate is the same uniform 401. `/healthz`
(registry host) and the public `/api/v1/stats` subtree (website
origin; "Download counts") are the only routes outside both planes.
Cookies are never read here.

Tokens are stored as the SHA-256 hex of the full token string; the plaintext
exists only in the client's hands (it is rendered exactly once, in the
create-token response). A valid token additionally opens the read
plane's `verify`-scope carve-outs;
`scopes` (a subset of `publish,yank,verify`) gates the mutation routes and
the verifier's admin plane.
A token row is live only from `created_at` through `expires_at` (`NULL`
never expires), enforced inside the single lookup's WHERE clause, so an
expired or not-yet-valid token produces the exact no-row answer an
unknown one does - the uniform 401, with no response or timing oracle.
A row with `scope_limit` set may perform write-side package operations
(publish, yank) only under exactly that scope; a mismatch
answers the write plane's uniform membership 403. Verdicts take no
registry token at all ("The verification pipeline"). `kind` is a closed
domain (`user` | `trustpub`), and the schema itself requires a
`trustpub` row to be expiring (within one day of its `created_at`
anchor), scope-limited, publish-only, and explicitly classed
(`tokens.quota_class`) - the minting path cannot widen the exchange
into a standing or privileged credential.
`last_used_at` is updated best-effort off the response path, and log lines
carry the token row id - never the token or its hash.

**The session plane is cookie-only.** Every response is JSON except the
source viewer's ranged byte reads ("Origins and roles"). `/login` and
`/callback` run the
GitHub OAuth sign-in (web application flow, no OAuth scopes requested,
explicit `redirect_uri` of `<WEB_ORIGIN>/callback`); `/claim/<scope>`
and `/callback/claim` run the scope-claim flow's dedicated roundtrip
("Scopes" below), the one flow that requests an OAuth scope
(`read:org`); and the `/api/v1/user` subtree is the JSON user API the
website frontend consumes:

- `GET /api/v1/user` -> `{"github_id":..,"login":..,"quota_class":..}`;
- `GET /api/v1/user/usage` -> quota class, package count, lifetime
  scope-claim count, stored bytes (rejected versions excluded - their
  bytes were refunded), today's publishes, per-status version counts,
  and the class's quotas;
- `GET /api/v1/user/packages` -> the packages the user created, each
  version carrying its verification state, yanked flag, and served-
  download count ("Download counts"; the dashboard's package list);
- `GET /api/v1/user/source/<scope>/<name>/<version>` -> a bounded
  ranged read of a verified version's archive bytes for the source
  viewer ("The source viewer's ranged reads" - the one non-JSON
  response on this plane);
- `GET /api/v1/user/search?q=<term>` -> the dashboard's package
  search over the visible (verified) packages ("Search and reverse
  dependencies"; a missing or malformed `q` is a `400`);
- `GET /api/v1/user/package/<scope>/<name>` -> one visible package's
  verified versions (newest first, with yanked state and downloads)
  and its newest version's runtime dependencies;
- `GET /api/v1/user/package/<scope>/<name>/reverse-dependencies` ->
  the packages with a verified version depending on the target (the
  contract lives in "Search and reverse dependencies"); both package
  routes answer the authenticated 404 for a package with no verified
  version;
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
only the account-age refusal (below) carries a query naming its reason and
the first eligible UTC date. All targets are fixed relative paths, never
derived from request input (the open-redirect guard).

- Identity is **registry-native**: a `users` row (registry id, quota
  class) plus one `identities` row per external account, keyed by
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
  removing an id locks it out immediately. A **first** sign-in is also
  account creation, and additionally requires the GitHub account to be
  at least 30 days old (`src/signup.rs`, a throwaway-account speed
  bump, judged from the profile's `created_at`): a younger account is
  refused with `/login/denied?reason=account-age&eligible=<date>`, the
  first UTC date on which the whole day is eligible. Returning users
  are never re-checked, and a new account whose profile lacks a
  parseable `created_at` fails closed into the uniform refusal. Write authorization is per
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
  separation (`src/session.rs`); the Worker refuses a configured key shorter
  than 32 bytes, matching the runbook's random-key provisioning command.
  Cookies are `HttpOnly; Secure; SameSite=Lax`, and
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
  token state is needed. Mutation JSON is streamed under a 4 KiB cap; the
  publish frame uses the same bounded reader with its documented 64 MiB cap,
  so a chunked request cannot bypass the declared-length preflight and force
  an unbounded buffer. A route the browser *navigates* to can carry neither
  header, so claim initiation is gated on fetch metadata instead
  (`session::navigation_is_user_initiated`, "Scopes" below).
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
(disputes are handled manually; there is no alias mechanism, and the
only unclaimable strings are the short reserved vocabulary in "Name
fidelity"). `scope_members` holds per-scope membership, where
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
`scope_members` plus one row in the append-only `scope_claims`
history, in one D1 batch - still a primary-key insert on the scope
name, so the loser of a claim race rolls back seedless (and with no
history row: a failed claim never consumes claim capacity) and is
refused. Claims are bounded per user for their lifetime
(`max_scope_claims_total` on the quota class, `src/quota.rs`):
each batch statement repeats a guard counting the claimant's
`scope_claims` rows, so two concurrent claims cannot race under the
limit, and an over-limit claim suppresses the whole batch (zero
changed rows) into the same refusal as every other. The count is
over grants ever made, deliberately not current ownership -
`scope_claims` rows are never updated or deleted, so a future
release or transfer of a scope will not restore capacity. Every
refusal is one uniform redirect with no detail. A claim is
**permanent**: an already-claimed scope refuses whoever asks - even an
account that now controls the GitHub name - and there are no transfer
or release endpoints; disputes are handled manually by the operator
(direct D1 surgery; the schema pins the role domain so a hand-run
typo cannot orphan a scope).

**Claim initiation is gated on intent.** The initiating GET mints the
matching state cookie itself, so the sealed state proves only that one
browser walked the whole roundtrip - never that its user asked to; and
once `read:org` has been granted (any prior claim) GitHub auto-approves
the authorize page, so a forced roundtrip completes without a click.
`/claim/<scope>` therefore refuses unless the browser's `Sec-Fetch-Site`
reads `same-origin` (the site's own page) or `none` (no in-browser
initiator at all), `Sec-Fetch-Mode` reads `navigate`, and no
`Sec-Purpose` is present - sealing nothing on refusal. `Referer` could
not carry this check: the *initiating* document picks its own referrer
policy, so an attacker page can suppress the header on the navigation
it forces and look exactly like an address-bar one. Fetch metadata is
set by the browser from the initiator it computed, and no page can
influence it. The mode and purpose conditions rule out same-origin
requests that are not navigations the user performed: the website
prefetches its own links wholesale (`prefetchAll`), so a future
dashboard claim link would otherwise seal claim state on hover, and a
prerender is a navigation that may never happen. An absent header
refuses too, because a client that sends no metadata proves nothing
about who initiated the request. What stays open is `none`, which
covers every navigation handed to the browser from outside it - a
mailed link, or an external protocol handler an attacker page invokes -
accepted while typing the URL is the only way to reach the route, since
no dashboard entry point exists yet. The hardening for open sign-up is
to initiate claims from a session-authenticated, CSRF-checked POST, at
which point `same-origin` becomes the only accepted value. Even then the blast
radius is bounded: a claim only ever binds a scope to the account that
genuinely controls the same-named GitHub account, with that account's
user as owner, so a forced claim can at worst spend one of the
victim's lifetime slots on a name the victim owns.

Scope-proof automation is GitHub-only **by policy**, even though the
schema (`proof_provider`, `identities.provider`) is provider-neutral.
Membership management is registry-side only: owners list, add, and
remove members through the session API ("Two credential planes"
above); there is no automatic GitHub org sync (TODO: revisit once
sign-up opens beyond the allowlist), so org membership changes on
GitHub propagate only when an owner edits the member list.

The client speaks the scoped protocol end to end: manifests, local
file registries, lockfiles, and the resolver carry `<scope>/<name>`
verbatim; `cabin publish` rejects bare names outright; the sparse
reads, the publish/yank routes, and the external verifier
(`crates/cabin-registry-verify` and its workflow) all address the
scoped routes, with the artifact filename flattening the `/` to `-`
and appending the packaging revision
(`<scope>-<name>-<version>-<revision>.zip`). The same grammar covers dependency
references: the registry dependency maps (`dependencies`,
`dev-dependencies`) key on canonical `<scope>/<name>` names -
enforced client-side before any network work and server-side in the
publish `400` sequence ("The write path") - while
`system-dependencies` is exempt: its keys name system packages, not
registry packages.

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
   `../../artifacts/<scope>/<name>/<scope>-<name>-<version>-<revision>.zip`,
   the filename embedding the scope and the packaging revision like the
   artifact route (the revision is derived from the digest of the uploaded
   bytes, computed before metadata validation so it can be checked here)
   (`400`);
6. the scope and name match the grammars in "Scopes", the name is not
   on the reserved list ("Name fidelity"; the package part only - a
   reserved scope can never be claimed, so the membership `403`
   answers for it), and the version is valid SemVer (`400`);
7. every key of the metadata's `dependencies` and `dev-dependencies`
   maps is a canonical `<scope>/<name>` name (`400`) - a bare key
   could never resolve against this registry (the read plane has no
   bare-name route), and published metadata is immutable, so one
   admitted here would be a permanent dark edge in the
   reverse-dependency graph; dev-dependency keys denote registry
   packages too, so one grammar covers both maps, while
   `system-dependencies` is exempt - its keys name system packages,
   not registry packages. The client enforces the same rule before any
   network work, and the verifier mirrors it structurally: the stored
   document passed this check, and its manifest-equality pass forces
   the archived manifest's maps to match;
8. when the document declares an `upstream` provenance block, it passes
   the lexical mirror of the client's provenance rules - an
   `https://` URL whose authority embeds no credentials, a
   `sha256:`-prefixed 64-hex `checksum`, a `"tar.gz"` / `"zip"`
   format, a single-component
   `strip-prefix`, non-escaping copy paths, and non-escaping
   `patches` entries distinct from copy paths and the root
   manifest (`400`).  A strict
   subset of the client parser's typed validation, so an honest
   client is never rejected here; the external verifier's
   metadata-equality pass plus its upstream-archive comparison are
   the authority (`docs/remote-registry.md`, "The verifier's
   checks").  The Worker itself never fetches the upstream URL -
   all archive fetching stays out-of-Worker by design;
9. `yanked` is `false` (`400`);
10. the archive bytes pass a container sanity check - a zip whose EOCD sits
   at the fixed `len - 22` offset with a zero comment and single-disk
   fields, and whose central directory abuts the EOCD (all O(1) reads; see
   [`archive-format.md`](archive-format.md)) - the cheapest rejection, taken
   before hashing so a non-zip never reaches SubtleCrypto (`400`);
11. the metadata's `checksum` equals `sha256:` + the digest the server itself
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

Only then storage is consulted, keyed on the revision the uploaded bytes
name: an existing pending or verified row with the same checksum answers
with the idempotent `200` no-op (reporting its revision and verification
status); the same revision id with a different checksum is a loud `409`
collision; different bytes for a version that still has a live revision
need the `?new-revision=true` opt-in and are otherwise the `409`
new-revision-required conflict; a rejected revision is revived in place by
its identical bytes (back to `pending`); and a new revision writes the R2
blob first (skipped when the content-addressed key already exists), then
one atomic D1 batch for the `packages`, `versions`, and `revisions` rows
plus the stored-bytes meta bump. A publish that would
create a **new** package additionally refuses (`400`, after the size cap
and before the quota `403`s - name validity does not depend on quota
state) when the name collides with an existing same-scope package under
`-`/`_` folding; the check is a preflight in the quota-count batch, and
the persistence batch repeats it as a transactional guard so two racing
twins cannot both land ("Name fidelity"). New and replaced rows
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
GitHub Actions) decides what becomes part of the registry. The status is
per revision: it lives in `revisions.verification` with
`verification_reason` / `verified_at` alongside; the pure transition
rules, the artifact read gate, and the verdict body live in
`src/verify.rs`.

- **Reads are gated on `verified`.** `/packages/<scope>/<name>.json`
  composes
  verified revisions only (the filter sits in the SQL query, so a package
  with none is an ordinary 404), and the artifact route serves verified
  revisions to ordinary tokens. Each served version's `revision` pointer
  comes from the `current_revisions` view - the verified revision with the
  greatest `published_at`, breaking ties on the greater revision id - and
  its `revisions` map carries every verified revision, so a superseded one
  stays fetchable by pin and a pending respin never disturbs what is
  served. The `verify` scope may additionally list
  pending revisions (`GET /api/v1/admin/versions?status=...`, one entry per
  revision, each carrying `revision` and its `name` the canonical
  `<scope>/<name>`), fetch the package
  corpus its name advisories compare against
  (`GET /api/v1/admin/packages` - "Name fidelity"), and
  download their artifacts - the verifier has to fetch what it inspects.
  Rejected revisions are served to no one.
- **Verdicts** (`PATCH /api/v1/admin/versions/<scope>/<name>/<version>` -
  authenticated not by a registry token but by a GitHub Actions OIDC
  JWT with audience `cabinpkg.com/verifier`, presented as the bearer;
  see "Trust model" below. Verdicts are deliberately exempt from
  the budget breaker: a verdict stores no new bytes, a rejection frees
  them, and verification must be able to drain the pending queue
  whatever the service mode - "Billing model: the governor and the breaker"):
  `verified` stamps
  `verified_at`; `rejected` records the reason, refunds the archive's
  bytes from `meta.total_stored_bytes` when the row was the checksum's
  sole live reference (decided inside the same transaction that flips
  the row, so a duplicate concurrent verdict cannot refund twice), and
  reclaims the blob best-effort - a failed delete leaves an orphan the
  governor's ledger keeps conservatively represented until an operator
  releases it, the replacement path retries the delete, and publishes
  re-check their blob after their batch commits, so a reclaim racing a
  deduplicating publish of the same bytes is self-healed. The body's
  `checksum` and `published_at` bind the verdict to what the verifier
  listed, and both are required for **both** verdicts: `checksum` names
  the revision under the route's `(scope, name, version)` triple, and
  `published_at` names the publish event - a byte-identical revival
  regenerates the row under the same checksum, so without the pair a
  delayed verdict of either direction could land on a generation it
  never judged,
  and the applying updates are guarded on the row still being pending
  with the bytes the request read, so a verdict racing a conflicting
  verdict or a replacement answers 409 instead of applying - the
  verified arm must never resurrect a row a concurrent rejection just
  reclaimed. Repeating the verdict a terminal version already carries
  is the idempotent 200 (a repeat rejection also re-drives the blob
  reclaim, so a retry after a failed reclaim converges); a conflicting
  verdict on a verified version and a verifying verdict on a rejected
  one are 409 (republish is the recovery path - a late duplicate
  verdict cannot race a replacement back to pending because the
  replacement changes `published_at` and fails the binding first).
- **Trust model.** The verdict endpoint accepts exactly one caller: the
  verify workflow of the repository the `VERIFIER_*` wrangler vars pin
  (numeric owner and repository ids, workflow filename, git ref). Its
  credential is the workflow run's own GitHub-signed OIDC JWT, verified
  by the same module as the trusted-publishing exchange
  (`src/trustpub.rs`) under the distinct audience
  `cabinpkg.com/verifier` - a token minted for either endpoint is dead
  on the other - with each JWT's `jti` consumed once in the shared
  `oidc_used_jtis` ledger, so a captured token cannot deliver a second
  verdict. Every authentication failure answers the byte-identical
  uniform 401 with the reason logged server-side only. The read side
  (listings, corpus, pending downloads) and the governor stay on
  `verify`-scoped registry tokens: they are operator surfaces, not the
  workflow's. The external workflow pins the expected API origin
  independently and requires `config.json` to match it before sending
  its credential, so registry-controlled discovery cannot redirect it
  to another host.
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
  endpoint's stored sum exclude rejected rows the same way.

Conformance is enforced from the monorepo: `cargo gen-fixtures` builds
the in-tree `cabin` binary and packages real fixture pairs, which the
`conformance` CI job (and a frozen pair under `tests/fixtures/`) feeds
through the full server-side validation path, so the client's canonical
output and the server's schema cannot silently drift.

## Name fidelity

**Threat model.** Scoped names already remove the classic registry
typosquat - only members of `fmtlib` can publish `fmtlib/*` - which
moves the confusability attack to **scope claims**, a fully automatic
path (GitHub proof in, grant out, no human). Package verification is
fully automatic too: the CI verifier PATCHes verdicts on a cron, so
without an extra layer no human would ever see a name before it goes
live. Cabin also has no reactive security apparatus (no report inbox,
no takedown rota), so review is front-loaded instead. The design is
three layers, each calibrated by what a false positive costs:

1. **Deterministic rejects** - zero false positives, so they refuse
   outright. A publish that would create a new package is a `400` when
   the name collides with an existing same-scope package under `-`/`_`
   folding ("The write path" has the ordering and the transactional
   guard), and a short reserved list refuses both package names
   (publish `400`) and scope names (the claim flow's uniform denial):
   the DOS device stems (`con`, `com1`, ... - package and scope names
   become client-side directory names in the vendor and cache layouts,
   which the archive-path predicate never sees) plus a small
   operator-maintained project vocabulary (`cabin`, `cabinpkg`, `std`,
   `core`). This is exactly the crates.io footprint: its only
   automated anti-typosquat rule is the `-`/`_` collision, plus
   reserved std-lib/keyword and Windows device names.
2. **Verifier name advisories with an abstain outcome** - checks that
   can false-positive must cost a **delay, never a rejection**. The
   verifier runs them before downloading any archive (they need no
   bytes; an abstained version costs a listing entry per cron pass,
   not a 16 MiB re-download): skeleton-fold confusability against the
   whole package corpus (`-`/`_` fold away, `{1, i} -> l`, `{0} -> o`;
   equality with any existing package or with a different existing
   scope's fold), edit distance 1 on the folded full name against
   other scopes' packages (the near-scope squat; same-scope siblings
   like `fmt`/`fmts` are members' own work and exempt), and a short
   unambiguous-slur list matched as folded substrings. A finding means
   **abstain**: no PATCH at all, the version stays `pending` (already
   invisible to readers - the fail-safe state), and the stuck-pending
   alert summons the operator, who resolves it with a manual verdict
   ([`runbook.md`](runbook.md), "Verification pipeline"). Abstain is a
   workflow outcome, not a fourth `revisions.verification` state and
   not a wire state - clients only ever see `pending`. Advisories run
   only for versions that would introduce a **new** name: once any
   version of the package is **verified**, the name was accepted -
   the advisories proceeded, or an operator approved it past an
   abstain - and every later version skips them (`libass` gets
   delayed once, ever). A rejection deliberately never vets a name:
   otherwise rejecting an abstained squat would exempt that very
   name's next version from the advisories. The corpus comes from
   `GET /api/v1/admin/packages` (`verify` scope, on the admin plane
   with the same no-budget-gate rationale as verdicts): every
   package's `(scope, name)` plus that `vetted` bit.
3. **Scope-claim confusability refusal** - at claim time, after the
   grammar and GitHub-proof checks, a scope whose skeleton equals an
   already-claimed scope's refuses through the same uniform denial as
   every other claim failure. **Skeleton equality only, no edit
   distance**: a claim refusal is hard and unexplained, and generic
   distance-1 collides with real login patterns (`jsmith` /
   `jsmith1`), while skeleton equality catches the homoglyph squat
   (`fmtl1b` vs `fmtlib`) with near-zero false positives. The
   operator escape hatch for a legitimate collision is
   `CLAIM_SKELETON_EXEMPT_SCOPES` (exact names; never bypasses the
   reserved list or claim permanence). The check is a preflight read,
   deliberately not an in-batch guard: closing the milliseconds-wide
   race between two independent OAuth roundtrips would need the
   skeleton fold mirrored into SQL, and claim disputes are handled
   manually anyway ("Scopes").

Two implementation notes, recorded because they diverge from the
obvious spelling. The worker cannot depend on `cabin-fs` (it is a
standalone wasm32 workspace - "Why a standalone workspace"), so its
DOS-stem list is a lowercase mirror pinned to the shared predicate by
a host-only dev-dependency parity test (brute force over every
grammar string of at most 4 bytes, the length of the longest ASCII
stem), the same structural-mirroring pattern as the dependency-key
grammar. And the skeleton fold exists twice - the worker and the
verifier - for the same reason, each copy pinned by its crate's
tests.

## Download counts

Every served artifact download of a **verified** version counts
toward `versions.downloads` through an in-isolate buffer flushed in
one batched D1 write (`src/telemetry.rs` holds the flush policy: the
first download 30 s after the last flush, or 50 pending versions,
whichever first - there is no timer, so a lone buffered count waits
for the next download or the isolate's end;
`DOWNLOAD_FLUSH_INTERVAL_MS` overrides the interval and the smoke
test pins 0) - deliberately **not** one D1 write per download, which
would make the counter a per-download cost channel, and deliberately
not another global hot object. Counts are buffered only once a 200
response was constructed - refusals and missing-blob 500s never
count, the verifier's pending fetches never count (the SQL guard
repeats the verified check inside the statement, so lifecycle races
cannot count either), and yanked versions keep counting because they
stay downloadable. Edge-cache hits still count: the Worker runs on
every request, hit or miss. The metric is approximate telemetry and
never part of the governor's hard ledger: an isolate that dies with a
non-empty buffer loses those counts, a failed flush is logged and
dropped, nothing about it can fail or delay a download, and the flush
(its breaker-mode read included) runs off the response path and is
suppressed while the breaker blocks writes - the write plane's
fail-closed direction, while the download itself was already served.
The counter tracks the registry artifact route **only**: the
website origin's source-viewer reads ("The source viewer's ranged
reads") never increment it - browsing files is not an install, and a
viewer session would otherwise count one download per file viewed.

`GET /api/v1/stats` on the website origin is the one unauthenticated
JSON route: registry-wide totals over verified versions only -
`{"packages":..,"versions":..,"downloads":..}` - consumed by the
website's homepage. This deliberately makes aggregate, verified-only
numbers public; the response names no packages and reveals nothing
about scopes, pending, or rejected versions, so the write plane's
uniform 403 and the claim flow's non-oracle posture are untouched.
The response is served through the Cache API under one fixed,
query-less key (the canonical stats URL, so `?nonce=` cannot bust the
edge cache) with `Cache-Control: public, max-age=300`
(`STATS_CACHE_TTL_SECS` overrides the TTL; 0 - the smoke test -
disables caching), so displayed numbers may lag by up to the TTL plus
the deferred write. Method and path discipline mirror the session
plane: non-GET is 405, unknown paths under `/api/v1/stats` are public
404s that never fall through to the bearer plane, and on the registry
hostname the subtree does not exist (uniform 401). A public
per-package stats route is deliberately deferred until the website
has registry-package pages to feed: it would be a high-cardinality,
unauthenticated, per-package existence oracle with no consumer today.

## Billing model: the governor and the breaker

Cost containment is two mechanisms with a deliberate division of labor:

- **The cost governor** ("The cost governor" below) is the hard,
  request-time authority for every billable R2 resource Cabin can
  initiate - stored bytes, Class A (write/list) operations, and Class B
  (read) operations. Nothing the deployed Worker does may start a
  billable R2 call without the governor's admission first, and a
  governor outage fails those paths closed. Abuse can exhaust an
  isolated allowance and turn requests into `503`s, but it cannot turn
  into unbounded R2 spend.
- **The service-wide breaker** (below) covers what cannot be exactly
  pre-authorized because the platform only reveals it after the fact -
  Workers requests per day and D1 rows read per day - plus broader
  degradation: it is the operator-visible service mode, and the
  Cloudflare GraphQL Analytics numbers it evaluates act as an
  **independent auditor** of the governor's ledger, never as an
  authority that could grant allowance.

**Per-user quotas** stop any single user from exhausting the shared free
budget. Quota classes are quota tiers granted manually on need (in the
spirit of crates.io's per-user limit increases); the hosted registry is
free and has no billing path. `users.quota_class` (default `'default'`)
selects a quota set from the map in
`src/quota.rs` - per-archive bytes, total stored bytes per user, new
packages per day, total packages, versions per package per day, a
lifetime scope-claim cap (counted over the append-only `scope_claims`
history - "Scopes" above), a
publish token bucket (burst plus per-minute refill, state on the token row
in `tokens.rl_tokens` / `tokens.rl_updated_at`), and the governor's
per-user daily read-fairness caps (charged artifact reads and
source-viewer reads). Two classes exist: `default`, and `operator` -
the bulk-publishing tier for the operator's own accounts, sized so the
cabin-ports conversion pipeline can seed the entire curated port set in
one serial run (values in `src/quota.rs`). Granting a class is a manual
`UPDATE users SET quota_class = '...'`; there is deliberately no admin
route. Class limits are read live on every request, but the token
bucket's *balance* persists on the token row, so a promotion becomes
fully effective only after the balance refills toward the new burst at
the new rate (minutes) - reset the account's buckets in the same change
(`UPDATE tokens SET rl_tokens = NULL, rl_updated_at = NULL WHERE
user_id = ...`; a NULL bucket reads as full) when the promotion must
take effect immediately. Daily windows are UTC calendar days. Publish
enforces, in order: the budget gate (`503`), the token scope (`403`),
the rate limit (`429`, `Retry-After`, charged per attempt), scope
membership (the uniform `403` - "The write path"), framing (`400`),
metadata and checksum (`400`), the idempotent no-op / immutability wall
(`200`/`409`), then - for genuinely new versions only - the
archive-size cap (`413`), the storage, package, and version quotas
(`403` with per-quota envelope codes), and finally the governor's
storage admission (`503`) - so a byte-identical re-publish, including
one grandfathered above a later cap, never consumes quota. The
per-package quota counts key on the full `(scope, name)` pair, so equal
package parts under two scopes never share a bucket. Attribution rides on
`revisions.published_by`, `revisions.archive_size`, and `packages.created_by`,
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
re-use under a second name never double-counts it). The other metrics
(Workers requests/day, R2 Class A operations/month, D1 rows read/day,
R2 Class B (read) operations/month) come from the Cloudflare GraphQL
Analytics API, queried by a cron pass every 15 minutes
(`src/analytics.rs` holds the dataset names; a rejected dataset degrades to
"metric unavailable", and partial data can escalate the mode but never
de-escalate it - missing analytics never unblocks writes). Completeness
is judged per plane: de-escalation at and below `writes_blocked` needs
the four write-side metrics, and lifting `reads_blocked` needs the
Class B metric (which counts only while the read breaker is armed -
dormant, warn-only monitoring must not keep a stale `writes_blocked`
pinned over a metric that could never have caused it). The planes are
independent on purpose: a write-side analytics outage drops a
`reads_blocked` whose read data proves recovery to `writes_blocked` and
no further, while a read-side outage never reopens reads.

The analytics numbers are approximate by Cloudflare's own
documentation, which is why they are **never the authority for the hard
R2 limits**: the governor enforces those from its own exact ledger, and
nothing the analytics cron observes - or fails to observe - can
increase a governor allowance, release a conservative reservation, or
prove recovery. The R2 metrics' remaining roles are auditing (a large
divergence between observed R2 operations and the governor's consumed
totals means spend outside the governed paths - alert and investigate)
and the emergency escalation below, which can only make the service
*more* closed.

Degradation order: `normal` -> `warn` (any metric at 80% of its budget)
-> `writes_blocked` (a write-side metric at budget) -> `reads_blocked`
(the configured read budget exhausted; strictly worse on the ladder, so
writes stay blocked too). Each metric carries an escalation ceiling
alongside its budget: the four write-side metrics escalate to
`writes_blocked`, while R2 Class B - the read path's metric - is
dormant monitoring by default. With `BUDGET_R2_CLASS_B_MONTH` unset it
evaluates against a built-in 8,000,000 budget (80% of the 10M free
limit) whose ceiling is `warn` and no higher: a write block cannot fix
read-driven spend, so escalating a read-driven metric to
`writes_blocked` would be incoherent. Setting the env var is the
deliberate act that makes `reads_blocked` reachable at all
([`runbook.md`](runbook.md), "Read budgets and paid-plan activation").
The mode and a human-readable reason live in `meta.service_mode` /
`meta.service_mode_reason`; mode changes are logged and optionally
POSTed to `NOTIFY_WEBHOOK_URL`. On the request path publish, yank, and
the read gate below share one isolate-memory mode cache (~60 s TTL, one
D1 point read on expiry; the smoke test pins `SERVICE_MODE_TTL_SECS` to
0 via `.dev.vars`); publish and yank answer `503 registry_over_budget`
with `Retry-After` at `writes_blocked` and above, and fail closed - an
unreadable or unknown mode blocks them ("Why 503, not 402").

**Reads gate only on an affirmatively read `reads_blocked`** - a
recorded revision (2026-07-18) of the original principle that reads
never consult the mode. The fail semantics stay asymmetric on purpose:
a missing, unreadable, or unknown mode, a failed cache fill, or any
error on the mode lookup leaves reads serving exactly as before, so
yanked-state lookups keep working throughout an outage of the breaker
itself. (The governor is the opposite by design: any R2-touching read
that cannot get an admission answer fails closed - the asymmetry is
possible because a cache hit needs no admission, so popular content
keeps flowing through either outage.) The gate covers the machine read
plane only (`/config.json`, `/packages/*`, `/artifacts/*`) and answers
one envelope for everyone: `503 registry_over_budget` with
`Retry-After`. With public reads ("Origins and roles") the gate runs
for anonymous readers too - it sits after the optional credential
check (which still needs to run first: the verifier exemption below is
scope-derived), and a public over-budget refusal necessarily reveals
service state, which is inherent to public reads and recorded with the
uniform-401 revision. Exempt
from the gate: `/healthz`, the public `/api/v1/stats`, the entire
session plane (the dashboard is where the operator and users see what
is happening while blocked), the admin plane, and the verifier's
`config.json` and artifact fetches (never the package documents, which
it does not read) - verification must be able to drain the pending
queue while reads are blocked, and its spend rides its own isolated
governor pool. For the same reason the admin verdict is exempt from
the budget gates entirely: a verdict stores no new bytes and a
rejection frees them ("The verification lifecycle").

## The cost governor

One named, SQLite-backed Durable Object (`GOVERNOR` binding, singleton
`"governor"`) is the serialized authority for the shared R2 budgets.
The accounting engine is pure Rust (`src/governor.rs`) over a tiny
store abstraction, so the same SQL and decision logic run against
`rusqlite` in host tests and against the object's SQLite storage in
production (`src/governor_do.rs`); the Worker talks to it through a
narrow, typed, idempotent JSON protocol (`src/governor_client.rs`:
`decide`, `usage`, `reconcile`). At the registry's scale one object is
the simplest design that makes every decision atomic; sharding is the
upgrade path if the singleton ever becomes a throughput problem.

**Pools.** Storage is three byte-stocks with one ledger row per R2
object: `primary` (BLOBS archive blobs), `backup` (BACKUP verified
copies), and `dump` (BACKUP `d1/` dumps and sidecars). Billable
operations are seven monthly flows: Class A `a_publish` / `a_infra`
and Class B `b_ordinary` (artifact cache misses), `b_source` (the
source viewer), `b_verifier` (pending-artifact fetches), `b_publish`
(publish-path existence heads), and `b_infra` (replication and dump
reads). The split is the isolation: read abuse can exhaust only
`b_ordinary`, the verifier keeps draining its queue on `b_verifier`,
and the infrastructure pools are reachable from no request
classification at all - only the cron jobs and the write path's
internals touch them, so no ordinary credential can spend them. Limits
are env-overridable (`GOVERNOR_*`, in-code defaults in
`src/governor.rs`) and sized with headroom under the R2 free tier; a
limit var that is set but unparsable fails closed to **zero** - a typo
in a hard cap must block its pool loudly, not silently revert to a
default ([`runbook.md`](runbook.md), "The cost governor").

**Semantics.** Operation budgets are consumed immediately before each
billable R2 call - a refusal means the call is never made, and a crash
after consumption merely wastes one op (conservative). Storage runs
reserve -> write -> settle: capacity is reserved before the R2 put,
and the reservation settles into committed usage only once the D1 rows
prove the object referenced. Reservations are keyed by the
content-addressed object key, which makes retries and concurrent
identical publishes *share* one reservation instead of double-counting
(same key means same bytes), makes the crash-retry heal itself (the
retry's settle lands on the crashed attempt's row), and turns a
conflicting byte count under one key into a refusal rather than a
silent merge. The invariant: **committed plus reserved usage never
exceeds a pool's limit at admission, and the ledger never understates
R2 reality.** A commit therefore never refuses - once bytes exist in
R2, refusing to record them would create unaccounted spend, so
admission catches up at the next reservation instead. Nothing is
released by age: a reservation whose write outcome is unknown stays
held until reconciliation proves the object live (settling it) or an
operator releases it with evidence; the automated release paths run
only where the code affirmatively knows the paid write did not stick -
a refused admission (the call was never made) and the dump jobs'
affirmative deletes of their cron-unique objects. The primary pool
never auto-releases, not even after a successful delete: a
content-addressed key can be recreated by a concurrent same-checksum
publish at any moment, so "deleted now" is no proof of "stays gone",
and reclaimed entries wait for the operator instead
([`runbook.md`](runbook.md), "The cost governor"). Operation windows are **explicit UTC calendar
months** - Cloudflare's actual billing window cannot be inferred
reliably, so the budgets carry headroom for the skew instead - and
roll forward only, so a regressed clock can never reset a window and
mint fresh allowance. A reinitialized object recreates its schema
idempotently; its primary rows rebuild conservatively from D1's
live-checksum view via reconciliation, the backup pool re-ledgers
through the backfill script's queue rows, dump entries regrow with
the nightly job, and operation windows restart at zero
([`runbook.md`](runbook.md), "The cost governor", "Known ceilings").

**The read plane and the edge cache.** Immutable verified archives are
served through the Cache API under a synthetic identity derived from
the content checksum alone (`https://registry.cabinpkg.com/__cache/...`;
never from the outward URL or query string, so request input cannot
alias or poison an entry, and the Worker runs on every request to its
hostnames, so the entry is unreachable without passing the D1
verified-version gate first - which also keeps the download counter
accurate: outward responses stay `no-store`, so no edge layer can
re-serve a body without the Worker running). A cache hit costs no R2
operation
and no governor round-trip - the deliberate reason a governor outage
can fail closed without taking popular downloads down. A miss takes an
in-isolate single-flight slot (one uncached checksum must not fan out
into simultaneous R2 reads; waiters poll the cache briefly and only
fall through to their own charged read if the loader vanished), then
charges one `b_ordinary` op - under a per-caller daily
fairness window - before the R2 `get`, and
fills the cache for everyone
else. With public reads the window's principal is the token's user
when one is presented and the edge client IP (`CF-Connecting-IP`,
which Cloudflare overwrites, so it is not caller-controlled)
otherwise, measured against the default class's cap: an anonymous
caller has no account, but it must still be bounded by something,
because the deployed WAF rate limiter deliberately covers only the
website origin's write/auth surface and never the read host
([`runbook.md`](runbook.md), "Zone security and rate limiting").
Without that window one unauthenticated caller could drain the shared
pool and turn everyone else's uncached downloads into `503`s. A
request carrying no client-IP header (local development; the edge
always sets it in production) falls back to one shared anonymous
window rather than none. Fairness refusals are per-caller `429 read_rate_limited`
with a
`Retry-After` reaching the next UTC day; pool refusals and governor
outages are the breaker's `503 registry_over_budget` envelope, which
clients already classify. Fairness caps come from the quota-class
model - global correctness
never depends on them, because the pool check always runs too. The
verifier's pending fetches are never cached (the bytes are not yet
part of the registry) and charge `b_verifier`; the source viewer's
ranged reads are never cached (every request is a distinct slice) and
charge `b_source` under their own per-user cap.

**Reconciliation.** Every breaker cron pass pushes D1's authoritative
live set - one size per distinct non-rejected checksum, the same shape
the storage self-accounting counts - to the governor, which commits
every named object (recording blobs the ledger missed, settling
reservations a lost acknowledgement stranded, growing understated byte
counts) and *reports* everything else: ledger entries D1 does not name
are candidate orphans or leaked reservations, logged for the operator
and never auto-released. Decreases require proof the object is gone,
which is the operator's explicit release
([`runbook.md`](runbook.md), "The cost governor"). The pass also logs
the full usage snapshot, giving `wrangler tail` the ledger next to the
analytics evaluation it audits.

**CI guardrails.** Two lexical guards keep the governed-R2 invariant
reviewable as the code grows: `cargo check-r2` pins the typed
acquisition spellings (`env.bucket` in every direct form) to an
allowlisted, reviewed function with its exact acquisition count, and
bans the generic accessors (`get_binding`, `unchecked_into`) outright
- the same assurance model as the SQL guard, a tripwire that forces
diff review at the seam, not a proof of admission; JS-reflection
acquisition (`Reflect::get` over the raw env plus a checked cast)
remains deliberate-evasion territory that only code review catches -
and `cargo check-deploy`
refuses a `wrangler.jsonc` whose bindings, Durable Object lifecycle,
crons, or `GOVERNOR_*`/`BUDGET_*` vars no longer match what the code
deploys against - including the bundle-export check that would
otherwise fail only at `wrangler deploy` time. Both run in CI
(`registry.yml`) and are themselves regression-tested
(`crates/xtask-registry-guard/tests/`).

**What stays best-effort on purpose.** Workers requests and D1 rows
are revealed by the platform only after the fact, so they keep the
breaker's approximate, cron-driven treatment - presenting them as
hard-enforced would be a lie. Download counters are approximate
telemetry ("Download counts") and never part of the ledger. And the
operator's out-of-band tools (`cargo registry-backup-backfill`, wipes,
restores) run outside the Worker and are deliberately ungoverned:
reconciliation absorbs their effects conservatively.

## Backups

Backups are a data-plane concern and run entirely inside Cloudflare:
R2/D1 bindings need no stored credentials, unlike an external pipeline,
which would spread powerful tokens to a second vendor. The one secret
involved (`D1_EXPORT_API_TOKEN`) is scoped to D1 alone. Three pieces,
all operationally documented in [`runbook.md`](runbook.md) ("Disaster
recovery"):

- **Verified-blob replication (durable queue).** Only versions that
  become **verified** enter the backup set: the verdict batch that
  flips a row to `verified` enqueues its blob into `backup_pending` in
  the same transaction, so the work is recorded exactly when the
  transition applies and a crash can never lose it - replication no
  longer rides the pending publish path at all (a pending upload that
  is later rejected was never worth a copy). The queue drains on the
  verdict's `waitUntil` (fast path) and on every breaker cron pass
  (retry path); each copy is governed (`b_infra`/`a_infra` plus a
  `backup`-pool reservation settled on success), shared checksums
  collapse onto one row, a rejection that lands after the enqueue
  retires the row without copying, and rows older than an hour raise
  the backup-health alert. `cargo registry-backup-backfill` is the
  manual recovery path; it deliberately leaves queue rows for the drain to
  settle. No code path deletes from
  the backup bucket's `blobs/sha256/` namespace, so the replicated
  blobs are append-only: a deletion in the primary - malicious or
  accidental - cannot propagate. (The `d1/` dump namespace is pruned
  by its own job's validation and retention, below.)
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
  (`src/backup.rs`; > 36 h without a successful dump, or any
  `backup_pending` queue row older than an hour) and alerts via log +
  webhook on every pass
  while unhealthy - a backup system that stops must not stop silently.

First-line recovery is D1 Time Travel (always on; 7-day retention on
the free plan), then the exported dumps, then the backup bucket's blobs
as the artifact store of last resort; `cargo registry-restore-drill`
(from the repository root) rehearses the dump-import path against a
scratch database. The backup
bucket doubles stored blob bytes account-wide, which is why the default
storage budget above sits under half the free limit.

## Code layout

Domain logic - token hashing, formatting, and scopes (`src/auth.rs`),
hostname roles, route matching, and path-component validation
(`src/routes.rs`), document composition (`src/documents.rs`), the error
envelope and the challenge header (`src/error.rs`), cookie signing, the
cookie shape, and the CSRF header and fetch-metadata rules
(`src/session.rs`), the session
API's JSON shapes and body validation (`src/user_api.rs`), the source
viewer's ranged-read policy (`src/source.rs`), the public
stats totals' JSON shape (`src/stats.rs`), the
scope-claim grant rules and GitHub-response parsing (`src/claim.rs`),
the sign-in allowlist (`src/allowlist.rs`), the
quota engine (`src/quota.rs`), the budget breaker (`src/breaker.rs`), the
analytics query shapes (`src/analytics.rs`), the governor's accounting
engine, pools, limits, and protocol types (`src/governor.rs`, with its
Durable Object SQL exercised by `rusqlite` in host tests), the
download-telemetry flush policy (`src/telemetry.rs`), the verification
lifecycle's
statuses, verdict rules, and read gate (`src/verify.rs`), the GitHub
Actions OIDC token verification and exchange-config matching for
trusted publishing - manual `RS256` JWT checks, ordered claim
validation, the `JwksProvider` trait, and `select_config`
(`src/trustpub.rs`; its wasm-only Cache-API-backed `GithubJwks`
provider lives in the same module, and the exchange endpoint's D1
plumbing is glue) -
and the backup
logic - retention, dump validation, freshness (`src/backup.rs`) - compiles
and unit-tests on the host target. The Cloudflare glue
(`src/glue.rs` for the role dispatch and the Bearer planes,
`src/web_glue.rs` for the OAuth and session planes,
`src/backup_glue.rs` for the nightly dump job and the backup-queue
drain, `src/governor_do.rs`
for the governor Durable Object and `src/governor_client.rs` for its
Worker-side client, wasm32 only) is thin
binding-and-I/O wiring covered by
`cargo registry-smoke`. Every D1 statement the glue executes is a named
const in `src/sql.rs`, schema-validated at test time and guarded in CI
(see "Why no ORM" below; the guard grants `src/governor.rs` and its
adapter the same consolidated-home treatment for the Durable Object's
SQLite statements). Read-plane path
components are validated before any lookup: scopes and names follow the
grammars in "Scopes", versions must parse as SemVer, and anything else
answers without touching storage - the artifact filename must additionally
repeat the `<scope>-<name>-` prefix its directory segments fix, so a
downloaded archive stays self-identifying and a disagreeing filename
never parses. The API routes only split their segments
(`src/routes.rs` documents why): publish validates them inside its `400`
sequence behind the membership gate, and yank and the admin verdict
answer unknown triples with an authenticated 404 straight from a
parameterized D1 query - no segment ever becomes a path or storage key
by itself.

Every authenticated response carries the debug header
`x-cabin-registry-generation` from `meta.registry_generation`, so a client
talking to a freshly wiped (pre-launch) registry is immediately visible
(see [`runbook.md`](runbook.md)) - except the trusted-publishing
`DELETE`, whose 204 and kind-guard 401 both go unstamped on purpose:
its refusal must stay header-identical to the unauthenticated 401, or
the stamp becomes a token-validity oracle on that route.

## Why 503, not 402

Breaker refusals answered `402 Payment Required` until 2026-07-22. They
answer `503 Service Unavailable` now; the envelope, the
`registry_over_budget` code, both details, and `Retry-After` are
unchanged.

RFC 9110 leaves `402` "reserved for future use", and its de facto
meaning is *the requesting account must pay*. Nothing the caller can pay
clears a tripped breaker - the constraint is entirely operator-side, and
the account that gets blocked is the registry's own. `503` plus
`Retry-After` is the standard "temporarily unavailable, retry later"
signal: `503` has explicit `Retry-After` semantics and `402` has none,
so the one header that makes the refusal actionable was landing on a
status no client can be relied on to read it off of.

The status also has to agree with the schema. The quota vocabulary was
deliberately scrubbed of billing language (`plan` -> `quota_class`,
`'free'` -> `'default'`) because the quota mechanism is an
exception-handling seam, not a commercialization signal; emitting
Payment Required on the wire contradicted that. And the registry is
consumed from CI, where HTTP tooling classifies `503` as transient and
retryable - `curl --retry`, for one, treats `503` as a transient error
worth retrying - which is the correct reading here.
Retrying is safe on these routes: the gates run before any mutation,
publish is byte-idempotent, and yank sets a state rather than toggling
one.

`429` was considered and rejected. It asserts per-client fault, but the
breaker is deliberately aggregate and never keyed on client or token
("Billing model: the governor and the breaker"); WAF rate limiting already
emits `429`, and conflating the two would cost diagnostics.

The cost of the move is that `503`, unlike `402`, is a status the
hosting platform emits on its own - Cloudflare documents `503` for edge
failures and for Workers exceeding CPU or memory. So the clients key on
the `registry_over_budget` code, not the status: an uncoded `503` stays
the generic server error it was before the breaker existed, and a
platform outage is never reported as a budget refusal.

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
  dialect for everything the service uses). `cargo check-sql`,
  run by CI, keeps executed SQL from growing outside that module.
- Dynamic query construction does not exist today; if it ever
  genuinely grows, the designated escape hatch is `sea-query` (a
  wasm-safe query builder, not an ORM).

## Why a strict zip profile

The canonical package archive is a zip container, not a tar.gz, and it is
pinned to a single narrow profile. The full normative spec is
[`archive-format.md`](archive-format.md); the reasoning:

- **Zip over tar.gz.** The source viewer ("Origins and roles") needs random
  access into a stored archive. The Worker keeps archives as opaque,
  content-addressed
  R2 blobs and has no archive dependency, so a tar.gz would force either a
  server-side repack job (Workers CPU) or a second derived zip sidecar (R2
  budget - see "Billing model: the governor and the breaker"); both were rejected.
  Zip is directly seekable, and its fixed-offset EOCD lets the publish path
  reject a non-zip with a few O(1) reads before it hashes anything ("The
  write path").
- **A single strict profile.** One producer (`cabin package`) and one
  consumer (the verifier) means the archive can be byte-reproducible and the
  container vocabulary can be minimal. Everything a hostile archive hides
  behind - data descriptors, extra fields, zip64, comments, local/central
  disagreement - is banned outright, so the format the verifier must reason
  about is small. Idempotent re-publish rides on content-addressing: same
  source bytes, same checksum, `200 no_op`. This must land before launch;
  afterwards a format change would collide with revision immutability and the
  stored checksums.
- **Hand-rolled container parsing.** The verifier parses the container by
  hand rather than through a general-purpose zip library. A library's
  conveniences - last-wins de-duplication of repeated names, transparent
  zip64, silently tolerated local/central mismatches - are exactly the
  hostile ambiguities the profile exists to reject. The strict profile makes
  hand-parsing small: a fixed EOCD offset, contiguous records, and no
  zip64/descriptors/extra fields.

## Why a standalone workspace

`registry/` is its own Cargo workspace, listed in the root workspace's
`exclude`. The root workspace builds host-native binaries with a large,
carefully audited dependency tree and lockfile; this crate targets
`wasm32-unknown-unknown` through `worker-build` and pulls in the `worker`
ecosystem. Excluding it keeps `cargo build`/`cargo test` at the repository
root byte-identical to before the service existed, keeps the two lockfiles
independent, and mirrors how `website/` coexists in the repository with its
own toolchain and workflow (`.github/workflows/registry.yml`).
