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
| `defines` | array of strings | Preprocessor definitions applied to the package's own C and C++ compiles; consumers' interface definitions are target [`interface-defines`](manifest.md#edge-visibility). |
| `include-dirs` | array of paths | Relative include directories applied to the package's own C and C++ compiles - the producer-only counterpart of target `include-dirs`, which consumers inherit. |
| `cflags` | array of strings | Arguments applied only to C compilation. |
| `cxxflags` | array of strings | Arguments applied only to C++ compilation. |
| `ldflags` | array of strings | Arguments applied to package link commands. |
| `link-libs` | array of strings | Validated bare system-library names. |
| `frameworks` | array of strings | Validated bare Apple framework names. |

The schema is closed: any other key is rejected with a clear error.  Specifically, capability-style
fields such as `compiler`, `toolchain`, `target`, `cfg`, `env`, `rustflags`, `linker`, `ar`,
`stdlib`, and `sanitizer` are **not accepted** here - toolchain selection lives under `[toolchain]`,
and capability probing is out of scope.

The array flag fields `cflags`, `cxxflags`, `ldflags`, `defines`, `include-dirs`, `link-libs`, and
`frameworks` are written directly on the `[profile.<name>]` table.  See *Inheritance and array
flags* below for the merge semantics across an inherits chain.

### `[target.'cfg(...)'.profile]`: general conditional flag layer

A target-conditional general profile layer applies to every selected profile when its predicate
matches:

```toml
[target.'cfg(os = "linux")'.profile]
defines = ["USE_EPOLL"]
link-libs = ["pthread", "dl"]
```

It accepts only `defines`, `include-dirs`, `cflags`, `cxxflags`, `ldflags`, `link-libs`, and
`frameworks`.
Conditional profile flag layers are package-level, so workspace members and dependencies may
describe flags for their own sources.

### `[target.'cfg(...)'.profile.<name>]`: named conditional flag overlay

A target-conditional named profile flag overlay applies only when both conditions hold:

- the `cfg(...)` predicate matches the current build context; and
- the selected profile's resolved inheritance chain contains `<name>`.

It does not define a profile.  The overlay name only has to satisfy Cabin's profile-name syntax; it
does not have to be declared in the current workspace.  An undeclared name is accepted and remains
inert unless a consumer workspace selects a profile chain containing that name.

Named overlays accept the same seven array fields as the general conditional layer: `defines`,
`include-dirs`, `cflags`, `cxxflags`, `ldflags`, `link-libs`, and `frameworks`.  They reject
`inherits`, `debug`, `opt-level`, `assertions`, `toolchain`, and unknown fields.  Inheritance and
scalar profile settings remain unconditional workspace-root policy under `[profile.<name>]`.

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

`link-libs` is an array of **bare system-library names** - e.g.  `["pthread", "dl", "m"]` - that
the declaring package's objects require at link time.  It differs from `ldflags` in two
load-bearing ways:

- **It propagates.** A package's `link-libs` are added to the final link command of every executable
  whose link reaches any of the package's targets, through private edges too, emitted as
  `-l<name>` *after* every archive so GNU `ld`'s left-to-right resolution finds the symbols.
  `ldflags`, by contrast, apply only to the declaring package's own link.  This is what lets a
  static-library dependency (e.g. sqlite3 needing `-lpthread -ldl -lm` on Unix) carry its
  system-library requirements to consumers without every consumer re-declaring them.
- **It is validated and trusted.** Each entry must be a bare library name (a leading
  alphanumeric/underscore followed by alphanumerics and `_ . + -`); a leading `-`, a path separator,
  or whitespace is rejected at parse time.  Because a `link-libs` entry therefore cannot smuggle a
  linker flag, it is kept even for untrusted (registry) dependencies, unlike the raw `cflags` /
  `cxxflags` / `ldflags` arrays which are dropped.

On the link line, packages contribute in link order: the executable's own package first, and every
package before the packages its linked targets depend on.  Each package's list stays in full, so a
name two packages list repeats: a linker that searches libraries left to right needs it after both.
Apple's linker resolves libraries in any order and warns on a repeated `-l`, so links for Apple
platforms keep only each name's first occurrence.

Pair it with `[target.'cfg(...)'.profile]` to scope libraries to the platforms that need them, e.g.

```toml
[target.'cfg(family = "unix")'.profile]
link-libs = ["pthread", "dl", "m"]
```

### `frameworks`

`frameworks` is an array of **bare Apple framework names** - e.g.  `["CoreFoundation",
"Security"]` - that the declaring package's objects require.  It is validated, trusted, and
propagated exactly like `link-libs`, and emitted as `-framework <name>` after the system libraries.
Only Apple platforms (`os = "macos"` or `"ios"`) have frameworks; links for any other platform
ignore the list.  A condition still states the intent:

```toml
[target.'cfg(os = "macos")'.profile]
frameworks = ["CoreFoundation"]
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
`defines`, `include-dirs`, `link-libs`, and `frameworks`.  These compose differently from the
scalar fields above:

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
<build-dir>/<profile>/cabin-toolchain.json
<build-dir>/<profile>/packages/<package>/<target>/...
<build-dir>/<profile>/build-machine/packages/<package>/<target>/...
<build-dir>/.targets/<triple>/<profile>/...
<build-dir>/.cabin-gc/...
<build-dir>/.cabin-build.lock
<build-dir>/.cabin-workspace
```

The `build-machine` tree holds [build dependencies](dependency-kinds.md#build-dependencies), built
for the build machine.  A [`--target <triple>`](toolchains.md#building-for-another-platform) build
writes the same profile trees under `.targets/<triple>/`; profile names cannot start with `.`, so a
profile named like a triple keeps a tree of its own.  `.cabin-gc` and `.cabin-build.lock` serve
[unused-output collection](#unused-build-outputs); `.cabin-workspace` names the
[workspace the directory serves](#build-directory-ownership).

Cabin releases before `.targets` wrote `--target` builds to `<build-dir>/<triple>/<profile>/`.  Such
a build directory also predates the [owner record](#build-directory-ownership), so builds refuse it
until `cabin clean` with no `--profile`, `--package`, or `--target` empties it, old trees included.
`cabin clean --profile` and `--package` refuse to remove one as the tree of a profile named like
the triple.

Two effects:

- `dev` and `release` builds never overwrite each other.
- A custom profile gets its own deterministic output tree.

Profile names are validated up front, with the same rules on every platform, so a malformed name is
rejected at parse time instead of slipping into filesystem layout.  A name is non-empty, uses only
ASCII alphanumerics, `_`, `-`, and `.`, and does not start with `.`.  Because the name is a directory
name, it also must not:

- end with `.`, which Windows drops, so `dev.` would build into `dev`;
- be a Windows device name - `CON`, `PRN`, `AUX`, `NUL`, `COM1` to `COM9`, or `LPT1` to `LPT9` - in
  any letter case, with or without an extension (`nul.fast`);
- differ from another profile name, built-in or declared, only in letter case: a case-insensitive
  filesystem (the Windows and macOS default) would give `Fast` and `fast`, or `Release` and
  `release`, one build directory.

Mixed-case names and internal dots are fine (`RelWithDebInfo`, `ci.fast`).  Profile names stay
case-sensitive: `--profile Release` does not select `release`.  The letter-case rule covers the
whole `[profile.*]` table, so every command that resolves a profile rejects such a manifest before
it reads or removes build outputs.

### Build directory ownership

A build directory serves the one workspace that first builds into it: two workspaces would overwrite
each other's outputs (both can have a package `app`), and one's `cabin clean` would remove the
other's.  `build`, `check`, `run`, `test`, and `tidy` record their workspace in `.cabin-workspace`
when the directory is missing or holds no subdirectory, only files such as a `.gitignore`, and every
command that writes or removes outputs, `cabin clean` and `cabin clean --gc` included, refuses a
build directory another workspace owns before doing so.  Symlinked and `..` spellings of either
directory count as one.  The workspace is the directory of the root manifest the command loads, so
a member loaded on its own with `--manifest-path member/cabin.toml` is another workspace, and an
absolute `[paths] build-dir` in the user-level configuration serves only the first workspace that
builds into it.  Recording the owner takes the build-directory lock exclusively, so of two
workspaces building into a new directory at once, one claims it and the other is refused.

The record names the workspace relative to the build directory when the build directory lies inside
it, so moving the workspace with its build directory keeps it; a build directory outside a moved
workspace still names where the workspace was.  `cabin clean` keeps the record, so a cleaned build
directory stays its workspace's: to hand one to another workspace, remove it.  A build directory
that holds subdirectories but no record, as one written by a Cabin release before the record does,
is refused until `cabin clean` empties it, since nothing in it reliably names the workspace that
wrote it.

## Unused build outputs

A build directory keeps the outputs of every profile, `--target` platform, package selection, and
source file it has ever built, so outputs of renamed sources, removed packages, and profiles no
longer used pile up.  Cabin removes those it can tell are unused.  A build never depends on that:
Ninja rebuilds any output that is missing, so collecting only trades disk space for rebuild time.

**What counts as unused.**  `build`, `check`, `run`, and `test` record, in `<build-dir>/.cabin-gc/`,
that their plan uses each object, depfile, static library, executable, and `cabin check` stamp it
names.  Age is counted in *active days*: days on which one of those commands used the build
directory.  A project left alone for a month ages nothing, and a build that skips a package,
profile, or platform does not make its outputs unused.  An output is unused once no plan has used it
for more than `max-age` active days (default 30).  A command records its use when it starts, so a
program that `cabin run` keeps running for days counts as used on the day it started.

**When it is removed.**  The first `build`, `check`, `run`, or `test` of an active day removes
unused outputs once it no longer uses its own: after a successful build, or once the program or
tests exit.  It stops removing after one second; the next build continues where it stopped.  It
skips collection when another Cabin process is using the build directory or its file system has no
file locks, and a collection problem is a warning, never a build failure.
`[gc] auto = false` turns this off (see [`config.md`](config.md#gc)).

`cabin clean --gc` collects now, without the time limit, waiting for other Cabin processes to finish
with the build directory.  `--dry-run` lists what it would remove and writes nothing.
`--max-age <DAYS>` and `--max-size <SIZE>` override the configured policy for one run.

**Size budget.**  With `max-size`, after the age rule, collection removes the least recently used
outputs until the recorded outputs fit, but never one used on the current or previous active day; it
warns when those alone exceed the budget.  Sizes are file lengths, not allocated disk space.

**What is never removed.**  Collection removes only a regular file at a path a plan recorded, never
through a symlink or junction below the build directory, and then the directories it emptied in
that output tree.  The build directory itself may be a link, to another disk for example; a build
directory `cabin clean` refuses for holding the workspace, a package, or sources is never
collected.  It never removes `build.ninja`, `compile_commands.json`, Ninja's logs, files other tools
write beside outputs (such as the `.pdb` and `.ilk` an MSVC link writes for a user-supplied
`/DEBUG`), outputs from before Cabin recorded usage, or anything outside the build directory: the
source-archive cache, vendored sources, and compiler-cache data are not build outputs.
`cabin clean` removes everything but the lock file and the
[owner record](#build-directory-ownership).

**Concurrent commands.**  `build`, `check`, `run`, and `test` hold a shared lock on
`<build-dir>/.cabin-build.lock` while they use outputs; `run` and `test` hold it until the program
or tests exit.  Collection and `cabin clean` take it exclusively, so a `cabin clean` that such a
program runs against its own build directory fails instead of waiting for it forever (see
[`CABIN_BUILD_DIRS_IN_USE`](environment-variables.md#variables-cabin-sets-for-cabin-run-and-cabin-test)).
The lock is advisory, so it orders Cabin processes only; Cabin releases without it do not take it,
and a file system without file locks orders nothing.  Commands record their use one at a time, under
a second lock inside `.cabin-gc` with the same limits, so two builds at once keep both records.  An
unreadable or newer-format usage record makes collection keep the outputs it lists.

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

- Conditional profile tables evaluate for the platform the build is for (the build machine, or the
  [`--target`](toolchains.md#building-for-another-platform) platform) and its detected compilers.
- Toolchain selection and capability probing are explicitly out of scope for profile tables.
- Profile names cannot escape the build root; invalid names are rejected at parse time rather than
  sanitized silently.
