# Build profiles

A **profile** is a named preset of compile-time settings — debug
information, optimization level, assertions — that Cabin applies
to a build. Profiles formalize the long-standing distinction
between "debug" and "release" builds and let projects declare
their own presets without having to drop into a raw compiler-flag
system.

This document is the canonical specification. The behavior
described here is what the manifest parser (`cabin-manifest`),
the typed model and resolver (`cabin-core::profile`), the build
planner (`cabin-build`), the CLI (`cabin-cli`), the canonical
package metadata (`cabin-package`), and the local / sparse-HTTP
index loaders (`cabin-index`, `cabin-index-http`) all agree on.

## Built-in profiles

Cabin always provides two profiles, even when the manifest has no
`[profile.*]` tables:

| Profile   | `debug` | `opt-level` | `assertions` | C compile flags       | C++ compile flags          |
| --------- | ------- | ----------- | ------------ | --------------------- | -------------------------- |
| `dev`     | `true`  | `0`         | `true`       | `-std=c11 -O0 -g`     | `-std=c++17 -O0 -g`        |
| `release` | `false` | `3`         | `false`      | `-std=c11 -O3 -DNDEBUG` | `-std=c++17 -O3 -DNDEBUG` |

`dev` is the default. It is also the profile a bare
`cabin build` and a bare `cabin metadata` invocation produce.

## CLI selection

`cabin build` and `cabin metadata` accept `--profile <name>`:

```sh
cabin build --profile dev
cabin build --profile release
cabin build --profile relwithdebinfo
```

`cabin build` also keeps the long-standing `--release` flag as a
**compatibility alias** for `--profile release`. Passing both
flags together is rejected:

```text
$ cabin build --release --profile release
error: the argument '--release' cannot be used with '--profile <NAME>'
```

`cabin resolve`, `cabin update`, `cabin fetch`, `cabin package`,
and `cabin publish` deliberately do **not** accept a profile flag.
Profiles are local build configuration; they have no effect on
dependency resolution, the lockfile, or the on-disk archive.

A `.cabin/config.toml` file may also pin a default profile via
`[build] profile = "<name>"`. The CLI flag still wins when
present; see [`config.md`](config.md) for the full discovery and
precedence ladder.

## Manifest syntax

Manifests may declare custom profiles or override built-in
defaults under top-level `[profile.<name>]` tables:

```toml
[profile.dev]
opt-level = 1            # override built-in: a faster dev cycle

[profile.release]
debug = true             # override built-in: keep debug info on

[profile.relwithdebinfo]  # custom: must declare `inherits`
inherits = "release"
debug = true
```

### Supported fields

| Field        | Type                                    | Notes                                                         |
| ------------ | --------------------------------------- | ------------------------------------------------------------- |
| `inherits`   | string (profile name)                   | Required on custom profiles; rejected on `dev` / `release`.   |
| `debug`      | `true` / `false`                        | Whether `-g` is added to C and C++ compile commands.          |
| `opt-level`  | `0` / `1` / `2` / `3` / `"s"` / `"z"`   | Maps directly onto `-O0` … `-O3` / `-Os` / `-Oz`.             |
| `assertions` | `true` / `false`                        | When `false`, `-DNDEBUG` is added to C and C++ compile commands. |

The schema is closed: any other key is rejected with a clear
error. Specifically, capability-style fields such as
`link-libs`, `compiler`, `toolchain`, `target`, `cfg`, `env`,
`rustflags`, `linker`, `ar`, `stdlib`, and `sanitizer` are
**not accepted** here — toolchain selection lives under
`[toolchain]`, and capability probing is out of scope.

The array flag fields `cflags`, `cxxflags`, `ldflags`,
`defines`, and `include-dirs` are written directly on the
`[profile.<name>]` table. See
*Inheritance and array flags* below for the merge semantics
across an inherits chain.

### Inheritance

- Built-in profiles (`dev`, `release`) have implicit defaults and
  are always selectable, even without a manifest entry.
- A `[profile.dev]` or `[profile.release]` entry **overrides**
  the matching built-in's defaults field-by-field. Unspecified
  fields keep their defaults.
- Custom profiles **must** declare `inherits`, which must point
  to a built-in or another custom profile.
- Inheritance is acyclic; cycles are rejected with a clear error.
- Final field values are resolved root-first: each layer in the
  chain overrides anything an ancestor set, and missing fields
  keep their inherited value.

### Inheritance and array flags

A `[profile.<name>]` table can contribute **array** flag
fields: `cflags`, `cxxflags`, `ldflags`, `defines`,
and `include-dirs`. These compose differently from the scalar
fields above:

- **Scalars replace** across the inherits chain
  (`opt-level`, `debug`, `assertions`). The leaf wins; an
  unset leaf field keeps its inherited value.
- **Array flag fields append**, root-first, across the
  inherits chain. Each ancestor's values come first, in the
  order the user wrote them; the selected profile's values
  come last.

The full effective order of array-flag layers, top to bottom
in the resulting argv, is:

1. The package's top-level `[profile]` block.
2. Each matching `[target.'cfg(...)'.profile]` block, in
   manifest order.
3. The profile inherits chain, root → selected — each step's
   `[profile.<name>]` flags appended after the previous step's.

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

This is Cabin adopting cargo-config-style array layering for
its profile flag arrays. Cargo's own profile tables do not
expose user-facing array fields; the closest analog is the
`rustflags` layering inside `.cargo/config.toml`. Cabin
profile flag arrays append across ancestors so a leaf profile
can extend its parent without re-stating every flag.

**Practical caveat.** Because arrays append, parent and leaf
flags coexist on every compile / link command. Mutually
exclusive compiler or codegen flags placed in shared parent
profiles will conflict with leaf overrides — `-O0` vs `-O3`,
`-fno-rtti` vs `-frtti`, `-flto` vs `-fno-lto`, incompatible
`-std=` / `/std:` flags. Cabin does not arbitrate; the
compiler's own last-wins or conflict behavior decides.
Reserve shared parent profiles for non-conflicting policy
flags (warnings, sanitizer-friendly debug-info knobs); keep
leaf-specific optimization / codegen choices in the leaf
profile itself.

### Workspace scope

Only the workspace root manifest's `[profile.*]` tables apply.
Member or path-dep manifests that declare profile tables are
rejected with the error
`profile tables may only appear in the workspace root manifest`,
so a single workspace key cannot mean different things in
different members.

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

Profile names are validated up front (ASCII alphanumerics, `_`,
`-`, `.`; non-empty; not `.` / `..`; not starting with `.`) so a
malformed name is rejected at parse time instead of slipping into
filesystem layout.

## Build configuration fingerprint

`BuildConfiguration::fingerprint` is a SHA-256 of every input
that affects build output: enabled features **and** the resolved
profile (its name, `debug`, `opt-level`, `assertions`). Switching
profiles changes
the fingerprint by design — a future cache layer would key on
the same value.

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
      "relwithdebinfo": { "inherits": "release", "debug": true }
    }
  }
}
```

The `available` array is sorted alphabetically; `definitions`
keys iterate alphabetically; the `selected.inherits_chain` is
deterministic (root first). Pass `--profile <name>` to compute
the metadata view as if that profile were selected — useful for
CI that wants to dump every profile's resolved fields without
re-reading the manifest.

## What profiles do *not* do

- They do **not** affect dependency resolution. `cabin resolve`
  and the lockfile are profile-independent.
- They do **not** enable or disable optional dependencies or
  gate features. Those remain orthogonal axes (see
  [`features.md`](features.md)).
- They do **not** introduce target-specific profile tables
  (`[target.'cfg(...)'.profile.*]`) or profile-specific dep
  tables (`[profile.<name>.dependencies]`).

## Limitations

- Cross-compilation is out of scope, so the build planner
  evaluates profiles against the host toolchain.
- Toolchain selection and capability probing are explicitly out
  of scope for profile tables.
- Profile names cannot escape the build root; invalid names are
  rejected at parse time rather than sanitized silently.
