# `cabin.toml` Reference

This document describes the `cabin.toml` schema currently understood by the manifest parser, the
workspace loader, the build planner, the resolver, and the artifact layer.

Registry packages declared with versioned dependencies must, after fetch and extraction, contain a
valid `cabin.toml` at the archive root.  `cabin-artifact` rejects an extracted package whose
`[package].name` or `[package].version` disagrees with the resolved entry.  See
[`artifacts.md`](artifacts.md) for the source-archive contract.

## Top-level structure

A manifest may contain these top-level sections:

- at most one `[package]` table
- zero or more `[target.<name>]` tables
- zero or more `[target.'cfg(...)'.<kind>]` conditional dependency or toolchain tables
- zero or more `[target.'cfg(...)'.profile]` general conditional flag layers
- zero or more `[target.'cfg(...)'.profile.<name>]` named conditional flag overlays
- zero or one `[dependencies]` table
- zero or one `[dev-dependencies]` table
- at most one `[workspace]` table
- at most one `[features]` table
- at most one `[profile]` table plus `[profile.<name>]` tables
- at most one `[toolchain]` table
- at most one `[patch]` table

A manifest must contain at least one of `[package]` and `[workspace]`.  Package-specific tables such
as targets, dependencies, and features require `[package]`.  Workspace policy tables such as
`[workspace]`, `[profile]`, `[toolchain]`, and `[patch]` may appear on a workspace root without
`[package]`.

Naming convention: manifest field names and value strings are kebab-case (`include-dirs`,
`header-only`, `opt-level`, `dev-dependencies`).  The single exception is `cfg(...)` predicate keys
(`target_os`, `cc_version`, `cxx_version`), which follow the cfg grammar's snake_case convention.

```toml
[package]
name = "my-project"
version = "0.1.0"
cxx-standard = "c++17"

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
| `name` | string | yes | Package name: a bare `name` or a scoped `<scope>/<name>` with exactly one `/`.  Each part must be non-empty, contain no whitespace, consist only of ASCII letters, digits, `_`, `-`, and `.`, not start with `.` or `-`, and not be `.` or `..`; the scope part is stricter (lowercase letters, digits, and interior `-` only, at most 39 characters - a claimable GitHub login).  Registry packages are always scoped; bare names are local-only and rejected by `cabin publish`. |
| `version` | string | yes | Valid [SemVer](https://semver.org/) string. |
| `c-standard` | string | no | Package-wide C implementation standard (`c89`, `c99`, `c11`, `c17`, `c23`; `c90` is an alias of `c89`).  There is no built-in default: every target that compiles C sources needs an effective value from this field or its own `[target.<name>]` override.  See [Language standards](language-standards.md). |
| `cxx-standard` | string | no | Package-wide C++ implementation standard (`c++98` … `c++26`; `c++03` is an alias of `c++98`).  There is no built-in default: every target that compiles C++ sources needs an effective value from this field or its own `[target.<name>]` override. |
| `interface-c-standard` | string | no | Package-wide default C interface requirement for `library` / `header-only` targets.  Also accepts `"none"` (headers not consumable from C). |
| `interface-cxx-standard` | string | no | Package-wide default C++ interface requirement for `library` / `header-only` targets.  Also accepts `"none"`. |
| `gnu-extensions` | boolean | no | Package-wide default for the per-target GNU-extensions dialect knob (default `false`).  See [Language standards](language-standards.md). |

Inside a workspace, each of the four standard fields also accepts the `{ workspace = true }` opt-in
form, inheriting the literal declared on the workspace root's `[workspace]` table - see
[Language standards](language-standards.md).  `gnu-extensions` has no marker form.

Source-language *classification* stays per-file (target kinds, `.c` vs `.cc` extensions - see
[Targets](targets.md)); the standard each language compiles with is governed by the fields above and
their per-target overrides ([Language standards](language-standards.md)).

## `[target.<name>]`

The table key (`<name>`) is the target name.  Target names must be non-empty, must not contain
whitespace, must consist only of ASCII letters, digits, `_`, `-`, and `.`, must not start with `.`
or `-`, must not be `.` or `..`, and must be unique within the manifest.

| Field | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `type` | string | yes | - | Target kind.  One of `library`, `header-only`, `executable`, `test`, `example`.  Each kind describes artifact role only; a target may freely mix `.c` and C++ sources.  See [Targets](targets.md). |
| `sources` | array of strings | no | `[]` | Source files, relative to the manifest directory (no `..`). |
| `include-dirs` | array of strings | no | `[]` | Additional include directories, relative to the manifest directory. |
| `defines` | array of strings | no | `[]` | Preprocessor definitions, e.g. `"FOO=1"`. |
| `deps` | array of strings or tables | no | `[]` | Target dependencies.  A string entry declares a private edge; the table form adds per-edge visibility: `{ name = "foo", public = true }`.  See [Target dependencies](#target-dependencies). |
| `required-features` | array of strings | no | `[]` | Package features (declared in this package's `[features]` table) that must all be enabled for this target to be built or used.  Unknown names are rejected at manifest load.  See [Feature-gated targets](features.md#feature-gated-targets). |
| `c-standard` | string | no | package value | Per-target C implementation standard override.  See [Language standards](language-standards.md). |
| `cxx-standard` | string | no | package value | Per-target C++ implementation standard override. |
| `interface-c-standard` | string | no | effective `c-standard` | C interface requirement (or `"none"`); `library` / `header-only` only.  A `header-only` target must have at least one interface standard (either language, target or package level). |
| `interface-cxx-standard` | string | no | effective `cxx-standard` | C++ interface requirement (or `"none"`); `library` / `header-only` only. |
| `gnu-extensions` | boolean | no | package value, else `false` | Per-target GNU-extensions dialect override. |

`include-dirs` of a `library` or `header-only` target are visible (transitively) to any target that
depends on it.

## `[dependencies]`

```toml
[dependencies]
# Local path dependency
greet = { path = "../greet" }

# Versioned dependency, string form
fmt = ">=10.0.0 <11.0.0"

# Scoped registry dependency: `/` is not a bare-key character,
# so the key must be quoted
"fmtlib/fmt" = ">=10.0.0 <11.0.0"

# Versioned dependency, table form
spdlog = { version = "^1.13.0" }

# Foundation-port dependency (bundled form)
zlib = { port = true, version = "^1.3" }

# Foundation-port dependency (filesystem path form)
zlib = { port-path = "../ports/zlib/1.3.1" }
```

Each entry declares a package-level dependency.  The dependency value is either:

- a **string** - interpreted as a SemVer requirement;
- a **table** - must specify exactly one source: `path`, `version`, `port = true`, `port-path`,
  `workspace = true`, or `system = true` (`port = false` is treated as absent).  The source may be
  combined with `features`, `default-features`, `optional`, or `ignore-interface-standard`
  (subject to per-source rules below).  Unknown keys are rejected by the manifest parser.

Foundation-port dependencies use one of two mutually-exclusive fields:

- `port = true` - bundled curated recipe resolved by the dependency's name against the set embedded
  in the Cabin binary.  `port = true` requires a sibling `version = "<requirement>"` field; the
  requirement is resolved against the bundled set's available versions.
- `port-path = "..."` - filesystem path to a recipe directory (containing `port.toml` plus an
  overlay `cabin.toml`); the path is interpreted relative to the consumer's `cabin.toml`.
  `port-path` is mutually exclusive with `version` (the recipe at the path supplies the version).
  The CLI prepares the port - downloading, verifying, extracting, and applying the overlay - before
  the workspace loader runs.

Both forms are mutually exclusive with `path`, `workspace`, and `system`.  They honor `features` and
`default-features` (a port overlay may declare a `[features]` table that the feature resolver gates
per edge), but do not support `optional`.

The dependency *key* (`greet`, `fmt`, `spdlog`, `zlib` above) must equal the depended-on package's
`[package].name` (path deps, port deps) or the registry package name (version deps).  Registry
dependency keys are always the canonical scoped `<scope>/<name>` name (lowercase throughout, the
registry grammars): `cabin publish` rejects a bare or non-canonical versioned dependency key in
`[dependencies]` or `[dev-dependencies]` (dev-dependency keys denote registry packages too - they
resolve when building the package's own tests), and the hosted registry enforces the same rule
server-side.  System dependencies (`system = true`) are exempt: their keys name system packages,
not registry packages (see [`system-dependencies.md`](system-dependencies.md)).

### `ignore-interface-standard`

`ignore-interface-standard = true` exempts exactly this dependency edge from the
post-resolution standard-compatibility check (see
[`language-standards.md`](language-standards.md#post-resolution-compatibility-errors)).
The check still evaluates the edge and prints a downgraded note that the edge is unchecked, so the
override cannot silently rot.  The exemption covers this check only: the always-on build-time
interface enforcement is unaffected, so it can unblock the interface-`"none"` and cross-language
violation classes but not interface-minimum violations (which that enforcement independently
rejects).  The field is deliberately per-edge: there is no package-wide or global variant.  It is accepted on every package-sourced
form (path, version, port, workspace) in `[dependencies]` and `[dev-dependencies]`, and rejected
alongside `system = true` (system dependencies never enter the check).

### Version requirement syntax

Cabin uses the [`semver` crate](https://crates.io/crates/semver) for parsing, with one extra
convenience: comparators may be separated by whitespace as well as by commas.  Recognized forms:

- exact / compatible: `=1.2.3`, `1.2.3` (treated as `^1.2.3` per cargo's convention)
- comparisons: `>1.2.3`, `>=1.2.3`, `<1.2.3`, `<=1.2.3`
- combined: `>=1.2.3 <2.0.0` or `>=1.2.3, <2.0.0`
- caret: `^1.2.3`, `^0.2.3`, `^0.0.3`
- wildcard: `*`

Other syntaxes (`~1.2.3`, npm-style OR `||`, pre-release metadata, ...) are not part of the
documented surface and may or may not work depending on what the `semver` crate accepts.

## `[features]`

Public, additive, named-boolean capabilities.  The reserved `default` key holds the list of features
Cabin enables when `--no-default-features` is not passed.

```toml
[features]
default = ["simd"]
simd = []
ssl = []
full = ["simd", "ssl"]
```

Rules:

- feature names must be non-empty ASCII letters / digits / `_` / `-`; `/`, `.`, `:`, and whitespace
  are rejected;
- a feature value is a list of feature names (possibly empty); every referenced name must be a
  declared feature in the same package;
- cycles are rejected;
- declaring a normal feature called `default` is rejected.

Feature entries may also use `dep:foo` to enable an optional package dependency, or
`dependency/feature` to request a feature on a dependency package.  See [`features.md`](features.md)
for the full resolver semantics.

## `[workspace]`

```toml
[workspace]
members = ["packages/*", "tools/hello"]
```

A `cabin.toml` with a `[workspace]` table is a workspace root.  Member patterns may be:

- exact relative paths (`tools/hello`); the directory must contain a `cabin.toml`;
- a single trailing-`*` glob (`packages/*`); every immediate subdirectory of `packages/` that
  contains a `cabin.toml` becomes a member.

More complex glob syntaxes (`**`, `?`, multiple `*`s) are intentionally not supported.

The workspace table accepts these additional fields, all optional:

- `exclude` - paths or trailing-`*` globs removed from the member set even when matched by
  `members`;
- `default-members` - the subset of members commands operate on when no package-selection flags are
  passed at the workspace root;
- `[workspace.dependencies]` and `[workspace.dev-dependencies]` - shared version requirements that
  member entries reference with `dep = { workspace = true }`;
- `c-standard` - shared C implementation-standard default (literal value only) that member packages
  opt into per field with `c-standard = { workspace = true }`;
- `cxx-standard` - shared C++ implementation-standard default (same opt-in form);
- `interface-c-standard` - shared C interface-requirement default (same opt-in form);
- `interface-cxx-standard` - shared C++ interface-requirement default (same opt-in form).  See
  [Language standards](language-standards.md).

See [`workspaces.md`](workspaces.md) for member expansion, selection flags, and inheritance
semantics.

## Target dependencies

Inside a target's `deps` array, each entry is a reference string or a table wrapping one:

- `"name"` - a same-package target.  When no local target matches and `name` is a declared package
  dependency, the entry is *shorthand for the dependency's same-named target* - `"fmt"` means
  `"fmt:fmt"`, and a scoped `"fmtlib/fmt"` means `"fmtlib/fmt:fmt"` (target names never contain
  `/`, so the shorthand target is the package's *base* name).  It resolves only when that
  dependency declares a `library` or `header-only` target with that name.  The shorthand is pure
  name matching; a package never exports a "default" or "unique" library target, and a name that
  matches neither form (including one that only matches a same-named executable) is a hard error
  suggesting the qualified spelling.
- `"package:target"` - qualified reference.  The `package` part must be either the current package
  or a declared package dependency; the `target` part must exist in that package.  Any dependency
  target whose name differs from its package name must be spelled this way.
- `{ name = "<reference>", public = <bool> }` - table form.  `name` takes either reference
  spelling above; `public` (default `false`) declares the edge's visibility.  A string entry is
  exactly equivalent to `{ name = "<reference>", public = false }`.

### Edge visibility

Every dependency edge is **private** unless the entry declares `public = true`:

```toml
[target.net]
type = "library"
sources = ["src/net.cc"]
deps = [
    "util",                            # private edge
    { name = "fmt", public = true },   # public edge, same-name shorthand
    { name = "foo:opt", public = true } # public edge, qualified reference
]
```

Rule of thumb: **an edge is public iff the target's public headers include headers of that
dependency.**  If only the target's `.c` / `.cc` files include the dependency's headers, the edge
is private.  Visibility applies to the resolved edge - the same-name shorthand resolves first, so
`{ name = "fmt", public = true }` declares a public edge to `fmt:fmt`.

Today the flag is declarative: it does not change how anything builds or links.  It exists so
interface requirements (see [Language standards](language-standards.md)) can propagate along
public edges only.  A linter that flags under-declaration - public headers including headers of a
dependency whose edge is private - may come later.

Declaring a package under `[dependencies]` only makes it *available*; nothing links until a target
names one of its targets in `deps`.  [Features](features.md) never add `deps` entries either - a
consumer that wants a [feature-gated target](features.md#feature-gated-targets) both enables the
feature and lists the target explicitly.

Versioned dependencies resolve through the configured local or sparse HTTP index and are
materialized through the artifact cache when a buildable graph needs them.  Resolved versioned
dependencies are recorded in `cabin.lock` next to the manifest - see
[`docs/lockfile.md`](lockfile.md).

## Validation

The parser and downstream tools reject manifests when:

- the manifest contains neither `[package]` nor `[workspace]`
- `[package].name` / `[package].version` is missing or invalid
- a name is empty or contains whitespace
- a target's `type` is unknown
- the same target name appears twice
- the same dependency key appears twice
- a dependency entry has neither `path`, `version`, `port = true`, `port-path`, `workspace`, nor
  `system = true`
- a dependency entry combines mutually exclusive source forms
- a dependency table combines `system = true` with another source form (`path`, `port`, `port-path`,
  `workspace`, `features`, `default-features`, or `optional`)
- a versioned dependency requirement is not parseable
- a referenced local manifest does not exist
- a dependency key does not match the referenced package's name
- two loaded packages share a `[package].name`
- the package or target dependency graph contains a cycle

## Example - direct version dependency

```toml
[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
```

Resolution requires `--index-path` or `--index-url` when the manifest uses versioned dependencies.

## Example - local path dependency

```toml
# app/cabin.toml
[package]
name = "app"
version = "0.1.0"

[dependencies]
greet = { path = "../greet" }
```

`cabin build` works; no resolver involvement is needed.

## Example - mixed

```toml
[package]
name = "app"
version = "0.1.0"

[dependencies]
greet = { path = "../greet" }
fmt = ">=10.0.0 <11.0.0"
```

`cabin metadata` reports both.  `cabin resolve --index-path index` resolves `fmt`; `cabin build
--index-path index` fetches and builds the resolved dependency when its archive metadata is present.

## Example - workspace

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
