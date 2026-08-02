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
| `interface-c-standard` | string or `{ min, max }` table | no | Package-wide default C interface requirement for `library` / `header-only` targets: a string minimum (`"c11"`), a bounded inclusive range (`{ min = "c99", max = "c17" }`, `max` optional), or `"none"` (headers not consumable from C). |
| `interface-cxx-standard` | string or `{ min, max }` table | no | Package-wide default C++ interface requirement for `library` / `header-only` targets; same forms as the C field. |
| `gnu-extensions` | boolean | no | Package-wide default for the per-target GNU-extensions dialect knob (default `false`).  See [Language standards](language-standards.md). |

Inside a workspace, each of the four standard fields also accepts the `{ workspace = true }` opt-in
form, inheriting the literal declared on the workspace root's `[workspace]` table - see
[Language standards](language-standards.md).  `gnu-extensions` has no marker form.

Source-language *classification* stays per-file (target kinds, `.c` vs `.cc` extensions - see
[Targets](targets.md)); the standard each language compiles with is governed by the fields above and
their per-target overrides ([Language standards](language-standards.md)).

### `[package.upstream]`

An optional machine-verifiable claim that the package's source tree came from a pinned upstream
archive.  The URL must be credential-free HTTPS and the archive format is declared explicitly,
never inferred from the URL.  Every [foundation port](foundation-ports.md) declares one.

```toml
[package.upstream]
url = "https://example.com/library-1.2.3.tar.gz"
sha256 = "1f9d0a4b2c8e63b7f0d5a9c8e7b6a5d4c3b2a1f0e9d8c7b6a5f4e3d2c1b0a9f8"
format = "tar.gz"
strip-prefix = "library-1.2.3"
patches = ["patches/0001-fix-msvc-build.patch"]

[[package.upstream.copy]]
from = "scripts/config.h.prebuilt"
to = "config.h"
```

The assembly pipeline is fixed and normative: extract the archive, strip the declared prefix,
apply the copy steps in declaration order, then apply the patches in declaration order.  Every
step is a declarative, deterministic transformation - never a script or build hook.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `url` | string | yes | HTTPS URL of the pinned upstream archive, at most 2048 bytes.  Other schemes and URLs embedding credentials are rejected. |
| `sha256` | string | yes | SHA-256 of the archive bytes: exactly 64 lowercase hexadecimal characters. |
| `format` | string | yes | Archive container format: `"tar.gz"` or `"zip"`. |
| `strip-prefix` | string | no | Single directory component stripped from every archive entry before comparison; omit it when the archive root is the source root.  Must be a portable component the extractor could accept (no `/` or `\`, at most 254 bytes so a child entry fits the 256-byte path cap, none of the Windows-hostile shapes). |
| `patches` | array of strings | no | Unified-diff patch files applied to the assembled tree after every copy step, in declaration order.  Each entry is a plain portable forward-slash relative path naming a file *inside this package* (the patch ships in the published archive; `cabin package` refuses to stage a declaration whose file is absent).  At most 16 entries and at most 1 MiB per file; an entry must not duplicate, case- or normalization-collide with, or nest with a copy path, another patch, or the root `cabin.toml`.  A patch names each file at most once, each patched file is capped at 16 MiB, and the whole list may carry at most 1024 file entries and read or rewrite at most 128 MiB of the tree in total.  Application is byte-exact - fixed `-p1` strip, no fuzz, no offset search, no newline normalization, text diffs only (binary hunks are rejected) - and paths inside diff headers must resolve safely within the assembled tree.  A patch file must contain nothing but the diff itself (git's per-file preamble lines are accepted in their exact shape; commit-message text or any other surrounding content is rejected - use `git diff` output, not `git format-patch`): patch files are exempt from the registry's upstream tree comparison, so their bytes must never be able to double as compilable source.  Note the key must appear before any `[[package.upstream.copy]]` table. |
| `[[package.upstream.copy]]` | array of tables | no | Declarative file placements applied to the extracted upstream tree, in declaration order: copy `from` to `to`.  Both are plain forward-slash relative paths with portable components (no absolute paths, no `.` / `..` components, no `\`), `from` and `to` must differ (including under case folding), steps' paths must not case-collide or nest one under another, at most 16 steps are accepted, and `strip-prefix` plus `/` plus `from` must fit the 256-byte archive entry-path cap.  Static file-to-file copies only - never a build script or codegen hook. |

The declaration is inert for consumers: resolving, fetching, and building a package never touch the
upstream URL.  It rides through the package's canonical metadata and registry index entries
([`package-format.md`](package-format.md)), and the hosted registry's external verifier downloads
the pinned archive and requires the published source tree to match it
([`remote-registry.md`](remote-registry.md#the-verifiers-checks)).  The pinned archive must be at
most 256 MiB: the verification pipeline downloads it under that cap, and a larger archive cannot be
verified - its versions stay pending until an operator intervenes.

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
| `interface-c-standard` | string or `{ min, max }` table | no | effective `c-standard` | C interface requirement: a string minimum, a bounded `{ min, max }` range, or `"none"`; `library` / `header-only` only.  A `header-only` target must have at least one interface standard (either language, target or package level). |
| `interface-cxx-standard` | string or `{ min, max }` table | no | effective `cxx-standard` | C++ interface requirement; same forms as the C field; `library` / `header-only` only. |
| `gnu-extensions` | boolean | no | package value, else `false` | Per-target GNU-extensions dialect override. |
| `links` | string | no | - | Native-library identity this target claims, e.g. `"z"` for a target embodying zlib; `library` only.  See [`links`](#links). |

`include-dirs` of a `library` or `header-only` target are visible (transitively) to any target that
depends on it.

### `links`

`links = "<identity>"` declares that the target embodies the named native library.  The claim is an
explicit opt-in and is never derived from the target name: target names are artifact stems, while a
`links` value names the native identity, and the two can legitimately differ (a target `ssl` may
link `"openssl"`).  Identities are non-empty, case-sensitive strings of ASCII letters, digits, `.`,
`_`, `+`, and `-`.

After dependency resolution, Cabin refuses a graph in which two distinct packages claim the same
identity, because duplicate native symbols fail at the final link regardless of dependency
visibility - the check sees private edges too.  The diagnostic names the shared identity and every
claimant (package, version, target); keep exactly one claimant, or move the extras behind features
or platform conditions so they never resolve together.  Claims are declared, not selection-aware: a
target whose `required-features` are disabled still claims, but a package reachable only through a
disabled optional dependency does not.

The check runs where its inputs are exact.  Resolution refuses collisions among the claims that
provably link: local claims under the selected features, resolved registry claims, and - for a
`[patch]` fork reached only through registry metadata - the claims that hold under every feature
configuration (the fork itself and its non-optional path dependencies; the fork's real feature set
is decided by its dependents' edges, which resolution does not interpret).  The building commands
re-check the final loaded graph, whose feature resolution applies the fetched manifests' real
per-edge requests: a collision created only by such a fork's feature-enabled optional dependency
is refused there.  A build refused this way may already
have written `cabin.lock` - the recorded resolution itself stays valid.

`links` is uniqueness enforcement only.  There is no metadata passing between packages and no
build-hook integration, and only `library` targets may claim (header-only targets emit no linkable
artifact, so two wrappers of one library must not conflict).

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

# Foundation port - an ordinary scoped registry package
"cabin-ports/zlib" = "=1.3.1"
```

Each entry declares a package-level dependency.  The dependency value is either:

- a **string** - interpreted as a SemVer requirement;
- a **table** - must specify exactly one source: `path`, `version`, `workspace = true`, or
  `system = true`.  The source may be combined with `features`, `default-features`, `optional`, or
  `ignore-interface-standard` (subject to per-source rules below).  Unknown keys are rejected by
  the manifest parser.

[Foundation ports](foundation-ports.md) have no dependency form of their own: they are published
registry packages, named like any other (`"cabin-ports/zlib" = "=1.3.1"`).

The dependency *key* (`greet`, `fmt`, `spdlog` above) must equal the depended-on package's
`[package].name` (path deps) or the registry package name (version deps).  Registry
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
violation classes but not interface-range violations - below the declared minimum or above the
declared maximum - which that enforcement independently rejects.  The field is deliberately per-edge: there is no package-wide or global variant.  It is accepted on every package-sourced
form (path, version, workspace) in `[dependencies]` and `[dev-dependencies]`, and rejected
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
  dependency, the entry resolves to *that dependency's sole `library` or `header-only` target*,
  whatever it is named - `"zlib"` links `zlib:z` when `z` is the only library target `zlib`
  declares, and a scoped `"fmtlib/fmt"` resolves the same way.  Non-library targets (such as
  executables) are never candidates.  A dependency that declares several library or header-only
  targets makes the bare name ambiguous - a hard error listing the qualified candidates - and one
  that declares none has nothing a bare name could link; both cases require the explicit
  `package:target` spelling.
- `"package:target"` - qualified reference.  The `package` part must be either the current package
  or a declared package dependency; the `target` part must exist in that package.  This is the
  only spelling for a non-library dependency target, and the required one when the dependency
  declares several library or header-only targets.
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
    { name = "fmt", public = true },   # public edge, bare dependency package
    { name = "foo:opt", public = true } # public edge, qualified reference
]
```

Rule of thumb: **an edge is public iff the target's public headers include headers of that
dependency.**  If only the target's `.c` / `.cc` files include the dependency's headers, the edge
is private.  Visibility applies to the resolved edge - the bare-name shorthand resolves first, so
`{ name = "fmt", public = true }` declares a public edge to the `fmt` package's sole library
target.

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
- a `links` value is empty or contains a character outside ASCII letters, digits, `.`, `_`, `+`,
  and `-`
- a `links` key appears on a non-`library` target
- two targets of one package claim the same `links` value
- the same dependency key appears twice
- a dependency entry has neither `path`, `version`, `workspace`, nor `system = true`
- a dependency entry combines mutually exclusive source forms
- a dependency table combines `system = true` with another source form (`path`, `workspace`,
  `features`, `default-features`, or `optional`)
- a versioned dependency requirement is not parseable
- a referenced local manifest does not exist
- a dependency key does not match the referenced package's name
- two loaded packages share a `[package].name`
- the package or target dependency graph contains a cycle
- a `[package.upstream]` table has a non-HTTPS or credential-bearing `url`, a `sha256` that is not
  64 lowercase hexadecimal characters, a `format` other than `"tar.gz"` / `"zip"`, a `strip-prefix`
  that is not a single relative path component, more than 16 copy steps, or a copy step whose
  `from` / `to` is not a plain portable forward-slash relative path or names the same file twice
- a `[package.upstream]` `patches` list has more than 16 entries, an entry that is not a plain
  portable forward-slash relative path, or an entry that duplicates, case-collides with, or nests
  with a copy path, another patch, or the root `cabin.toml`

## Example - direct version dependency

```toml
[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
```

Resolving versioned dependencies uses `--index-path` / `--index-url`, the `[registry]` config
setting, or - when none of those apply - Cabin's default hosted registry
([`remote-registry.md`](remote-registry.md#the-default-registry)).

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
