# Cabin Registry Service

The hosted side of Cabin's experimental remote registry: a Cloudflare Worker
(Rust, [`workers-rs`](https://github.com/cloudflare/workers-rs)) serving the
sparse HTTP file-registry contract in
[`../docs/remote-registry.md`](../docs/remote-registry.md) - the authoritative
protocol document - with D1 as the canonical store and R2 for immutable
archive blobs. See [`docs/architecture.md`](docs/architecture.md) for the
design and [`docs/runbook.md`](docs/runbook.md) for operations.

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

## Development

```sh
cargo test                                # host-target unit tests
cargo clippy --all-targets -- -D warnings
cargo clippy --target wasm32-unknown-unknown -- -D warnings
CABIN_REGISTRY_SMOKE_TOKEN=cabin_smoke scripts/smoke.sh   # end-to-end, local
```

`scripts/smoke.sh` runs `wrangler dev` with local D1/R2 state under
`.wrangler/` and checks `/healthz`, the uniform unauthenticated `401`, and the
three authenticated read routes. Prerequisites: `rustup target add
wasm32-unknown-unknown` and Node (for `npx wrangler`); `worker-build` installs
itself on first build.

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
npx wrangler secret put GITHUB_CLIENT_ID --env dev
npx wrangler secret put GITHUB_CLIENT_SECRET --env dev
npx wrangler secret put SESSION_SECRET --env dev
npx wrangler deploy --env dev
```

Secrets back the future GitHub sign-in flow; `ALLOWED_GITHUB_IDS` (plain var
in `wrangler.jsonc`) limits who may sign in. None of them are read by the
read routes shipped so far.
