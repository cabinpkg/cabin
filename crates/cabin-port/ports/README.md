# Cabin foundation ports

This directory holds **curated foundation ports**: Cabin recipes that adapt important existing C/C++
libraries - libraries that do not yet ship a native `cabin.toml` - to Cabin's build model.

A foundation port consists of:

- `port.toml` - pins a single upstream release archive by URL and SHA-256, optionally with a
  `strip_prefix` for the archive's root directory, and optionally one or more `[[copy]]` steps (see
  below).
- `cabin.toml` - a Cabin overlay manifest that describes the upstream sources as ordinary Cabin
  C/C++ targets.

When a Cabin package declares a bundled dependency (`{ port = true, version = "^1.3" }`) or a
local-recipe dependency (`{ port-path = "../ports/<name>/<version>" }`), Cabin downloads the
archive, verifies the SHA-256, safely extracts it, applies any `[[copy]]` steps, copies the overlay
manifest into the extracted source tree, and treats the result as a normal Cabin path dependency.  A
port's overlay may itself depend on another port (libpng depends on the bundled zlib), and discovery
follows those edges transitively.

### Placing prebuilt files with `[[copy]]`

Some libraries ship a build-time file under a name the compiler does not expect - for example,
libpng ships its configuration as `scripts/pnglibconf.h.prebuilt`, which its build normally copies
to `pnglibconf.h`.  A port may declare that placement declaratively:

```toml
[[copy]]
from = "scripts/pnglibconf.h.prebuilt"
to = "pnglibconf.h"
```

Each step copies one already-present file in the extracted source to a second in-tree location.
Both paths are validated as relative paths inside the source tree, the copy runs after extraction
and before the overlay is applied (so the overlay `cabin.toml` always wins), and the source file is
covered by the archive's pinned SHA-256.  This is a **static file copy**, not a build script: it
cannot run commands, generate content, or read anything outside the extracted archive.

Recipes under this directory are also embedded in the `cabin` binary via `cabin-port::builtin` (see
`crates/cabin-port/src/builtin.rs`).  Retiring a specific *version* of a foundation port removes
`ports/<name>/<version>/` and the matching `BUILTIN` entry in `cabin-port::builtin`.  Retiring an
entire port (all versions) removes the whole `ports/<name>/` directory and every same-name `BUILTIN`
entry.

## What ports are not

- They are **not Cabin's public registry**.
- They are **not a submission queue** for arbitrary C/C++ libraries; this directory is curated.
- They are **not** a mechanism for distributing pre-built binaries or compiled artifacts.
- They are **not** a workaround for missing build-script support - they only describe libraries
  whose source layout fits Cabin's existing target model (a list of sources plus include
  directories), optionally placing a prebuilt file with a static `[[copy]]` step.  Ports that need
  code generation or a configure run do not belong here.

## Policy

- Sources must be pinned by URL and SHA-256.  Floating references (`latest`, branches, tag-only
  without integrity) are rejected.
- No upstream build-system invocation.  Cabin never runs CMake, Autotools, Meson, Make, or upstream
  `configure` scripts.
- Patches under `patches/` (if any) should be limited to what is strictly required to make a port
  build through Cabin.
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
  on the bundled zlib port and places its prebuilt `pnglibconf.h` with a `[[copy]]` step.
- [`miniz/3.1.2/`](miniz/3.1.2/) - the miniz single-file zlib-replacement compression library
  (upstream amalgamated release zip), version 3.1.2.
- [`nlohmann_json/3.12.0/`](nlohmann_json/3.12.0/) - JSON for Modern C++ (header-only), version
  3.12.0.
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
