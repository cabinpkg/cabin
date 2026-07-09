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
- **No KV.** The data is relational and small; a second store would only add
  consistency questions. Response caching can come later at the edge if read
  volume ever warrants it.

## Deny-by-default auth

Every data route - including `/config.json` - requires
`Authorization: Bearer cabin_<base62>`. The uniform
`401 {"errors":[{"detail":"authentication required"}]}` is emitted before any
route matching or D1/R2 data lookup, so unauthenticated callers cannot
distinguish existing from non-existing packages. `/healthz` is the only
unauthenticated route.

Tokens are stored as the SHA-256 hex of the full token string; the plaintext
exists only in the client's hands. Any valid, unrevoked token grants read
access; `scopes` (a subset of `publish,yank`) only gates future mutation
routes. `last_used_at` is updated best-effort off the response path, and log
lines carry the token row id - never the token or its hash.

## Code layout

Domain logic - token hashing and scopes (`src/auth.rs`), route matching and
path-component validation (`src/routes.rs`), document composition
(`src/documents.rs`), the error envelope (`src/error.rs`) - compiles and
unit-tests on the host target. The Cloudflare glue (`src/glue.rs`, wasm32
only) is thin binding-and-I/O wiring covered by `scripts/smoke.sh`. Path
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
