# Cabin Registry Service

The hosted side of Cabin's experimental remote registry: a Cloudflare Worker
(Rust, [`workers-rs`](https://github.com/cloudflare/workers-rs)) serving the
sparse HTTP file-registry contract in
[`../docs/remote-registry.md`](../docs/remote-registry.md) - the authoritative
protocol document - with D1 as the canonical store and R2 for immutable
archive blobs: the authenticated read routes plus the `publish` and `yank`
API routes (validation order and the immutability rule are described in
[`docs/architecture.md`](docs/architecture.md), "The write path"), and a
GitHub-sign-in web page at `/me` for issuing and revoking tokens. See
[`docs/architecture.md`](docs/architecture.md) for the design and
[`docs/runbook.md`](docs/runbook.md) for operations.

Everything here is experimental, matching the client's `-Z remote-registry`
gate: routes and storage formats may change without migration paths.

## Environments

Both environments share this Worker code (`wrangler.jsonc`); always pass an
explicit `--env`:

| | dev | production |
| --- | --- | --- |
| Domain | `dev-registry.cabinpkg.com` | `registry.cabinpkg.com` |
| D1 database | `cabin-registry-dev` | `cabin-registry-prod` |
| R2 bucket | `cabin-registry-dev-blobs` | `cabin-registry-prod-blobs` |

**Dev data is disposable.** The dev environment exists to exercise the
experimental protocol; its database and bucket are wiped and recreated instead
of migrated whenever the storage format changes
([`docs/runbook.md`](docs/runbook.md)). Production data is permanent and never
wiped.

## Getting a token

Sign in with GitHub at `<origin>/me` (for example
`https://dev-registry.cabinpkg.com/me`), create a token there with the scopes
you need, and copy it - the plaintext is shown exactly once; the registry
stores only its hash. Then hand it to the client with `cabin login`
(`-Z remote-registry`).

Sign-in is restricted to the numeric GitHub user ids listed in
`ALLOWED_GITHUB_IDS` (a plain var in `wrangler.jsonc`); adding a user later
means adding their numeric id there and redeploying. Per-package ownership is
intentionally out of scope for now: every allowlisted user can publish and
yank any package.

## Development

```sh
cargo test                                # host-target unit tests
cargo clippy --all-targets -- -D warnings
cargo clippy --target wasm32-unknown-unknown -- -D warnings
CABIN_REGISTRY_SMOKE_TOKEN=cabin_smoke scripts/smoke.sh   # end-to-end, local
```

`scripts/smoke.sh` runs `wrangler dev` with local D1/R2 state under
`.wrangler/` and checks `/healthz`, the uniform unauthenticated `401`, the
unauthenticated `/me` redirect to `/login`, the three authenticated read
routes, and the full publish / yank flow (first publish, idempotent
re-publish, immutability conflict, yank transitions, artifact checksum). Prerequisites: `rustup target add wasm32-unknown-unknown`
and Node (for `npx wrangler`); `worker-build` installs itself on first build.

`scripts/gen-fixtures.sh <dir>` builds the in-tree `cabin` binary and
packages real archive + canonical-metadata pairs; the `#[ignore]`d
conformance test in `tests/publish_validation.rs` feeds them through the
server's publish validation (`CABIN_REGISTRY_FIXTURES=<dir> cargo test --
--include-ignored`). CI runs it whenever the registry or the client's
publish-pipeline crates change. The frozen pair under `tests/fixtures/` is a
checked-in copy of its `withdep` output for offline unit tests; regenerate it
with the script when the canonical metadata format changes intentionally.

This directory is a standalone Cargo workspace, excluded from the root
workspace: `cargo build`/`cargo test` at the repository root never touch it.
CI runs `.github/workflows/registry.yml` on `registry/**` changes; deploys are
manual for now.

## First-time provisioning

Resources are created per environment with wrangler (no account ids or
resource ids are hardcoded; fill `database_id` in `wrangler.jsonc` after
creating each database):

```sh
npx wrangler d1 create cabin-registry-dev
npx wrangler r2 bucket create cabin-registry-dev-blobs
npx wrangler d1 migrations apply DB --env dev --remote
npx wrangler deploy --env dev
npx wrangler secret put GITHUB_CLIENT_SECRET --env dev
npx wrangler secret put SESSION_SECRET --env dev
```

The secrets back the GitHub sign-in flow ("Getting a token" above):
`GITHUB_CLIENT_ID` (plain var in `wrangler.jsonc`; client ids are public) and
`GITHUB_CLIENT_SECRET` identify the GitHub OAuth app (its authorization
callback URL is `<origin>/callback`), and `SESSION_SECRET` keys the HMAC
behind the state, session, and CSRF values; `ALLOWED_GITHUB_IDS` (plain var
in `wrangler.jsonc`) limits who may sign in. See
[`docs/runbook.md`](docs/runbook.md) for the exact procedure.
