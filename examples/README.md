# Cabin examples

User-facing runnable Cabin example projects, one per subdirectory.  Each example has its own
`cabin.toml`, sources, and `README.md`.

## What lives here vs. elsewhere

- `examples/` (this directory) - **user-facing Cabin (C/C++) example projects.** Each one is a real
  Cabin package that you can `cd` into and run `cabin build` against.  Cargo does not look at this
  directory.
- `crates/<name>/examples/` - **Cargo example targets for the Rust crates.** None exist today; this
  is where they would go if added.
- `crates/cabin-port/ports/` - **curated foundation ports.** Port directories that adapt real
  upstream C/C++ libraries that do not yet ship a native `cabin.toml`, and the source the
  `cabin-ports/*` registry packages these examples depend on are published from.  Most are
  committed as provenance-bearing packages; the rest are still recipes while the collapse
  lands.  Not example projects; see
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
| [`workspace-app-and-lib/`](workspace-app-and-lib) | A workspace whose internal `greeter` library depends on the `cabin-ports/fmt` registry package; the `app` member reaches fmt transitively through its path dependency. |
| [`feature-gated-targets/`](feature-gated-targets) | A feature-gated optional library: `required-features` on `[target.tls]`, the `tls` feature enabled on the dependency edge, and explicit `deps = ["netlib:net", "netlib:tls"]` links. |
| [`zlib-usage/`](zlib-usage) | Consuming the `cabin-ports/zlib` registry package. |
| [`cjson-usage/`](cjson-usage) | Consuming the `cabin-ports/cjson` registry package. |
| [`xxhash-usage/`](xxhash-usage) | Consuming the `cabin-ports/xxhash` registry package. |
| [`tinyxml2-usage/`](tinyxml2-usage) | Consuming the `cabin-ports/tinyxml2` C++ registry package. |
| [`sqlite3-usage/`](sqlite3-usage) | Consuming the `cabin-ports/sqlite3` registry package (SQLite amalgamation), including a `single-threaded` feature. |
| [`libpng-usage/`](libpng-usage) | Consuming the `cabin-ports/libpng` registry package, which itself depends transitively on `cabin-ports/zlib`. |
| [`fmt-usage/`](fmt-usage) | Consuming the `cabin-ports/fmt` C++ registry package. |
| [`spdlog-usage/`](spdlog-usage) | Consuming the header-only `cabin-ports/spdlog` C++ registry package. |
| [`googletest-usage/`](googletest-usage) | A `test` target linking the `cabin-ports/googletest` registry package, run with `cabin test`. |
| [`catch2-usage/`](catch2-usage) | A `test` target linking the `cabin-ports/catch2` registry package (amalgamation, package-supplied `main`), run with `cabin test`. |
| [`nlohmann-json-usage/`](nlohmann-json-usage) | Consuming the header-only `cabin-ports/nlohmann_json` registry package. |
| [`cli11-usage/`](cli11-usage) | Consuming the header-only `cabin-ports/cli11` registry package. |
| [`miniz-usage/`](miniz-usage) | Consuming the `cabin-ports/miniz` registry package (zip-sourced amalgamation). |
| [`stb-usage/`](stb-usage) | Consuming the header-only `cabin-ports/stb` registry package (implementation-macro pattern). |
| [`uthash-usage/`](uthash-usage) | Consuming the header-only `cabin-ports/uthash` registry package. |
| [`inih-usage/`](inih-usage) | Consuming the `cabin-ports/inih` C registry package. |
| [`picohttpparser-usage/`](picohttpparser-usage) | Consuming the `cabin-ports/picohttpparser` C registry package. |
| [`cli-with-spdlog/`](cli-with-spdlog) | A CLI app combining three `cabin-ports` packages - CLI11 flags, {fmt} formatting, spdlog logging - including the `SPDLOG_FMT_EXTERNAL` opt-in to the external fmt package. |
| [`unit-test-gtest/`](unit-test-gtest) | A library unit-tested with GoogleTest through `cabin test`: a fixture, value assertions, and exception assertions.  The example to read for framework-based testing. |
| [`json-cli/`](json-cli) | A JSON round trip on the header-only `cabin-ports/nlohmann_json` package: parse a document, read typed values, emit a derived summary. |
| [`sqlite-todo/`](sqlite-todo) | An in-memory todo list on the `cabin-ports/sqlite3` package: DDL/DML through `sqlite3_exec`, then a prepare/step/finalize query loop. |
| [`png-info/`](png-info) | An in-memory PNG encode/decode roundtrip on the `cabin-ports/libpng` package, pushing real image data across the transitive `libpng -> zlib` C package edge. |
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
accumulates build output.  Tests that compile real sources fail with a clear message when Ninja or
a C/C++ compiler is missing.  The tests
for the examples consuming `cabin-ports/*` packages are `#[ignore]`d because they need outbound
network; run them with `cargo test --test cabin_examples -- --ignored`, which stages the committed
ports into a local file registry through the publisher pipeline - downloading the pinned upstream
archives - and builds each example against that registry with `--index-path`.
