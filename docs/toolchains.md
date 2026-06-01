# Toolchains and conditional build flags

Cabin builds C/C++ packages with three external tools: a C
compiler, a C++ compiler driver, and a static-library archiver.
Cabin makes the choice of those tools, and the semantic flags
applied during compile / link, *explicit, typed, and
deterministic*.

This document is the canonical specification for the toolchain
selection model and the `[profile]` flag schema. The behavior
described here is what the manifest parser
(`cabin-manifest`), the typed model (`cabin-core::toolchain` and
`cabin-core::build_flags`), the resolver (`cabin-toolchain`), the
build planner (`cabin-build`), the CLI (`cabin`), and the
canonical package metadata (`cabin-package`) all agree on.

## Tool kinds

Three tool kinds participate:

| Kind  | Manifest key | CLI flag  | Env var | Default fallback list |
| ----- | ------------ | --------- | ------- | --------------------- |
| `cc`  | `cc`         | `--cc`    | `CC`    | `cc`, `clang`, `gcc`  |
| `cxx` | `cxx`        | `--cxx`   | `CXX`   | `c++`, `clang++`, `g++` |
| `ar`  | `ar`         | `--ar`    | `AR`    | `ar`                  |

The C compiler (`cc`) and the C++ compiler (`cxx`) are
**separate** tool selections. They may resolve to the same
underlying executable when the system installs a unified driver
(`gcc` / `clang` is also a valid C front-end), but the model
keeps the slots independent so Cabin can pick the right driver
per source language. The build planner classifies each source
file by its filename extension (`.c` is C; `.cc` / `.cpp` /
`.cxx` / `.c++` / `.C` are C++) and dispatches each compile
through the matching driver.

The link driver is *also* selected per target: every linked
executable inspects its own objects plus every transitively
reachable library object, picks **C++** if any of those are C++,
and otherwise picks **C**. There is no separate `--linker`
selection; the C/C++ driver decision is what controls whether
the C++ runtime (libstdc++ / libc++) is pulled in.

A linker-style env variable (`LD`) is intentionally not honored
— adding linker selection would require a linker-command
abstraction the backend lacks. MSVC `cl.exe` /
`link.exe` are recognized and explicitly rejected with a clear
error so a misconfigured build does not silently flow through a
compiler that does not accept GCC-style flags.

## Precedence

For every tool kind, the resolver walks the layers below in
order and keeps the first one that yields a value:

1. **CLI flag** — `--cc`, `--cxx`, `--ar`. Highest precedence.
2. **Environment variable** — `CC`, `CXX`, `AR`. An empty value
   is treated as unset; values are not shell-split.
3. **`[toolchain]` config-file layer** — `[toolchain]` in
   `<root>/.cabin/config.toml` (workspace or project) or
   `~/.config/cabin/config.toml` (user). See
   [`config.md`](config.md) for the full file-discovery rules.
4. **Matching `[target.'cfg(...)'.toolchain]`** — the first
   conditional toolchain block whose predicate evaluates to
   `true` against the host platform.
5. **`[toolchain]`** — the workspace root manifest's general
   toolchain table.
6. **Built-in defaults** — Cabin's documented fallback list.

Each [`ResolvedTool`] records which layer it came from on its
`source` field. `cabin metadata` surfaces the same value
verbatim under `toolchain.tools.<kind>.source`, so users can
audit a build without re-deriving the precedence by hand.

## CLI surface

```sh
cabin build --cxx clang++
cabin build --cc /opt/llvm/bin/clang --cxx /opt/llvm/bin/clang++
cabin metadata --cxx clang++         # report the toolchain a build *would* pick
```

`--cc` / `--cxx` / `--ar` accept either a bare command name
(resolved against `PATH`) or an explicit filesystem path.
Whitespace-only values are rejected at parse time. The flags
apply only to the current command invocation; nothing is written
back to the manifest.

`cabin build` and `cabin metadata` accept the flags. `cabin
resolve`, `cabin update`, `cabin fetch`, `cabin package`, and
`cabin publish` deliberately do **not** accept them — toolchain
selection has no effect on dependency resolution, the lockfile,
or the published archive.

## Environment variables

Cabin honors `CC`, `CXX`, and `AR` from the running process'
environment. Values are interpreted as a single executable
path or command name; they are not shell-split. Empty values
are ignored, so `CXX=` looks identical to no `CXX` at all.

Cabin also honors `CPPFLAGS`, `CFLAGS`, `CXXFLAGS`, and
`LDFLAGS` as conventional C/C++ build flag sources. Each
variable is parsed into argv tokens using POSIX shell-style
word splitting (via the [`shlex`] crate; no real shell is
invoked) and the parsed tokens are appended to the corresponding
`[profile]` bucket *after* the manifest and pkg-config layers
have already contributed. The build configuration fingerprint
folds in the per-bucket values, so changing a relevant variable
moves the fingerprint exactly the way changing the matching
`[profile]` field would. See
[`environment-variables.md`](environment-variables.md)
for the full per-variable routing table, parsing rules, and the
list of commands that participate.

[`shlex`]: https://crates.io/crates/shlex

`LD` is intentionally **not consumed**. Linker selection would
require a linker-command abstraction the backend lacks; the C /
C++ driver decision is what controls whether the C++ runtime is
pulled in.

## Manifest syntax

The workspace root manifest may declare a general `[toolchain]`
table plus any number of conditional `[target.'cfg(...)'.toolchain]`
overrides. Member or path-dep manifests that declare a
`[toolchain]` table are rejected with the error
`toolchain selection may only appear in the workspace root manifest`.

```toml
[toolchain]
cc = "clang"
cxx = "clang++"
ar = "llvm-ar"

[target.'cfg(os = "linux")'.toolchain]
ar = "llvm-ar-18"
```

`[profile.<name>.toolchain]` is **not** supported in this step.
Profiles are presets for compile-time *behavior* (debug,
optimization, assertions); they do not switch the compiler
binary. Mixing profile-specific tool selection into the
precedence model would create surprising interactions with
target-conditioned overrides; it is deferred to a future step
if a use case emerges.

## `[profile]` flags

`[profile]` declares semantic build flags that compose with each
target's per-target `defines` / `include_dirs`:

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
```

| Field | Type | Notes |
| --- | --- | --- |
| `defines` | `Array<String>` | `"NAME"` or `"NAME=value"`. Sorted and deduplicated across layers; emitted as `-DNAME[=value]`. Applied to both C/C++ compiles. |
| `include-dirs` | `Array<Path>` | Relative paths only. Absolute or `..`-traversal is rejected. Duplicates collapse to first-occurrence order. Emitted as `-I<path>`. Applied to both C/C++ compiles. |
| `cflags` | `Array<String>` | Escape hatch. Passed verbatim only to **C** compile commands (e.g. `-std=c99`). |
| `cxxflags` | `Array<String>` | Escape hatch. Passed verbatim only to **C++** compile commands (e.g. `-fno-rtti`, `-std=c++20`). |
| `ldflags` | `Array<String>` | Escape hatch. Passed verbatim to the link command. |

CFLAGS and CXXFLAGS spaces are kept strictly separate: a flag in
`cflags` never reaches the C++ compile line, and a flag in
`cxxflags` never reaches the C compile line.

Unknown fields — `compiler`, `toolchain`, etc. — are rejected at
parse time.

### Layer order

`[profile]` flags compose layered, in this order (later layers append
to / override earlier ones per field):

1. Built-in backend defaults (currently empty for `[profile]` —
   Cabin's `-std=c11` (for C compiles), `-std=c++17` (for C++
   compiles), `-O0`, `-g`, etc. come from the profile, not from
   `[profile]`).
2. The package's own general `[profile]` table.
3. The package's matching `[target.'cfg(...)'.profile]` blocks.
4. The selected profile's `[profile.<name>]` block from the
   workspace root manifest.

CLI flags for `[profile]` fields are not exposed in this step;
adding `--define` / `--include-dir` requires a deliberate
schema decision and is deferred.

### Field-level merging

| Field | Merge rule |
| --- | --- |
| `defines` | Union across layers, sorted and deduplicated. Define order does not matter for `-D`. |
| `include_dirs` | Concatenated in layer order, deduplicated keeping the first occurrence. Order matters for include search. |
| `cflags` | Concatenated in layer order, preserving user-given order within each layer. No deduplication. |
| `cxxflags` | Same as `cflags`, applied only to C++ compiles. |
| `ldflags` | Same as `cflags`, applied only to link commands. |

Per-package include directories declared under `[profile]` /
`[target.'cfg(...)'.profile]` are resolved against the package
manifest directory before they reach the planner.

## Compiler / tool capability detection

After tool resolution, Cabin runs each selected tool with
`--version`, parses the output, and assembles a typed
[`ToolchainDetectionReport`]. The CLI uses the report to validate
that the compiler / archiver Cabin picked can actually run the
commands the C++ backend emits before any Ninja file is written.
`cabin metadata` surfaces the same report under
`toolchain.detected` so users can audit detection without
re-running anything.

Each `--version` subprocess has a bounded deadline. A compiler,
archiver, or wrapper that never exits is treated as a detection
failure instead of hanging Cabin indefinitely.

### Recognized compiler families

| Detected `kind`  | Trigger in `--version` output                                  | Backend status        |
| ---------------- | -------------------------------------------------------------- | --------------------- |
| `clang`          | `clang version <semver>`                                       | Supported             |
| `apple-clang`    | `Apple clang version <semver>`                                 | Supported             |
| `gcc`            | `g++` / `gcc` banner with the `Free Software Foundation` line  | Supported (GCC ≥ 5)   |
| `msvc`           | `Microsoft (R) C/C++ Optimizing Compiler …`                    | **Detected, rejected with a clear error** — the current backend emits GCC-style commands and cannot drive `cl.exe`. |
| `unknown`        | Anything else (or a `--version` invocation that exits non-zero) | **Detected, rejected** when the build needs GCC-style flags. The compiler may still appear in `cabin metadata`, but `cabin build` refuses rather than emitting commands the tool likely cannot run. |

### Recognized archiver families

| Detected `kind` | Trigger in `--version` output                                    | Name-based fallback                         | Backend status |
| --------------- | ---------------------------------------------------------------- | ------------------------------------------- | -------------- |
| `ar`            | `GNU ar` / `GNU Binutils` banner                                  | basename `ar` / `ar-<suffix>` (covers BSD `ar`, which has no `--version`) | Supported |
| `llvm-ar`       | `LLVM version <semver>` line in multi-line banner                | basename `llvm-ar` / `llvm-ar-<suffix>`     | Supported |
| `lib`           | `Microsoft (R) Library Manager` banner                            | basename `lib` / `lib.exe`                  | **Detected, rejected** — `lib.exe` cannot run `ar crs` |
| `unknown`       | Anything else                                                     | —                                           | **Detected, rejected** when the build needs `ar crs`-compatible behavior |

The basename-based fallback is intentionally narrow: only
archivers literally named `ar`, `llvm-ar`, or `lib` (with optional
version suffix or `.exe`) are reclassified by name. Anything else
remains `unknown` and is rejected.

### Capability set

Each compiler detection records a typed
[`CompilerCapabilities`] with these fields:

| Field                       | Used by the planner | Notes |
| --------------------------- | ------------------- | ----- |
| `gcc_style_flags`           | Yes                 | Required for `-O…`, `-DNAME`, `-Idir`, `-c`, `-o`. Missing → unsupported. |
| `msvc_style_flags`          | No                  | Detection-only; the planner does not emit MSVC syntax. |
| `depfile_mmd_mf`            | Yes                       | Required for `-MMD -MF <file>`. Missing → unsupported. |
| `std_flag`                  | Yes                       | Required for `-std=…`. Missing → unsupported. |
| `cxx_standard_17`           | Yes                       | The planner emits `-std=c++17`; detection rejects compilers older than GCC 5. |
| `color_diagnostics_flag`    | No                        | Detection-only. |
| `response_files`            | No                        | Detection-only. |
| `json_diagnostics`          | No                        | Detection-only — Cabin does not emit JSON diagnostics. |
| `sarif_diagnostics`         | No                        | Detection-only — Cabin does not emit SARIF. |

Each capability records both `supported: bool` and a
`source: "version" | "probe" | "assumed-default" | "unsupported"`
so `cabin metadata` shows whether the answer came from a
recognized version banner or a conservative fallback.

Archiver detection records a smaller [`ArchiverCapabilities`]
set:

| Field                    | Used by the planner today | Notes |
| ------------------------ | ------------------------- | ----- |
| `ar_crs`                 | Yes                       | Required for the `ar crs <lib> <objs>` archive command. Missing → unsupported. |
| `static_library_output`  | Yes                       | Required to produce `.a` archives. Missing → unsupported. |

### Validation against the C++ backend

Before any Ninja file is written, `cabin build` runs the
detection report through `cabin_build::validate_toolchain_for_backend`.
The validator surfaces clear errors when:

- the C++ compiler is `msvc` or `unknown` (with a missing
  `gcc_style_flags` capability);
- the C++ compiler lacks `depfile_mmd_mf` or `cxx_standard_17`;
- the archiver is `lib` or otherwise lacks `ar_crs`.

`cabin metadata` is fail-soft after toolchain resolution succeeds:
detection failures, including version-probe timeouts, are logged to
stderr and the JSON view's `toolchain.detected` field becomes
`null`. Missing required tools are still resolution errors; set
`CXX`, `AR`, or `[toolchain]` when metadata needs to report a build
configuration on a machine without the default tool names.

### Determinism and network access

Detection runs only `tool --version`. It never compiles probe
sources, never reads sources outside the package being built,
and never touches the network. Output is captured deterministically;
identical toolchains on identical machines yield identical
`ToolchainDetectionReport`s.

## Build configuration fingerprint

`BuildConfiguration::fingerprint` is a SHA-256 over the
resolved build-configuration inputs that Cabin exposes to
metadata, `cabin run`, and `cabin test`. The hash folds in:

| Section                  | Inputs                                                                                  |
| ------------------------ | --------------------------------------------------------------------------------------- |
| features                 | every enabled feature, sorted                                                           |
| profile                  | name, debug, opt-level, assertions                                                      |
| toolchain                | `(kind, spec)` pairs, sorted by tool key                                                |
| compiler-wrapper         | kind, spec, version (or `none`)                                                         |
| build-flags              | `defines`, `include-dirs`, `cflags`, `cxxflags`, `ldflags`, plus language-neutral compile args from environment and system dependencies |

The build-flags section uses one labeled sub-bucket per field,
so language-neutral, C-only, C++-only, and link arguments each
contribute independently. Moving a flag between `cflags` and
`cxxflags` produces a *different* fingerprint, even when the argv
string is identical, because the two slots route to different
compile commands.

Switching `--cxx`, changing a `[profile]` define, flipping a
target-conditioned `[target.'cfg(...)'.profile]` to a different
host platform, adding a flag to `cflags`, or
selecting a different profile all produce a different
fingerprint by design — a future cache layer keys on the
same value.

The summary stored on the configuration deliberately records
the user-visible *spec* (`clang++`, `/opt/llvm/bin/clang++`),
not the absolute resolved path from PATH. That keeps the
fingerprint stable across machines that happen to install the
same compiler at different filesystem paths.

### Inputs that are *not* part of the fingerprint

The fingerprint is keyed on Cabin's typed resolved inputs.
Anything Cabin does not consume is not an input:

- `LD` is not consumed by any layer of Cabin (see
  "Environment variables" above); changing it never moves the
  fingerprint.
- `CPPFLAGS`, `CFLAGS`, `CXXFLAGS`, and `LDFLAGS` *are*
  consumed and *do* move the fingerprint when they change.
  They participate in exactly the same per-bucket fashion as
  the matching compile or link argument bucket.
- The local absolute path to a config file (`.cabin/config.toml`,
  `~/.config/cabin/config.toml`) is not part of the
  fingerprint — only the *resolved values* the file
  contributed. Two contributors who run Cabin from different
  working directories with identical config content see the
  same fingerprint.
- The `CABIN_CACHE_DIR` and `--cache-dir` selection drive where
  artifacts land on disk but do not change the compile / link
  argv, so they are not in the fingerprint either.

### Direct `ninja` invocation

`cabin build` regenerates `build.ninja` and
`compile_commands.json` from the current manifest / config /
toolchain inputs every time it runs. Direct `ninja -C
<build-dir>` invocations do *not* re-read `cabin.toml`,
`.cabin/config.toml`, or the environment — Ninja rebuilds an
output only when the command line, an explicit input, the
depfile, or the output path changes. Run `cabin build` after
manifest, config, or toolchain edits so the fingerprint and
the generated commands reflect the new inputs; running Ninja
directly against a stale `build.ninja` is intentionally
supported (it's how IDEs and editor watchers re-invoke the
incremental build) but does not pick up Cabin-level changes.

## Package + index metadata

`cabin package` writes the manifest's declared `[toolchain]` and
`[profile]` tables into the canonical `<name>-<version>.json`
so consumers who rebuild from source can reproduce the same
compile flags. **Environment- or
CLI-derived selections are never written.** A user-set `CXX=...`
or `--cxx /opt/llvm/...` only affects the local invocation; it
never leaks into a published archive.

The local file registry and the sparse-HTTP index round-trip the
same fields opaquely. Older registries that omit the new fields
continue to load. The resolver itself does not consult any of
these values — registry resolution remains profile- and
toolchain-independent.

## Deferred / out of scope

- Compiler probe compilations beyond running `--version`.
- Compiler-specific conditional flags
  (`cfg(compiler = "clang")` / `cfg(compiler_version = ...)`).
- distcc / icecc wrapper integration. (`ccache` / `sccache` are
  supported — see [docs/compiler-cache.md](compiler-cache.md).)
- Sysroot or SDK discovery.
- Full Windows / MSVC support (`cl.exe`, `link.exe`, MSBuild,
  Visual Studio detection); MSVC is *detected* but the C++
  backend cannot drive it.
- A diagnostics abstraction (SARIF / JSON output formats).
  Capabilities for these are detected, but Cabin does not emit
  either format.
- Per-package profile overrides for `[toolchain]`.
- A CLI escape hatch for raw compile / link flags (`--cflag`).
- `LD` env-var honoring (linker selection requires a
  linker-command abstraction the backend lacks).
