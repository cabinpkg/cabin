# Cargo-inspired interface

Cabin is *Cargo-inspired*, not Cargo-compatible. The interface
borrows Cargo's vocabulary where the semantics line up — so a
Cargo user picks Cabin up quickly — and deliberately diverges
where Cargo's terminology would be misleading for C/C++.

This page is the audit checkpoint for that contract: it lists
what Cabin adopts, what it renames, what it intentionally
leaves out, and where to look for the rule when in doubt.

## What Cabin adopts from Cargo

### Subcommands

| Cabin command | Cargo analogue | Notes |
|---|---|---|
| `cabin init` | `cargo init` | Single-package generator (current directory).  `--bin` / `--lib` select scaffold kind (binary by default).  See [`new-and-init.md`](new-and-init.md). |
| `cabin new` | `cargo new` | Single-package generator (new directory).  `--bin` / `--lib` select scaffold kind (binary by default).  See [`new-and-init.md`](new-and-init.md). |
| `cabin add` | `cargo add` | Adds a dependency to `cabin.toml`, editing the manifest format-preservingly. v1 covers foundation ports (`--port`) and local path dependencies (`--path`); bare registry names are rejected until a registry exists.  See [`dependency-kinds.md`](dependency-kinds.md). |
| `cabin remove` | `cargo remove` | Removes a `[dependencies]` (or, with `--dev`, `[dev-dependencies]`) entry from `cabin.toml`. |
| `cabin build` | `cargo build` | Plans + invokes Ninja |
| `cabin check` | `cargo check` | Reuses the build graph but compiles in syntax-only mode (`-fsyntax-only`; `/Zs` under MSVC); no objects or binaries |
| `cabin clean` | `cargo clean` | Removes Cabin-generated build artifacts |
| `cabin run` | `cargo run` | Builds and runs an exec target; `--` forwards args |
| `cabin test` | `cargo test` | Builds + runs `test` targets |
| `cabin fetch` | `cargo fetch` | Downloads + verifies registry artifacts |
| `cabin update` | `cargo update` | Re-resolves, refreshes lockfile |
| `cabin metadata` | `cargo metadata` | Deterministic JSON state |
| `cabin tree` | `cargo tree` | Resolved dependency tree |
| `cabin explain` | (no direct analogue) | Typed answers about the resolved graph |
| `cabin vendor` | `cargo vendor` | File-registry materialization |
| `cabin package` | `cargo package` | Source-archive + canonical metadata |
| `cabin publish` | `cargo publish` | Local file-registry publish (no remote yet) |
| `cabin fmt` | `cargo fmt` | Formats workspace C/C++ sources with `clang-format` |
| `cabin version` | `cargo version` | Prints Cabin's version; with `-v` adds release, commit-hash, commit-date, host, and OS fields when available. `cabin --version` keeps working as the concise framework spelling. |

### Flags / options

| Flag | Semantics | Cargo analogue |
|---|---|---|
| `-p`, `--package <name>` | Workspace package selection | identical |
| `--workspace` / `--exclude <name>` | Workspace-wide selection | identical |
| `--manifest-path <path>` | Manifest discovery | identical |
| `--features <names>` | Enable named features | identical |
| `--all-features` / `--no-default-features` | Feature selection | identical |
| `--release` | Compatibility alias for `--profile release` | identical |
| `--profile <name>` | Build profile | identical |
| `-j`, `--jobs <N>` | Number of parallel jobs for the build backend | identical |
| `--locked` / `--frozen` | Lockfile policy | identical |
| `--offline` | Forbid network access | identical |
| `--bin <name>` | Pick an `executable` to run (`cabin run` only) | matches Cargo's `cargo run --bin`; Cabin does *not* offer a Cargo-style `--target <name>` manifest-target selector on `cabin build` / `cabin test` (see below) |
| `--test <name>` | Run only the named `test` target(s) (`cabin test` only; repeatable) | matches Cargo's `cargo test --test` |
| `--color <when>` | Terminal-color choice (`auto` / `always` / `never`) | identical wording; Cabin's env-var spelling is `CABIN_TERM_COLOR` |
| `-v`, `--verbose` | Increase Cabin's status output volume; specify twice for very verbose output | identical |
| `-q`, `--quiet` | Suppress Cabin-owned status messages (errors are unaffected) | identical |
| `--list` | Print every subcommand (including distribution helpers hidden from `--help`) and exit | identical wording; `cabin --help` is curated for day-to-day use, `cabin --list` is the full directory |

### Environment variables

Read-side and write-side `CABIN_*` env vars mirror Cargo's
`CARGO_*` shape (see [`environment-variables.md`](environment-variables.md)
for the full list). Highlights:

- `CABIN_BUILD_DIR` analogous to `CARGO_TARGET_DIR`, but spelled
  with Cabin's own name to match the `--build-dir` flag.
- `CABIN_NET_OFFLINE` analogous to `CARGO_NET_OFFLINE`.
- `CABIN_TERM_COLOR` analogous to `CARGO_TERM_COLOR`.

## What Cabin deliberately diverges on

### `--build-dir` instead of `--target-dir`

Cabin keeps the word *target* for **platform / toolchain
targets** (`x86_64-unknown-linux-gnu`, …). Cargo's
`--target-dir` would be ambiguous in C/C++ where "target" also
means a manifest-declared library / executable target. The
build output directory flag is therefore `--build-dir`, the
config key is `[paths] build-dir`, and the env var is
`CABIN_BUILD_DIR`.

Default build directory: `build/`.

### No `--target` overload for manifest-target selection

Cargo overloads `--target` for binary selection in some
contexts. Cabin does not offer a `--target <name>`
manifest-target selector on any command.

Selection works through three other surfaces:

- **`cabin run --bin <name>`** picks an `executable` to
  build and run — the same shape Cargo uses for
  `cargo run --bin`.
- **`cabin test`** builds every `test` target in the
  selected packages by default; **`cabin test --test <name>`**
  (repeatable) narrows the run to the named `test` targets —
  the same shape Cargo uses for `cargo test --test`. Package
  selection narrows where the names are looked up.
- **`cabin build`** builds every default-buildable target
  (`library`, `header-only`, `executable`) in the
  selected packages. Dev-only kinds (`test`,
  `example`) are excluded from this default and reach the
  build graph only as transitive deps of a selected target.

Each explicit-kind selector uses a distinct flag name
(`--bin`, `--test`), keeping `--target` reserved for the
future platform / toolchain target.

### Per-language separation

Cabin keeps CC vs. CXX, and CFLAGS vs. CXXFLAGS (whether passed
via manifest, profile, or env vars) strictly separated.
Cargo's `RUSTFLAGS` is single-language by definition; C/C++ has
two language slots and they do not share argv space. The
fingerprint distinguishes the two, so a flag moved between
slots has a distinct build identity even when the argv string
is identical.

### Package-metadata as compile-time macros

Cargo exposes `CARGO_PKG_*` to Rust code as compile-time env
vars through `env!()`. C/C++ has no `env!()`: turning package
metadata into compiler `-D` flags by default would (a) leak
into public headers, (b) change ABI when the version bumps,
and (c) churn every affected object file. Cabin's default is
therefore **no automatic `-DCABIN_PACKAGE_*` macros**.
`cabin run` and `cabin test` receive a scoped subset of the
metadata as env vars; compile commands stay clean. Explicit
opt-in macros may be added in a future change behind a manifest
flag, with the orchestration layer threading them through the
build-configuration fingerprint so a future binary-artifact
cache can key on the changed inputs.

### `cabin explain`

`cabin explain` has no direct Cargo equivalent. It exposes a
typed query model (`package`, `target`, `source`, `feature`,
`build-config`) so users can ask "why is this package /
target / feature / configuration in the resolved state?"
without re-deriving it by hand. See
[`metadata-tree-explain.md`](metadata-tree-explain.md).

## What Cabin intentionally does not have

These are Cargo / Rust concepts that do not (yet) translate to
Cabin's C/C++ scope:

- `cargo doc` — Cabin has no doc generator yet; rustdoc has no
  C/C++ equivalent that would justify the surface.
- `cargo install` — install / prefix semantics are not designed.
- `cargo search` / `cargo login` / `cargo logout` /
  `cargo owner` / `cargo yank` — registry-server work is out
  of scope until a Cabin registry server exists.
- `cargo rustc` / `cargo rustdoc` / `cargo fix` — Rust-specific.
- `cargo bench` — Cabin has no benchmark target kind and no
  benchmark harness model. Users who need to time a binary
  declare an `executable` and run it themselves.
- Doctest / book / fix / clippy / miri analogues.

If a later iteration wants to add one of these, it should
land alongside an explicit motivation rather than as an
opportunistic addition.

## Where the audit lives

The full audit checklist is captured by the Cargo-inspired
interface section in [`architecture.md`](architecture.md).  When
this page changes, that section in the architecture document
should be updated in the same commit.
