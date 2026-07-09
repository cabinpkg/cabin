# Registry Service Architecture

The service implements the server side of
[`../../docs/remote-registry.md`](../../docs/remote-registry.md) (the
authoritative protocol contract). This page covers only the decisions local to
the service.

## Storage

- **D1 is canonical.** Users, tokens, packages, versions, and the `meta`
  key-value table all live in one D1 database (`migrations/0001_init.sql`).
  Everything the read routes serve is composed from D1 rows; in particular
  each version's canonical index entry is stored verbatim at publish time in
  `versions.metadata_json`, and only its `yanked` field is overwritten from
  the row on the way out, so yank state has exactly one home.
- **R2 holds immutable, content-addressed blobs.** Archive bytes live at
  `blobs/sha256/<checksum-hex>` (the lowercase hex in `versions.checksum`).
  Nothing in R2 is ever mutated or deleted in production; yanking is a D1 row
  update, and the artifact route deliberately keeps serving yanked versions
  so locked-in consumers keep building.
- **Published versions are immutable.** A `(name, version)` row is written
  once. Re-publishing byte-identical metadata (which embeds the archive
  checksum, so identical metadata means an identical archive) is an
  idempotent `200 {"ok":true,"no_op":true}` that touches neither store;
  anything else is `409 published versions are immutable`. There is no
  unpublish or delete.
- **No KV.** The data is relational and small; a second store would only add
  consistency questions. Response caching can come later at the edge if read
  volume ever warrants it.

## Two credential planes

Authentication is split into two planes that never accept each other's
credential.

**The data plane is Bearer-only and deny-by-default.** Every data route -
including `/config.json` - requires `Authorization: Bearer cabin_<base62>`.
The uniform `401 {"errors":[{"detail":"authentication required"}]}` is
emitted before any route matching or D1/R2 data lookup, so unauthenticated
callers cannot distinguish existing from non-existing packages. `/healthz`
is the only route outside both planes. Cookies are never read here.

Tokens are stored as the SHA-256 hex of the full token string; the plaintext
exists only in the client's hands (it is rendered exactly once, on the `/me`
response that creates it). Any valid, unrevoked token grants read access;
`scopes` (a subset of `publish,yank`) gates the mutation routes.
`last_used_at` is updated best-effort off the response path, and log lines
carry the token row id - never the token or its hash.

**The web plane is session-cookie-only.** `/login`, `/callback`, `/me`, and
the `/me/tokens...` POSTs are the human-facing side: GitHub OAuth sign-in
(web application flow, no extra scopes) and a server-rendered token page.
The `Authorization` header is never read here, and the web dispatch serves
no package data, so the data plane's uniform 401 stays intact.

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
  separation (`src/session.rs`); `HttpOnly; Secure; SameSite=Lax`.
- Both POSTs require a CSRF token - an HMAC derived from the session,
  embedded as a hidden form field and recomputed server-side - on top of
  the `SameSite=Lax` cookie semantics.
- HTML responses carry `Content-Security-Policy: default-src 'none';
  style-src 'unsafe-inline'`, `X-Content-Type-Options: nosniff`,
  `Referrer-Policy: no-referrer`, and `Cache-Control: no-store`.
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

Only then storage is consulted: an existing row answers with the
idempotent `200` no-op or the `409` immutability conflict, and a new version
writes the R2 blob first (skipped when the content-addressed key already
exists), then one atomic D1 batch for the `packages` and `versions` rows.
A crash between the two writes can only leave an unreferenced blob - see
[`runbook.md`](runbook.md).

Yank is a single-column `UPDATE` on the `versions` row (`404` when the pair
is unknown), idempotent, reporting the resulting state and whether the
request changed it. The read path overrides the stored entry's `yanked`
field from the column, so the verbatim `metadata_json` never goes stale on
the one field that mutates.

Conformance is enforced from the monorepo: `scripts/gen-fixtures.sh` builds
the in-tree `cabin` binary and packages real fixture pairs, which the
`conformance` CI job (and a frozen pair under `tests/fixtures/`) feeds
through the full server-side validation path, so the client's canonical
output and the server's schema cannot silently drift.

## Code layout

Domain logic - token hashing, formatting, and scopes (`src/auth.rs`), route
matching and path-component validation (`src/routes.rs`), document
composition (`src/documents.rs`), the error envelope (`src/error.rs`),
cookie/CSRF signing (`src/session.rs`), the sign-in allowlist
(`src/allowlist.rs`), HTML rendering and escaping (`src/pages.rs`) -
compiles and unit-tests on the host target. The Cloudflare glue
(`src/glue.rs` for the data plane, `src/web_glue.rs` for the web plane,
wasm32 only) is thin binding-and-I/O wiring covered by
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
