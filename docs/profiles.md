# Build profiles

A **profile** is a named preset of compile-time settings - debug information, optimization level,
assertions - that Cabin applies to a build.  Profiles formalize the long-standing distinction
between "debug" and "release" builds and let projects declare their own presets without having to
drop into a raw compiler-flag system.

This document is the canonical specification.  The behavior described here is what the manifest
parser (`cabin-manifest`), the typed model and resolver (`cabin-core::profile`), the build planner
(`cabin-build`), the CLI (`cabin`), the canonical package metadata (`cabin-package`), and the local
/ sparse-HTTP index loaders (`cabin-index`, `cabin-index-http`) all agree on.

## Built-in profiles

Cabin always provides two profiles, even when the manifest has no `[profile.*]` tables:

| Profile   | `debug` | `opt-level` | `assertions` | C compile flags       | C++ compile flags          |
| --------- | ------- | ----------- | ------------ | --------------------- | -------------------------- |
| `dev`     | `true`  | `0`         | `true`       | `-std=<c-standard> -O0 -g`     | `-std=<cxx-standard> -O0 -g`        |
| `release` | `false` | `3`         | `false`      | `-std=<c-standard> -O3 -DNDEBUG` | `-std=<cxx-standard> -O3 -DNDEBUG` |

`dev` is the default.  It is also the profile a bare `cabin build` and a bare `cabin metadata`
invocation produce.

The standard flag comes from the language-standards layer, not from the profile: `<c-standard>` /
`<cxx-standard>` are the target's effective standards, which the manifest must declare - see
[Language standards](language-standards.md).

## CLI selection

`cabin build` and `cabin metadata` accept `--profile <name>`:

```sh
cabin build --profile dev
cabin build --profile release
cabin build --profile relwithdebinfo
```

`cabin build` also keeps the long-standing `--release` flag as a **compatibility alias** for
`--profile release`.  Passing both flags together is rejected:

```text
$ cabin build --release --profile release
error: the argument '--release' cannot be used with '--profile <NAME>'
```

`cabin resolve`, `cabin update`, `cabin fetch`, `cabin package`, and `cabin publish` deliberately do
**not** accept a profile flag.  Profiles are local build configuration; they have no effect on
dependency resolution, the lockfile, or the on-disk archive.

A `.cabin/config.toml` file may also pin a default profile via `[build] profile = "<name>"`.  The
CLI flag still wins when present; see [`config.md`](config.md) for the full discovery and precedence
ladder.

## Manifest syntax

Manifests may declare custom profiles or override built-in defaults under top-level
`[profile.<name>]` tables:

```toml
[profile.dev]
opt-level = 1            # override built-in: a faster dev cycle

[profile.release]
debug = true             # override built-in: keep debug info on

[profile.relwithdebinfo]  # custom: must declare `inherits`
inherits = "release"
debug = true
```

Profiles define compile-time presets. Compiler-wrapper selection is build execution configuration,
not a profile field:

```toml
[build]
compiler-wrapper = "ccache"
```

That setting prefixes C and C++ compile commands regardless of the selected profile. See
[Compiler wrappers](compiler-cache.md).

### `[profile.<name>]`: selectable profile definition

Only `[profile.<name>]` defines a selectable profile.  Custom profiles must declare `inherits`;
the built-in `dev` and `release` profiles exist without manifest entries.

| Field        | Type                                    | Notes                                                         |
| ------------ | --------------------------------------- | ------------------------------------------------------------- |
| `inherits`   | string (profile name)                   | Required on custom profiles; rejected on `dev` / `release`.   |
| `debug`      | `true` / `false`                        | Whether `-g` is added to C/C++ compile commands.          |
| `opt-level`  | `0` / `1` / `2` / `3` / `"s"` / `"z"`   | Maps directly onto `-O0` … `-O3` / `-Os` / `-Oz`.             |
| `assertions` | `true` / `false`                        | When `false`, `-DNDEBUG` is added to C/C++ compile commands. |
| `defines` | array of strings | Preprocessor definitions applied to C and C++. |
| `include-dirs` | array of paths | Relative include directories applied to C and C++. |
| `cflags` | array of strings | Arguments applied only to C compilation. |
| `cxxflags` | array of strings | Arguments applied only to C++ compilation. |
| `ldflags` | array of strings | Arguments applied to package link commands. |
| `link-libs` | array of strings | Validated bare system-library names. |

The schema is closed: any other key is rejected with a clear error.  Specifically, capability-style
fields such as `compiler`, `toolchain`, `target`, `cfg`, `env`, `rustflags`, `linker`, `ar`,
`stdlib`, and `sanitizer` are **not accepted** here - toolchain selection lives under `[toolchain]`,
and capability probing is out of scope.

The array flag fields `cflags`, `cxxflags`, `ldflags`, `defines`, `include-dirs`, and `link-libs`
are written directly on the `[profile.<name>]` table.  See *Inheritance and array flags* below for
the merge semantics across an inherits chain.

### `[target.'cfg(...)'.profile]`: general conditional flag layer

A target-conditional general profile layer applies to every selected profile when its predicate
matches:

```toml
[target.'cfg(os = "linux")'.profile]
defines = ["USE_EPOLL"]
link-libs = ["pthread", "dl"]
```

It accepts only `defines`, `include-dirs`, `cflags`, `cxxflags`, `ldflags`, and `link-libs`.
Conditional profile flag layers are package-level, so workspace members and dependencies may
describe flags for their own sources.

### `[target.'cfg(...)'.profile.<name>]`: named conditional flag overlay

A target-conditional named profile flag overlay applies only when both conditions hold:

- the `cfg(...)` predicate matches the current build context; and
- the selected profile's resolved inheritance chain contains `<name>`.

It does not define a profile.  The overlay name only has to satisfy Cabin's profile-name syntax; it
does not have to be declared in the current workspace.  An undeclared name is accepted and remains
inert unless a consumer workspace selects a profile chain containing that name.

Named overlays accept the same six array fields as the general conditional layer: `defines`,
`include-dirs`, `cflags`, `cxxflags`, `ldflags`, and `link-libs`.  They reject `inherits`, `debug`,
`opt-level`, `assertions`, `toolchain`, and unknown fields.  Inheritance and scalar profile settings
remain unconditional workspace-root policy under `[profile.<name>]`.

A custom profile and its overlay therefore use separate tables:

```toml
[profile.release-lto]
inherits = "release"

[target.'cfg(os = "linux")'.profile.release-lto]
cxxflags = ["-fno-semantic-interposition"]
ldflags = ["-flto"]
```

The overlay table alone would parse, but `cabin build --profile release-lto` would fail without the
`[profile.release-lto]` definition.

These are invalid because an overlay is not a profile definition:

```toml
[target.'cfg(os = "linux")'.profile.release-lto]
inherits = "release" # invalid
```

```toml
[target.'cfg(os = "linux")'.profile.release]
opt-level = "z" # invalid
```

### Linux-only static linking

Define a release-derived profile, then put the platform-specific flag on the release overlay:

```toml
[profile.static]
inherits = "release"

[target.'cfg(os = "linux")'.profile.release]
ldflags = ["-static"]
```

On Linux, `--profile release` and `--profile static` both receive `-static` because the static
profile's chain is `release -> static`.  A default `dev` build does not.  On macOS and Windows the
predicate does not match, so `--profile static` does not receive the flag.

Cabin passes `ldflags` verbatim to the selected compiler driver's link command.  The build succeeds
only if that driver supports `-static` and static versions of every required library are available.

### `link-libs`

`link-libs` is an array of **bare system-library names** - e.g.  `["pthread", "dl", "m"]` - that a
target's objects require at link time.  It differs from `ldflags` in two load-bearing ways:

- **It propagates.** A library's `link-libs` are added to the final link command of every executable
  that depends on that library (transitively), emitted as `-l<name>` *after* the library's archive
  so GNU `ld`'s left-to-right resolution finds the symbols.  `ldflags`, by contrast, apply only to
  the declaring package's own link.  This is what lets a static-library port (e.g. sqlite needing
  `-lpthread -ldl -lm` on Unix) carry its system-library requirements to consumers without every
  consumer re-declaring them.
- **It is validated and trusted.** Each entry must be a bare library name (a leading
  alphanumeric/underscore followed by alphanumerics and `_ . + -`); a leading `-`, a path separator,
  or whitespace is rejected at parse time.  Because a `link-libs` entry therefore cannot smuggle a
  linker flag, it is kept even for untrusted (registry) dependencies, unlike the raw `cflags` /
  `cxxflags` / `ldflags` arrays which are dropped.

Pair it with `[target.'cfg(...)'.profile]` to scope libraries to the platforms that need them, e.g.

```toml
[target.'cfg(family = "unix")'.profile]
link-libs = ["pthread", "dl", "m"]
```

### Inheritance

- Built-in profiles (`dev`, `release`) have implicit defaults and are always selectable, even
  without a manifest entry.
- A `[profile.dev]` or `[profile.release]` entry **overrides** the matching built-in's defaults
  field-by-field.  Unspecified fields keep their defaults.
- Custom profiles **must** declare `inherits`, which must point to a built-in or another custom
  profile.
- Inheritance is acyclic; cycles are rejected with a clear error.
- Final field values are resolved root-first: each layer in the chain overrides anything an ancestor
  set, and missing fields keep their inherited value.

### Inheritance and array flags

A `[profile.<name>]` table can contribute **array** flag fields: `cflags`, `cxxflags`, `ldflags`,
`defines`, `include-dirs`, and `link-libs`.  These compose differently from the scalar fields above:

- **Scalars replace** across the inherits chain (`opt-level`, `debug`, `assertions`).  The leaf
  wins; an unset leaf field keeps its inherited value.
- **Array flag fields append**, root-first, across the inherits chain.  Each ancestor's values come
  first, in the order the user wrote them; the selected profile's values come last.

The full effective order of array-flag layers, top to bottom in the resulting argv, is:

```text
[profile]
[target.'cfg(...)'.profile]

[profile.<root>]
[target.'cfg(...)'.profile.<root>]

[profile.<child>]
[target.'cfg(...)'.profile.<child>]
```

The profile chain is resolved root to selected.  At each step, ordinary profile flags are appended
before matching named overlays for that profile.  Multiple matching target tables retain manifest
order.

So with

```toml
[profile]
cxxflags = ["-Wall"]

[profile.release]
cxxflags = ["-O3"]

[profile.profiling]
inherits = "release"
cxxflags = ["-pg"]
```

selecting `profiling` resolves to

```
cxxflags = ["-Wall", "-O3", "-pg"]
```

This is Cabin adopting cargo-config-style array layering for its profile flag arrays.  Cargo's own
profile tables do not expose user-facing array fields; the closest analog is the `rustflags`
layering inside `.cargo/config.toml`.  Cabin profile flag arrays append across ancestors so a leaf
profile can extend its parent without re-stating every flag.

**Practical caveat.** Because arrays append, parent and leaf flags coexist on every compile / link
command.  Mutually exclusive compiler or codegen flags placed in shared parent profiles will
conflict with leaf overrides - `-O0` vs `-O3`, `-fno-rtti` vs `-frtti`, `-flto` vs `-fno-lto`,
incompatible `-std=` / `/std:` flags.  Cabin does not arbitrate; the compiler's own last-wins or
conflict behavior decides.  Reserve shared parent profiles for non-conflicting policy flags
(warnings, sanitizer-friendly debug-info knobs); keep leaf-specific optimization / codegen choices
in the leaf profile itself.

### Workspace scope

Only the workspace root manifest's `[profile.*]` tables apply.  Member or path-dep manifests that
declare profile tables are rejected with the error `profile tables may only appear in the workspace
root manifest`, so a single workspace key cannot mean different things in different members.

Package-level `[profile]`, `[target.'cfg(...)'.profile]`, and
`[target.'cfg(...)'.profile.<name>]` flag layers remain valid in each package.  A package can add
flags for a profile name used by its consumer workspace, but it cannot define that profile.

## Build directories

Build outputs are profile-aware:

```text
<build-dir>/<profile>/build.ninja
<build-dir>/<profile>/compile_commands.json
<build-dir>/<profile>/packages/<package>/<target>/...
```

Two effects:

- `dev` and `release` builds never overwrite each other.
- A custom profile gets its own deterministic output tree.

Profile names are validated up front (ASCII alphanumerics, `_`, `-`, `.`; non-empty; not `.` / `..`;
not starting with `.`) so a malformed name is rejected at parse time instead of slipping into
filesystem layout.

## Build configuration fingerprint

`BuildConfiguration::fingerprint` is a SHA-256 of every input that affects build output: enabled
features, the resolved profile (its name, `debug`, `opt-level`, `assertions`), and final resolved
flags.  An applicable named overlay changes the fingerprint.  An overlay whose target does not
match or whose name is outside the selected profile chain does not.

## `cabin metadata`

`cabin metadata` reports a top-level `profiles` block:

```jsonc
{
  "profiles": {
    "selected": {
      "name": "relwithdebinfo",
      "debug": true,
      "opt_level": "3",
      "assertions": false,
      "source": "custom",
      "inherits_chain": ["release", "relwithdebinfo"]
    },
    "available": ["dev", "release", "relwithdebinfo"],
    "definitions": {
      "relwithdebinfo": { "name": "relwithdebinfo", "inherits": "release", "debug": true }
    }
  }
}
```

The `available` array is sorted alphabetically; `definitions` keys iterate alphabetically; the
`selected.inherits_chain` is deterministic (root first).  Pass `--profile <name>` to compute the
metadata view as if that profile were selected - useful for CI that wants to dump every profile's
resolved fields without re-reading the manifest.  The existing
`toolchain.build_flags_per_package` view contains the final flags after matching named overlays.

## What profiles do *not* do

- They do **not** affect dependency resolution.  `cabin resolve` and the lockfile are
  profile-independent.
- They do **not** enable or disable optional dependencies or gate features.  Those remain orthogonal
  axes (see [`features.md`](features.md)).
- `profile` is not a `cfg(...)` key; use nested
  `[target.'cfg(...)'.profile.<name>]` overlay tables.
- They do **not** introduce profile-specific dependency tables or make dependency resolution
  profile-dependent.
- They do **not** make toolchain selection profile-dependent.  A `toolchain` table inside a named
  overlay is rejected.

## Limitations

- Cross-compilation is out of scope, so the build planner evaluates profiles against the host
  toolchain.
- Toolchain selection and capability probing are explicitly out of scope for profile tables.
- Profile names cannot escape the build root; invalid names are rejected at parse time rather than
  sanitized silently.
