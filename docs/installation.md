# Installation

[![Packaging status](https://repology.org/badge/vertical-allrepos/cabin-cpp-package-manager.svg)](https://repology.org/project/cabin-cpp-package-manager/versions)

## Supported Operating Systems

- Linux
- macOS

Windows / MSVC is not supported. See
[architecture.md](architecture.md) for the full scope.

## Install Methods

- **From crates.io.** Cabin is published as the `cabinpkg` package
  on crates.io. With a Rust toolchain on `$PATH`, run:

    ```sh
    cargo install cabinpkg
    ```

  The installed command is `cabin`.
- **From a third-party package manager.** Community-maintained
  packages — not provided or endorsed by the Cabin maintainers. The
  Repology badge above tracks downstream availability as it appears.
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
| GCC- or Clang-style C++ compiler (`c++`, `clang++`, `g++`) | `cabin build` / `cabin run` / `cabin test` / `cabin tidy` / `cabin metadata` / `cabin explain build-config` | `CXX` |
| GCC- or Clang-style C compiler (`cc`, `clang`, `gcc`) | the same commands when the selected targets contain `.c` sources | `CC` |
| `ar` archiver | `cabin build` / `cabin run` / `cabin test` / `cabin tidy` / `cabin metadata` / `cabin explain build-config` | `AR` |
| Ninja (≥ 1.10) | `cabin build` / `cabin run` / `cabin test` | `NINJA` |
| `pkg-config` | targets that declare `system = true` dependencies | `CABIN_PKG_CONFIG` |
| `clang-format` | [`cabin fmt`](fmt.md) | `CABIN_FMT` |
| `run-clang-tidy` | [`cabin tidy`](tidy.md) | `CABIN_TIDY` |

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
