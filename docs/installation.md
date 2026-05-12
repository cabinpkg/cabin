# Installation

[![Packaging status](https://repology.org/badge/vertical-allrepos/cabin-cpp-package-manager.svg)](https://repology.org/project/cabin-cpp-package-manager/versions)

## Supported Operating Systems

- Linux
- macOS

Windows / MSVC is not supported. See
[architecture.md](architecture.md) for the full scope.

## Install Methods

- **From source.** See [INSTALL.md](https://github.com/cabinpkg/cabin/blob/main/INSTALL.md)
  for the prerequisites and build steps. This is the supported
  install path while pre-1.0; the release workflow does not
  attach pre-built binaries today.

Cabin is pre-1.0, so packaging into system package managers is still
ad-hoc; the Repology badge above tracks downstream availability as it
appears.

## Runtime Requirements

`cabin` itself has no required runtime dependencies, but the
subcommands that drive the C / C++ toolchain need the relevant tools
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
