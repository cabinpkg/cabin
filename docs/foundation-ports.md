# Foundation ports

Foundation ports are **curated packages** that adapt important existing C/C++ libraries - libraries
that do not yet ship a native `cabin.toml` - to Cabin's build model.  Their sources live under the
cabin-port crate's
[`crates/cabin-port/ports/`](https://github.com/cabinpkg/cabin/tree/main/crates/cabin-port/ports/)
directory and are explicitly **not** a public registry; this directory is closed to arbitrary
submissions and is intended to be retired incrementally as upstreams adopt native `cabin.toml`.

You can search published foundation ports at https://cabinpkg.com.

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
[remote-registry.md](remote-registry.md#the-default-registry)), and verified packages download
without an account or token - `cabin login` is only needed to publish.

There is no port-specific dependency form.  A port is not something a consumer's manifest can name
directly: the recipes below are a *publishing* input, and a manifest that tries to declare one
(`port = true`, `port-path = "..."`) is rejected as an unknown dependency field.

## Anatomy of a foundation port recipe

A recipe is a directory containing two files, plus an optional `patches/` subdirectory.  **No port
is committed this way any more** - every one has migrated to a single provenance-bearing
`cabin.toml` (see "Migration to provenance-bearing packages" below).  The publisher still accepts
the shape, and this section documents it until that path is removed:

```
crates/cabin-port/ports/<name>/<version>/
  port.toml - recipe (pinned source archive + identity)
  cabin.toml - overlay manifest (describes the upstream
                   sources as a Cabin C/C++ target)
  patches/    - optional unified-diff files declared by
                   [source].patches
```

zlib 1.3.1 was the last port committed this way, at
`crates/cabin-port/ports/zlib/1.3.1/`; that directory now holds a package manifest, so the layout
above is historical.

### `port.toml` schema

```toml
[port]
name = "zlib"
version = "1.3.1"
description = "Compression library"   # optional
license = "Zlib"                      # optional
homepage = "https://zlib.net/"        # optional URL
upstream = "https://github.com/madler/zlib"  # optional URL

[source]
type = "archive"
url = "https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz"
sha256 = "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"
strip_prefix = "zlib-1.3.1"           # optional
# optional: unified diffs applied after every [[copy]] step
patches = ["patches/0001-fix-msvc-build.patch"]

[overlay]
manifest = "cabin.toml"

# optional, repeatable: place a prebuilt file under a build-time name
[[copy]]
from = "scripts/pnglibconf.h.prebuilt"
to = "pnglibconf.h"
```

| Field | Required | Notes |
| --- | --- | --- |
| `[port].name` | yes | Must equal the overlay manifest's `[package].name`. |
| `[port].version` | yes | SemVer string; must equal the overlay manifest's `[package].version`. |
| `[port].description` / `license` / `homepage` / `upstream` | no | Plain documentation fields, read by the schema parser and **not** published: the conversion never copies them into the package manifest. |
| `[source].type` | yes | Only `"archive"` is supported.  Every other value (`git`, `tag`, `branch`, `latest`, …) is rejected with `unsupported source type`. |
| `[source].url` | yes | `https://` URL pointing at the upstream archive.  The parser also accepts `file://` and `http://`, but a recipe is a publishing input: published provenance must pin a credential-free `https://` URL, so either of those fails the conversion.  A URL whose path ends in `.zip` (case-insensitive) is treated as a zip archive; every other URL is treated as a `.tar.gz`.  Zip support exists for upstreams whose only official release artifact is a zip (miniz's amalgamation, for example); prefer `.tar.gz` when the upstream publishes one. |
| `[source].sha256` | yes | Lower-case 64-character hex digest.  Upper-case and wrong-length values are rejected. |
| `[source].strip_prefix` | no | Single relative path component that must equal the first path segment of every archive entry.  The component is stripped before extraction so the overlay manifest sits at the prepared directory's root. |
| `[source].patches` | no | Unified-diff files applied to the assembled source after every `[[copy]]` step, in declaration order (see below).  Each entry must be `patches/<file>`: a single file directly under the port directory's `patches/` subdirectory, at most 16 entries, 1 MiB per file. |
| `[overlay].manifest` | yes | Relative path inside the port directory pointing at the overlay `cabin.toml`.  Absolute paths and `..` are rejected. |
| `[[copy]]` | no | Zero or more static file placements applied to the extracted source (see below).  Each has `from` and `to`. |

Unknown fields and unknown top-level tables are rejected by the parser (`deny_unknown_fields`).

### Placing prebuilt files with `[[copy]]`

A few upstreams ship a build-time file under a name the compiler does not expect. libpng, for
example, ships its configuration as `scripts/pnglibconf.h.prebuilt` and its own build copies that to
`pnglibconf.h` before compiling.  Cabin never runs a port's upstream build, so a port declares such
a placement declaratively:

```toml
[[copy]]
from = "scripts/pnglibconf.h.prebuilt"
to = "pnglibconf.h"
```

Each step copies one file that already exists in the extracted source to a second location inside
the same tree.  `from` and `to` are both validated as relative paths inside the source directory
(absolute paths and `..` are rejected), the copy runs after extraction and *before* the overlay
`cabin.toml` is written (so the overlay always wins on any conflicting `to`), and the source file is
covered by the archive's pinned SHA-256.  A missing `from` fails preparation with a clear error.
This is a **static file copy**, not a build script: it runs no commands, generates nothing, and
reads nothing outside the extracted archive.

### Patching upstream sources with `[source].patches`

Some upstreams need a small correction - a build-system fix, a portability tweak - that no copy
step can express.  A port declares such corrections as unified-diff files:

```toml
[source]
# ... url / sha256 / strip_prefix ...
patches = ["patches/0001-fix-msvc-build.patch"]
```

Each entry names a file under the port directory's `patches/` subdirectory.  The declared order is
the application order, and application is **byte-exact**: the strip level is fixed at the `-p1`
equivalent (git's `a/` / `b/` prefixes), context must match the assembled tree byte for byte -
no fuzz, no offset search, no newline normalization - and only text diffs are accepted (binary
hunks are rejected).  The patch itself must end with a newline (or a `\ No newline at end of file`
marker), git rename/copy headers and multi-hunk file creations are rejected because their
effect is not a plain sequence of hunks, and a patch may name each file at most once (as
`git diff` emits).  Application is bounded: each patched file is capped at 16 MiB, and the whole
list may carry at most 1024 file entries and read or rewrite at most 128 MiB of the source tree.  A patch file must contain nothing but the diff itself:
git's per-file preamble lines (`diff --git`, `index`, mode lines) are accepted in their exact
shape, and any other surrounding text - a `format-patch` commit message, a signature trailer - is
rejected, so use `git diff` output rather than `git format-patch`.  Paths inside diff headers are
validated like copy paths, so a patch cannot escape the source tree.  Patches run after every `[[copy]]` step and before the overlay
`cabin.toml` is written; the assembly pipeline is fixed and normative: extract, strip the
declared prefix, apply copies, then apply patches.  A patch that does not apply fails preparation
with a clear error.  Like `[[copy]]`, this is **declarative transformation**, not a build script:
it runs no commands and reads nothing outside the extracted archive and the declared patch files.

Each patch file is also placed into the prepared tree at its declared `patches/<file>` path, so a
published conversion ships the patches its `[package.upstream]` declaration names and the hosted
registry's verifier can re-run the same transformation from the published package alone.

### Overlay manifest

The overlay is an ordinary Cabin manifest with one constraint: its `[package].name` and
`[package].version` must match the authoritative identity declared in `port.toml`.  Mismatches
surface as `overlay manifest for port \`<name> <version>\` declares package \`<actual_name>
<actual_version>\`; expected to match the port identity`.

`zlib`'s overlay declares a single `library` target with the 15 canonical zlib C sources and the
archive root on the include path:

```toml
[package]
name = "zlib"
version = "1.3.1"

[target.zlib]
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

Library recipes claim the native identity they embody via [`links`](manifest.md#links) (`zlib`
claims `"z"`, `libpng` claims `"png"`), judged against upstream conventions - the identity, not the
target key, and only where a real native library exists.  Header-only ports claim nothing, and an
alternative implementation with its own symbol namespace claims its own identity (`miniz` claims
`"miniz"`, not `"z"`: its objects export only `mz_*` symbols).

## Materialization pipeline

Materializing a recipe is what the publisher does before packaging it, through
`cabin_port::prepare`.  It is a *separate* implementation from
[`cabin_artifact::materialize_upstream`](architecture.md), the pipeline a migrated package stages
through and the one the hosted registry's verifier replays against a published package's
`[package.upstream]` declaration - so a hardening change to the shared materializer does **not**
reach this path:

1. Load the `port.toml` and validate it.
2. Fetch the pinned archive.  Published provenance pins an `https://` URL, downloaded via the
   same HTTP client `cabin-index-http` uses, with a five-hop redirect budget.  Following redirects is safe because the SHA-256 pin in
    `port.toml` is verified against the final response bytes, and upstream release archives commonly
    hop from a forge (e.g.  GitHub) to a CDN.  Compressed archives larger than 64 MiB are refused by
    the HTTP client; no foundation port currently approaches that limit.
  - A previously prepared archive (matching SHA-256 already in the port cache) short-circuits the
    download so repeat invocations stay network-free.
3. Verify the archive's SHA-256 against `port.toml`.  Mismatch surfaces `checksum mismatch for port
   \`<name> <version>\`: expected sha256:..., got sha256:...`.
4. Safely extract the archive into a scratch directory beside the port cache entry, with the
   declared `strip_prefix`.  This step reuses `cabin-artifact`'s extraction primitives (tar.gz or
   zip, chosen by the URL's path extension), so the full
   [extraction safety contract](package-format.md#extraction-safety-contract) - decompression-bomb
   caps, path-traversal protection, duplicate and conflicting entries - applies identically to both
   formats.  Symlink entries are **skipped** rather than failing the port: upstream release
   archives commonly carry convenience symlinks (uthash ships `include -> src`), nothing is ever
   materialized on disk for a skipped entry, and a port overlay only references real files.  Every
   other special entry type (hard links, devices, fifos) is still rejected, and package archives
   keep the strict reject-symlinks default.
5. Apply any `[[copy]]` steps, placing prebuilt files under their build-time names inside the
   extracted tree.
6. Apply any declared `[source].patches` as byte-exact unified diffs, then place each patch file
   itself into the tree at its declared `patches/<file>` path.
7. Copy the overlay manifest into the extracted source dir as `cabin.toml`.
8. Cross-check the overlay's `[package]` identity against `port.toml`.  Mismatch surfaces an
   explicit error.
9. Rename the scratch directory into its final place in the port cache.  On a cold cache, steps 4
   through 8 all run against the scratch tree, so a hostile archive, a broken `[[copy]]` plan, an
   inapplicable patch, or a mismatched overlay leaves no partially prepared port at the final
   path.  (On a warm cache hit — the completion marker's plan fingerprint matches, covering the
   copy steps and each patch's path *and content* — there is no scratch tree, and the copy and
   patch steps are **not** re-run: re-copying would revert a patched copy target and re-patching
   would fail its context match.  Only the overlay refresh and the identity cross-check repeat.)
10. Drop a sibling `.ok` completion marker so the next run can reuse the materialized directory
   without re-extracting.

Once materialized, the port directory is an ordinary Cabin package directory, which is what the
publisher packages and uploads.

## Cache layout

Materialized ports live under the same root the rest of Cabin's artifact cache uses:

```
<cache>/ports/
  archives/sha256/<hex>.tar.gz   (or <hex>.zip for zip sources)
  sources/<name>/<version>/sha256/<hex>/
    cabin.toml         (overlay)
    <upstream files>
  sources/<name>/<version>/sha256/<hex>.ok    (completion marker)
```

The cache root resolution follows the documented chain: `--cache-dir` - > `CABIN_CACHE_DIR` - >
`CABIN_CACHE_HOME` - > `$XDG_CACHE_HOME/cabin` - > `$HOME/.cache/cabin`.  The default on a Unix-like
system with no overrides is `$HOME/.cache/cabin/`, so the example layout above lives at
`~/.cache/cabin/ports/archives/sha256/<hex>.tar.gz` etc.  The cache is shared across projects on the
same machine - content is checksum-addressed, so repeated publisher runs over the same pinned
archive reuse the same on-disk tree.

## What foundation ports are **not**

- Not Cabin's public registry.  Cabin's registry layer is documented in
  [`registry-design.md`](registry-design.md) and evolves independently.
- Not published metadata.  A port recipe's `[source]` / `[[copy]]` / `patches` vocabulary has a
  published counterpart - the optional `[package.upstream]` provenance table on a package manifest
  ([`manifest.md`](manifest.md#packageupstream)), which the hosted registry's external verifier
  checks against the pinned upstream archive.  The recipe stays a publishing input;
  `cabin publish` never archives port recipes.
- Not a submission queue.  New foundation ports require a curated review; this directory is
  intentionally small.
- Not a vehicle for binary distribution.  Only source archives are supported.
- Not a workaround for missing build-script support.  Ports describe libraries whose source layout
  already fits Cabin's target model (a fixed list of sources plus include directories), optionally
  placing a prebuilt file with a static `[[copy]]` step or correcting one with a declared
  byte-exact patch.  Libraries that need configure-time generation, CMake / Meson / Autotools
  driving, or custom build commands are out of scope.
- Not a variant-selection mechanism.  A published port carries whatever `[features]` its manifest
  declares (`cabin-ports/sqlite3` ships a `single-threaded` feature), but nothing in the port
  vocabulary selects a shared or static build.

## Error catalog

| Diagnostic | Trigger |
| --- | --- |
| `unsupported source type` | `port.toml`'s `[source].type` is anything other than `"archive"`. |
| `is missing [source].sha256` | The sha256 field is absent. |
| `invalid SHA-256` | The sha256 field is the wrong length or contains non-lower-case-hex characters. |
| ``invalid `<field>` URL`` | `[source].url`, `homepage`, or `upstream` is not a valid URL. |
| `unsafe overlay manifest path` | `[overlay].manifest` is absolute or contains `..`. |
| `unsupported archive URL scheme` | The archive URL is not `file://`, `http://`, or `https://`.  Publication narrows this further to `https://` only. |
| `checksum mismatch` | The downloaded archive's SHA-256 does not match the recipe. |
| `source archive does not contain the declared strip_prefix directory` | The archive's first path component does not equal the declared prefix. |
| `overlay manifest was not found at <path>` | `[overlay].manifest` points at a non-existent file inside the port directory. |
| ``declares an unsafe `patches` entry`` | A `[source].patches` entry is not `patches/<file>` or fails the portable-path rules. |
| `patch file for port ... was not found at <path>` | A declared `[source].patches` file is absent from the port directory. |
| `patch file for port ... is <n> bytes` | A declared patch file exceeds the 1 MiB per-file cap. |
| `failed to apply patches for port ...` | A declared patch is malformed, binary, unsafe, or does not match the assembled tree byte for byte. |
| `overlay manifest declares package \`<actual>\`` | The overlay's `[package]` identity disagrees with `port.toml`. |

## Migration to provenance-bearing packages

The recipe tree has collapsed into ordinary package directories: a port
migrated by replacing its `port.toml` + overlay pair with a single
`cabin.toml` that carries the canonical scoped identity
(`cabin-ports/<name>`), the full `[package.upstream]` provenance block
(pinned archive, checksum, strip prefix, copies, and declared patch
files), and the same targets the overlay described. **Every port has
now migrated**, so `crates/cabin-port/ports/` holds package directories
only:

- A directory with only a `cabin.toml` is a migrated package: published
  verbatim (nothing is converted) and materialized through the same
  shared pipeline the registry verifier replays.
- A directory with a `port.toml` is a recipe, converted at publish time.
  The publisher still accepts the shape and the test fixtures still
  exercise it; nothing is committed in it.

Both shapes publish to the same place, so the shape a port is committed
in is invisible to consumers.

A migrated port's manifest must agree with its directory layout (name
and version), publish under the `cabin-ports/` scope, and declare only
registry dependencies - unconditional ones: no `system = true`, no
path or workspace sources, and no `cfg(...)` table, because
publication ordering reads the declared edges without evaluating
conditions (the publisher refuses each of these with a message
naming it).  Each migrated version stands alone - published verbatim and
probed on its own targets - so two versions of one port may expose
different library targets, which a recipe's versions may not.  The
preflight probe links the library targets available with the
package's *default* features: one gated behind a non-default
`required-features` entry still publishes, but the probe does not
compile it, because the probe enables no extra features.  A target
declaring `interface-cxx-standard = "none"` is linked by a C consumer
and the rest by a C++ one, so a package mixing the two is probed from
both languages.

The publisher orders publication across both shapes by dependency, in
either direction, so a recipe and a package may depend on each other.
There is no longer a port-specific edge - a recipe reaches
another port through an ordinary scoped registry dependency, exactly
as any other package would, and that edge orders publication and
enters the cycle and satisfiability checks like any other.

## Publishing ports as registry packages

The repository tool `cabin-port-publish` (`crates/cabin-port-publish`, a `publish = false`
workspace crate that is not part of the shipped `cabin` binary) publishes every committed port as
an ordinary registry package under the **`cabin-ports`** scope.  Every committed port is now a
migrated package directory, published verbatim from the manifest committed in it.  The tool also
still accepts a recipe, converting it (the recipe stays the source of truth; the tool rewrites a
copy of each overlay).  For a recipe the rewrite would be:

- **Identity.**  `cabin-ports/<lowercase name>` - the registry name grammar is lowercase, so a
  recipe named `Foo` would convert to `cabin-ports/foo`.  The published version is the upstream
  version, verbatim.  A migrated package meets the same rule from the other direction: its
  committed manifest declares the scoped name itself, and the publisher refuses one that does not
  fold onto its directory - which is what keeps the mixed-case directories coherent (`cJSON/` must
  declare `cabin-ports/cjson`, `CLI11/` must declare `cabin-ports/cli11`).
- **Target keys.**  A target key directly determines its artifact stem, so the conversion picks
  the intended native artifact name: `zlib` publishes a sole library target named `z` (producing
  `libz.a` / `z.lib`), and every other key lowercases.  A migrated
  package states the key it wants directly (`cabin-ports/googletest` keys its target `gtest`).
  No target mangling and no output-name mechanism exist; the key *is* the stem.
- **Links identities.**  A declared [`links`](manifest.md#links) claim is independent of the target
  key: for a recipe it rides through the conversion unchanged, so the renamed `z` target still
  carries `links = "z"`, and a migrated package declares key and claim directly.  The two often
  coincide (`z`, `png`) but need not - `cabin-ports/catch2` keys its target `catch2` while claiming
  `"Catch2"`, upstream's case-sensitive library name.  The post-resolution uniqueness check reads
  the published claims from the index.
- **Provenance.**  A recipe's published `[package.upstream]` is stamped from its pinned `[source]`
  (URL, SHA-256, format, `strip_prefix`), its `[[copy]]` operations, and its `[source].patches`; a
  migrated package already declares `[package.upstream]`, `[[package.upstream.copy]]`, and
  `[package.upstream].patches` itself and publishes them verbatim.  Either way the hosted
  registry's verifier can check the published tree against the upstream archive.  The patch files themselves ship inside the published package at their
  declared paths.
- **Dependencies.**  A published port names its dependencies as scoped registry packages
  (`cabin-ports/libpng` depends on `"cabin-ports/zlib" = "^1.3"`), and target `deps` references use
  the bare-package shorthand when the dependency exposes exactly one library-like target, or an
  explicit `package:target` selector otherwise.
- **Order.**  Publication order derives from those declared edges (zlib publishes before libpng).

Both modes run the complete local preflight first: fetch or reuse every pinned upstream archive
(SHA-256 verified, reusing the standard `<cache>/ports` archive cache), run the standard safe
materialization pipeline (extraction, `strip_prefix`, copy steps, patches - plus the overlay for a
recipe), package each port, publish it into a temporary file registry, and build every port against the generated packages in
publication order.  Each port is built through a generated probe package that depends on the
just-published version, so the build consumes the registry artifact itself - resolution, checksum,
archived manifest, source materialization, and compilation of every compiled library target.  The
probe deliberately stops there: exercising a port's *API* (including its headers, calling its
symbols) needs per-library knowledge a generic converter does not have, and is what the
per-port packages under `examples/` - covered by the CLI integration tests over the same
upstream sources - are for.

```console
$ cargo build -p cabinpkg
$ cargo run -p cabinpkg-port-publish -- --dry-run
$ cargo run -p cabinpkg-port-publish -- --publish --index-url https://registry.cabinpkg.com
```

`--dry-run` stops after the preflight.  `--publish` performs the same preflight and then uploads
every package through the registry API in publication order.  It never skips a version based on
the public index - pending (not yet verified) revisions are hidden there - and instead relies on
the registry's idempotency rule: re-publishing byte-identical bytes is a no-op.  Every upload
passes `--new-revision`, because a change that has been merged to `main` *is* the deliberate
respin (see [Packaging revisions](#packaging-revisions)) - which is why editing a migrated
package directory changes the published bytes; an unchanged tree still produces byte-identical
archives, so a rerun stays a no-op either way.

### Publish automation

The workflow `.github/workflows/ports-publish.yml` automates the tool.  Pull requests that touch
the ports tree, the `cabin-port` pipeline, or the publisher run the complete `--dry-run` preflight
with no secrets; pushes to `main` with such changes publish the set to
`https://registry.cabinpkg.com`; manual dispatch from `main` republishes everything, which is
the recovery path after a pre-launch registry wipe (dispatching the workflow on any other ref
runs the dry-run instead, never a publish).  Publish runs are serialized and an active run is
never cancelled; a run superseded by a newer matching commit on `main` skips its upload instead
of immutably publishing an intermediate state.  The check runs immediately before the publish
command, whose own preflight still takes minutes - a commit landing inside that residual window
can make a stale run publish first, in which case the newer run's differing bytes land as a new
packaging revision (see below) rather than overwriting anything, never silent divergence.
The publish job authenticates through the `CABIN_PORTS_TOKEN` repository secret (a registry
token for the account owning the `cabin-ports` scope), exposed to `cabin publish` as
`CABIN_REGISTRY_TOKEN`.  Upstream archives restored from the CI cache are never trusted: the
tool re-hashes every cached archive against the SHA-256 its port pins - a recipe's `[source]`, a
migrated package's `[package.upstream]` - and re-downloads on a mismatch.  The ports tree and the
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
resolving and fetching exactly those bytes; a fresh resolution, or an explicit `cabin update`, picks
up the corrected revision.

Because the publisher always passes `--new-revision`, the merged change *is* the intent
signal - for a migrated package directory that includes an edit to the committed manifest itself,
which publishes verbatim.  A correction landing on `main` is by construction a deliberate respin,
and a rerun over an unchanged tree still produces byte-identical archives and stays a no-op.  A revision may not
change what resolution consumes, so a correction that alters the published package's
`dependencies`, `features`, or `standards` is not a respin - it needs a new upstream version.
`links` is the one-way exception: a respin may stamp a claim table onto a version published
without one (that is how existing ports gained their identities), but changing or removing an
existing claim still needs a new upstream version.

## Retiring a foundation port

When an upstream project ships and maintains a native `cabin.toml`, the corresponding foundation
port should be retired.  The retirement steps are:

1. Switch downstream `[dependencies]` entries from `"cabin-ports/<name>"` to the appropriate
   `path` / `version` / `workspace` form pointing at the new upstream-maintained package.
2. Delete the `crates/cabin-port/ports/<name>/<version>/` directory.
3. Update `crates/cabin-port/ports/README.md`
   to remove the entry from the "Available ports" list.
4. Note the retirement in the relevant release notes.
