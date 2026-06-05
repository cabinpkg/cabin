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

| Kind  | Manifest key | CLI flag  | Env var | Default fallback list (Unix) | Default fallback list (Windows) |
| ----- | ------------ | --------- | ------- | --------------------- | --------------------- |
| `cc`  | `cc`         | `--cc`    | `CC`    | `cc`, `clang`, `gcc`  | `cl`, `clang`, `gcc`  |
| `cxx` | `cxx`        | `--cxx`   | `CXX`   | `c++`, `clang++`, `g++` | `cl`, `clang++`, `g++` |
| `ar`  | `ar`         | `--ar`    | `AR`    | `ar`                  | `lib`, `llvm-ar`, `ar` |

The default fallbacks are host-dependent: on Windows the MSVC toolchain
(`cl` for both C and C++, `lib` for archiving) comes first, so a stock
Windows install resolves to MSVC without any configuration. See
[Windows / MSVC](#windows--msvc) for the dialect model and its
limitations.

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
abstraction the backend lacks. The C++ compiler drives linking via
`cl /Fe… /link …` (MSVC) or the GCC/Clang driver, so `link.exe` is
never selected directly.

Cabin drives **two** command-line dialects: GCC/Clang style
(`-std=…`, `-c`, `-o`, `-MMD`) and MSVC style (`cl /std:… /c /Fo…
/showIncludes`, `lib /OUT:…`). The C++ compiler picks the dialect —
`cl.exe` selects MSVC, every other recognized family selects
GCC/Clang — and the archiver and optional C compiler must agree (see
[Validation](#validation-against-the-c-backend)). The dialect lowering
lives in [`cabin-driver`]; see [architecture.md](architecture.md) for
the IR.

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
| `clang-cl`       | `clang version <semver>` **and** invoked as `clang-cl` (the banner alone is plain Clang; the name is the deciding signal) | Supported — Clang's `cl.exe`-compatible driver; drives the MSVC dialect with Clang's diagnostics. |
| `gcc`            | `g++` / `gcc` banner with the `Free Software Foundation` line  | Supported (GCC ≥ 5)   |
| `msvc`           | `Microsoft (R) C/C++ Optimizing Compiler …` (printed to stderr; `cl` exits non-zero with no input, which detection tolerates) | Supported — drives the MSVC dialect (`cl /std:… /c /Fo…`, `lib /OUT:…`). |
| `unknown`        | Anything else (or a `--version` invocation that exits non-zero) | **Detected, rejected** when the build needs a recognized dialect. The compiler may still appear in `cabin metadata`, but `cabin build` refuses rather than emitting commands the tool likely cannot run. |

### Recognized archiver families

| Detected `kind` | Trigger in `--version` output                                    | Name-based fallback                         | Backend status |
| --------------- | ---------------------------------------------------------------- | ------------------------------------------- | -------------- |
| `ar`            | `GNU ar` / `GNU Binutils` banner                                  | basename `ar` / `ar-<suffix>` (covers BSD `ar`, which has no `--version`) | Supported |
| `llvm-ar`       | `LLVM version <semver>` line in multi-line banner                | basename `llvm-ar` / `llvm-ar-<suffix>`     | Supported |
| `lib`           | `Microsoft (R) Library Manager` banner                            | basename `lib` / `lib.exe`                  | Supported — the MSVC-dialect archiver (`lib /OUT:<lib> <objs>`) |
| `unknown`       | Anything else                                                     | —                                           | **Detected, rejected** when the build needs a recognized archiver |

The basename-based fallback is intentionally narrow: only
archivers literally named `ar`, `llvm-ar`, or `lib` (with optional
version suffix or `.exe`) are reclassified by name. Anything else
remains `unknown` and is rejected.

### Capability set

Each compiler detection records a typed
[`CompilerCapabilities`] with these fields:

| Field                       | Used by the planner | Notes |
| --------------------------- | ------------------- | ----- |
| `gcc_style_flags`           | Yes (GCC/Clang dialect) | Required for `-O…`, `-DNAME`, `-Idir`, `-c`, `-o`. Missing on a GCC/Clang compiler → unsupported. |
| `msvc_style_flags`          | Yes (MSVC dialect)  | Required for `/O…`, `/D`, `/I`, `/c`, `/Fo`, `/Tp`/`/Tc`. Missing on `cl` → unsupported. |
| `depfile_mmd_mf`            | Yes                       | Required for `-MMD -MF <file>`. Missing → unsupported. |
| `std_flag`                  | Yes                       | Required for `-std=…`. Missing → unsupported. |
| `cxx_standard_17`           | Yes                       | The planner emits `-std=c++17` / `/std:c++17`; version-gated — rejects GCC < 5 and `cl` < 19.11 (VS2017 15.3). Modern Clang/`clang-cl` always supported. |
| `c_standard_11`             | Yes                       | The planner emits `-std=c11` / `/std:c11`; version-gated — rejects `cl` < 19.28 (VS2019 16.8). GCC/Clang/`clang-cl` always supported. Checked only when a `.c` source exists. |
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
| `ar_crs`                 | Yes (GCC/Clang dialect)   | Required for the GNU `ar crs <lib> <objs>` archive command. MSVC `lib /OUT:` does not need it. |
| `static_library_output`  | Yes                       | Required to produce a static library. Reported `supported` for `ar` / `llvm-ar` (`ar crs`) and `lib.exe` (`lib /OUT:`) alike, so `cabin metadata` is honest about the MSVC archiver. |

### Validation against the C++ backend

Before any Ninja file is written, `cabin build` runs the
detection report through `cabin_build::validate_toolchain_for_backend`.
The validator surfaces clear errors when:

- the C++ compiler is `unknown`, or lacks the capabilities its dialect
  needs (GCC/Clang: `gcc_style_flags`, `depfile_mmd_mf`; MSVC:
  `msvc_style_flags`) or `cxx_standard_17` (both dialects), so a too-old
  `cl` is rejected up front. The C compiler, when a `.c` source exists,
  must likewise satisfy its dialect plus `c_standard_11`;
- the archiver is `unknown`, or cannot produce a static library in its
  dialect (GNU `ar crs`, or MSVC `lib /OUT:`);
- the resolved tools span **both** dialects — an MSVC `cl` paired with
  a GNU `ar`, or a GCC/Clang `c++` paired with `lib`
  (`MixedToolchainDialects`).

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

## Windows / MSVC

Windows is a supported platform, driven by the **MSVC** dialect. CI
builds, links, runs, and tests the example packages on a
`windows-2025-vs2026` runner on every change.

### What works

- **Default toolchain, auto-discovered.** On Windows the resolver
  defaults to `cl` for both C and C++ and `lib` for archiving (see the
  [tool-kinds table](#tool-kinds)), and locates them — plus the
  `INCLUDE` / `LIB` a compile needs — from the installed Visual Studio
  even when no Developer Command Prompt is active (see
  [Toolchain discovery](#toolchain-discovery)). No configuration is
  needed on a stock MSVC install.
- **All toolchain-driven subcommands.** `cabin build`, `run`, `test`,
  `check`, `fmt`, `tidy`, `metadata`, and `explain build-config` work
  with MSVC. Executables get the host `.exe` suffix; static libraries
  are `.lib`; objects are `.obj`.
- **Cabin's full source-extension set.** `.c` compiles as C; `.cc`,
  `.cpp`, `.cxx`, `.c++`, and `.C` compile as C++. The language is
  driven explicitly (`cl /Tp<file>` / `/Tc<file>`) rather than left to
  `cl`'s extension inference, so every supported extension compiles as
  the language Cabin classified it.
- **Command mapping.** `cl /nologo /std:c++17 /EHsc /O2 /Z7 /showIncludes
  /D… /I… /c /Tp<src> /Fo<obj>` for compiles (Ninja consumes
  `/showIncludes` via `deps = msvc`); `lib /nologo /OUT:<lib> <objs>`
  for archives; `cl /nologo <inputs> /Fe<exe> /link <ldflags>` for
  links. `cabin check` runs its syntax-only compile through a
  shell-free `cabin __check-stamp` runner that stamps the output on a
  zero exit, so the rule is identical on every host and build paths
  containing shell metacharacters (`&`, `|`, `(`, `)`) never need
  escaping.
- **`clang-cl`.** LLVM's `cl.exe`-compatible driver is detected by its
  invoked name (its `--version` banner is a plain `clang version`) and
  drives the MSVC dialect, so `CXX=clang-cl AR=lib` is a coherent MSVC
  toolchain. It keeps Clang's diagnostics while emitting MSVC-style
  flags. Pair it with `lib.exe` (the MSVC archiver).
- **MSVC version is validated.** Cabin emits `/std:c++17` (Visual Studio
  2017 15.3+, `cl` 19.11) and `/std:c11` (Visual Studio 2019 16.8+, `cl`
  19.28); a `cl` too old for the standard it would emit is rejected with
  a clear error up front rather than failing at the first compile. The
  capability is recorded in the detection report (`cxx_standard_17` /
  `c_standard_11`) and surfaced by `cabin metadata`.
- **Foundation ports.** The bundled zlib port builds under MSVC
  (Unix-only defines such as `HAVE_UNISTD_H` are gated behind
  `cfg(family = "unix")`).

### Toolchain discovery

Cabin needs a Visual Studio (or Build Tools) installation, but **not** a
pre-activated environment. When `cl.exe` / `lib.exe` and `INCLUDE` /
`LIB` are not already on the environment, Cabin discovers the installed
toolchain via the
[`find-msvc-tools`](https://crates.io/crates/find-msvc-tools) crate —
resolving the absolute paths to `cl` / `lib` / `link` and layering the
`INCLUDE` / `LIB` / `PATH` the compile needs onto Ninja's environment.
So a stock install builds without a Developer Command Prompt.

If Cabin is already running inside an activated environment (a Developer
Command Prompt, or `vcvarsall.bat` /
[`ilammy/msvc-dev-cmd`](https://github.com/ilammy/msvc-dev-cmd) in CI —
detected by `INCLUDE` / `LIB` being set), it uses that environment
unchanged and skips discovery, so an explicitly selected toolset is
honored.

### Known limitations

- **A GCC/Clang-style toolchain on Windows (MinGW, clang) is
  best-effort, not a CI-tested configuration.** The names resolve — the
  Windows fallback lists include `clang` / `gcc` / `g++` / `llvm-ar` /
  `ar` — and the rough edges that used to make it fail outright are gone:
  a C++-only GNU build no longer trips the single-dialect check on a
  defaulted `cc=cl` (the C compiler is validated only when a `.c` source
  exists); `cabin tidy` spells its compile database in the *resolved*
  compiler's dialect, not the host default; and the archiver fallback is
  `llvm-ar` (an `ar crs`-compatible tool) rather than the
  `lib.exe`-only `llvm-lib`. Even so, MSVC is the only Windows dialect CI
  exercises, so a GNU toolchain on Windows is unsupported in the sense
  that it is untested. Set `CC`, `CXX`, **and** `AR` together to one
  toolchain's tools: the resolver fills each slot from its own host
  default, so mixing a GNU `CXX` with a defaulted MSVC `AR` (or a
  defaulted MSVC `CC` once a `.c` source is present) is still rejected by
  the [single-dialect validation](#validation-against-the-c-backend).
- **`system = true` dependencies are not supported under MSVC.** They are
  resolved with pkg-config, whose GNU-style `-L` / `-lfoo` / `-pthread`
  output the MSVC `cl` / `link` command line cannot consume; on Windows
  the `.pc` files also come from MinGW/msys2 and reference the MinGW ABI,
  so translating the tokens would link the wrong libraries. A build,
  run, or test that needs an active system dependency under an MSVC
  toolchain is rejected with a clear error before any probe runs. Use a
  GCC/Clang toolchain for packages with system dependencies.
- **Very old MSVC is rejected, not supported.** A `cl` older than the
  `/std:` switch it would emit (Visual Studio 2017 15.3 for `/std:c++17`,
  Visual Studio 2019 16.8 for `/std:c11`) is refused at validation with a
  clear error rather than down-shifting the standard. Cabin targets a
  current Visual Studio (CI uses VS 2026).

## Deferred / out of scope

- Compiler probe compilations beyond running `--version`.
- Compiler-specific conditional flags
  (`cfg(compiler = "clang")` / `cfg(compiler_version = ...)`).
- distcc / icecc wrapper integration. (`ccache` / `sccache` are
  supported — see [docs/compiler-cache.md](compiler-cache.md).)
- Sysroot or SDK discovery.
- A fully supported GCC/Clang-style toolchain on Windows (MinGW /
  clang). MSVC is the supported Windows dialect.
- A diagnostics abstraction (SARIF / JSON output formats).
  Capabilities for these are detected, but Cabin does not emit
  either format.
- Per-package profile overrides for `[toolchain]`.
- A CLI escape hatch for raw compile / link flags (`--cflag`).
- `LD` env-var honoring (linker selection requires a
  linker-command abstraction the backend lacks).
