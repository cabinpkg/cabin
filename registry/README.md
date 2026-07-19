# Cabin Registry Service

The hosted side of Cabin's experimental remote registry: a Cloudflare Worker
(Rust, [`workers-rs`](https://github.com/cloudflare/workers-rs)) serving the
sparse HTTP file-registry contract in
[`../docs/remote-registry.md`](../docs/remote-registry.md) - the authoritative
protocol document - with D1 as the canonical store and R2 for immutable
archive blobs: the authenticated read routes plus the `publish` and `yank`
API routes and the verifier's `verify`-scoped admin routes (validation
order, the immutability rule, and the verification lifecycle are described
in [`docs/architecture.md`](docs/architecture.md)), and the browser
plane - GitHub OAuth sign-in plus a session-cookie JSON user API for
issuing and revoking tokens - served on the **website origin**
(`cabinpkg.com`) while the registry domain serves only the machine read
plane: one role per hostname, dispatched on the Host header. See
[`docs/architecture.md`](docs/architecture.md) ("Origins and roles") for
the design and [`docs/runbook.md`](docs/runbook.md) for operations.

Everything here is experimental, matching the client's `-Z remote-registry`
gate: routes and storage formats may change without migration paths. Use of
the hosted service is governed by the
[Usage Policy](https://cabinpkg.com/policies).

## Deployment

One deployment, one top-level `wrangler.jsonc` configuration (no wrangler
environments, no `--env`), running under its final production names while
the registry remains closed to allowlisted maintainers:

| | |
| --- | --- |
| Index domain | `registry.cabinpkg.com` |
| Browser/API origin | `https://cabinpkg.com` (zone routes `/api/*`, `/login`, `/callback*`, `/claim/*`) |
| D1 database | `cabin-registry` |
| R2 buckets | `cabin-registry-blobs`, `cabin-registry-backup` |

**The disposable/permanent data boundary is temporal, not spatial.** Until
launch (`meta.launched` = `'false'`), the database and the primary blob
data are wiped and recreated instead of migrated whenever the storage
format changes (the backup bucket is append-only and never wiped); from
launch onward the data is permanent and never wiped. The launch guard in
`scripts/wipe.sh` enforces the boundary, and flipping the flag is a
launch-checklist item ([`docs/runbook.md`](docs/runbook.md), "Launch
checklist").

## Getting a token

Sign in with GitHub on the website origin (`https://cabinpkg.com/login`)
and create a token with the scopes you need through the token page (its
URL is what the `WWW-Authenticate` challenge on every unauthenticated
response names, and what `cabin login` prints) - the plaintext is shown
exactly once; the registry stores only its hash. Then hand it to the
client with `cabin login` (`-Z remote-registry`).

Sign-in is restricted to the numeric GitHub user ids listed in
`ALLOWED_GITHUB_IDS` (a plain var in `wrangler.jsonc`); adding a user later
means adding their numeric id there and redeploying. The first sign-in
creates a registry-native user, bound to the GitHub account through the
`identities` table - the numeric id (never the login, which can be renamed
and reassigned) is the external identity, and everything package- or
token-related keys on the registry's own user id
([`docs/architecture.md`](docs/architecture.md), "Two credential planes").
Package names are scoped (`<scope>/<name>`), and publish/yank
authorization is per scope: the token's user must be a member of the
target scope ([`docs/architecture.md`](docs/architecture.md),
"Scopes" and "The write path"). A scope is claimed from the website
(`https://cabinpkg.com/claim/<scope>`) by proving control of the
same-named GitHub account through a dedicated OAuth roundtrip: the
scope must be your own login (lowercased) or an organization you are an
active admin of. The claim freezes the scope string to that account's
**numeric** GitHub id forever - a later GitHub account reusing the
login can never re-claim it, and disputes are handled manually by the
operator. Scope owners manage members through the session API
(members are added by GitHub numeric id and must have signed in once);
authorization consults only registry-side membership, never a live
GitHub call, and the proof automation is GitHub-only by policy even
though the schema is provider-neutral. Per-package ownership within a
scope is intentionally out of scope: every member can act on every
package under the scope.

## Development

```sh
cargo test                                # host-target unit tests
cargo clippy --all-targets -- -D warnings
cargo clippy --target wasm32-unknown-unknown -- -D warnings
CABIN_REGISTRY_SMOKE_TOKEN=cabin_smoke scripts/smoke.sh   # end-to-end, local
```

`scripts/smoke.sh` runs two `wrangler dev` instances over shared local
D1/R2 state under `.wrangler/` - one per hostname role - and checks
`/healthz`, the uniform `401` with its byte-identical `WWW-Authenticate`
challenge, the hostname dispatch (no read plane on the website origin, no
API surface on the registry domain), the OAuth and session planes'
refusals and cookie attributes, the three authenticated read routes,
claim -> publish -> fetch end to end - the scope-claim flow against a
local GitHub mock (self-claim, org claim, refusals) and membership
management through the session API, then the full publish / yank flow
on the website origin under the just-claimed scope (first publish,
idempotent re-publish, immutability conflict, yank transitions,
artifact checksum) -
and the verification lifecycle (pending -> verify -> resolvable with a
verify-scoped token, plus the reject -> reclaim -> refund -> republish
flow). Prerequisites: `rustup target add wasm32-unknown-unknown`
and Node (for the pinned `npx --yes wrangler@4.112.0`); `worker-build`
installs itself on first build.

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
CI runs `.github/workflows/registry.yml` on `registry/**` changes; a green
run on `main` also deploys the Worker (its `deploy-registry` job). That
deploy never applies D1 migrations; those stay manual (docs/runbook.md).

## First-time provisioning

Resources are created with wrangler (`wrangler.jsonc` carries the account
id and the database id - both public identifiers, not secrets; fill
`database_id` after creating the database):

```sh
npx --yes wrangler@4.112.0 d1 create cabin-registry
npx --yes wrangler@4.112.0 r2 bucket create cabin-registry-blobs
npx --yes wrangler@4.112.0 r2 bucket create cabin-registry-backup
npx --yes wrangler@4.112.0 d1 migrations apply DB --remote
npx --yes wrangler@4.112.0 deploy
npx --yes wrangler@4.112.0 secret put GITHUB_CLIENT_SECRET
npx --yes wrangler@4.112.0 secret put SESSION_SECRET
```

The secrets back the GitHub sign-in flow ("Getting a token" above):
`GITHUB_CLIENT_ID` (plain var in `wrangler.jsonc`; client ids are public) and
`GITHUB_CLIENT_SECRET` identify the GitHub OAuth app (its authorization
callback URL is `https://cabinpkg.com/callback`), and `SESSION_SECRET`
keys the HMAC behind the state and session cookies; `ALLOWED_GITHUB_IDS`
(plain var in `wrangler.jsonc`) limits who may sign in. See
[`docs/runbook.md`](docs/runbook.md) for the exact procedure.
