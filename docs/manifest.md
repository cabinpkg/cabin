# `cabin.toml` Reference

This document describes the `cabin.toml` schema currently understood
by the manifest parser, the workspace loader, the build planner, the
resolver, and the artifact layer.

Registry packages declared with versioned dependencies must, after
fetch and extraction, contain a valid `cabin.toml` at the archive
root. `cabin-artifact` rejects an extracted package whose
`[package].name` or `[package].version` disagrees with the resolved
entry. See [`artifacts.md`](artifacts.md) for the source-archive
contract.

## Top-level structure

A manifest may contain these top-level sections:

- at most one `[package]` table
- zero or more `[target.<name>]` tables
- zero or more `[target.'cfg(...)'.<kind>]` conditional dependency,
  toolchain, or profile tables
- zero or one `[dependencies]` table
- zero or one `[build-dependencies]` table
- zero or one `[dev-dependencies]` table
- at most one `[workspace]` table
- at most one `[features]` table
- at most one `[options]` table
- at most one `[variants]` table
- at most one `[profile]` table plus `[profile.<name>]` tables
- at most one `[toolchain]` table
- at most one `[patch]` table
- at most one `[lint]` table

A manifest must contain at least one of `[package]` and `[workspace]`.
Package-specific tables such as targets, dependencies, features,
options, variants, and lint settings require `[package]`.
Workspace policy tables such as `[workspace]`, `[profile]`,
`[toolchain]`, and `[patch]` may appear on a workspace root
without `[package]`.

```toml
[package]
name = "my-project"
version = "0.1.0"

[dependencies]
greet = { path = "../greet" }
fmt = ">=10.0.0 <11.0.0"

[target.my-app]
type = "cpp_executable"
sources = ["src/main.cc"]
deps = ["greet", "fmt"]
```

## `[package]`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `name` | string | yes | Package name. Must be non-empty and contain no whitespace. |
| `version` | string | yes | Valid [SemVer](https://semver.org/) string. |

Cabin's language semantics live at the target and source level
(target kinds, per-source classification, toolchain selection).
`[package]` does not carry a language field; declaring one is
rejected as an unknown field. See
[Targets](targets.md) for how C and C++ are picked per target.

## `[target.<name>]`

The table key (`<name>`) is the target name. Target names must be
non-empty, must not contain whitespace, must consist only of ASCII
letters, digits, `_`, `-`, and `.`, must not start with `.` or `-`,
must not be `.` or `..`, and must be unique within the manifest.

| Field | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `type` | string | yes | — | Target kind. One of `cpp_library`, `cpp_header_only`, `cpp_executable`, `cpp_test`, `cpp_example`, `rust_library`, `rust_executable`. `rust_executable` is parsed but not buildable yet. |
| `sources` | array of strings | no | `[]` | Source files, relative to the manifest directory (no `..`). |
| `include_dirs` | array of strings | no | `[]` | Additional include directories, relative to the manifest directory. |
| `defines` | array of strings | no | `[]` | Preprocessor definitions, e.g. `"FOO=1"`. |
| `deps` | array of strings | no | `[]` | Target dependencies. See [Target dependencies](#target-dependencies). |

`include_dirs` of a `cpp_library` or `cpp_header_only` target are
visible (transitively) to any target that depends on it.

### Rust-target fields (`type = "rust_library"`)

These fields are valid only when `type = "rust_library"`. Putting
any of them on a non-Rust target is rejected.

| Field | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `manifest_path` | string | yes | — | Path to a `Cargo.toml`, relative to the Cabin package root. Absolute paths and `..` components are rejected. |
| `crate_type` | string | no | `"staticlib"` | Cargo crate type. Only `"staticlib"` is supported; other values (including `"cdylib"`) are rejected. |
| `crate_name` | string | no | inferred from target name | Override the crate name used to predict the staticlib filename. Hyphens are normalised to underscores. |
| `features` | array of strings | no | `[]` | Forwarded to `cargo build --features <comma-separated>`. |
| `default_features` | bool | no | `true` | When `false`, Cabin passes `--no-default-features` to Cargo. |

A C++ target that lists a `rust_library` in its `deps` automatically
links the produced staticlib. Rust targets must not declare a C++
target in their `deps`. See
[`docs/rust-interop.md`](rust-interop.md) for the full protocol,
limitations, and troubleshooting.

## `[dependencies]`

```toml
[dependencies]
# Local path dependency
greet = { path = "../greet" }

# Versioned dependency, string form
fmt = ">=10.0.0 <11.0.0"

# Versioned dependency, table form
spdlog = { version = "^1.13.0" }
```

Each entry declares a package-level dependency. The dependency value is
either:

- a **string** — interpreted as a SemVer requirement;
- a **table** — must specify exactly one of `path` or `version`. Other
  keys (`git`, `features`, `optional`, …) are rejected.

The dependency *key* (`greet`, `fmt`, `spdlog` above) must equal the
depended-on package's `[package].name` (path deps) or the registry
package name (version deps). Aliases are not supported.

### Version requirement syntax

Cabin uses the [`semver` crate](https://crates.io/crates/semver) for
parsing, with one extra convenience: comparators may be separated by
whitespace as well as by commas. Recognised forms:

- exact / compatible: `=1.2.3`, `1.2.3` (treated as `^1.2.3` per
  cargo's convention)
- comparisons: `>1.2.3`, `>=1.2.3`, `<1.2.3`, `<=1.2.3`
- combined: `>=1.2.3 <2.0.0` or `>=1.2.3, <2.0.0`
- caret: `^1.2.3`, `^0.2.3`, `^0.0.3`
- wildcard: `*`

Other syntaxes (`~1.2.3`, npm-style OR `||`, pre-release metadata, …)
are not part of the documented surface and may or may not work
depending on what the `semver` crate accepts.

## `[features]`

Public, additive, named-boolean capabilities. The reserved `default`
key holds the list of features Cabin enables when `--no-default-features`
is not passed.

```toml
[features]
default = ["simd"]
simd = []
ssl = []
full = ["simd", "ssl"]
```

Rules:

- feature names must be non-empty ASCII letters / digits / `_` / `-`;
  `/`, `.`, `:`, and whitespace are rejected;
- a feature value is a list of feature names (possibly empty); every
  referenced name must be a declared feature in the same package;
- cycles are rejected;
- declaring a normal feature called `default` is rejected.

Feature entries may also use `dep:foo` to enable an optional package
dependency, or `dependency/feature` to request a feature on a
dependency package. See
[`features-options-variants.md`](features-options-variants.md) for
the full resolver semantics.

## `[options]`

Local build knobs. Declared with a `type` and a `default`.

```toml
[options]
warnings_as_errors = { type = "bool", default = false }
allocator = { type = "enum", values = ["system", "mimalloc"], default = "system" }
namespace = { type = "string", default = "cabin_example" }
log_buffer_kb = { type = "integer", default = 64 }
```

Supported `type` values: `bool`, `enum`, `string`, `integer`. For
`enum`, `values` is required and `default` must be one of them. The
`values` key is rejected on `bool` / `string` / `integer`.

Options propagate to metadata only; they are not wired to
compiler / linker flags.

## `[variants]`

Artifact / ABI / build-identity dimensions.

```toml
[variants]
linkage = { values = ["static", "shared"], default = "static" }
stdlib = { values = ["default", "libstdc++", "libc++"], default = "default" }
```

Each variant declares a non-empty `values` list and a `default`
drawn from it. Variant values participate in the build configuration
fingerprint; future work may use them to drive artifact identity.

For the full protocol see [`features-options-variants.md`](features-options-variants.md).

## `[workspace]`

```toml
[workspace]
members = ["packages/*", "tools/hello"]
```

A `cabin.toml` with a `[workspace]` table is a workspace root. Member
patterns may be:

- exact relative paths (`tools/hello`); the directory must contain a
  `cabin.toml`;
- a single trailing-`*` glob (`packages/*`); every immediate
  subdirectory of `packages/` that contains a `cabin.toml` becomes a
  member.

More complex glob syntaxes (`**`, `?`, multiple `*`s) are intentionally
not supported.

## Target dependencies

Inside a target's `deps` array, each entry is one of:

- `"name"` — same-package target, **or** the name of a declared
  package dependency (resolves to that package's unique
  `cpp_library`).
- `"package:target"` — qualified reference. The `package` part must be
  either the current package or a declared package dependency; the
  `target` part must exist in that package.

Versioned dependencies resolve through the configured local or sparse
HTTP index and are materialised through the artifact cache when a
buildable graph needs them. Resolved versioned dependencies are
recorded in `cabin.lock` next to the manifest — see
[`docs/lockfile.md`](lockfile.md).

## Validation

The parser and downstream tools reject manifests when:

- the manifest contains neither `[package]` nor `[workspace]`
- `[package].name` / `[package].version` is missing or invalid
- a name is empty or contains whitespace
- a target's `type` is unknown
- the same target name appears twice
- the same dependency key appears twice
- a dependency entry has neither `path`, `version`, `workspace`,
  nor `system = true`
- a dependency entry combines mutually exclusive source forms
- a dependency table has unsupported keys or combinations
  (e.g. `git`, `source`, `system = true` with `features`)
- a versioned dependency requirement is not parseable
- a referenced local manifest does not exist
- a dependency key does not match the referenced package's name
- two loaded packages share a `[package].name`
- the package or target dependency graph contains a cycle
- a Rust target depends on a C / C++ target

## Example — direct version dependency

```toml
[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
```

Resolution requires `--index-path` or `--index-url` when the
manifest uses versioned dependencies.

## Example — local path dependency

```toml
# app/cabin.toml
[package]
name = "app"
version = "0.1.0"

[dependencies]
greet = { path = "../greet" }
```

`cabin build` works; no resolver involvement is needed.

## Example — mixed

```toml
[package]
name = "app"
version = "0.1.0"

[dependencies]
greet = { path = "../greet" }
fmt = ">=10.0.0 <11.0.0"
```

`cabin metadata` reports both. `cabin resolve --index-path index`
resolves `fmt`; `cabin build --index-path index` fetches and builds
the resolved dependency when its archive metadata is present.

## Example — workspace

```toml
# Workspace root cabin.toml
[workspace]
members = ["packages/*"]
```

```toml
# packages/app/cabin.toml
[package]
name = "app"
version = "0.1.0"

[dependencies]
greet = { path = "../greet" }
```

## `[lint.cpplint]`

Per-package lint configuration consumed by `cabin lint`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `filters` | array of strings | no | `cpplint` filter entries (e.g. `"-build/c++11"`).  Order is preserved verbatim.  Each entry must be non-empty.|

Filters declared here apply only to *this* package's files;
they do not leak into sibling workspace members.  See
[`docs/lint.md`](lint.md) for the full behaviour, including
how Cabin interacts with `CPPLINT.cfg`.

```toml
[lint.cpplint]
filters = [
  "-build/c++11",
  "-whitespace/braces",
]
```

## Not supported yet

The following are **not** part of the current manifest schema:

- git / URL dependency sources (only `path` and `version`)
- alias dependencies (`fmt = { package = "..." }`)
- shared-library generation from variants
- C++ modules, install rules

These remain out of scope for the local core.
