# Toolchains and conditional build flags

Cabin builds C/C++ packages with three external tools: a C compiler, a C++ compiler driver, and a
static-library archiver.  Cabin makes the choice of those tools, and the semantic flags applied
during compile / link, *explicit, typed, and deterministic*.

This document is the canonical specification for the toolchain selection model and the `[profile]`
flag schema.  The behavior described here is what the manifest parser (`cabin-manifest`), the typed
model (`cabin-core::toolchain` and `cabin-core::build_flags`), the resolver (`cabin-toolchain`), the
build planner (`cabin-build`), the CLI (`cabin`), and the canonical package metadata
(`cabin-package`) all agree on.

## Tool kinds

Three tool kinds participate:

| Kind  | Manifest key | CLI flag  | Env var | Default fallback list (Unix) | Default fallback list (Windows) |
| ----- | ------------ | --------- | ------- | --------------------- | --------------------- |
| `cc`  | `cc`         | `--cc`    | `CC`    | `cc`, `clang`, `gcc`  | `cl`, `clang`, `gcc`  |
| `cxx` | `cxx`        | `--cxx`   | `CXX`   | `c++`, `clang++`, `g++` | `cl`, `clang++`, `g++` |
| `ar`  | `ar`         | `--ar`    | `AR`    | `ar`                  | `lib`, `llvm-lib`, `llvm-ar`, `ar` |

The default fallbacks are host-dependent: on Windows the MSVC toolchain (`cl` for both C and C++,
`lib` for archiving) comes first, so a stock Windows install resolves to MSVC without any
configuration.  See [Windows / MSVC](#windows--msvc) for the dialect model and its limitations.

A compiler chosen for one language above the defaults (CLI, environment, config, or manifest; the
C++ compiler if both are) also leads the slots left to the defaults: each first tries the tool
beside it, with the same directory, target prefix, and version suffix.  Beside
`x86_64-w64-mingw32-g++` the C compiler is `x86_64-w64-mingw32-gcc` and the archiver
`x86_64-w64-mingw32-ar`; beside `clang++-18` they are `clang-18` and `llvm-ar-18`; beside `g++`
the archiver is `ar`; `cl` and `clang-cl` compile both languages and archive with `lib` and
`llvm-lib`.  Without such a tool the fallback list applies, minus `cl`, `lib`, and `llvm-lib`
beside a GCC-style compiler, so a defaulted slot does not mix in another toolchain.

The C compiler (`cc`) and the C++ compiler (`cxx`) are **separate** tool selections.  They may
resolve to the same underlying executable when the system installs a unified driver (`gcc` / `clang`
is also a valid C front-end), but the model keeps the slots independent so Cabin can pick the right
driver per source language.  The build planner classifies each source file by its filename extension
(`.c` is C; `.cc` / `.cpp` / `.cxx` / `.c++` / `.C` are C++) and dispatches each compile through the
matching driver.

The link driver is *also* selected per target: every linked executable inspects its own objects plus
every transitively reachable library object, picks **C++** if any of those are C++ or none are C
(assembly only), and otherwise picks **C**.  There is no separate `--linker` selection; the C/C++ driver decision is what controls
whether the C++ runtime (libstdc++ / libc++) is pulled in.

A linker-style env variable (`LD`) is intentionally not honored - adding linker selection would
require a linker-command abstraction the backend lacks.  The C++ compiler drives linking via MSVC
`cl` with `/Fe<exe> /link ...` or via the GCC/Clang driver, so `link.exe` is never selected
directly.

Cabin drives **two** command-line dialects: GCC/Clang style (`-std=c++17`, `-c`, `-o <file>`,
`-MD`) and MSVC style (`cl /std:c++17 /c /Fo<obj> /showIncludes`, `lib /OUT:<lib>`).  The C++
compiler picks the dialect - `cl.exe` selects MSVC, every other recognized family selects
GCC/Clang - and the archiver and optional C compiler must agree (see
[Validation](#validation-against-the-c-backend)).

## System include directories

Include directories contributed by third-party code - extracted registry packages and
`pkg-config`-probed `system = true` dependencies - are marked as *system* search paths on every
consuming compile: `-isystem <dir>` in the GCC/Clang dialect, `/external:W0 /external:I <dir>` in
the MSVC dialect.  Compilers suppress warnings whose location is inside a system header, so a strict
warning profile (`-Wall -Wextra -Werror`) keeps gating the user's own code without failing on
headers the user cannot fix.  Pass `-Wsystem-headers` (GCC/Clang) through `cxxflags` / `cflags` to
see those warnings again.

The user's own code always stays on plain `-I`: the package's own `include-dirs`, workspace members,
plain `path` dependencies, and `[patch]`ed packages (patching a dependency intentionally surfaces
its warnings again, since the patched tree is yours to edit).  A package also always sees its *own*
headers via `-I`, even when consumers see them as system includes.  System include dirs are searched
after the `-I` set and before the compiler's built-in default directories, so a vendored dependency
still shadows a stale system-wide copy of the same library.

Two consequences of the spelling are worth knowing:

- The GCC/Clang depfile block is `-MD -MF <file>` (not `-MMD`, which omits system headers): edits
  under an extracted registry tree or a `pkg-config` include dir keep invalidating incremental
  rebuilds.  A depfile whose paths Ninja cannot read reaches it another way; see
  [Paths Ninja cannot spell](#paths-ninja-cannot-spell).
- On MSVC, `/external:I` is version-gated (see the [capability set](#capability-set)); an older `cl`
  falls back to plain `/I` for those directories: the historical command shape without warning
  isolation.

### Standard header shadowing

Every dialect searches dependency include dirs, `-isystem` and `/external:I` ones included, before
the compiler's own standard headers.  A file named `stdint.h` or `version` directly inside a
dependency's include dir is therefore what `#include <stdint.h>` or `#include <version>` opens, in
the consumer's own sources and inside standard headers that include it.  On a case-insensitive
file system (the macOS and Windows default) a name differing in letter case matches too: an
upstream `VERSION` file is opened for `<version>`.

So while planning, a build refuses a registry package (an extracted registry archive or a
foundation port) whose library or header-only target, depended on by another target, has a file
directly inside one of its `include-dirs` that a C or C++ standard library header include would
open: the C headers through C23 (`<assert.h>` ... `<uchar.h>`, `<stdbit.h>`, `<stdckdint.h>`) and
the C++ headers through C++26, including the `<cxxx>` forms and `<version>`.  Two comparisons
decide it:

- each entry's name against the standard names, ignoring letter case (including the non-ASCII
  letters that fold to ASCII, such as `ſ`), on every host - so `VERSION` is refused on Linux too
  and a package is judged the same everywhere;
- each standard name looked up in the include dir by the host file system, which applies whatever
  else it equates when the compiler opens the name (Unicode normalization, or code points HFS+
  ignores).

Directories are ignored, as compilers skip them; a header in a subdirectory (`<foo/string.h>`)
never matches.  The error names the package, target, include dir, file, and standard header.
Workspace members, `path` dependencies, and `[patch]`ed packages are the user's own code and are
not checked.

The check covers standard header names only: implementation-internal headers the standard
headers include (`__config`, `bits/*`, `sys/cdefs.h`, ...) and platform headers such as
`<unistd.h>` can be shadowed the same way and are not checked.  It guards against accidental
collisions, such as an upstream `VERSION` file at an include root; it is not a sandbox against a
malicious dependency, whose code is compiled and linked into the build anyway.

A package meant to provide standard headers, such as a polyfill, is allowed by name in the
workspace root manifest's [`[standard-header-shadowing]`](manifest.md#standard-header-shadowing)
table; the table is ignored in any other manifest, so nothing in a dependency's own metadata can
allow it.

## Precedence

For every tool kind, the resolver walks the layers below in order and keeps the first one that
yields a value:

1. **CLI flag** - `--cc`, `--cxx`, `--ar`.  Highest precedence.
2. **Environment variable** - `CC`, `CXX`, `AR`.  An empty value is treated as unset; values are not
   shell-split.
3. **`[toolchain]` config-file layer** - `[toolchain]` in `<root>/.cabin/config.toml` (workspace or
   project) or `~/.config/cabin/config.toml` (user).  See [`config.md`](config.md) for the full
   file-discovery rules.
4. **Matching `[target.'cfg(...)'.toolchain]`** - the first conditional toolchain block whose
   predicate evaluates to `true` for the platform the build is for (the build machine, or the
   `--target` platform).
5. **`[toolchain]`** - the workspace root manifest's general toolchain table.
6. **Built-in defaults** - the tool beside the compiler chosen for the other language, if any (see
   [Tool kinds](#tool-kinds)), then Cabin's documented fallback list.

Each [`ResolvedTool`] records which layer it came from on its `source` field.  `cabin metadata`
surfaces the same value verbatim under `toolchain.tools.<kind>.source`, so users can audit a build
without re-deriving the precedence by hand.

## CLI surface

```sh
cabin build --cxx clang++
cabin build --cc /opt/llvm/bin/clang --cxx /opt/llvm/bin/clang++
cabin metadata --cxx clang++         # report the toolchain a build *would* pick
```

`--cc` / `--cxx` / `--ar` accept either a bare command name (resolved against `PATH`) or an explicit
filesystem path, relative to the current directory.  Whitespace-only values are rejected at parse
time.  The flags apply only to the current command invocation; nothing is written back to the
manifest.

`cabin build`, `cabin check`, `cabin run`, `cabin test`, `cabin metadata`, and `cabin explain`
accept the flags, along with `--target`.  `cabin resolve`, `cabin update`, `cabin
fetch`, `cabin package`, and `cabin publish` deliberately do **not** accept them - toolchain
selection has no effect on dependency resolution, the lockfile, or the published archive.

## Building for another platform

`--target <triple>` builds for the platform a target triple names (`aarch64-unknown-linux-gnu`,
`x86_64-apple-macos13`, `x86_64-pc-windows-msvc`) instead of the build machine:

```sh
cabin build --target x86_64-apple-macos13
cabin build --target aarch64-unknown-linux-gnu \
  --cc aarch64-linux-gnu-gcc --cxx aarch64-linux-gnu-g++
```

- **Platform.**  `cfg(...)` conditions evaluate for the triple's `os`, `arch`, `family`, `env`, and
  `abi` (see [the evaluation context](target-dependencies.md#evaluation-context)); `cabin metadata
  --target <triple>` reports them under `target_platform`.  A triple with an operating system Cabin
  does not recognize, or with characters other than ASCII letters, digits, `-`, `_`, and `.`, is a
  usage error.  `--target` never selects a manifest target.
- **Compilers.**  Clang and Apple Clang build any triple: every compile, assemble, and link command
  passes `--target=<triple>` right after the driver.  GCC, MSVC, and clang-cl build one fixed
  target, which must name the triple's machine (same `arch` and `os`, and the same `env` when both
  name one): GCC's `-dumpmachine`, the MSVC banner's architecture.  Select the cross compiler with
  `--cc` / `--cxx` (or `CC` / `CXX`); the C and C++ compilers must both be Clang-family or both
  not.  A compiler Cabin cannot classify fails the build rather than guessing.  A Windows GNU
  triple (`x86_64-w64-mingw32`) builds with MinGW-w64 GCC (see
  [Known limitations](#known-limitations)); a MinGW compiler does not build an MSVC triple, nor
  MSVC a GNU one.  Executables for any Windows triple are named `<name>.exe`.
- **Output.**  The build writes under `<build-dir>/.targets/<triple>/<profile>/`, apart from the
  build machine's `<build-dir>/<profile>/`, so the two builds never share an object, even with a
  profile named like the triple; `cabin clean --target <triple>` cleans that tree.  The triple
  names a directory, so it must be lowercase, must not end with `.`, and its part before the first
  `.` must not be a Windows device name (`nul.x-linux`); any of those would let two triples, or a
  triple and a device, share one on some platform.
- **Running.**  `cabin run --target` and `cabin test --target` run each executable through the
  [`[runner]`](config.md#runner) configured for the triple, such as an emulator:

  ```toml
  [runner]
  aarch64-unknown-linux-gnu = ["qemu-aarch64", "-L", "/usr/aarch64-linux-gnu"]
  ```

  Without one, a triple with the build machine's `os` and `arch` runs as usual, as does x86 on
  Windows x64; for another machine they build, then fail with an error naming what they built:
  Cabin never runs a binary built for another machine on its own.
- **Build dependencies.**  [Build dependencies](dependency-kinds.md#build-dependencies) build for
  the build machine under `<build-dir>/.targets/<triple>/<profile>/build-machine/`, with the build machine's
  own toolchain: `HOST_CC`, `HOST_CXX`, and `HOST_AR` in place of `CC`, `CXX`, and `AR`, then the
  manifest's `[toolchain]` tables evaluated for the build machine, then the defaults.  `--cc`,
  `--cxx`, `--ar`, `CC`, `CXX`, `AR`, and config's `[toolchain]` choose the target's tools only.
  These units pass no `--target=`, so each build-machine compiler must already build for the build
  machine (GCC's `-dumpmachine`, the others' banner): a cross compiler from an unconditional
  `[toolchain]` table is an error naming `HOST_*`.  Both toolchains must take the same command-line
  dialect, so an MSVC or clang-cl build machine
  with a GCC-style target, or the reverse, is an error, and an MSVC or clang-cl build for another
  machine cannot build them at all: both contexts would link against the one `LIB` environment.  `CFLAGS` and the other environment flags
  describe the target and do not reach these units.
- **Sysroots.**  `--sysroot <dir>`, or the [`[sysroot]`](config.md#sysroot) entry for the
  `--target` triple, names the directory holding the target's headers and libraries; `--sysroot`
  wins.  Every GCC and Clang compile, assemble, and link command passes `--sysroot=<dir>`, or
  `-isysroot <dir>` for an Apple triple, right after `--target=`.  MSVC and clang-cl take the
  Windows SDK from `INCLUDE` and `LIB`, so a sysroot with either is an error.  Without a choice,
  an Apple Clang build for macOS or iOS, and `cabin tidy`'s compile database, pass the
  platform's SDK: an absolute `SDKROOT`, else what `xcrun --sdk <sdk> --show-sdk-path` names
  (`macosx`, `iphoneos`, or `iphonesimulator`).  A sysroot is part of every command line, so
  switching it, or switching Xcode's SDK, rebuilds.  `cabin metadata` reports it under
  `toolchain.sysroot`, with a `source` of `cli`, the config file's source, or `detected`.  Build
  dependencies build with the compilers' own default.  [System dependencies](system-dependencies.md) need a
  pkg-config for the target.

Without `--target`, a compiler whose `--version` banner names another machine draws a warning:
`cfg(...)` conditions still evaluate for the build machine, so pass `--target` to match them to the
compiler.

## Environment variables

Cabin honors `CC`, `CXX`, and `AR` from the running process' environment.  Values are interpreted as
a single executable path or command name; they are not shell-split.  Empty values are ignored, so
`CXX=` looks identical to no `CXX` at all.  Under `--target`, `HOST_CC`, `HOST_CXX`, and `HOST_AR`
choose the build machine's tools for [build dependencies](#building-for-another-platform) the same
way.

Cabin also honors `CPPFLAGS`, `CFLAGS`, `CXXFLAGS`, and `LDFLAGS` as conventional C/C++ build flag
sources.  Each variable is parsed into argv tokens using POSIX shell-style word splitting (via the
[`shlex`] crate; no real shell is invoked) and the parsed tokens are appended to the corresponding
`[profile]` bucket *after* the manifest and pkg-config layers have already contributed.  The build
configuration fingerprint folds in the per-bucket values, so changing a relevant variable moves the
fingerprint exactly the way changing the matching `[profile]` field would.  See
[`environment-variables.md`](environment-variables.md) for the full per-variable routing table,
parsing rules, and the list of commands that participate.

[`shlex`]: https://crates.io/crates/shlex

`LD` is intentionally **not consumed**.  Linker selection would require a linker-command abstraction
the backend lacks; the C / C++ driver decision is what controls whether the C++ runtime is pulled
in.

## Manifest syntax

The workspace root manifest may declare a general `[toolchain]` table plus any number of conditional
`[target.'cfg(...)'.toolchain]` overrides.  Member or path-dep manifests that declare a
`[toolchain]` table are rejected with the error `toolchain selection may only appear in the
workspace root manifest`.  As with `--cxx`, a relative tool path resolves against the current
directory.  Like a project's `.cabin/config.toml`, the manifest may name a tool by path only when
the user [trusts the project](config.md#trust); a bare name is looked up on `PATH`.

```toml
[toolchain]
cc = "clang"
cxx = "clang++"
ar = "llvm-ar"

[target.'cfg(os = "linux")'.toolchain]
ar = "llvm-ar-18"
```

`[profile.<name>.toolchain]` is **not** supported in this step.  Profiles are presets for
compile-time *behavior* (debug, optimization, assertions); they do not switch the compiler binary.
Mixing profile-specific tool selection into the precedence model would create surprising
interactions with target-conditioned overrides; it is deferred to a future step if a use case
emerges.

## Compiler wrappers

Compiler wrappers are build execution configuration, separate from profiles and tool selection:

```toml
[build]
compiler-wrapper = "ccache"
```

The value may be any executable name or path. Cabin resolves it before compiler detection and
prefixes both C and C++ compile commands. It never prefixes link or archive commands and does not
shell-split the value.

Wrapper selection is unconditional. It cannot use target or compiler-family predicates, and it
cannot change `cc`, `cxx`, or `ar`. See [Compiler wrappers](compiler-cache.md) for precedence,
metadata, and fingerprint details.

## `[profile]` flags

`[profile]` declares semantic build flags that compose with each target's per-target `defines` /
`include-dirs`:

```toml
[profile]
defines = ["MY_LIB_BUILD"]
include-dirs = ["include"]
cflags = ["-std=c99"]
cxxflags = ["-fno-rtti"]
ldflags = ["-Wl,--no-undefined"]

[target.'cfg(os = "linux")'.profile]
defines = ["USE_EPOLL"]

[profile.release]
defines = ["NDEBUG_LITE"]

[target.'cfg(os = "linux")'.profile.release]
ldflags = ["-static"]
```

| Field | Type | Notes |
| --- | --- | --- |
| `defines` | `Array<String>` | `"NAME"` or `"NAME=value"`.  One entry per macro across layers, sorted by name; emitted as `-DNAME[=value]`.  A target's own `defines` entry for the same macro wins.  Applied to both C/C++ compiles. |
| `include-dirs` | `Array<Path>` | Relative paths only.  Absolute or `..`-traversal is rejected.  Duplicates collapse to first-occurrence order.  Emitted as `-I<path>`.  Applied to both C/C++ compiles. |
| `cflags` | `Array<String>` | Escape hatch.  Passed verbatim only to **C** compile commands (e.g. `-std=c99`). |
| `cxxflags` | `Array<String>` | Escape hatch.  Passed verbatim only to **C++** compile commands (e.g. `-fno-rtti`, `-std=c++20`). |
| `ldflags` | `Array<String>` | Escape hatch.  Passed verbatim to the link command. |
| `link-libs` | `Array<String>` | Validated bare system-library names.  Propagated transitively to final link commands. |
| `frameworks` | `Array<String>` | Validated bare Apple framework names.  Propagated like `link-libs`, emitted as `-framework <name>`; ignored for non-Apple platforms. |

CFLAGS and CXXFLAGS spaces are kept strictly separate: a flag in `cflags` never reaches the C++
compile line, and a flag in `cxxflags` never reaches the C compile line.

Unknown fields - `compiler`, `toolchain`, etc. - are rejected at parse time.
`[target.'cfg(...)'.profile.<name>]` is a named conditional flag overlay, not a profile definition.
It accepts only the seven array fields above; `inherits`, `debug`, `opt-level`, `assertions`, and
`toolchain` are rejected.  See [Build profiles](profiles.md) for its inheritance-chain semantics.

### Layer order

`[profile]` flags compose layered, in this order (later layers append to / override earlier ones per
field):

1. Built-in backend defaults (currently empty for `[profile]` - the effective per-target standard
   flag (from the manifest-declared standards; see
   [Language standards](language-standards.md)) and the profile's `-O0`, `-g`, etc. come from their
   own layers, not from `[profile]`).
2. The package's own general `[profile]` table.
3. The package's matching `[target.'cfg(...)'.profile]` blocks.
4. For each profile in the selected inheritance chain, root to leaf:
   1. that profile's `[profile.<name>]` array flags from the workspace root;
   2. the package's matching `[target.'cfg(...)'.profile.<name>]` blocks, in manifest order.

CLI flags for `[profile]` fields are not exposed in this step; adding `--define` / `--include-dir`
requires a deliberate schema decision and is deferred.

### Field-level merging

| Field | Merge rule |
| --- | --- |
| `defines` | One entry per macro name, sorted by name.  A later layer's entry replaces an earlier one for the same macro, whatever the spelling (`LEVEL`, `LEVEL=1`). |
| `include-dirs` | Concatenated in layer order, deduplicated keeping the first occurrence.  Order matters for include search. |
| `cflags` | Concatenated in layer order, preserving user-given order within each layer.  No deduplication. |
| `cxxflags` | Same as `cflags`, applied only to C++ compiles. |
| `ldflags` | Same as `cflags`, applied only to link commands. |
| `link-libs` | Concatenated in layer order, deduplicated keeping the first occurrence, then propagated to final link commands in link order ([`link-libs`](profiles.md#link-libs)). |
| `frameworks` | Same as `link-libs`. |

Per-package include directories declared under `[profile]`, `[target.'cfg(...)'.profile]`, or
`[target.'cfg(...)'.profile.<name>]` are resolved against the package manifest directory before
they reach the planner.

### Compiler-conditioned flags

General and named `[target.'cfg(...)'.profile...]` predicates can test the **detected** compiler
with the `cc` / `cxx` (family) and `cc_version` / `cxx_version` (SemVer requirement) keys:

```toml
[target.'cfg(all(cxx = "clang", cxx_version = ">=18"))'.profile]
cxxflags = ["-stdlib=libc++"]
```

The conditions read the same detection report `cabin metadata` shows under `toolchain.detected` (see
[Compiler / tool capability detection](#compiler--tool-capability-detection) below for the family
ids and [target-dependencies](target-dependencies.md) for the full key semantics and placement
rules).  Because a matched general or named layer changes the resolved per-package flags, flipping
a compiler condition - e.g. by upgrading the compiler across a version bound - moves the
[build configuration fingerprint](#build-configuration-fingerprint) automatically.  On the
fail-soft commands (`cabin metadata`, `cabin tidy`, `cabin explain build-config`) a failed detection
evaluates compiler conditions as family `unknown` with no version.

## Compiler / tool capability detection

After tool resolution, Cabin runs each selected tool with `--version`, parses the output, and
assembles a typed [`ToolchainDetectionReport`].  The CLI uses the report to validate that the
compiler / archiver Cabin picked can run the commands the C++ backend emits before any Ninja file is
written.  `cabin metadata` surfaces the same report under `toolchain.detected` so users can audit
detection without re-running anything.

Each `--version` subprocess has a 60-second deadline.  A compiler, archiver, or wrapper that never
exits is treated as a detection failure instead of hanging Cabin indefinitely.  The deadline allows
for a slow first run: under an `SDKROOT` its lookup cache has not seen, an `xcrun` shim such as
`/usr/bin/c++` runs `xcodebuild` twice before the tool.

### Recognized compiler families

The detected `kind` ids below are also the value space for `cfg(cc = "...")` / `cfg(cxx = "...")`
conditions on profile flag tables (see [target-dependencies](target-dependencies.md)).

| Detected `kind`  | Trigger in `--version` output                                  | Backend status        |
| ---------------- | -------------------------------------------------------------- | --------------------- |
| `clang`          | `clang version <semver>`                                       | Supported             |
| `apple-clang`    | `Apple clang version <semver>` (or the pre-Xcode-10 `Apple LLVM version <semver>` banner) | Supported             |
| `clang-cl`       | `clang version <semver>` **and** invoked as `clang-cl` (the banner alone is plain Clang; the name is the deciding signal) | Supported - Clang's `cl.exe`-compatible driver; drives the MSVC dialect with Clang's diagnostics. |
| `gcc`            | `g++` / `gcc` banner, a `(GCC)` vendor parenthetical, or the `Free Software Foundation` line | Supported (GCC ≥ 5)   |
| `msvc`           | `Microsoft (R) C/C++ Optimizing Compiler …` (printed to stderr; `cl` exits non-zero with no input, which detection tolerates) | Supported - drives the MSVC dialect (`cl /std:… /c /Fo…`, `lib /OUT:…`). |
| `unknown`        | Anything else | **Detected, rejected** when the build needs a recognized dialect.  The compiler may still appear in `cabin metadata`, but `cabin build` refuses rather than emitting commands the tool likely cannot run. |

### Recognized archiver families

| Detected `kind` | Trigger in `--version` output                                    | Name-based fallback                         | Backend status |
| --------------- | ---------------------------------------------------------------- | ------------------------------------------- | -------------- |
| `ar`            | `GNU ar` / `GNU Binutils` banner                                  | basename `ar` / `ar-<suffix>` (covers BSD `ar`, which has no `--version`) | Supported |
| `llvm-ar`       | `LLVM version <semver>` line in multi-line banner                | basename `llvm-ar` / `llvm-ar-<suffix>`     | Supported |
| `lib`           | `Microsoft (R) Library Manager` banner                            | basename `lib` / `lib.exe`                  | Supported - the MSVC-dialect archiver (`lib /OUT:<lib> <objs>`) |
| `unknown`       | Anything else                                                     | -                                           | **Detected, rejected** when the build needs a recognized archiver |

The basename-based fallback is intentionally narrow: only archivers named `ar`, `llvm-ar`, or `lib`
(with optional version suffix or `.exe`) are reclassified by name.  Anything else remains `unknown`
and is rejected.

`ar crs` adds and replaces members but never drops one, so the GCC/Clang dialect removes the
library before each archive step, through the same kind of shell-free runner as `cabin stamp`
(`cabin recreate <lib> -- ar crs <lib> <objs>`).  A source removed from, or renamed in, a library's
`sources` therefore leaves no stale object behind on the next build.  `lib /OUT:` always writes the
library from scratch, so the MSVC dialect runs it directly.

### Capability set

Each compiler detection records a typed [`CompilerCapabilities`] with these fields:

| Field                       | Used by the planner | Notes |
| --------------------------- | ------------------- | ----- |
| `gcc_style_flags`           | No (follows the kind)     | Supported exactly for GCC, Clang, and Apple Clang, the compilers driven with `-O…`, `-DNAME`, `-Idir`, `-c`, `-o`. |
| `msvc_style_flags`          | No (follows the kind)     | Supported exactly for `cl` and `clang-cl`, the compilers driven with `/O…`, `/D`, `/I`, `/c`, `/Fo`, `/Tp`/`/Tc`. |
| `depfile_mmd_mf`            | No (follows the kind)     | Supported exactly for the GCC/Clang dialect, which writes the make-style `-MD -MF <file>` depfile. |
| `external_include_dirs`     | Yes (MSVC dialect)        | Whether dependency include dirs can be marked as *system* search paths (see [System include directories](#system-include-directories)). `-isystem` is part of the base GCC/Clang dialect; `cl /external:W0 /external:I` is version-gated - `cl` ≥ 19.29 (VS2019 16.10), `clang-cl` ≥ 13.  Never rejects a toolchain: an older `cl` falls back to plain `/I`. |

Each capability records both `supported: bool` and a `source: "version" | "assumed-default" |
"unsupported"` so `cabin metadata` shows whether the answer came from a recognized version banner or
a fallback.  `assumed-default` covers an `unknown` tool (unsupported), `msvc_style_flags` on a
GCC/Clang compiler (unsupported), and a recognized compiler whose version did not parse, which a
version-gated capability treats as supported.

Language-standard support is validated separately, against the standards the selected packages
request rather than a fixed capability field: see the threshold tables in
[Language standards](language-standards.md).

Archiver detection records a smaller [`ArchiverCapabilities`] set:

| Field                    | Used by the planner today | Notes |
| ------------------------ | ------------------------- | ----- |
| `ar_crs`                 | No (follows the kind)     | Supported exactly for `ar` and `llvm-ar`, which run the GNU `ar crs <lib> <objs>` archive command.  MSVC `lib /OUT:` does not need it. |
| `static_library_output`  | No                        | Required to produce a static library.  Reported `supported` for `ar` / `llvm-ar` (`ar crs`) and `lib.exe` (`lib /OUT:`) alike, so `cabin metadata` is honest about the MSVC archiver. |

### Validation against the C++ backend

Before any Ninja file is written, `cabin build` runs the detection report through
`cabin_build::validate_toolchain_for_backend`.  The validator surfaces clear errors when:

- the C++ compiler is `unknown`; every recognized compiler fits the GCC/Clang or the MSVC dialect.
  The C compiler, when a C compile is planned, must likewise be recognized;
- a compiler does not accept the flag spelling for one of the *requested* language standards (the
  effective standards of the planned compiles), so e.g. a too-old `cl` or GCC is rejected up front
  with the offending standard named.  See the threshold tables in
  [Language standards](language-standards.md);
- the archiver is `unknown`; every recognized archiver produces a static library in its dialect (GNU
  `ar crs`, or MSVC `lib /OUT:`);
- the resolved tools span **both** dialects - an MSVC `cl` paired with a GNU `ar`, or a GCC/Clang
  `c++` paired with `lib` (`MixedToolchainDialects`).

The C-compiler checks (its dialect contract, dialect coherence with the C++ compiler, and its
requested C standards) run after planning, gated on the compiles the plan emits: a `cc` slot no
planned compile uses - even when an unrelated target in a dependency package carries `.c` sources -
never gates the build.

`cabin metadata` is fail-soft after toolchain resolution succeeds: detection failures, including
version-probe timeouts, are logged to stderr and the JSON view's `toolchain.detected` field becomes
`null`.  Missing required tools are still resolution errors; set `CXX`, `AR`, or `[toolchain]` when
metadata needs to report a build configuration on a machine without the default tool names.

### Determinism and network access

Detection runs only `tool --version`, plus GCC's `-dumpmachine` under `--target`.  It never
compiles probe sources, never reads sources outside the package being built, and never touches the
network.  Output is captured deterministically; identical toolchains on identical machines yield
identical `ToolchainDetectionReport`s.

### Probes

Where a question needs the compiler itself, Cabin builds a small probe program the way the build
compiles and links it: the same compiler, `--target=`, sysroot, language mode, and flags, spelled by
the same dialect and run from the directory Ninja runs commands in.  A probe checks that a header compiles, that a type is complete, that a symbol is declared
and links, or that a source compiles, links, or runs, and answers one of:

- *yes*;
- *no*, naming the step that failed (compile, link, or run) with the tool's output: the toolchain
  does not support it;
- *unknown*: the program built but would have to run for a platform the build machine cannot run,
  and no [`[runner]`](config.md#runner) is configured, so nothing established the answer.

A compiler or linker that cannot start, crashes, is killed, or runs past the 60-second deadline is
an error, not a *no*; a probe program that crashes is a *no*.  A probe builds in a fresh directory beneath the build's and removes it, and nothing is
cached, so every probe asks the current toolchain.

One command path probes today.  When `cabin build`, `run`, or `test` fails with a configured sysroot
(`--sysroot` or `[sysroot]`), Cabin builds a minimal program with each link driver of the plan and
the flags of one of its compiles, and adds a `help:` naming the sysroot when the program compiles
but does not link, or does not compile.  A build that succeeds, a detected Apple SDK, and every
other command run no probe.

## Build configuration fingerprint

`BuildConfiguration::fingerprint` is a SHA-256 over resolved build-configuration inputs.  The hash
folds in:

| Section                  | Inputs                                                                                  |
| ------------------------ | --------------------------------------------------------------------------------------- |
| features                 | every enabled feature, sorted                                                           |
| profile                  | name, debug, opt-level, assertions                                                      |
| toolchain                | `(kind, spec)` pairs, sorted by tool key                                                |
| compiler-wrapper         | kind, spec, version (or `none`)                                                         |
| target                   | the `--target` triple; absent for a build-machine build                                 |
| sysroot                  | the `--sysroot` or `[sysroot]` value as written; absent for none or a detected Apple SDK |
| build-flags              | `defines`, `include-dirs`, `cflags`, `cxxflags`, `ldflags`, `link-libs`, `frameworks`, plus language-neutral compile args from environment and system dependencies |
| language-standards       | The effective C / C++ standards (package level plus every target, implementation and interface values).  Provenance labels are excluded - only the values move the fingerprint. |
| instruction-sets         | Every target's per-source [`instruction-sets`](targets.md#per-source-instruction-sets), as declared |

The build-flags section uses one labeled sub-bucket per field, so language-neutral, C-only,
C++-only, and link arguments each contribute independently.  Moving a flag between `cflags` and
`cxxflags` produces a *different* fingerprint, even when the argv string is identical, because the
two slots route to different compile commands.

Switching `--cxx`, changing a `[profile]` define, flipping a target-conditioned
`[target.'cfg(...)'.profile]` to a different host platform, adding a flag to `cflags`, or selecting
a different profile all produce a different fingerprint by design - a future cache layer keys on the
same value.

The summary stored on the configuration deliberately records the user-visible *spec* (`clang++`,
`/opt/llvm/bin/clang++`), not the absolute resolved path from PATH.  That keeps the fingerprint
stable across machines that happen to install the same compiler at different filesystem paths.

### Inputs that are *not* part of the fingerprint

The fingerprint is keyed on Cabin's typed resolved inputs.  Anything Cabin does not consume is not
an input:

- `LD` is not consumed by any layer of Cabin (see "Environment variables" above); changing it never
  moves the fingerprint.
- `CPPFLAGS`, `CFLAGS`, `CXXFLAGS`, and `LDFLAGS` *are* consumed and *do* move the fingerprint when
  they change.  They participate in exactly the same per-bucket fashion as the matching compile or
  link argument bucket.
- The local absolute path to a config file (`.cabin/config.toml`, `~/.config/cabin/config.toml`) is
  not part of the fingerprint - only the *resolved values* the file contributed.  Two contributors
  who run Cabin from different working directories with identical config content see the same
  fingerprint.
- The `CABIN_CACHE_DIR` and `--cache-dir` selection drive where artifacts land on disk but do not
  change the compile / link argv, so they are not in the fingerprint either.
- A detected Apple SDK is reported (`toolchain.sysroot`) but not hashed: like a resolved tool path,
  it differs between machines, and a change still reaches every compile and link argv.

### Direct `ninja` invocation

`cabin build` regenerates `build.ninja` and `compile_commands.json` from the current manifest /
config / toolchain inputs every time it runs, rewriting each only when its contents change so an
unchanged file keeps its modification time.  Direct `ninja -C <build-dir>/<profile>` invocations
do *not* re-read `cabin.toml`, `.cabin/config.toml`, or the environment - Ninja rebuilds an output
only when the command line, an explicit input, the depfile, or the output path changes, or when
`cabin-toolchain.json` beside `build.ninja` does.  Every edge depends on that file, which records
each detected compiler's and the archiver's family, version, `--version` line, and target (the
build machine's too, under `--target` with build dependencies), and the `INCLUDE` and `LIB`
search paths the build runs with, and which `cabin build` rewrites only when one of them changes:
a compiler whose reported version changes rebuilds everything, even at the same path, and so does
an `INCLUDE` or `LIB` naming other directories, which `cl`, `clang-cl`, and `link` search for
what no command line names.  Run `cabin build` after manifest, config, or toolchain
edits so the fingerprint and the generated commands reflect the new inputs; running Ninja directly
against a stale `build.ninja` is intentionally supported (it's how IDEs and editor watchers
re-invoke the incremental build) but does not pick up Cabin-level changes.

### Paths Ninja cannot spell

Ninja reads paths in two syntaxes: its own manifest, and the Makefile depfiles GCC and Clang write.

- In `build.ninja`, `$`, space, and `:` have `$` escapes.  `|` has none and is spelled `${pipe}`,
  a reference to a file-level variable holding `|`.  A newline or carriage return cannot appear in
  a Ninja path, so a path containing one is rejected.
- Ninja's depfile parser ends a path at `;`, `|`, `*`, `<`, `>`, `^`, `` ` ``, a tab, or a control
  character, and before Ninja 1.13 also at `'`, `"`, `&`, and `?`, with no escape for any of them;
  the split path never exists, so such a compile would rerun on every build.  A GCC/Clang compile
  whose source, output, compiler, sysroot, include directories, or extra flags contain one of these
  characters runs as `cabin depfile <depfile> -- <compile>`: after the compiler succeeds, Cabin
  reads the depfile itself and reports each dependency on the line-based channel Ninja's
  `deps = msvc` reads.  Every other compile keeps `deps = gcc` and pays nothing extra.

`cabin depfile` reads a path the way GCC and Clang write it: every byte other than an unescaped
space, carriage return, or newline is part of it, so a tab, which GCC escapes and Clang writes as
is, stays in the name.  Where Ninja's parser reads a depfile right, both record the same dependencies.

What remains:

- Cabin decides from the paths it passes, so a header whose own file name contains such a
  character, below an ordinary include directory, still reaches Ninja's parser: that compile reruns
  on every build instead of going stale.
- On Linux and macOS, Clang writes `\` in a path as `/`, so such a path loses its identity in
  Clang's depfile, with the same effect.
- Ninja's `.ninja_log` separates its fields with tabs, so a command whose output path holds a tab,
  such as one below a build directory named with a tab, is never found there and reruns on every
  build.
- Ninja's `deps = msvc` drops a dependency whose path contains `program files` or
  `microsoft visual studio` (case-insensitively) as a system header, and on Windows rejects one
  longer than 260 characters, failing the build.  `cabin depfile` reports a compile with such a
  dependency as depending on a file that never exists instead: it reruns on every build rather
  than going stale or failing.  On Windows, Ninja resolves that file's name against the
  directory it runs in (`build/dev`) within 260 characters, so a path to that directory of 233
  characters or more still fails such a build.  `cabin depfile` checks the absolute paths the
  compiler writes, not the relative ones Ninja checks on Windows (see below), so it also reruns
  such a compile in a workspace below `Program Files`.  A GNU-driver Clang targeting MSVC takes
  the standard headers from MSVC and the Windows SDK, below `Program Files` where they install by
  default, so there a compile that goes through `cabin depfile` and includes any of them reruns on
  every build.
- A `cl` compile, and a `clang-cl` compile that keeps `/showIncludes` (see
  [`clang-cl`](#windows--msvc)), report headers through `/showIncludes`, whose output goes through
  the same filter, so its object does not depend on a header whose path contains either string,
  which the Windows SDK's and MSVC's headers do where they install by default.  Ninja first makes each path relative to the
  directory it runs in, unless the header is on another drive, so only the part below the deepest
  directory the two share counts: a workspace inside `Program Files` with the default build
  directory is tracked, but a header below a directory named `Microsoft Visual Studio` inside the
  workspace is not.  Ninja compares the paths as spelled, and Cabin spells the build directory from
  the working directory but the headers from the workspace's canonical path, so a working directory
  spelled differently, such as with an 8.3 short name, shares only the part before the first
  difference.  Editing or updating
  such a header rebuilds nothing unless `cabin-toolchain.json` changes too, as it does when a
  compiler or the archiver reports a new version or `INCLUDE` or `LIB` changes, such as for
  another Windows SDK, and that rebuilds everything.

### Long command lines

Archive and link commands list every object, so a large target can outgrow the host's command-line
limit: 32,767 characters for `CreateProcess` on Windows, and 128 KiB for the single `/bin/sh -c`
argument Ninja runs on Linux.  Past 24 KiB on Windows or 120 KiB on Linux, the command passes its
arguments through a response file beside the output (`ar @libfoo.a+.rsp`, `cl @app.exe+.rsp`) that
Ninja writes before running it and deletes after it succeeds.  GCC, Clang, and GNU `ar` read it with
backslash escapes, and so does `llvm-ar` except from LLVM 11 on a Windows host, where it reads one
like a command line and Cabin quotes it to match.  There Cabin also passes `--rsp-quoting=windows`:
an `llvm-ar` whose version it could not read is taken to be 11 or later, and an older one rejects
the option instead of misreading the file.  MSVC tools read it like a command line, as UTF-8
marked by a byte order mark, and reject a line of 128 KiB or more (LNK1170), so their inputs go one
per line through Ninja's `$in_newline`, with the options around them, `/link` and the linker options
it starts last, on the first and last lines.  Other hosts limit only the total argument size, and
the `ar` of macOS and the BSDs reads no response files, so commands there stay inline.  Compile
commands stay inline everywhere: their length grows with include directories, not with the number of
sources in a target.

## Package + index metadata

`cabin package` writes the manifest's declared `[toolchain]` and `[profile]` tables into the
canonical `<name>-<version>.json` so consumers who rebuild from source can reproduce the same
compile flags. **Environment- or CLI-derived selections are never written.** A user-set `CXX=...` or
`--cxx /opt/llvm/...` only affects the local invocation; it never leaks into a published archive.

The local file registry and the sparse-HTTP index round-trip the same fields opaquely.  Older
registries that omit the new fields continue to load.  The resolver itself does not consult any of
these values - registry resolution remains profile- and toolchain-independent.

## Windows / MSVC

Windows is a supported platform, driven by the **MSVC** dialect.  CI builds, links, runs, and tests
the example packages that need no outbound network on a `windows-2025-vs2026` runner.

### What works

- **Default toolchain, auto-discovered.** On Windows the resolver defaults to `cl` for both C and
  C++ and `lib` for archiving (see the [tool-kinds table](#tool-kinds)), and locates them - plus the
  `INCLUDE` / `LIB` a compile needs - from the installed Visual Studio even when no Developer
  Command Prompt is active (see [Toolchain discovery](#toolchain-discovery)).  No configuration is
  needed on a stock MSVC install.
- **All toolchain-driven subcommands.** `cabin build`, `run`, `test`, `check`, `fmt`, `tidy`,
  `metadata`, and `explain build-config` work with MSVC.  Executables get the host `.exe` suffix;
  static libraries are `.lib`; objects are `.obj`.
- **Cabin's full source-extension set.** `.c` compiles as C; `.cc`, `.cpp`, `.cxx`, `.c++`, and `.C`
  compile as C++.  The language is driven explicitly (`cl /Tp<file>` / `/Tc<file>`) rather than left
  to `cl`'s extension inference, so every supported extension compiles as the language Cabin
  classified it.  A `.asm` source assembles with the `ml64.exe` beside the resolved `cl.exe`.
- **Command mapping.** `cl /nologo /utf-8 /std:c++17 /EHsc /O2 /Z7 /showIncludes /D ... /I ... /c
  /Tp<src> /Fo<obj>` for compiles (Ninja consumes `/showIncludes` via `deps = msvc`); `lib /nologo
  /OUT:<lib> <objs>` for archives; `ml64 /nologo /c /D ... /I ... /Fo <obj> <src>` for assembly;
  `cl /nologo <inputs> /Fe<exe> /link /SUBSYSTEM:CONSOLE <ldflags>` for links.  Naming the
  subsystem lets `main` come from a static library: `link.exe` infers the subsystem before it
  searches archives, so a `main` that only an archive member defines, with nothing else pulling
  that member in, otherwise fails with LNK1561.
  Every executable therefore links as a console program, as CMake and Meson default to.  A
  `WinMain` executable passes `/SUBSYSTEM:WINDOWS` in `ldflags`, which comes later and wins but
  applies to every executable in the package.
  `/utf-8` pins the source and execution character sets to UTF-8, matching the GCC/Clang default -
  without it `cl` interprets sources in the machine's active code page, and UTF-8-requiring headers
  (such as {fmt}'s) fail to compile.  With it `/showIncludes` names headers in UTF-8 too, which
  Ninja's Windows release binaries from 1.11 on read as UTF-8 on Windows 10 version 1903 or later,
  so a header path outside the active code page is tracked.  `cabin check`
  runs its syntax-only compile through a shell-free `cabin stamp` witness writer that stamps the
  output on a zero exit, so the rule is identical on every host and build paths containing shell
  metacharacters (`&`, `|`, `(`, `)`) never need escaping.
- **`clang-cl`.** LLVM's `cl.exe`-compatible driver is detected by its invoked name (its `--version`
  banner is a plain `clang version`) and drives the MSVC dialect, so `CXX=clang-cl AR=lib` is a
  coherent MSVC toolchain.  It keeps Clang's diagnostics while emitting MSVC-style flags.  Pair it
  with `lib.exe` (the MSVC archiver).  In place of `/showIncludes` it writes a Makefile depfile
  (`/clang:-MD /clang:-MF<file>`) that Ninja reads with `deps = gcc`, so its objects depend on the
  Windows SDK's and MSVC's headers, which `deps = msvc` drops (see
  [Paths Ninja cannot spell](#paths-ninja-cannot-spell)).  It keeps `/showIncludes` run through a
  compiler wrapper, whose cache hit replays that output but need not write the depfile, and where
  one of the paths the depfile is spelled from would send it through `cabin depfile`, which would
  rerun every compile that includes a Windows SDK header.  `compile_commands.json` records
  `/showIncludes` either way, which clang-tidy drops.
- **MSVC version is validated.** Cabin checks every requested language standard against the detected
  `cl` version before the build - `/std:c++17` needs `cl` 19.11 (VS2017 15.3), `/std:c++20` needs
  `cl` 19.29 (VS2019 16.11), `/std:c11` / `/std:c17` need `cl` 19.28 (VS2019 16.8), and standards
  with no stable `/std:` flag are rejected outright - so a `cl` too old for the standard it would
  emit fails with a clear error up front rather than at the first compile.  See
  [Language standards](language-standards.md) for the full tables.
- **Foundation ports.** The zlib port builds under MSVC (Unix-only defines such as
  `HAVE_UNISTD_H` are gated behind `cfg(family = "unix")`).

### Toolchain discovery

Cabin needs a Visual Studio (or Build Tools) installation, but **not** a pre-activated environment.
When `cl.exe` / `lib.exe` and `INCLUDE` / `LIB` are not already on the environment, Cabin discovers
the installed toolchain via the [`find-msvc-tools`](https://crates.io/crates/find-msvc-tools) crate
- resolving the absolute paths to `cl` / `lib` / `link` and layering the `INCLUDE` / `LIB` / `PATH`
the compile needs onto Ninja's environment.  So a stock install builds without a Developer Command
Prompt.

If Cabin is already running inside an activated environment (a Developer Command Prompt, or
`vcvarsall.bat` / [`ilammy/msvc-dev-cmd`](https://github.com/ilammy/msvc-dev-cmd) in CI - detected
by `INCLUDE` / `LIB` being set), it uses that environment unchanged and skips discovery, so an
explicitly selected toolset is honored.

### Known limitations

- **Of the GCC/Clang-style toolchains on Windows, CI tests only MinGW-w64 GCC, under `--target
  x86_64-w64-mingw32`.** On the `windows-2025-vs2026` image, `cabin run --target
  x86_64-w64-mingw32 --cxx g++` builds, links, and runs C, C++, and mixed C/C++ examples, and
  `cabin test` runs a package's tests the same way.  A GCC-style `CXX` (or `CC`) brings the C
  compiler and archiver beside it rather than `cl` and `lib` (see [Tool kinds](#tool-kinds)),
  executables are named `<name>.exe`, and `cabin tidy` spells its compile database in the
  *resolved* compiler's dialect, not the host default.  Tools chosen explicitly from two dialects
  are still rejected by the [single-dialect validation](#validation-against-the-c-backend).
  Without `--target`, `cfg(target_env)` evaluates `msvc` on Windows whatever the compiler; with it,
  conditions evaluate for MinGW, and build dependencies then need `HOST_CC`, `HOST_CXX`, and
  `HOST_AR`, since the build machine's default is `cl`.  Clang targeting MinGW, MSYS2's other
  environments, and build or `system = true` dependencies under MinGW are untested.
- **`system = true` dependencies are not supported under MSVC.** They are resolved with pkg-config,
  whose GNU-style `-L` / `-lfoo` / `-pthread` output the MSVC `cl` / `link` command line cannot
  consume; on Windows the `.pc` files also come from MinGW/msys2 and reference the MinGW ABI, so
  translating the tokens would link the wrong libraries.  A build, run, or test that needs an active
  system dependency under an MSVC toolchain is rejected with a clear error before any probe runs.
  Use a GCC/Clang toolchain for packages with system dependencies.
- **Very old MSVC is rejected, not supported.** A `cl` older than the `/std:` switch it would emit
  (Visual Studio 2017 15.3 for `/std:c++17`, Visual Studio 2019 16.8 for `/std:c11`) is refused at
  validation with a clear error rather than down-shifting the standard.  Cabin targets a current
  Visual Studio (CI uses VS 2026).

## Deferred / out of scope

- Probe checks a manifest declares, such as those behind a generated configuration header.
- Sysroot discovery beyond Apple SDKs, and a sysroot for the build-machine context.
- A diagnostics abstraction (SARIF / JSON output formats).  Cabin neither detects nor emits either
  format.
- Per-package profile overrides for `[toolchain]`.
- A CLI escape hatch for raw compile / link flags (`--cflag`).
- `LD` env-var honoring (linker selection requires a linker-command abstraction the backend lacks).
