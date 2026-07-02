# System dependencies

System dependencies are external libraries that Cabin does not fetch, build, or install.  Users
provide them through their OS package manager (or any other local installation mechanism) and Cabin
discovers their compile / link flags by querying `pkg-config`.

See [`docs/dependency-kinds.md`](dependency-kinds.md) for the dependency kinds and the declaration
syntax; this page covers the build-time behavior of system-sourced (`system = true`) entries
specifically.

> **Not supported with an MSVC toolchain.** `pkg-config` emits
> GNU-style `-L` / `-lfoo` / `-pthread` flags that the MSVC
> `cl` / `link` command line cannot consume, and on Windows the
> `.pc` files come from MinGW/msys2 and reference the MinGW ABI.
> A build, run, or test that needs an active system dependency
> under MSVC is therefore rejected with a clear error before any
> probe runs.  Use a GCC/Clang toolchain for packages with system
> dependencies.  See
> [`docs/toolchains.md`](toolchains.md#windows--msvc).

## Manifest declaration

A system-sourced dependency lives in one of the regular dependency tables (`[dependencies]`,
`[dev-dependencies]`) with `system = true` set:

```toml
[dependencies]
zlib    = { version = ">=1.2", system = true }
fmt     = { version = "^9.0",  system = true }
```

Every declared `system = true` dependency is required.  There is no `required` field; declaring
`required` on any dependency entry is rejected with a clear "unknown field" diagnostic.

Per-kind activation matches the Cabin-package rule for the table the entry lives in:

| Table                  | Probed by `cabin build` / `run` / `metadata` | Probed by `cabin test` |
| ---------------------- | -------------------------------------------- | ---------------------- |
| `[dependencies]`       | always                                       | always                 |
| `[dev-dependencies]`   | **no**                                       | selected primaries     |

A test-only system dep (e.g.  `gtest`) is declared under `[dev-dependencies]` with `system = true`,
so an ordinary `cabin build` does not require it to be installed.

Conditional declarations work the same way as the other dependency kinds:

```toml
[target.'cfg(os = "linux")'.dependencies]
systemd = { version = ">=240", system = true }
```

A conditional system dependency is probed only when its condition matches the host platform.

## What Cabin does with system-sourced entries

When the selected primary packages declare at least one active system dependency, Cabin invokes
`pkg-config` (once per dependency) to:

1. verify the library is present (`pkg-config --exists 'name op version'`),
2. retrieve compile-time flags (`pkg-config --cflags name`),
3. retrieve link-time flags (`pkg-config --libs name`).

Discovered include directories from `--cflags` are added to the package's *system* include-dir set
and reach the compile commands as `-isystem <dir>`, so diagnostics inside the system library's
headers stay quiet (see [System include directories](toolchains.md#system-include-directories)).
The compiler's default search directories (`/usr/include`, `/usr/local/include`) are the one
exception: GCC documents that re-spelling them `-isystem` reorders the system include chain and
breaks `#include_next` in libc headers, so when a `.pc` file forces one through it stays in the
plain `-I` bucket - which the compiler then ignores for directories it already searches.  All other
`--cflags` tokens are appended verbatim to the package's language-neutral compile argument bucket.
All `--libs` tokens are appended verbatim to the package's `ldflags` list, preserving the order
`pkg-config` emitted them so C/C++ link semantics are not disturbed.

Probe scope is intentionally primary-only.  For a single-package project, the root package is
primary; for a workspace, the members selected by the command are primary.  Local path dependencies
and registry / extracted dependencies may preserve `system = true` declarations in metadata, package
archives, and index entries, but those dependency manifests do not trigger `pkg-config` probing for
the downstream build.

Probing happens for every command that produces compile or link commands (`cabin build`, `cabin
run`, `cabin test`, `cabin tidy`) and for `cabin metadata` and `cabin explain build-config` so the
build-configuration fingerprint stays consistent across those commands.  Other `cabin explain`
subcommands (`package`, `target`, `source`, `feature`) do not invoke `pkg-config` because they never
materialize a build configuration.

## Version requirements

Cabin tries to convert each Cabin / npm-flavored SemVer requirement into a list of
`pkg-config`-native operator pairs:

| Cabin requirement | pkg-config form                |
|-------------------|--------------------------------|
| `^1.2`            | `>= 1.2`, `< 2.0.0`            |
| `~1.2.3`          | `>= 1.2.3`, `< 1.3.0`          |
| `>=1.2 <2`        | `>= 1.2`, `< 2`                |
| `=1.0.0`          | `= 1.0.0`                      |

Requirements that do not parse as SemVer are forwarded to `pkg-config` as one or more explicit
`operator version` pairs, with the module name repeated before each pair (`>= 1.0.1f < 3.0.0z`
becomes `name >= 1.0.1f name < 3.0.0z`).  Tokens that do not form such pairs - a bare non-SemVer
token (e.g.  `vendor-special`), a dangling operator, or a pair that does not start with a
`pkg-config` comparison operator - produce a clear error rather than being silently dropped.

`pkg-config` itself does the version comparison.

## Missing dependency semantics

A declared `system = true` dependency that fails to probe stops the build with an actionable
diagnostic that names the package, the requirement Cabin attempted, and the installed version when
available.  Cabin never silently drops a missing system dependency from the build graph.

## Executable resolution

Cabin uses the `pkg-config` executable from `PATH` by default.  The `CABIN_PKG_CONFIG` environment
variable overrides the default with a verbatim executable path or command name:

```sh
CABIN_PKG_CONFIG=/opt/pkgconf/bin/pkg-config cabin build
```

If a project declares no `system = true` entries, Cabin never spawns `pkg-config` and the executable
is not required.  When the project does declare system dependencies and the resolved executable is
missing, Cabin produces a single diagnostic suggesting the install / override fix.

## Pkg-config environment

Cabin does not sanitize the standard `pkg-config` environment.  Variables `PKG_CONFIG_PATH`,
`PKG_CONFIG_LIBDIR`, and `PKG_CONFIG_SYSROOT_DIR` are inherited by the child process, so users can
point Cabin at custom `.pc` search paths the same way they would for any other `pkg-config`
consumer.

## Fingerprint and rebuild

Discovered compile / link flags participate in `cabin_core::BuildConfiguration::fingerprint` through
the per-package `ResolvedProfileFlags`.  When `pkg-config` reports different flags (for example
because the user upgraded the system library or changed `PKG_CONFIG_PATH`), the build configuration
fingerprint moves; Ninja then rebuilds from the changed command lines.

## Output / diagnostics

Normal mode: Cabin prints no probe-specific output on successful runs.

Verbose mode (`cabin -v ...`): Cabin prints one line per probed dependency naming the resolved
version, and one summary line per workspace identifying the `pkg-config` executable that was used.

Very-verbose mode (`cabin -vv ...`): Cabin also prints the per-dependency probe request (name,
version requirement) before the probe runs.

All probe-related output is written to stderr so that machine-readable stdout (`cabin metadata`,
`cabin explain --format json`) stays a parseable document.

Quiet mode (`cabin -q ...`): probe status output is suppressed.  Errors are not.

## Diagnostics

| Code                                            | When                                                                |
|-------------------------------------------------|---------------------------------------------------------------------|
| `cabin::system_deps::executable_not_found`      | Cabin tried to spawn pkg-config but the binary was missing.         |
| `cabin::system_deps::package_not_found`         | `pkg-config --exists` reported the package as absent.               |
| `cabin::system_deps::version_mismatch`          | The installed version does not satisfy the manifest requirement.    |
| `cabin::system_deps::invalid_version_requirement` | The requirement string could not be interpreted.                  |
| `cabin::system_deps::pkg_config_failed`         | `pkg-config` exited non-zero for `--cflags` / `--libs`.             |
| `cabin::system_deps::invocation_failed`         | A `pkg-config` invocation failed for a reason other than not-found. |
| `cabin::system_deps::malformed_output`          | `pkg-config` produced output Cabin could not split into argv tokens. |

## What Cabin does not do

Cabin does not install system packages.  Use your OS package manager (`apt`, `dnf`, `brew`,
`pacman`, ...), an external toolchain manager, or a manual install.  Cabin does not bundle or wrap
any third-party package manager.

Cabin does not integrate with vcpkg, Conan, or Cargo-style system probes beyond the `pkg-config`
query path described above.
