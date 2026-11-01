# Threat model

Cabin is a package manager and build system for C/C++: the `cabin` CLI
(`crates/`), a hosted registry (`registry/`, one Cloudflare Worker), and an
archive verifier run from GitHub Actions (the driver
`crates/xtask-registry-admin/src/verify.rs` and the inspector
`crates/cabin-registry-verify`). All three and their interactions are in scope;
`SECURITY.md` is the disclosure policy. The contracts are documented, so this
file points rather than restates: the crate-level `//!` docs and
`crates/AGENTS.md` (crate ownership and boundaries), `docs/package-format.md`
("Extraction safety contract"), `docs/remote-registry.md` (the protocol, its
routes, authentication, and caps; "One role per hostname"; "Publish";
"Verification lifecycle"; "The verifier's checks"; "Server checks versus client
extraction"), `registry/docs/archive-format.md`, `registry/docs/runbook.md`
("Data policy", "The cost governor"), and the module table in
`registry/AGENTS.md` with the module docs it names.

## Trust boundaries and attacker-controlled input

**CLI.** Trusted: the user's own `cabin.toml`, `.cabin/config.toml`,
environment, and toolchain; a dependency manifest driving the compiler is the
same trust as building third-party code. Hostile: every byte from an index or
registry (`crates/cabin-index-http`, `crates/cabin-registry-api`); registry and
upstream archives, with the manifests inside them (`crates/cabin-artifact`;
upstream patch files are applied only by the verifier and the port-publish
preflight, never by the `cabin` binary); GitHub's responses during `cabin login`.
Semi-trusted: lockfiles and local indexes in a cloned project. Secrets: the
keychain or `credentials.toml` (`crates/cabin-credentials`) and
`CABIN_REGISTRY_TOKEN`.

**Registry.** Every request is untrusted: the public read plane; the OAuth and
scope-claim flows; the pre-credential routes (the session-token and
trusted-publishing mints, whose credential is the body, and the verdict route,
whose bearer is an OIDC JWT); the bearer plane (publish, yank, admin); the
session plane (`/api/v1/user`, including the source viewer); and stored rows
read back later. Routes and the bearer plane are specified in
`docs/remote-registry.md`, the session plane in `registry/src/session.rs`; body
caps are constants in `registry/src/`. `wrangler.jsonc` vars and secrets are
operator input, and so is the deploy toolchain: the deploy job installs
`wrangler` through npm at deploy time, locked by `registry/package-lock.json`,
so its dependency closure runs with the deploy token. That is an accepted
boundary, not a hardened one.

**Verifier.** `cargo registry-verify` (the driver, run by
`.github/workflows/registry-verify.yml`) downloads each pending archive with a
`verify`-scoped token and, for provenance-bearing versions, the publisher-pinned
upstream URL, runs the inspector over the files, and PATCHes a verdict with its
own OIDC JWT. Both archives are hostile; the inspector must never extract the
registry archive to disk (the upstream archive goes through the client's
hardened extraction), and the token must never travel to the upstream URL.

**Browser.** `website/` is a static site on the session API's origin.
`website/src/lib/account.ts` and `website/src/scripts/source.ts` are the browser
clients (same-origin credentials, the CSRF header, registry data rendered as
text); `website/public/_headers` carries the CSP. In scope: the Worker's cookie,
CSRF, and redirect posture, and any path by which registry-controlled data
(names, metadata, archive bytes through the source viewer) reaches the DOM as
markup or a navigation target. The site is not built in this image, so review it
statically; the session API itself runs under `wrangler dev` (below).

**Cross-component.** `crates/cabin-package` (used by `crates/cabin-publish` and
the verifier) owns canonical metadata and the archive rules;
`registry/src/publish.rs` re-implements the checks independently, kept in step
only by `cargo gen-fixtures` plus the `publish_validation` conformance test. A
disagreement between client, server, and verifier that changes what gets
verified, served, or extracted is a finding. The documented asymmetries (the
server's cheap container check admits archives the verifier rejects;
verification is stricter than client extraction) are the baseline to measure
against, not an exemption.

## What to look for

Roughly in priority order:

- **Archive handling.** Extraction or patch application writing outside the
  cache target, materializing a symlink or hard link, or inflating past a
  documented cap, on the client or the verifier; the verifier admitting an
  archive the client would refuse.
- **Integrity.** Bytes cached, extracted, or served under a checksum they do not
  hash to; a published revision changing; a verdict landing on a revision it did
  not judge; anything resolvable without a `verified` verdict; the read plane
  serving pending or rejected content to anyone the lifecycle does not allow.
- **Authorization.** Publish, yank, verdict, or membership change without a
  credential, or with one acting beyond its grant (`scope_limit`, another
  scope's membership, one plane's credential accepted by the other, the verdict
  route accepting an unpinned identity, an OIDC `jti` accepted twice); forging a
  token, OAuth state, or cookie; claiming a scope without controlling the GitHub
  account.
- **Credential leakage.** A session token, device-flow grant, or
  `credentials.toml` sent anywhere the protocol does not name (another origin,
  including an `api` origin that a different index's `config.json` declares;
  cleartext beyond loopback; a package archive; logs; `Debug` output); a GitHub
  token persisted by the Worker; the verifier's token reaching an upstream URL.
- **Browser boundary.** CSRF on session mutations, open redirects in the OAuth
  or claim flows, registry data reaching the DOM as markup.
- **Oracles, cost, availability.** Scope-existence or lifecycle oracles the
  documents promise away; governor, quota, or storage-accounting bypasses that
  make the operator pay; a Worker-side fetch of a publisher-controlled URL;
  input that crashes or hangs the verifier or the Worker.

## Severity

Rate by demonstrated exploitability, attacker prerequisites, and impact; the
anchors below yield to both, in either direction (a CSRF that can only log the
victim out is low, not high). An unauthenticated attacker (a hostile registry,
archive, or request) running code or writing outside the cache on a user's
machine, publishing, yanking, delivering a verdict, or forging a credential
without one, serving pending or rejected content to the public, or leaking a
credential with no user action, is critical. A credential acting beyond its
grant, an integrity bypass, a leaked credential, CSRF on a session mutation, a
Worker-side fetch of a publisher-controlled URL, a cost bypass that makes the
operator pay, or registry data reaching the DOM as markup is high. A bomb past a
cap, an oracle, an open redirect, or a crash or hang (both services fail safe)
is medium unless stored state is left inconsistent, which rates by what the
inconsistency allows. Denial of service without amplification and disclosure of
non-secret state are low.

A violated crate-boundary rule is a finding only when you can show
what it lets an attacker do; otherwise report it as a correctness issue. Context
rather than exemptions: a cap at a value you would set differently; an
allowlisted user exhausting their own quota; anything needing the operator's
Cloudflare credentials or an edited `wrangler.jsonc`; the smoke test's
placeholder secrets and `registry/tests/fixtures/`; RUSTSEC-2023-0071 (the
Worker only verifies RS256 with public keys; see `registry/Cargo.toml`); and the
documented residuals (per-colo rate-limit counters; the `Sec-Fetch-Site: none`
arm of claim initiation, through which a mailed link can spend one of a
signed-in user's lifetime claim slots on a name that user owns; unsynced GitHub
org membership; the dashboard-managed WAF rule). Report any of these when you
can show an exploit beyond what is documented.

## Private alpha: current versus future conditions

Sign-in is allowed only for the GitHub ids in `ALLOWED_GITHUB_IDS`
(`registry/wrangler.jsonc`; today one maintainer account), which gates sessions,
session-token minting, scope claims, and so every publish and yank except
trusted publishing, which needs an operator-registered `trustpub_configs` row;
the verdict route accepts only the workflow identity the `VERIFIER_*` vars pin.
`meta.launched` is `'false'`, so stored data may be wiped; launch is the same
service with the write-access policy (a wider allowlist or open sign-up) decided
then. `cabin publish` against an HTTP index and `cabin yank` sit behind `-Z
remote-registry`.

Every anonymous surface above, and the whole CLI against a hostile index or
archive, is reachable now. A finding that needs one valid credential doing what
it should not is current. Findings that need a second account, a cross-scope
relationship, or wider sign-up are **"requires future deployment conditions"**:
report them, but not as currently exploitable.

## Reproduce offline

No network at scan time; `cargo` resolves from its cache and wrangler from
`/src/registry/node_modules`. `/src` is the checkout; put `/src/target/debug`
(the `cabin` binary) on `PATH`.

- `cargo test --workspace --all-targets --all-features --locked --no-fail-fast`:
  unit tests and the CLI integration tests (`crates/cabin/tests/`), which serve
  hostile-registry fixtures from an in-process HTTP server, the pattern for a
  reproducer; `crates/cabin/tests/cli/remote_registry.rs` drives `cabin login`
  and publishing against mocks. One test expecting a permission-denied read
  fails as root; the `#[ignore]`d tests in
  `crates/cabin/tests/cabin_examples.rs` need the network.
- `cabin build` in `examples/hello-c` or `examples/hello-cpp`; `cabin package`
  writes a conforming archive to mutate; `cabin publish --registry-dir <dir>`
  publishes into a local file registry.
- `cd /src/registry && cargo test --locked`; `cargo clippy --target
  wasm32-unknown-unknown --locked -- -D warnings`; `worker-build --release`.
  From `/src`, `cargo check-sql`, `cargo check-r2`, and `cargo check-deploy
  --require-bundle` are the repository's own guards for the SQL and R2 admission
  invariants, and `cargo gen-fixtures /tmp/fixtures && cd registry &&
  CABIN_REGISTRY_FIXTURES=/tmp/fixtures cargo test --locked --test
  publish_validation -- --include-ignored` runs archives the in-tree `cabin`
  built through the server's publish validation.
- `CABIN_REGISTRY_SMOKE_TOKEN=cabin_smoke cargo registry-smoke` from `/src`: the
  whole write path end to end against two `wrangler dev` instances with GitHub
  and Cloudflare mocks. To probe by hand, keep a dev server up the way
  `crates/xtask-registry-smoke/src/servers.rs` does (`render_dev_vars`,
  `seed_sql`, `node_modules/.bin/wrangler dev`); its `.dev.vars` values are
  local fixtures, not secrets.

The best reproducer is a `curl` transcript against `wrangler dev`, a Rust test
in the owning module, or a crafted archive plus the `cabin` command that
consumes it. Patches: minimal, in the owning module, with a focused test.
