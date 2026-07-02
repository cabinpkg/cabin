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

A port is a directory containing two files:

```
crates/cabin-port/ports/<name>/<version>/
  port.toml - recipe (pinned source archive + identity)
  cabin.toml - overlay manifest (describes the upstream
                   sources as a Cabin C/C++ target)
```

For example, zlib 1.3.1 lives at `crates/cabin-port/ports/zlib/1.3.1/`.

Optional `patches/` may be added later if a port needs one; this milestone ships no
patch-application code.

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
```

## Depending on a foundation port

A downstream package opts in via either the bundled form (`port = true`, resolved by name against
the embedded set) or the filesystem form (`port-path`, pointing at a recipe directory):

```toml
[dependencies]
zlib = { port = true, version = "^1.3" }  # bundled recipe
# -- or for local development --
zlib = { port-path = "../ports/zlib/1.3.1" }
```

`port = true` requires a sibling `version = "<requirement>"` field (see "Bundled ports" above).
`port-path` is mutually exclusive with `version` - the recipe at the path supplies the version.
Both forms are mutually exclusive with `path`, `workspace`, and `system`.  Both **do** honor
`features` and `default-features` - a port's overlay can declare a `[features]` table, and the
feature resolver threads per-edge feature requests onto the prepared port package exactly as it does
for a path dependency (so, e.g., `sqlite3 = { port = true, version = "^3", features =
["single-threaded"] }` enables that feature on the bundled recipe).  `optional` is still rejected on
port dependencies with a typed error, because the port forms never enter the version resolver that
optional gating drives.

## Preparation pipeline

When Cabin runs a workspace-loading command, the CLI orchestrates preparation **before** the
workspace loader sees the manifest:

1. Walk the manifest tree to discover every reachable port dependency.  A port's own overlay may
   declare further port dependencies (libpng depends on the bundled zlib), so discovery recurses
   into each port's overlay and follows those edges transitively.  The walk stays network-free: a
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
5. Safely extract the archive into the port cache with the declared `strip_prefix`.  This step
   reuses `cabin-artifact`'s extraction primitives (tar.gz or zip, chosen by the URL's path
   extension), so the decompression-bomb caps, symlink rejection, and path-traversal protection
   apply identically to both formats.
6. Apply any `[[copy]]` steps, placing prebuilt files under their build-time names inside the
   extracted tree.
7. Copy the overlay manifest into the extracted source dir as `cabin.toml`.
8. Cross-check the overlay's `[package]` identity against `port.toml`.  Mismatch surfaces an
   explicit error.
9. Drop a sibling `.ok` completion marker so the next invocation can reuse the prepared directory
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
- Not a submission queue.  New foundation ports require a curated review; this directory is
  intentionally small.
- Not a vehicle for binary distribution.  Only source archives are supported.
- Not a workaround for missing build-script support.  Ports describe libraries whose source layout
  already fits Cabin's target model (a fixed list of sources plus include directories), optionally
  placing a prebuilt file with a static `[[copy]]` step.  Libraries that need configure-time
  generation, CMake / Meson / Autotools driving, or custom build commands are out of scope.
- Limited feature surface.  The `port` dependency form honors `features` / `default-features` (a
  port overlay can declare a `[features]` table; see sqlite's `single-threaded` feature), but it
  does not support optional gating or shared/static variant selection.

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
| `overlay manifest declares package \`<actual>\`` | The overlay's `[package]` identity disagrees with `port.toml`. |
| `cannot download port \`<name>\` because --offline was specified` | A remote URL was reached while running in offline mode. |
| `cannot prepare port \`<name>\` because --frozen was specified and the port is not cached` | The cache does not already hold a prepared copy under `--frozen`. |
| `foundation-port dependency <name> declared by package <parent> has not been prepared` | Internal invariant violation: the CLI orchestration layer did not run before the workspace loader. |
| `foundation-port directory <port_dir> does not exist` | The consumer's `port-path = "..."` value does not resolve to an existing directory. |

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
