# Foundation ports

Foundation ports are **curated packages** that adapt important existing C/C++ libraries - libraries
that do not yet ship a native `cabin.toml` - to Cabin's build model.  Their sources live in the
repository-root
[`ports/`](https://github.com/cabinpkg/cabin/tree/main/ports/)
directory and are explicitly **not** a public registry; this directory is closed to arbitrary
submissions and is intended to be retired incrementally as upstreams adopt native `cabin.toml`.

You can browse the foundation-port catalog at https://cabinpkg.com.

## Consuming a foundation port

Foundation ports are consumed as ordinary registry packages under the `cabin-ports` scope, which is
the form the package pages on [cabinpkg.com](https://cabinpkg.com) show:

```toml
[dependencies]
"cabin-ports/zlib" = "=1.3.1"
```

The dependency key needs quotes (scoped names contain `/`), and the requirement names the plain
upstream version: [packaging revisions](package-index.md#packaging-revisions) are not part of the
version string, and consumers pin them through the lockfile's checksum instead.  Registry
dependencies resolve through the hosted registry by default (no flag; see
[remote-registry.md](remote-registry.md#the-default-registry)).  The hosted registry is in private
alpha: it is restricted to its maintainer, so hosted foundation ports are not available to the
public yet, and resolving one fails with an explanation of that status.  The recipes in `ports/`
stay public, and local builds and local file registries (`--index-path`) are unaffected.

There is no port-specific dependency form.  A port is an ordinary registry package, and a manifest
that tries to name one specially (`port = true`, `port-path = "..."`) is rejected as an unknown
dependency field.

## Anatomy of a foundation port

A port is an ordinary Cabin package directory: one `cabin.toml`, plus a `patches/` subdirectory
when it declares any and a `tests/` consumer test when it has one.

```
ports/<name>/<version>/
  cabin.toml - the published manifest: scoped identity,
                   [package.upstream] provenance, and the
                   upstream sources described as Cabin targets
  patches/    - optional unified-diff files declared by
                   [package.upstream].patches
  tests/      - optional consumer test the preflight probe
                   compiles and runs; never published
```

The manifest publishes **verbatim**, so its committed bytes are the published bytes - a comment
edit changes the published archive.  Its identity must agree with the directory layout: `<name>/`
fixes `[package].name` at exactly `cabin-ports/<name>`, so the directory is itself a valid package
name, and `<version>/` fixes `[package].version`.  A port may declare only unconditional registry
dependencies: no `system = true`, no path or workspace sources, and no `cfg(...)` dependency
table, because publication ordering reads the declared edges without evaluating conditions.  The
publisher refuses each of these with a message naming it.

### Provenance

Every port declares [`[package.upstream]`](manifest.md#packageupstream): the pinned archive URL,
its algorithm-prefixed checksum, the container format, an optional `strip-prefix`, declarative
`[[package.upstream.copy]]` placements, and declared `patches`.  There is no port-specific
provenance vocabulary - that published table is the whole of it, and
[`manifest.md`](manifest.md#packageupstream) is its normative reference.

Zip support exists for upstreams whose only official release artifact is a zip (miniz's
amalgamation, for example); prefer `.tar.gz` when the upstream publishes one.

### Manifest

`zlib`'s manifest declares a single `library` target with the 15 canonical zlib C sources and the
archive root on the include path.  The target key is the artifact stem, so it is `z` (producing
`libz.a` / `z.lib`), not `zlib`:

```toml
[package.upstream]
url = "https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz"
checksum = "sha256:9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"
format = "tar.gz"
strip-prefix = "zlib-1.3.1"

[package]
name = "cabin-ports/zlib"
version = "1.3.1"

[target.z]
type = "library"
sources = [
    "adler32.c", "compress.c", "crc32.c", "deflate.c",
    "gzclose.c", "gzlib.c", "gzread.c", "gzwrite.c",
    "infback.c", "inffast.c", "inflate.c", "inftrees.c",
    "trees.c", "uncompr.c", "zutil.c",
]
include-dirs = ["."]
c-standard = "c11"
links = "z"
```

Library ports claim the native identity they embody via [`links`](manifest.md#links) (`zlib`
claims `"z"`, `libpng` claims `"png"`), judged against upstream conventions - the identity, not the
target key, and only where a real native library exists.  Header-only ports claim nothing, and an
alternative implementation with its own symbol namespace claims its own identity (`miniz` claims
`"miniz"`, not `"z"`: its objects export only `mz_*` symbols).

## Materialization pipeline

A port materializes through
`cabin_artifact::materialize_upstream` - the one upstream-provenance
materialization pipeline, and the same implementation the hosted registry's external verifier
replays against a published package's `[package.upstream]` declaration.  What publishes is
therefore assembled by exactly what verifies:

1. Fetch the pinned archive.  Published provenance pins a credential-free `https://` URL,
   downloaded via the same HTTP client `cabin-index-http` uses, with a five-hop redirect budget.
   Following redirects is safe because the declared SHA-256 is verified against the final response
   bytes, and upstream release archives commonly hop from a forge (e.g. GitHub) to a CDN.
   Compressed archives larger than 64 MiB are refused by the HTTP client; no foundation port
   currently approaches that limit.  A cached archive already hashing to the pin short-circuits
   the download, so repeat invocations stay network-free.
2. Verify the archive's SHA-256 against the declaration.
3. Safely extract the archive with the declared `strip-prefix`, under the full
   [extraction safety contract](package-format.md#extraction-safety-contract) - decompression-bomb
   caps, path-traversal protection, duplicate and conflicting entries - applied identically to
   both formats.
4. Apply the declared `[[package.upstream.copy]]` steps, placing prebuilt files under their
   build-time names inside the extracted tree.
5. Apply the declared `patches` as byte-exact unified diffs, then place each patch file itself
   into the tree at its declared path, so the published archive carries the patches its
   declaration names.
6. Write the committed `cabin.toml` last, so an upstream archive shipping its own root manifest
   can never displace the published one.

Every step is declarative: no commands run, and the only inputs are the pinned archive and the
port's own committed files - its `cabin.toml` and its declared patches.  The result is an ordinary
Cabin package directory, which is what the publisher packages and uploads.  A port whose upstream
archive carries documentation or test trees consumers never build lists the paths they need in
[`[package] include`](manifest.md#package) (for example `include = ["include", "src"]`, plus
`"patches"` when it declares patches), which keeps the rest out of the published archive; the
verifier expects only those paths.

## Cache layout

Pinned upstream archives live under the same root the rest of Cabin's artifact cache uses:

```
<cache>/ports/
  archives/sha256/<hex>.tar.gz   (or <hex>.zip for zip sources)
```

The cache root resolution follows the documented chain: `--cache-dir` - > `CABIN_CACHE_DIR` - >
`CABIN_CACHE_HOME` - > `$XDG_CACHE_HOME/cabin` - > `$HOME/.cache/cabin`.  The default on a Unix-like
system with no overrides is `$HOME/.cache/cabin/`, so the example layout above lives at
`~/.cache/cabin/ports/archives/sha256/<hex>.tar.gz`.  The cache is shared across projects on the
same machine - content is checksum-addressed, and every entry is re-hashed against the declared
pin before use, so a stale or corrupt entry is a miss, never a substitution.

## What foundation ports are **not**

- Not Cabin's public registry.  Cabin's registry layer is documented in
  [`registry-design.md`](registry-design.md) and evolves independently.
- Not a private metadata dialect.  A port's provenance is the ordinary `[package.upstream]` table
  every package may declare ([`manifest.md`](manifest.md#packageupstream)), which the hosted
  registry's external verifier checks against the pinned upstream archive.
- Not a submission queue.  New foundation ports require a curated review; this directory is
  intentionally small.
- Not a vehicle for binary distribution.  Only source archives are supported.
- Not a workaround for missing build-script support.  Ports describe libraries whose source layout
  already fits Cabin's target model (a fixed list of sources plus include directories), optionally
  placing a prebuilt file with a static `[[package.upstream.copy]]` step or correcting one with a
  declared byte-exact patch.  Libraries that need configure-time generation, CMake / Meson / Autotools
  driving, or custom build commands cannot be ported until the build graph runs generated actions.
- Not a variant-selection mechanism.  A published port carries whatever `[features]` its manifest
  declares (`cabin-ports/sqlite3` ships a `single-threaded` feature), but nothing in the port
  vocabulary selects a shared or static build.

## Error catalog

Every diagnostic below is refused by the publisher before anything is uploaded.  The first group
is refused during planning, before any port is staged; the symlinked-patch and cache-miss
diagnostics come from materialization, which may already have extracted a scratch tree (the
publisher owns and clears that whole scratch root).

| Diagnostic | Trigger |
| --- | --- |
| ``must publish under the `cabin-ports` scope`` | `[package].name` carries a different scope, or none. |
| ``disagrees with its `<name>/` directory`` | `[package].name` is not `cabin-ports/<name>` for the port directory's name. |
| ``disagrees with its `<version>/` directory`` | `[package].version` does not equal the version directory's name. |
| ``package directory `<name>/` `` | The port directory's name is not a valid package name (`[a-z0-9][a-z0-9_-]*`); the message suggests the lowercase spelling when that alone fixes it. |
| `declares no [package.upstream] provenance` | The manifest has no provenance table; every port is materialized from a pinned upstream archive. |
| `declares the system dependency` | A dependency sets `system = true`; a port resolving through host pkg-config would make its published build host-dependent. |
| `depends on ... through a non-registry source` | A dependency uses a path or workspace source. |
| ``may only depend on other `cabin-ports/*` packages`` | A registry dependency names a package outside the scope. |
| `under a cfg-conditional table` | A dependency sits in a `[target.'cfg(...)'.dependencies]` table; ordering does not evaluate conditions. |
| `which no committed port version satisfies` | A declared requirement matches no version published in the same run. |
| `inter-port dependency cycle among` | The declared edges do not form a DAG. |
| `is not a regular file` / `is a symlinked directory` | A manifest or port directory is a symlink; published bytes must be the committed files. |
| `is (or crosses) a symlink` | A declared patch path resolves through a symlink. |
| `no cached archive for ... and downloads are disabled` | A hermetic (cache-only) staging run found no archive matching the declared SHA-256. |

Provenance-shape diagnostics - an unparsable or non-HTTPS URL, a malformed SHA-256, an unsafe
`strip-prefix`, copy, or patch path, an inapplicable patch - come from the manifest parser and the
shared materializer, and are catalogued with
[`[package.upstream]`](manifest.md#packageupstream) itself.

## Publishing ports as registry packages

The repository tool `xtask-port-publish` (`crates/xtask-port-publish`, a `publish = false`
workspace crate that is not part of the shipped `cabin` binary) publishes every committed port as
an ordinary registry package under the **`cabin-ports`** scope, verbatim from the manifest
committed in it.  The rules that manifest must meet:

- **Identity.**  `cabin-ports/<name>`, declared by the manifest itself, with `<name>` spelled
  exactly as the port directory (`cjson/` declares `cabin-ports/cjson`; the upstream `cJSON`
  spelling stays in the upstream URL and sources).  The publisher refuses a directory that is not
  a valid package name and a manifest that declares any other name.  The published version is the
  upstream version, verbatim.
- **Target keys.**  A target key directly determines its artifact stem, so a port states the
  intended native artifact name directly: `cabin-ports/zlib` keys its sole library target `z`
  (producing `libz.a` / `z.lib`), `cabin-ports/googletest` keys its target `gtest`.  No target
  mangling and no output-name mechanism exist; the key *is* the stem.
- **Links identities.**  A declared [`links`](manifest.md#links) claim is independent of the target
  key.  The two often coincide (`z`, `png`) but need not - `cabin-ports/catch2` keys its target
  `catch2` while claiming `"Catch2"`, upstream's case-sensitive library name.  The
  post-resolution uniqueness check reads the published claims from the index.
- **Provenance.**  A port declares `[package.upstream]`, `[[package.upstream.copy]]`, and
  `[package.upstream].patches` itself and publishes them verbatim, so the hosted registry's
  verifier can check the published tree against the upstream archive.  The patch files themselves
  ship inside the published package at their declared paths.
- **Dependencies.**  A published port names its dependencies as scoped registry packages
  (`cabin-ports/libpng` depends on `"cabin-ports/zlib" = "^1.3"`), and target `deps` references use
  the bare-package shorthand when the dependency exposes exactly one library-like target, or an
  explicit `package:target` selector otherwise.
- **Order.**  Publication order derives from those declared edges (zlib publishes before libpng),
  and they enter the cycle and satisfiability checks like any other package's.

Each version stands alone - published verbatim and probed on its own targets - so two versions of
one port may expose different library targets.  The preflight probe links the library targets
available with the package's *default* features: one gated behind a non-default
`required-features` entry still publishes, but the probe does not compile it, because the probe
enables no extra features.  A target declaring `interface-cxx-standard = "none"` is linked by a C
consumer and the rest by a C++ one, so a package mixing the two is probed from both languages.
Each probe compiles at the lowest standard the linked targets' declared interfaces accept (`c11` /
`c++17` when they declare none), except that an MSVC host raises a floor below C11 / C++14 to
those: `cl.exe` has no `/std:` flag for C99 or C++11.

Both modes run the complete local preflight first: fetch or reuse every pinned upstream archive
(SHA-256 verified, reusing the standard `<cache>/ports` archive cache), run the standard safe
materialization pipeline (extraction, `strip-prefix`, copy steps, patches),
package each port and publish it into a temporary file registry in publication order, and build
every port against the generated packages.  The probe builds run in parallel, one worker per CPU
with two build jobs each, each probe starting as soon as its port is staged; a failure still
reports the earliest failing port in publication order, and a successful run ends with each phase's
time summed over the ports and the slowest ports.  Each port is built through a generated probe
package that depends on the
just-published version, so the build consumes the registry artifact itself - resolution, checksum,
archived manifest, source materialization, and compilation of every compiled library target.
Exercising a port's *API* (including its headers, calling its symbols) needs per-library
knowledge a generic converter does not have, so that knowledge is committed in the port: a probe
consumer compiles the port's `tests/main.cc` (or `tests/main.c` for the C consumer) when it has
one, and an empty `main` otherwise; any other file in `tests/` (a misspelled name, or `main.c` for
a port with no C consumer) fails the preflight rather than being skipped.  `--dry-run` then runs
every probe, and a non-zero exit fails the preflight, so a consumer test is an ordinary program
that returns non-zero on failure.  `--publish` builds the probes but never runs them: running
upstream code in the job that holds the trusted-publishing OIDC token would hand that code the
token, so running them is the pull-request dry-run's job.  Packaging copies only the manifest and
its declared patches from the port directory, so `tests/` is never published: editing a test
changes no archive bytes.  The ports pull-request dry-run runs on Linux, macOS, and Windows.

```console
$ cargo build -p cabinpkg
$ cargo port-publish --dry-run
$ cargo port-publish --publish --index-url https://registry.cabinpkg.com
```

`--dry-run` stops after the preflight.  `--publish` performs the same preflight (building, not
running, the probes) and then uploads
every package through the registry API in publication order, in consecutive batches of 50 (sized
for the operator quota class's publish rate): each batch is one `cabin publish` invocation, which
under GitHub Actions exchanges for its own trusted-publishing token and revokes it, best-effort, on
exit, so a large set never rides one token's 30 minutes.  A failed batch - including one slow
enough to outlive its token - stops the run before the next one and fails it; the rerun resumes by
uploading every package again.  It never skips a version based on the public index - pending
(not yet verified) revisions are hidden there - and instead relies on the registry's idempotency
rule: re-publishing byte-identical bytes is a no-op.  Every upload
passes `--new-revision`, because a change that has been merged to `main` *is* the deliberate
respin (see [Packaging revisions](#packaging-revisions)) - which is why editing a port's
manifest or patches changes the published bytes; an unchanged tree still produces byte-identical
archives, so a rerun stays a no-op either way.

### Publish automation

The workflow `.github/workflows/ports-publish.yml` automates the tool.  Every pull request that
can change the dry-run's result runs the complete `--dry-run` preflight with no secrets, on
Linux, macOS, and Windows: the `ports-dry-run` filter in `.github/path-filters.yml` matches the
ports, every crate in the dependency closure of the `cabin` binary and the publisher (checked by
`cargo workflow-filter-coverage`), and the build configuration.  Publishing is manual only: dispatching the
workflow from `main` publishes the set to `https://registry.cabinpkg.com` (dispatching it on any
other ref runs the dry-run instead, never a publish), so a ports change merged to `main` reaches
the registry at the next dispatch - and the same dispatch is the recovery path after a
pre-launch registry wipe.  Dispatching with the `exchange-check` input
instead exchanges and immediately revokes a trusted-publishing token, proving the registered
binding end to end while publishing nothing.  Publish runs are serialized and an active run is
never cancelled; a run superseded by a newer commit on `main` skips its upload instead
of immutably publishing an intermediate state, and nothing publishes again until the next
dispatch.  The check runs immediately before the publish
command, whose own preflight still takes minutes - a commit landing inside that residual window
can make a stale run publish first, in which case the next dispatch's differing bytes land as a
new packaging revision (see below) rather than overwriting anything, never silent divergence.
The publish job holds no long-lived credential: it grants `permissions: id-token: write`, and
`cabin publish` exchanges the run's own OIDC token for a short-lived registry token confined
to the `cabin-ports` scope, revoking it on the way out (see
[Trusted publishing](remote-registry.md#trusted-publishing); which workflows may exchange is
operator-registered registry data).  Upstream archives restored from the CI cache are never trusted: the
tool re-hashes every cached archive against the checksum its port's `[package.upstream]` pins,
and re-downloads on a mismatch.  The ports tree and the
workflow live in the `cabinpkg/cabin` repository; the
`cabin-ports` GitHub organization is the registry scope authority, not a separate source
repository.

### Packaging revisions

A published version always preserves the upstream version, and the bytes published under it are
immutable.  A packaging-only correction - a manifest fix, a `[[package.upstream.copy]]` addition, a
new or revised patch under `[package.upstream].patches` - therefore reaches the
registry as a new [packaging revision](package-index.md#packaging-revisions) of the same version:
the archive's changed bytes give it a new revision id, and both revisions stay listed and
fetchable.  There is no version-string marker and no sidecar file to maintain; edit the port's
committed `cabin.toml` and merge it.  It publishes verbatim, so a comment is a byte change like
any other.

Consumers are unaffected until they ask to be.  A lockfile that already pins the old revision keeps
resolving its graph and fetching exactly those bytes; a fresh resolution, or an explicit
`cabin update`, picks up the corrected revision.

Because the publisher always passes `--new-revision`, the merged change *is* the intent
signal - including an edit to the committed manifest itself, which publishes verbatim.  A correction landing on `main` is by construction a deliberate respin,
and a rerun over an unchanged tree still produces byte-identical archives and stays a no-op.  A
respin may also correct what resolution consumes - the published package's `dependencies`,
`features`, `standards`, or `links`: each revision keeps its own, so the old revision's pins keep
their graph.  The package's index document then becomes schema 2, which earlier clients refuse
rather than misread.

## Retiring a foundation port

When an upstream project ships and maintains a native `cabin.toml`, the corresponding foundation
port should be retired.  The retirement steps are:

1. Switch downstream `[dependencies]` entries from `"cabin-ports/<name>"` to the appropriate
   `path` / `version` / `workspace` form pointing at the new upstream-maintained package.
2. Delete the `ports/<name>/<version>/` directory.
3. Note the retirement in the relevant release notes.
