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
- zero or one `[dev-dependencies]` table
- at most one `[workspace]` table
- at most one `[features]` table
- at most one `[profile]` table plus `[profile.<name>]` tables
- at most one `[toolchain]` table
- at most one `[patch]` table

A manifest must contain at least one of `[package]` and `[workspace]`.
Package-specific tables such as targets, dependencies, and features
require `[package]`. Workspace policy tables
such as `[workspace]`, `[profile]`,
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
type = "executable"
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
See [Targets](targets.md) for how C/C++ are picked per
target.

## `[target.<name>]`

The table key (`<name>`) is the target name. Target names must be
non-empty, must not contain whitespace, must consist only of ASCII
letters, digits, `_`, `-`, and `.`, must not start with `.` or `-`,
must not be `.` or `..`, and must be unique within the manifest.

| Field | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `type` | string | yes | — | Target kind. One of `library`, `header_only`, `executable`, `test`, `example`. Each kind describes artifact role only; a target may freely mix `.c` and C++ sources. See [Targets](targets.md). |
| `sources` | array of strings | no | `[]` | Source files, relative to the manifest directory (no `..`). |
| `include_dirs` | array of strings | no | `[]` | Additional include directories, relative to the manifest directory. |
| `defines` | array of strings | no | `[]` | Preprocessor definitions, e.g. `"FOO=1"`. |
| `deps` | array of strings | no | `[]` | Target dependencies. See [Target dependencies](#target-dependencies). |

`include_dirs` of a `library` or `header_only` target are visible
(transitively) to any target that depends on it.

## `[dependencies]`

```toml
[dependencies]
# Local path dependency
greet = { path = "../greet" }

# Versioned dependency, string form
fmt = ">=10.0.0 <11.0.0"

# Versioned dependency, table form
spdlog = { version = "^1.13.0" }

# Foundation-port dependency (bundled form)
zlib = { port = true, version = "^1.3" }

# Foundation-port dependency (filesystem path form)
zlib = { port-path = "../ports/zlib/1.3.1" }
```

Each entry declares a package-level dependency. The dependency value is
either:

- a **string** — interpreted as a SemVer requirement;
- a **table** — must specify exactly one source: `path`, `version`,
  `port = true`, `port-path`, `workspace = true`, or `system = true`
  (`port = false` is treated as absent). The source may be combined
  with `features`, `default-features`, or `optional` (subject to
  per-source rules below). Unknown keys are rejected by the manifest
  parser.

Foundation-port dependencies use one of two mutually-exclusive fields:

- `port = true` — bundled curated recipe resolved by the dependency's
  name against the set embedded in the Cabin binary. `port = true` requires
  a sibling `version = "<requirement>"` field; the requirement is resolved
  against the bundled set's available versions.
- `port-path = "..."` — filesystem path to a recipe directory
  (containing `port.toml` plus an overlay `cabin.toml`); the path is
  interpreted relative to the consumer's `cabin.toml`. `port-path` is
  mutually exclusive with `version` (the recipe at the path supplies the
  version). The CLI prepares the port — downloading, verifying, extracting,
  and applying the overlay — before the workspace loader runs.

Both forms are mutually exclusive with `path`, `workspace`, and `system`,
and do not yet support `features`, `default-features`, or `optional`.

The dependency *key* (`greet`, `fmt`, `spdlog`, `zlib` above)
must equal the depended-on package's `[package].name` (path
deps, port deps) or the registry package name (version deps).

### Version requirement syntax

Cabin uses the [`semver` crate](https://crates.io/crates/semver) for
parsing, with one extra convenience: comparators may be separated by
whitespace as well as by commas. Recognized forms:

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
dependency package. See [`features.md`](features.md) for the full
resolver semantics.

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
  package dependency (resolves to that package's unique `library`
  or `header_only` target).
- `"package:target"` — qualified reference. The `package` part must be
  either the current package or a declared package dependency; the
  `target` part must exist in that package.

Versioned dependencies resolve through the configured local or sparse
HTTP index and are materialized through the artifact cache when a
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
- a dependency table combines `system = true` with another source
  form (`path`, `workspace`, `features`, `default-features`, or
  `optional`)
- a versioned dependency requirement is not parseable
- a referenced local manifest does not exist
- a dependency key does not match the referenced package's name
- two loaded packages share a `[package].name`
- the package or target dependency graph contains a cycle

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

