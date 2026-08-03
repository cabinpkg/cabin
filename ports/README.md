# Cabin foundation ports

This directory holds **curated foundation ports**: Cabin packages that adapt important existing
C/C++ libraries - libraries that do not yet ship a native `cabin.toml` - to Cabin's build model.

Every `<name>/<version>/` directory is a single `cabin.toml` carrying the canonical scoped identity
(`cabin-ports/<name>`) and a complete `[package.upstream]` block, plus its `patches/` when it
declares any.  It publishes verbatim: editing a byte here - a comment included - changes the
published archive.  See [`docs/foundation-ports.md`](../docs/foundation-ports.md).

Ports reach consumers as published `cabin-ports/*` registry packages; nothing in this directory is
consumed from a user's manifest directly.  To publish one, the tool downloads the archive, verifies
the SHA-256, safely extracts it, applies the declared `[[package.upstream.copy]]` steps and then
the declared `[package.upstream].patches` as byte-exact unified diffs.

### Placing prebuilt files with `[[package.upstream.copy]]`

Some libraries ship a build-time file under a name the compiler does not expect - for example,
libpng ships its configuration as `scripts/pnglibconf.h.prebuilt`, which its build normally copies
to `pnglibconf.h`.  A port declares that placement declaratively:

```toml
[[package.upstream.copy]]
from = "scripts/pnglibconf.h.prebuilt"
to = "pnglibconf.h"
```

Each step copies one already-present file in the extracted source to a second in-tree location.
Both paths are validated as relative paths inside the source tree, the copy runs after extraction
and before the manifest is written (so the committed `cabin.toml` always wins), and the source file
is covered by the archive's pinned SHA-256.  This is a **static file copy**, not a build script: it
cannot run commands, generate content, or read anything outside the extracted archive.

### Correcting sources with `[package.upstream].patches`

Small corrections that no copy step can express - build-system fixes, portability tweaks - are
declared as unified-diff files under the port's `patches/` directory:

```toml
[package.upstream]
# ... url / sha256 / format / strip-prefix ...
patches = ["patches/0001-fix-msvc-build.patch"]
```

Patches apply in declared order, after every copy step, and application is byte-exact:
fixed `-p1` strip, no fuzz, no offset search, no newline normalization, text diffs only.  Like
the copy steps, this is declarative transformation, never a script; a patch that does not apply
fails the materialization.  See [`foundation-ports.md`](../docs/foundation-ports.md) for the
full rules.

Retiring a specific *version* of a foundation port removes `ports/<name>/<version>/`; retiring an
entire port removes the whole `ports/<name>/` directory.

## What ports are not

- They are **not Cabin's public registry**.
- They are **not a submission queue** for arbitrary C/C++ libraries; this directory is curated.
- They are **not** a mechanism for distributing pre-built binaries or compiled artifacts.
- They are **not** a workaround for missing build-script support - they only describe libraries
  whose source layout fits Cabin's existing target model (a list of sources plus include
  directories), optionally placing a prebuilt file with a static `[[package.upstream.copy]]` step
  or correcting one with a declared byte-exact patch.  Ports that need code generation or a configure run do not
  belong here.

## Policy

- Sources must be pinned by URL and SHA-256.  Floating references (`latest`, branches, tag-only
  without integrity) are rejected.
- No upstream build-system invocation.  Cabin never runs CMake, Autotools, Meson, Make, or upstream
  `configure` scripts.
- Patches under `patches/` (if any) should be limited to what is strictly required to make a port
  build through Cabin: build-system fixes and portability tweaks, never feature changes or
  divergence from upstream behavior.
- A `library` target that embodies a well-known native library claims its identity with
  [`links`](../docs/manifest.md#links) (`zlib` claims `"z"`), judged against upstream
  conventions.  Header-only ports claim nothing, and an alternative implementation with its own
  symbol namespace claims its own identity (`miniz` claims `"miniz"`, not `"z"`).
- A foundation port should be **retired** once its upstream project ships and maintains a native
  `cabin.toml`.

## Available ports

- [`catch2/3.15.1/`](catch2/3.15.1/) - the Catch2 modern C++ test framework (upstream
  amalgamation, default main plus a `custom-main` feature), version 3.15.1.
- [`cJSON/1.7.18/`](cJSON/1.7.18/) - the cJSON ultralightweight JSON parser, version 1.7.18.
- [`CLI11/2.6.2/`](CLI11/2.6.2/) - the CLI11 command line parser for C++11 and beyond
  (header-only), version 2.6.2.
- [`fmt/12.2.0/`](fmt/12.2.0/) - the {fmt} fast and safe C++ formatting library, version 12.2.0.
- [`googletest/1.17.0/`](googletest/1.17.0/) - Google's C++ testing framework (GoogleTest library
  only, no gtest_main / GoogleMock), version 1.17.0.
- [`inih/62.0.0/`](inih/62.0.0/) - the inih simple INI file parser in C (C core only; upstream tag
  r62 spelled as SemVer), version 62.0.0.
- [`libpng/1.6.50/`](libpng/1.6.50/) - the official PNG reference library, version 1.6.50.  Depends
  on `"cabin-ports/zlib"` and places its prebuilt `pnglibconf.h` with a
  `[[package.upstream.copy]]` step.
- [`miniz/3.1.2/`](miniz/3.1.2/) - the miniz single-file zlib-replacement compression library
  (upstream amalgamated release zip), version 3.1.2.
- [`nlohmann_json/3.12.0/`](nlohmann_json/3.12.0/) - JSON for Modern C++ (header-only), version
  3.12.0.
- [`picohttpparser/2026.4.6/`](picohttpparser/2026.4.6/) - the picohttpparser tiny HTTP
  request/response parser in C (commit-pinned, date-versioned - upstream publishes no releases),
  snapshot 2026.4.6.
- [`spdlog/1.17.0/`](spdlog/1.17.0/) - the spdlog fast C++ logging library (header-only form),
  version 1.17.0.
- [`sqlite3/3.53.2/`](sqlite3/3.53.2/) - the SQLite self-contained SQL database engine
  (amalgamation), version 3.53.2.
- [`stb/2026.4.15/`](stb/2026.4.15/) - the stb single-file public domain libraries (header-only;
  commit-pinned, date-versioned - stb publishes no releases), snapshot 2026.4.15.
- [`tinyxml2/11.0.0/`](tinyxml2/11.0.0/) - the tinyxml2 small, efficient C++ XML parser, version
  11.0.0.
- [`uthash/2.4.0/`](uthash/2.4.0/) - the uthash hash table and container macros for C structures
  (header-only), version 2.4.0.
- [`xxhash/0.8.3/`](xxhash/0.8.3/) - the xxHash extremely fast non-cryptographic hash algorithm,
  version 0.8.3.
- [`zlib/1.3.1/`](zlib/1.3.1/) - the zlib compression library, version 1.3.1.

## Publishing ports as `cabin-ports/*` registry packages

The repository tool `xtask-port-publish` (`crates/xtask-port-publish`; not part of the shipped
`cabin` binary) publishes every port in this directory as an ordinary registry package under the
`cabin-ports` scope.

Every committed port is published **verbatim**: its `cabin.toml` already carries the canonical
identity, provenance, and target names, so nothing is rewritten.  Its sources are materialized
through the same `cabin-artifact` pipeline the registry verifier replays, and it reaches another
port through an ordinary scoped registry dependency.

```console
$ cargo build -p cabinpkg
$ cargo port-publish --dry-run     # full local preflight, no remote mutation
$ cargo port-publish --publish --index-url https://registry.cabinpkg.com
```

CI automates the tool (`.github/workflows/ports-publish.yml`): pull requests touching this
directory or the publisher run the complete `--dry-run` preflight, pushes to `main` publish to
`https://registry.cabinpkg.com`, and manual dispatch from `main` republishes the full set (the
recovery path after a pre-launch registry wipe; dispatching any other ref runs the dry-run).

Published bytes are immutable, and the published version is always the upstream version.  A
packaging-only correction to an already-published version therefore reaches the registry as a new
*packaging revision* of that version: edit the port's committed `cabin.toml` (or a file under its
`patches/`) and merge it - the changed archive bytes give the revision its identity, and both
revisions stay listed and fetchable.  A package directory publishes verbatim, so a comment is a
byte change like any other.  There is no sidecar file
and no version-string marker.  Consumers keep their existing pin until they run `cabin update`.

A revision must not change what resolution consumes, so a correction that alters the published
package's dependencies, features, or language-standard table is not a respin - it needs a new
upstream version.  See `docs/foundation-ports.md`, "Publishing ports as registry packages", for the
full behavior.
