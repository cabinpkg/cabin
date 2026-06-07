# Cabin examples

User-facing runnable Cabin example projects, one per subdirectory.
Each example has its own `cabin.toml`, sources, and `README.md`.

## What lives here vs. elsewhere

- `examples/` (this directory) — **user-facing Cabin (C/C++) example
  projects.** Each one is a real Cabin package that you can `cd` into
  and run `cabin build` against. Cargo does not look at this
  workspace-root directory.
- `crates/<name>/examples/` — **Cargo example targets for the Rust
  crates.** None exist today; this is where they would go if added.
- `crates/cabin-port/ports/` — **curated foundation ports.** Cabin
  recipes that adapt real upstream C/C++ libraries that do not yet ship
  a native `cabin.toml`. Not example projects; see
  [`../crates/cabin-port/ports/README.md`](../crates/cabin-port/ports/README.md).

## Available examples

| Directory | What it demonstrates |
|---|---|
| [`hello-c/`](hello-c) | Smallest useful C project: one `executable` target with a `.c` source. |
| [`hello-cpp/`](hello-cpp) | Smallest useful C++ project: one `executable` target with a `.cc` source. |
| [`library-and-app/`](library-and-app) | A library target consumed by an executable target in the same package, with `include_dirs` propagation. |
| [`library-with-tests/`](library-with-tests) | A library plus two `test` targets, run with `cabin test`. The example to read for unit testing. |
| [`workspace-basic/`](workspace-basic) | A virtual workspace root with two members (`util` library, `cli` executable depending on `util` via a path dependency). |
| [`zlib-usage/`](zlib-usage) | Consuming the curated zlib foundation port from [`crates/cabin-port/ports/zlib/`](../crates/cabin-port/ports/zlib). |
| [`cjson-usage/`](cjson-usage) | Consuming the curated cJSON foundation port from [`crates/cabin-port/ports/cJSON/`](../crates/cabin-port/ports/cJSON). |
| [`platform-cfg/`](platform-cfg) | Per-platform `[target.'cfg(...)']` defines: one source that compiles a different macro on Windows (MSVC) vs. Unix (GCC/Clang). |

## Running an example manually

```sh
cd examples/hello-cpp
cabin build
cabin run
```

(`cabin run` builds and launches the package's `executable` target.
Each example's README spells out the exact command if it differs.)

## Running every example's tests through Cargo

The repository ships integration tests that build and run each
example using the in-tree `cabin` binary. From the repository root:

```sh
cargo test --test cabin_examples
```

The tests copy each example into a temporary directory before
building, so the source tree never accumulates build output. Tests
skip cleanly when Ninja or a C/C++ compiler is missing; the
`zlib-usage` test additionally skips when `CABIN_NET_OFFLINE` is set
or when the host cannot reach `github.com:443` (the source of the
zlib foundation port archive).
