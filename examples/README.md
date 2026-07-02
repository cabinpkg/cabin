# Cabin examples

User-facing runnable Cabin example projects, one per subdirectory.  Each example has its own
`cabin.toml`, sources, and `README.md`.

## What lives here vs. elsewhere

- `examples/` (this directory) - **user-facing Cabin (C/C++) example projects.** Each one is a real
  Cabin package that you can `cd` into and run `cabin build` against.  Cargo does not look at this
  directory.
- `crates/<name>/examples/` - **Cargo example targets for the Rust crates.** None exist today; this
  is where they would go if added.
- `crates/cabin-port/ports/` - **curated foundation ports.** Cabin recipes that adapt real upstream
  C/C++ libraries that do not yet ship a native `cabin.toml`.  Not example projects; see
  [`../crates/cabin-port/ports/README.md`](../crates/cabin-port/ports/README.md).

## Available examples

| Directory | What it demonstrates |
|---|---|
| [`hello-c/`](hello-c) | Smallest useful C project: one `executable` target with a `.c` source. |
| [`hello-cpp/`](hello-cpp) | Smallest useful C++ project: one `executable` target with a `.cc` source. |
| [`library-and-app/`](library-and-app) | A library target consumed by an executable target in the same package, with `include-dirs` propagation. |
| [`library-with-tests/`](library-with-tests) | A library plus two `test` targets, run with `cabin test`.  The example to read for unit testing. |
| [`header-only-lib/`](header-only-lib) | Authoring a `header-only` target (include-dirs, nothing compiled) consumed by an executable in the same package. |
| [`workspace-basic/`](workspace-basic) | A virtual workspace root with two members (`util` library, `cli` executable depending on `util` via a path dependency). |
| [`zlib-usage/`](zlib-usage) | Consuming the curated zlib foundation port from [`crates/cabin-port/ports/zlib/`](../crates/cabin-port/ports/zlib). |
| [`cjson-usage/`](cjson-usage) | Consuming the curated cJSON foundation port from [`crates/cabin-port/ports/cJSON/`](../crates/cabin-port/ports/cJSON). |
| [`xxhash-usage/`](xxhash-usage) | Consuming the curated xxHash foundation port from [`crates/cabin-port/ports/xxhash/`](../crates/cabin-port/ports/xxhash). |
| [`tinyxml2-usage/`](tinyxml2-usage) | Consuming the curated tinyxml2 C++ foundation port from [`crates/cabin-port/ports/tinyxml2/`](../crates/cabin-port/ports/tinyxml2). |
| [`sqlite3-usage/`](sqlite3-usage) | Consuming the curated SQLite foundation port (amalgamation) from [`crates/cabin-port/ports/sqlite3/`](../crates/cabin-port/ports/sqlite3), including a `single-threaded` feature. |
| [`libpng-usage/`](libpng-usage) | Consuming the curated libpng foundation port from [`crates/cabin-port/ports/libpng/`](../crates/cabin-port/ports/libpng), which itself depends transitively on the bundled zlib port. |
| [`fmt-usage/`](fmt-usage) | Consuming the curated {fmt} C++ foundation port from [`crates/cabin-port/ports/fmt/`](../crates/cabin-port/ports/fmt). |
| [`spdlog-usage/`](spdlog-usage) | Consuming the curated spdlog header-only C++ foundation port from [`crates/cabin-port/ports/spdlog/`](../crates/cabin-port/ports/spdlog). |
| [`googletest-usage/`](googletest-usage) | A `test` target linking the curated GoogleTest foundation port from [`crates/cabin-port/ports/googletest/`](../crates/cabin-port/ports/googletest), run with `cabin test`. |
| [`catch2-usage/`](catch2-usage) | A `test` target linking the curated Catch2 foundation port (amalgamation, port-supplied `main`) from [`crates/cabin-port/ports/catch2/`](../crates/cabin-port/ports/catch2), run with `cabin test`. |
| [`nlohmann-json-usage/`](nlohmann-json-usage) | Consuming the curated header-only nlohmann_json foundation port from [`crates/cabin-port/ports/nlohmann_json/`](../crates/cabin-port/ports/nlohmann_json). |
| [`cli11-usage/`](cli11-usage) | Consuming the curated header-only CLI11 foundation port from [`crates/cabin-port/ports/CLI11/`](../crates/cabin-port/ports/CLI11). |
| [`miniz-usage/`](miniz-usage) | Consuming the curated miniz foundation port (zip-sourced amalgamation) from [`crates/cabin-port/ports/miniz/`](../crates/cabin-port/ports/miniz). |
| [`stb-usage/`](stb-usage) | Consuming the curated header-only stb foundation port (implementation-macro pattern) from [`crates/cabin-port/ports/stb/`](../crates/cabin-port/ports/stb). |
| [`uthash-usage/`](uthash-usage) | Consuming the curated header-only uthash foundation port from [`crates/cabin-port/ports/uthash/`](../crates/cabin-port/ports/uthash). |
| [`inih-usage/`](inih-usage) | Consuming the curated inih C foundation port from [`crates/cabin-port/ports/inih/`](../crates/cabin-port/ports/inih). |
| [`picohttpparser-usage/`](picohttpparser-usage) | Consuming the curated picohttpparser C foundation port from [`crates/cabin-port/ports/picohttpparser/`](../crates/cabin-port/ports/picohttpparser). |
| [`unit-test-gtest/`](unit-test-gtest) | A library unit-tested with GoogleTest through `cabin test`: a fixture, value assertions, and exception assertions.  The example to read for framework-based testing. |
| [`json-cli/`](json-cli) | A JSON round trip on the header-only nlohmann_json port: parse a document, read typed values, emit a derived summary. |
| [`sqlite-todo/`](sqlite-todo) | An in-memory todo list on the sqlite3 port: DDL/DML through `sqlite3_exec`, then a prepare/step/finalize query loop. |
| [`platform-cfg/`](platform-cfg) | Per-platform `[target.'cfg(...)']` defines: one source that compiles a different macro on Windows (MSVC) vs.  Unix (GCC/Clang). |

## Running an example manually

```sh
cd examples/hello-cpp
cabin build
cabin run
```

(`cabin run` builds and launches the package's `executable` target.  Each example's README spells
out the exact command if it differs.)

## Running every example's tests through Cargo

The repository ships integration tests that build and run each example using the in-tree `cabin`
binary.  From the repository root:

```sh
cargo test --test cabin_examples
```

The tests copy each example into a temporary directory before building, so the source tree never
accumulates build output.  Tests skip cleanly when Ninja or a C/C++ compiler is missing; the
foundation-port example tests additionally skip when `CABIN_NET_OFFLINE` is set or when the host
cannot reach the archive host - `github.com:443` for most ports, `www.sqlite.org:443` for sqlite3,
and `downloads.sourceforge.net:443` for libpng.
