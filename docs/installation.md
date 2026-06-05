# Installation

## Supported Operating Systems

- Linux (GCC / Clang)
- macOS (Clang / Apple Clang)
- Windows (MSVC — `cl.exe` / `lib.exe`)

On Windows the default toolchain is MSVC; a GCC/Clang-style toolchain
(MinGW, clang) is **not** a supported configuration there (see
[toolchains.md](toolchains.md#windows--msvc) for the dialect model,
what is supported, and the known limitations).

## Install Methods

- **From crates.io.** Cabin is published as the `cabinpkg` package
  on crates.io. With a Rust toolchain on `$PATH`, run:

    ```sh
    cargo install cabinpkg
    ```

  The installed command is `cabin`.
- **From a third-party package manager.** Community-maintained
  packages — not provided or endorsed by the Cabin maintainers.
  [Repology](https://repology.org/project/cabin-cpp-package-manager/versions)
  tracks downstream availability as it appears.
- **From source.** See [INSTALL.md](https://github.com/cabinpkg/cabin/blob/main/INSTALL.md)
  for the prerequisites and build steps. Use this when you need an
  unreleased revision or want to verify a build locally.

## Runtime Requirements

`cabin` itself has no required runtime dependencies, but the
subcommands that drive the C/C++ toolchain need the relevant tools
installed on `$PATH`. Each tool only matters for the subcommand it
backs:

| Tool | Required by | Override |
| --- | --- | --- |
| C++ compiler — GCC/Clang (`c++`, `clang++`, `g++`) on Unix; MSVC (`cl`) on Windows | `cabin build` / `cabin run` / `cabin test` / `cabin tidy` / `cabin metadata` / `cabin explain build-config` | `CXX` |
| C compiler — GCC/Clang (`cc`, `clang`, `gcc`) on Unix; MSVC (`cl`) on Windows | the same commands when the selected targets contain `.c` sources | `CC` |
| Static-library archiver — `ar` on Unix; `lib` on Windows | `cabin build` / `cabin run` / `cabin test` / `cabin tidy` / `cabin metadata` / `cabin explain build-config` | `AR` |
| Ninja (≥ 1.10) | `cabin build` / `cabin run` / `cabin test` | `NINJA` |
| `pkg-config` | targets that declare `system = true` dependencies | `CABIN_PKG_CONFIG` |
| `clang-format` | [`cabin fmt`](fmt.md) | `CABIN_FMT` |
| `run-clang-tidy` | [`cabin tidy`](tidy.md) | `CABIN_TIDY` |

On **Windows**, Cabin defaults to the MSVC toolchain and
**auto-discovers** it: if `cl.exe` / `lib.exe` and the `INCLUDE` / `LIB`
environment are not already present, Cabin locates the installed Visual
Studio toolchain (via the
[`find-msvc-tools`](https://crates.io/crates/find-msvc-tools) crate) and
supplies them for the build. A stock Visual Studio / Build Tools install
therefore works **without** a Developer Command Prompt. You may still run
from a *Developer Command Prompt* or a shell activated by `vcvarsall.bat`
(or the [`ilammy/msvc-dev-cmd`](https://github.com/ilammy/msvc-dev-cmd)
GitHub Action) to pin a specific toolset — Cabin uses an already-active
environment as-is. `cabin fmt` and `cabin tidy` still shell out to
`clang-format` / `run-clang-tidy` from an LLVM install, exactly as on
Unix.

`cabin resolve`, `cabin update`, `cabin tree`, and the graph-only
`cabin explain` subcommands (`package`, `target`, `source`, and
`feature`) do not require a compiler, archiver, or Ninja.

The C++ compiler and archiver are resolved when Cabin renders or
plans a build configuration. The C compiler is resolved
opportunistically and becomes required when the selected targets
contain `.c` sources. `cabin metadata` and `cabin explain
build-config` report toolchain details, so they need the required
tool slots for that build configuration to be resolvable; their
`--version` capability probes are fail-soft after resolution
succeeds. Ninja is only invoked by commands that actually run the
Ninja backend. See [toolchains.md](toolchains.md) for compiler
selection precedence and capability detection.
