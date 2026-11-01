# Cabin Registry Service

The hosted side of Cabin's experimental remote registry: a Cloudflare Worker
(Rust, [`workers-rs`](https://github.com/cloudflare/workers-rs)) serving the
sparse HTTP file-registry contract in
[`../docs/remote-registry.md`](../docs/remote-registry.md) - the authoritative
protocol document - with D1 as the canonical store and R2 for immutable
archive blobs: the read routes (verified
content; during the private alpha, closed to every account
`ALLOWED_GITHUB_IDS` does not admit) plus the authenticated `publish` and `yank`
API routes and the verifier's `verify`-scoped admin routes (publish
validation, the immutability rule, and the verification lifecycle are part
of that protocol document), and the browser
plane - GitHub OAuth sign-in plus a session-cookie JSON user API backing
the account dashboard - served on the **website origin**
(`cabinpkg.com`) while the registry domain serves only the machine read
plane: one role per hostname, dispatched on the Host header (see
[`../docs/remote-registry.md`](../docs/remote-registry.md#one-role-per-hostname)).
See [`docs/runbook.md`](docs/runbook.md) for operations.

The service is in private alpha: the client's read path is stable, but the
mutation surfaces stay behind the client's `-Z remote-registry` gate, and
routes and storage formats may change without migration paths. Use of the
hosted service is governed by the
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
launch (`meta.launched` = `'false'`), the database, the primary blob
data, and the whole backup bucket are wiped and recreated instead of
migrated whenever the storage format changes; from launch onward the
data is permanent and never wiped, and the backup bucket keeps its
append-only retention. The launch guard in
`cargo registry-wipe` enforces the boundary, and flipping the flag is a
launch-checklist item ([`docs/runbook.md`](docs/runbook.md), "Launch
checklist").

## Getting a token

The registry's authentication mechanisms are specified in
[`../docs/remote-registry.md`](../docs/remote-registry.md), the
authoritative protocol document: trusted publishing for CI, and
login-session tokens for interactive use. The mutation surface's uniform
`401` carries a `WWW-Authenticate` challenge naming that document's login
URL - the same URL `cabin login` prints. The registry stores only a
token's SHA-256 hash; a minted plaintext is shown exactly once.

Sign-in is restricted to the numeric GitHub user ids listed in
`ALLOWED_GITHUB_IDS` (a plain var in `wrangler.jsonc`); adding a user later
means adding their numeric id there and redeploying. The first sign-in
creates a registry-native user - it additionally requires the GitHub
account to be at least 30 days old (`src/signup.rs`; a younger account
is told the date it becomes eligible) - bound to the GitHub account
through the `identities` table - the numeric id (never the login, which can be renamed
and reassigned) is the external identity, and everything package- or
token-related keys on the registry's own user id.
Package names are scoped (`<scope>/<name>`), and publish/yank
authorization is per scope: the token's user must be a member of the
target scope. A scope is claimed from the website
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
cargo kani                                # proof harnesses (`cargo install kani-verifier@=0.68.0 --locked && cargo kani setup`)
(cd proofs && java -XX:+UseParallelGC -jar tla2tools.jar -workers auto -cleanup -config RegistryLifecycle.cfg RegistryLifecycle.tla)  # TLA+ lifecycle model (proofs/README.md: the pinned jar)
(cd .. && CABIN_REGISTRY_SMOKE_TOKEN=cabin_smoke cargo registry-smoke)  # end-to-end, local
```

`cargo registry-smoke` runs two `wrangler dev` instances - one per hostname
role, the website role's only forwarding to the registry role's worker,
which alone opens the local D1, R2 and governor state under `.wrangler/` -
and checks
`/healthz`, the mutation surface's uniform `401` with its byte-identical
`WWW-Authenticate` challenge, the hostname dispatch (no read plane on the
website origin, no API surface on the registry domain), the OAuth and
session planes' refusals and cookie attributes, the read plane's
private-alpha refusals (anonymous and unadmitted callers alike),
claim -> publish -> fetch end to end - the scope-claim flow against a
local GitHub mock (self-claim, org claim, refusals) and membership
management through the session API, then the full publish / yank flow
on the website origin under the just-claimed scope (first publish,
idempotent re-publish, immutability conflict, yank transitions,
artifact checksum) -
and the verification lifecycle (pending -> verify -> resolvable with a
verify-scoped token, plus the reject -> reclaim -> refund -> republish
flow). Prerequisites: `rustup target add wasm32-unknown-unknown`
and Node, then `npm ci --ignore-scripts` in this directory (again after
`package-lock.json` changes) to install the wrangler `package.json` pins,
and `cargo install worker-build@=0.8.5 --locked`, the version CI
installs: every `wrangler dev` and `wrangler deploy` runs
`worker-build --release` from `PATH`.

Before its servers start, the smoke also restores its local D1 export into a
scratch database and requires the same schema and rows: the backup cron leg
proves the stored backup is that export, and this proves it restores.

`cargo gen-fixtures <dir>`, run from the repository root (the alias names a
root-workspace package), builds the in-tree `cabin` binary and
packages real archive + canonical-metadata pairs; the `#[ignore]`d
conformance test in `tests/publish_validation.rs` feeds them through the
server's publish validation (`CABIN_REGISTRY_FIXTURES=<dir> cargo test --
--include-ignored`). CI runs it whenever the registry or the client's
publish-pipeline crates change. The frozen pair under `tests/fixtures/` is a
checked-in copy of its `withdep` output for offline unit tests; regenerate it
with the alias when the canonical metadata format changes intentionally.

This directory is a standalone Cargo workspace, excluded from the root
workspace: `cargo build`/`cargo test` at the repository root never touch it.
CI gates `.github/workflows/registry.yml`'s jobs on registry-relevant
changes (the `registry` filter in `.github/path-filters.yml`; the build
and Kani jobs on the narrower `registry-build` filter, the TLC jobs on
the narrower `tlc` filter), except `merge-readiness`, which runs the
deploy's migrations gate on every pull request and merge group, so a
stale `migrations-applied` stamp blocks unrelated merges too; a run on
`main` also deploys the Worker (its `deploy-registry` job), unless a
newer registry-relevant commit superseded it (green, Deploy step
skipped, the newer run deploys) or unapplied D1 migrations block it
(red). A deploy that exits 0 still fails the job unless Cloudflare then
shows the run's tagged version serving all traffic. That deploy never
applies D1 migrations; those stay manual (docs/runbook.md, "Deploy
skew").

## First-time provisioning

The resources, the GitHub OAuth app, and the secrets are set up by
[`docs/runbook.md`](docs/runbook.md), "First-time provisioning".
`wrangler.jsonc` carries the account id and the database id; both are
public identifiers, not secrets.
