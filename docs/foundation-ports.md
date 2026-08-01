# Foundation ports

Foundation ports are **curated recipes** that adapt important existing C/C++ libraries - libraries
that do not yet ship a native `cabin.toml` - to Cabin's build model.  They live under the cabin-port
crate's
[`crates/cabin-port/ports/`](https://github.com/cabinpkg/cabin/tree/main/crates/cabin-port/ports/)
directory and are explicitly **not** a public registry; this directory is closed to arbitrary
submissions and is intended to be retired incrementally as upstreams adopt native `cabin.toml`.

You can search published foundation ports at https://cabinpkg.com.

## Bundled ports

The curated set of foundation ports is embedded in the `cabin` binary at compile time, so a user
with only `cabin` installed can depend on a bundled port without copying any recipe files:

```toml
[dependencies]
zlib = { port = true, version = "^1.3" }
```

The embedded set shrinks as the recipe layer collapses: a port committed as a
provenance-bearing package is deliberately *not* embedded, so `{ port = true }` no longer
resolves it and the dependency moves to `"cabin-ports/<name>"` (see "Migrating a port to a
package" below).  `cabin port list` reports what your binary actually ships, and
[`crates/cabin-port/ports/README.md`](https://github.com/cabinpkg/cabin/tree/main/crates/cabin-port/ports/README.md)
marks the migrated entries.

`port = true` declarations require a `version = "<requirement>"` field.  The bundled set is resolved
by `(name, version_req)`; the highest-versioned entry whose `version` satisfies the requirement
wins.  With the current single-entry bundled set, the only effective check is that the request is
satisfiable.  Run `cabin port list` to see the names and versions shipped in your binary.  The
dependency name must match a bundled entry exactly; unknown names surface
`PortError::UnknownBuiltin`.

Cabin's source repository under
[`crates/cabin-port/ports/`](https://github.com/cabinpkg/cabin/tree/main/crates/cabin-port/ports/)
is the authoritative location for each recipe.  `cabin-port`'s `builtin` module embeds the same
files via `include_str!`, so edits to `crates/cabin-port/ports/zlib/1.3.1/port.toml` flow into the
binary on the next `cargo build`.  A round-trip test in `cabin-port::builtin` asserts the embedded
text and the on-disk recipe stay in sync.

## Local recipes (for recipe development)

`{ port-path = "../ports/zlib/1.3.1" }` keeps working - the path is interpreted relative to the
consumer's `cabin.toml`.  This form is intended for developing or vetting a recipe before it lands
in the bundled set, and for users who vendor a recipe into their own tree.

## Anatomy of a foundation port

A port is a directory containing two files, plus an optional `patches/` subdirectory:

```
crates/cabin-port/ports/<name>/<version>/
  port.toml - recipe (pinned source archive + identity)
  cabin.toml - overlay manifest (describes the upstream
                   sources as a Cabin C/C++ target)
  patches/    - optional unified-diff files declared by
                   [source].patches
```

For example, zlib 1.3.1 lives at `crates/cabin-port/ports/zlib/1.3.1/`.

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
| `[port].description` / `license` / `homepage` / `upstream` | no | Plain documentation fields.  Surfaced via `cabin metadata`. |
| `[source].type` | yes | Only `"archive"` is supported.  Every other value (`git`, `tag`, `branch`, `latest`, …) is rejected with `unsupported source type`. |
| `[source].url` | yes | `file://`, `http://`, or `https://` URL pointing at the upstream archive.  A URL whose path ends in `.zip` (case-insensitive) is treated as a zip archive; every other URL is treated as a `.tar.gz`.  Zip support exists for upstreams whose only official release artifact is a zip (miniz's amalgamation, for example); prefer `.tar.gz` when the upstream publishes one. |
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

## Depending on a foundation port

A port still committed as a recipe is opted into via either the bundled form (`port = true`,
resolved by name against the embedded set) or the filesystem form (`port-path`, pointing at a
recipe directory).  Neither form reaches a *migrated* port: those are registry packages, and the
`"cabin-ports/<name>"` spelling below is the only one that resolves them.

```toml
[dependencies]
zlib = { port = true, version = "^1.3" }  # bundled recipe
# -- or for local development --
zlib = { port-path = "../ports/zlib/1.3.1" }
```

The same libraries are also published to the Cabin registry as ordinary `cabin-ports/*` packages
(see "Publishing ports as registry packages" below), which is the form the package pages on
[cabinpkg.com](https://cabinpkg.com) show:

```toml
[dependencies]
"cabin-ports/zlib" = "=1.3.1"
```

The dependency key needs quotes (scoped names contain `/`), and the requirement names the plain
upstream version: [packaging revisions](package-index.md#packaging-revisions) are not part of the
version string, and consumers pin them through the lockfile's checksum instead.  Registry
dependencies resolve through the hosted registry by default (no flag; see
[remote-registry.md](remote-registry.md#the-default-registry)), and verified packages download
without an account or token - `cabin login` is only needed to publish; the bundled
`port = true` form works offline-cached with no configuration on a stock `cabin` install.

`port = true` requires a sibling `version = "<requirement>"` field (see "Bundled ports" above).
`port-path` is mutually exclusive with `version` - the recipe at the path supplies the version.
Both forms are mutually exclusive with `path`, `workspace`, and `system`.  Both **do** honor
`features` and `default-features` - a port's overlay can declare a `[features]` table, and the
feature resolver threads per-edge feature requests onto the prepared port package exactly as it does
for a path dependency.  No port still committed as a recipe declares a `[features]` table today -
the feature-bearing ports have all migrated to packages - so the live example is the registry
form (`"cabin-ports/sqlite3" = { version = "=3.53.2", features = ["single-threaded"] }`); a
recipe that declared one would thread the same way.  `optional` is still rejected on
port dependencies with a typed error, because the port forms never enter the version resolver that
optional gating drives.

## Preparation pipeline

When Cabin runs a workspace-loading command, the CLI orchestrates preparation **before** the
workspace loader sees the manifest:

1. Walk the manifest tree to discover every reachable port dependency.  A port's own overlay may
   declare further port dependencies, so discovery recurses into each port's overlay and follows
   those edges transitively - no port still committed as a recipe declares one today, the last
   such edge (libpng's on zlib) having migrated with libpng.  The walk stays network-free: a
   bundled port's overlay is read from the embedded recipe text, a `port-path` port's from disk.
2. Load each `port.toml` and validate it.
3. Decide a fetch source per port:
  - `file://` URLs become a `LocalArchive` pointing at the filesystem path;
  - `http://` / `https://` URLs are downloaded via the same HTTP client `cabin-index-http` uses,
    with a five-hop redirect budget.  Following redirects is safe because the SHA-256 pin in
    `port.toml` is verified against the final response bytes, and upstream release archives commonly
    hop from a forge (e.g.  GitHub) to a CDN.  Compressed archives larger than 64 MiB are refused by
    the HTTP client; no foundation port currently approaches that limit.
  - A previously prepared archive (matching SHA-256 already in the port cache) short-circuits the
    download so repeat invocations stay network-free.
4. Verify the archive's SHA-256 against `port.toml`.  Mismatch surfaces `checksum mismatch for port
   \`<name> <version>\`: expected sha256:..., got sha256:...`.
5. Safely extract the archive into a scratch directory beside the port cache entry, with the
   declared `strip_prefix`.  This step reuses `cabin-artifact`'s extraction primitives (tar.gz or
   zip, chosen by the URL's path extension), so the full
   [extraction safety contract](package-format.md#extraction-safety-contract) - decompression-bomb
   caps, path-traversal protection, duplicate and conflicting entries - applies identically to both
   formats.  Symlink entries are **skipped** rather than failing the port: upstream release
   archives commonly carry convenience symlinks (uthash ships `include -> src`), nothing is ever
   materialized on disk for a skipped entry, and a port overlay only references real files.  Every
   other special entry type (hard links, devices, fifos) is still rejected, and package archives
   keep the strict reject-symlinks default.
6. Apply any `[[copy]]` steps, placing prebuilt files under their build-time names inside the
   extracted tree.
7. Apply any declared `[source].patches` as byte-exact unified diffs, then place each patch file
   itself into the tree at its declared `patches/<file>` path.
8. Copy the overlay manifest into the extracted source dir as `cabin.toml`.
9. Cross-check the overlay's `[package]` identity against `port.toml`.  Mismatch surfaces an
   explicit error.
10. Rename the scratch directory into its final place in the port cache.  On a cold cache, steps 5
   through 9 all run against the scratch tree, so a hostile archive, a broken `[[copy]]` plan, an
   inapplicable patch, or a mismatched overlay leaves no partially prepared port at the final
   path.  (On a warm cache hit — the completion marker's plan fingerprint matches, covering the
   copy steps and each patch's path *and content* — there is no scratch tree, and the copy and
   patch steps are **not** re-run: re-copying would revert a patched copy target and re-patching
   would fail its context match.  Only the overlay refresh and the identity cross-check repeat.)
11. Drop a sibling `.ok` completion marker so the next invocation can reuse the prepared directory
   without re-extracting.

Once prepared, each port directory looks exactly like a regular Cabin path dependency: the existing
workspace loader, build planner, and Ninja backend take over unchanged.  Foundation ports are tagged
`PackageKind::Local` because their on-disk contents are local working state; they never enter the
lockfile and never round-trip through the registry layer.

## Cache layout

Prepared ports live under the same root the rest of Cabin's artifact cache uses:

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
same machine - content is checksum-addressed, so two projects depending on the same port reuse the
same on-disk recipe and source tree.

## Offline / frozen interaction

- `--offline` blocks remote downloads; preparation still succeeds when the archive is already in the
  cache or when the port declares a `file://` URL.
- `--frozen` forbids populating the cache.  If the prepared source tree is not already on disk,
  preparation fails with `cannot prepare port \`<name> <version>\` because --frozen was specified
  and the port is not cached`.

## `cabin metadata` provenance

The metadata view exposes one entry per prepared port under a top-level `ports` array, sorted by
canonical port directory.  Each entry records the upstream URL, the verified SHA-256, the declared
`strip_prefix`, the overlay manifest path, and the cache directory the upstream sources were
extracted into:

```json
"ports": [
  {
    "name": "zlib",
    "version": "1.3.1",
    "origin": { "kind": "builtin", "name": "zlib" },
    "source_dir": "/home/<user>/.cache/cabin/ports/sources/zlib/1.3.1/sha256/<hex>",
    "source": {
      "kind": "archive",
      "url": "https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz",
      "sha256": "sha256:9a93b2b7...",
      "strip_prefix": "zlib-1.3.1"
    }
  }
]
```

For a `port-path` dependency the entry looks the same except `origin` carries `{ "kind": "path",
"port_dir": "/.../ports/zlib/1.3.1" }` and `overlay_manifest` is present (pointing at the on-disk
`cabin.toml`).  `overlay_manifest` is omitted for bundled ports.

The dependency itself appears under the consumer package's `dependencies` array.  For a `port-path`
dependency the source shape carries an `origin` block matching the top-level `ports` array:

```json
"source": { "kind": "port", "origin": { "kind": "path", "port_dir": "../ports/zlib/1.3.1" } }
```

For a bundled (`port = true`) dependency the shape is:

```json
"source": { "kind": "port", "origin": { "kind": "builtin", "name": "zlib" } }
```

## What foundation ports are **not**

- Not Cabin's public registry.  Cabin's registry layer is documented in
  [`registry-design.md`](registry-design.md) and evolves independently.
- Not published metadata.  A port recipe's `[source]` / `[[copy]]` / `patches` vocabulary has a
  published counterpart - the optional `[package.upstream]` provenance table on a package manifest
  ([`manifest.md`](manifest.md#packageupstream)), which the hosted registry's external verifier
  checks against the pinned upstream archive.  The recipe stays local development policy;
  `cabin publish` never archives port recipes.
- Not a submission queue.  New foundation ports require a curated review; this directory is
  intentionally small.
- Not a vehicle for binary distribution.  Only source archives are supported.
- Not a workaround for missing build-script support.  Ports describe libraries whose source layout
  already fits Cabin's target model (a fixed list of sources plus include directories), optionally
  placing a prebuilt file with a static `[[copy]]` step or correcting one with a declared
  byte-exact patch.  Libraries that need configure-time generation, CMake / Meson / Autotools
  driving, or custom build commands are out of scope.
- Limited feature surface.  The `port` dependency form honors `features` / `default-features` (a
  port overlay can declare a `[features]` table, though no committed recipe does today -
  `cabin-ports/sqlite3` carries its `single-threaded` feature as a package), but it does not
  support optional gating or shared/static variant selection.

## Error catalog

| Diagnostic | Trigger |
| --- | --- |
| `no bundled foundation port named ...` | `port = true` references a name not present in `cabin_port::builtin::BUILTIN`. |
| `foundation-port dependency ... must specify a version ...` | `port = true` without a sibling `version` field. (`ManifestError::PortDependencyMissingVersion`) |
| `no bundled foundation port ... satisfies ...` | `port = true, version = "<req>"` where no bundled entry's `version` matches `<req>`.  The message lists the available versions. (`PortError::BuiltinVersionNotFound`) |
| `unsupported source type` | `port.toml`'s `[source].type` is anything other than `"archive"`. |
| `is missing [source].sha256` | The sha256 field is absent. |
| `invalid SHA-256` | The sha256 field is the wrong length or contains non-lower-case-hex characters. |
| ``invalid `<field>` URL`` | `[source].url`, `homepage`, or `upstream` is not a valid URL. |
| `unsafe overlay manifest path` | `[overlay].manifest` is absolute or contains `..`. |
| `unsupported archive URL scheme` | The archive URL is not `file://`, `http://`, or `https://`. |
| `checksum mismatch` | The downloaded archive's SHA-256 does not match the recipe. |
| `source archive does not contain the declared strip_prefix directory` | The archive's first path component does not equal the declared prefix. |
| `overlay manifest was not found at <path>` | `[overlay].manifest` points at a non-existent file inside the port directory. |
| ``declares an unsafe `patches` entry`` | A `[source].patches` entry is not `patches/<file>` or fails the portable-path rules. |
| `patch file for port ... was not found at <path>` | A declared `[source].patches` file is absent from the port directory. |
| `patch file for port ... is <n> bytes` | A declared patch file exceeds the 1 MiB per-file cap. |
| `failed to apply patches for port ...` | A declared patch is malformed, binary, unsafe, or does not match the assembled tree byte for byte. |
| `overlay manifest declares package \`<actual>\`` | The overlay's `[package]` identity disagrees with `port.toml`. |
| `cannot download port \`<name>\` because --offline was specified` | A remote URL was reached while running in offline mode. |
| `cannot prepare port \`<name>\` because --frozen was specified and the port is not cached` | The cache does not already hold a prepared copy under `--frozen`. |
| `foundation-port dependency <name> declared by package <parent> has not been prepared` | Internal invariant violation: the CLI orchestration layer did not run before the workspace loader. |
| `foundation-port directory <port_dir> does not exist` | The consumer's `port-path = "..."` value does not resolve to an existing directory. |

## Migration to provenance-bearing packages

The recipe tree is collapsing into ordinary package directories: a port
migrates by replacing its `port.toml` + overlay pair with a single
`cabin.toml` that carries the canonical scoped identity
(`cabin-ports/<name>`), the full `[package.upstream]` provenance block
(pinned archive, checksum, strip prefix, copies, and declared patch
files), and the same targets the overlay described. Both shapes coexist
under `crates/cabin-port/ports/` while the migration lands, one port at
a time:

- A directory with a `port.toml` is a recipe: converted at publish
  time, embedded into the `cabin-port` crate's builtin table, and
  preparable offline through `{ port = true }`.
- A directory with only a `cabin.toml` is a migrated package: published
  verbatim (nothing is converted), materialized through the same
  shared pipeline the registry verifier replays, and **no longer
  embedded** - it is consumed as an ordinary registry dependency, not
  through the builtin port mechanism.

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

The publisher orders publication across both shapes by dependency, and a
migrated package may depend on a port still committed as a recipe.

The reverse does not hold, and fixes the migration order: a recipe's
`{ port = true }` dependency resolves only through the bundled
builtin table, which deliberately no longer carries migrated ports, so
**a port may not migrate while a recipe still consumes it that way**.
Migrate such a dependency together with (or after) its recipe
dependents; the publisher refuses the intermediate state rather than
letting a consumer's build fail with `UnknownBuiltin`.

## Publishing ports as registry packages

The repository tool `cabin-port-publish` (`crates/cabin-port-publish`, a `publish = false`
workspace crate that is not part of the shipped `cabin` binary) publishes every committed port as
an ordinary registry package under the **`cabin-ports`** scope.  A port still committed as a recipe
is converted (the recipe stays the source of truth and keeps working as a bundled port; the tool
rewrites a copy of each overlay); a migrated package directory is published verbatim, from the
manifest committed in it.  For a recipe the rewrite is:

- **Identity.**  `cabin-ports/<lowercase name>` - the registry name grammar is lowercase, so a
  recipe named `Foo` would convert to `cabin-ports/foo`; every port still committed as a recipe is
  already lowercase, so the fold is a no-op for them today.  The published version is the upstream
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
- **Provenance.**  Each package carries `[package.upstream]` stamped from the recipe's pinned
  `[source]` (URL, SHA-256, format, `strip_prefix`), declared `[[copy]]` operations, and declared
  `[source].patches`, so the hosted registry's verifier can check the published tree against the
  upstream archive.  The patch files themselves ship inside the published package at their
  declared paths.
- **Dependencies.**  Inter-port `{ port = true }` edges become scoped registry dependencies; a
  migrated package declares that scoped dependency itself (`cabin-ports/libpng` depends on
  `"cabin-ports/zlib" = "^1.3"`), and target `deps` references use the
  bare-package shorthand when the dependency exposes exactly one library-like target, or an
  explicit `package:target` selector otherwise.
- **Order.**  Publication order derives from the inter-port dependency edges (zlib publishes
  before libpng).

Both modes run the complete local preflight first: fetch or reuse every pinned upstream archive
(SHA-256 verified, reusing the standard `<cache>/ports` archive cache), run the standard safe
preparation pipeline (extraction, `strip_prefix`, `[[copy]]`, `patches`, overlay), package each conversion,
publish it into a temporary file registry, and build every port against the generated packages in
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
passes `--new-revision`, because a changed recipe that has been merged to `main` *is* the
deliberate respin (see [Packaging revisions](#packaging-revisions)); unchanged recipes still
produce byte-identical archives, so a rerun stays a no-op either way.

### Publish automation

The workflow `.github/workflows/ports-publish.yml` automates the tool.  Pull requests that touch
the recipes, the `cabin-port` pipeline, or the publisher run the complete `--dry-run` preflight
with no secrets; pushes to `main` with such changes publish the converted set to
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
tool re-hashes every cached archive against the recipe's pinned SHA-256 and re-downloads on a
mismatch.  The recipes and the workflow live in the `cabinpkg/cabin` repository; the
`cabin-ports` GitHub organization is the registry scope authority, not a separate source
repository.

### Packaging revisions

A published version always preserves the upstream version, and the bytes published under it are
immutable.  A recipe-only correction - an overlay fix, a `[[copy]]` addition, a new or revised
patch under `[source].patches` - therefore reaches the
registry as a new [packaging revision](package-index.md#packaging-revisions) of the same version:
the archive's changed bytes give it a new revision id, and both revisions stay listed and
fetchable.  There is no version-string marker and no sidecar file to maintain; edit the recipe and
merge it.

Consumers are unaffected until they ask to be.  A lockfile that already pins the old revision keeps
resolving and fetching exactly those bytes; a fresh resolution, or an explicit `cabin update`, picks
up the corrected revision.

Because the publisher always passes `--new-revision`, the merged recipe change *is* the intent
signal: a correction landing on `main` is by construction a deliberate respin, and a rerun over
unchanged recipes still produces byte-identical archives and stays a no-op.  A revision may not
change what resolution consumes, so a correction that alters the converted package's
`dependencies`, `features`, or `standards` is not a respin - it needs a new upstream version.
`links` is the one-way exception: a respin may stamp a claim table onto a version published
without one (that is how existing ports gained their identities), but changing or removing an
existing claim still needs a new upstream version.

## Retiring a foundation port

When an upstream project ships and maintains a native `cabin.toml`, the corresponding foundation
port should be retired.  The retirement steps are:

1. Switch downstream `[dependencies]` entries from `{ port = true, version = "..." }` or `{
   port-path = "../ports/<name>/<version>" }` to the appropriate `path` / `version` / `workspace`
   form pointing at the new upstream-maintained package.
2. Remove the corresponding entry from `BUILTIN` in `crates/cabin-port/src/builtin.rs`.
3. Delete the `crates/cabin-port/ports/<name>/<version>/` directory in the same commit.
4. Update `crates/cabin-port/ports/README.md`
   to remove the entry from the "Available ports" list.
5. Note the retirement in the relevant release notes.
