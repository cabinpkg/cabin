# System dependencies

A system dependency is a library Cabin does not fetch, build, or install.  The machine holds it
already, from an OS package manager or any other installation.  A build takes one through a package
whose [build script](build-scripts.md#supplying-a-library) finds the library, by asking
`pkg-config` or by reading a directory it is pointed at, and reports it to Cabin in place of a
library built from sources.

## Using a system library

Depend on a package that supplies the library, as on any other package, and name its target in
`deps`.

- A package that offers both the system's copy and its own sources chooses between them by a
  [feature](features.md) it documents; see
  [A package for a system library](build-scripts.md#a-package-for-a-system-library).
- The library is looked for only by a build that builds a target of the package that supplies it,
  since only then does that package's [build script run](build-scripts.md#which-build-scripts-run).
  A package under `[dev-dependencies]` is needed by `cabin test` alone, and one under
  `[target.'cfg(...)'.dependencies]` only on the platforms its condition matches; see
  [dependency kinds](dependency-kinds.md) and [target dependencies](target-dependencies.md).
- A library that is missing, or is older than the package's script accepts, fails the build.
  Cabin never leaves it out of the build, and builds no other copy in its place.

## Writing a package for a system library

When no package supplies the library, write one.  It needs no source of its own, only a `library`
target for dependents to name and a build script that says where the library is:

```toml
# zlib-sys/cabin.toml
[package]
name = "zlib-sys"
version = "0.1.0"
interface-c-standard = "c89"
interface-cxx-standard = "c++98"

[target.z]
type = "library"
sources = []
links = "z"
```

```c
/* zlib-sys/build.c */
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    FILE *file = fopen(getenv("CABIN_RESULT_FILE"), "w");
    if (!file) return 1;
    fputs("external z\npkg-config z zlib >=1.2\n", file);
    return fclose(file) != 0;
}
```

A package that needs zlib depends on it:

```toml
# app/cabin.toml
[dependencies]
zlib-sys = { path = "../zlib-sys" }

[target.app]
type = "executable"
sources = ["src/main.c"]
deps = ["zlib-sys"]
```

- `external z` says Cabin builds nothing for the target, and the `pkg-config` line names the module
  whose flags stand for it and the version it has to have.
  [Supplying a library](build-scripts.md#supplying-a-library) lists everything a script may report.
- A library with no listed source declares the [interface standards](language-standards.md) its
  headers need, so that Cabin can check them against the C and C++ dependents.
- [`links`](manifest.md#links) claims the native library, so a build that reaches two packages for
  the same one stops.
- One package serves every package that needs the library: a workspace member, a `path`
  dependency, or a published package.
- pkg-config's flags are a GNU toolchain's, so a `pkg-config` line is refused in a build with an
  MSVC toolchain.  A script that supports one reports `include-dir` and `link-file` lines for a
  directory it is given through a `CABIN_SDK_*` variable, as the `build-script-sdk` example does.

## What Cabin asks pkg-config

For each `pkg-config` line of a build script that ran, Cabin invokes `pkg-config` to:

1. verify the module is present at an accepted version (`pkg-config --exists 'name op version'`),
2. retrieve compile-time flags (`pkg-config --cflags name`),
3. retrieve link-time flags (`pkg-config --libs name`).

What depends on the library compiles and links with those flags:

- Include directories from `--cflags` reach the compile commands as `-isystem <dir>`, so
  diagnostics inside the system library's headers stay quiet (see
  [System include directories](toolchains.md#system-include-directories)).  The compiler's default
  search directories (`/usr/include`, `/usr/local/include`) are the one exception: GCC documents
  that re-spelling them `-isystem` reorders the system include chain and breaks `#include_next` in
  libc headers, so when a `.pc` file forces one through it stays a plain `-I`, which the compiler
  then ignores for directories it already searches.
- Every other `--cflags` token is passed to those compile commands as written, and every `--libs`
  token to the link, in the order `pkg-config` emitted them.

Cabin asks on every command that [runs build scripts](build-scripts.md#which-commands-run-build-scripts),
whether or not the script itself runs again, so a module that is gone or answers other flags is
seen at once.  `cabin metadata`, `cabin tree`, and `cabin explain` run no build script and ask
nothing.  [`cabin tidy`](tidy.md) runs no build script either: it refuses a library that lists no
source, as `zlib-sys` above does, and otherwise sees a system library's headers only where the
compiler finds them by itself.

## Version requirements

The requirement of a `pkg-config` line is written as a Cabin version requirement.  Cabin tries to
convert each Cabin / npm-flavored SemVer requirement into a list of `pkg-config`-native operator
pairs:

| Cabin requirement | pkg-config form                |
|-------------------|--------------------------------|
| `^1.2`            | `>= 1.2`, `< 2`                |
| `~1.2.3`          | `>= 1.2.3`, `< 1.3`            |
| `>=1.2 <2`        | `>= 1.2`, `< 2`                |
| `=1.0.0`          | `= 1.0.0`                      |
| `=1.2`            | `>= 1.2`, `< 1.3`              |
| `>1.2`            | `>= 1.3`                       |
| `<=1.2`           | `< 1.3`                        |

A partial version names its whole series, as in SemVer: `=1.2` accepts `1.2.12`, and `>1.2`
does not.  Cabin reads an installed version with fewer segments as zero-padded (`1.2` is
`1.2.0`).  `pkg-config` sorts `2` below `2.0` below `2.0.0`, so each `>=` and `<` bound is
written without trailing zeros: `< 2` rejects an installed `2.0`, which `< 2.0.0` would accept,
and `>= 1.2` admits an installed `1.2`, which `>= 1.2.0` would reject.  An exact `=1.2.0` stays
`= 1.2.0`, so an installed `1.2` does not satisfy it: any range around both spellings would also
admit `1.2rc1`, which `pkg-config` sorts between them.  A pre-release in a requirement is compared
as written, because `pkg-config` has no SemVer pre-release ordering.

Requirements that do not parse as SemVer are forwarded to `pkg-config` as one or more explicit
`operator version` pairs, with the module name repeated before each pair (`>= 1.0.1f < 3.0.0z`
becomes `name >= 1.0.1f name < 3.0.0z`).  Tokens that do not form such pairs - a bare non-SemVer
token (e.g.  `vendor-special`), a dangling operator, a pair that does not start with a
`pkg-config` comparison operator, or a version that does not start with an ASCII letter or digit
or holds a character other than those, `.`, `_`, `+`, `-`, and `~` (`pkg-config` could read it as
an option, or after a comma as another module) - produce a clear error rather than being silently
dropped.

`pkg-config` itself does the version comparison.

## Building for another machine

pkg-config reports the build machine's libraries, so a `--target` build for another machine that
asks it fails unless `PKG_CONFIG_LIBDIR` points pkg-config into the target sysroot (usually
together with `PKG_CONFIG_SYSROOT_DIR`) or `CABIN_PKG_CONFIG` names a pkg-config for the target,
such as `aarch64-linux-gnu-pkg-config`.  A triple naming another `env` counts as another machine,
and Cabin assumes `gnu` for every Linux build machine: on a musl host, an explicit
`--target x86_64-unknown-linux-musl` needs `CABIN_PKG_CONFIG=pkg-config`.

## Executable resolution

Cabin uses the `pkg-config` executable from `PATH` by default.  The `CABIN_PKG_CONFIG` environment
variable overrides the default with a verbatim executable path or command name:

```sh
CABIN_PKG_CONFIG=/opt/pkgconf/bin/pkg-config cabin build
```

A build in which no build script reports a `pkg-config` line never spawns `pkg-config`, and the
executable is not required.  When one does and the executable is missing, Cabin produces a single
diagnostic suggesting the install / override fix.

## Pkg-config environment

Cabin does not sanitize the standard `pkg-config` environment.  Variables `PKG_CONFIG_PATH`,
`PKG_CONFIG_LIBDIR`, and `PKG_CONFIG_SYSROOT_DIR` are inherited by the child process, so users can
point Cabin at custom `.pc` search paths the same way they would for any other `pkg-config`
consumer.

## Diagnostics

| Code                                            | When                                                                |
|-------------------------------------------------|---------------------------------------------------------------------|
| `cabin::system_deps::executable_not_found`      | Cabin tried to spawn pkg-config but the binary was missing.         |
| `cabin::system_deps::package_not_found`         | `pkg-config --exists` reported the module as absent.                |
| `cabin::system_deps::version_mismatch`          | The installed version does not satisfy the requirement.             |
| `cabin::system_deps::invalid_version_requirement` | The requirement string could not be interpreted.                  |
| `cabin::system_deps::pkg_config_failed`         | `pkg-config` exited non-zero for `--cflags` / `--libs`.             |
| `cabin::system_deps::invocation_failed`         | A `pkg-config` invocation failed for a reason other than not-found. |
| `cabin::system_deps::malformed_output`          | `pkg-config` produced output Cabin could not split into argv tokens. |

## The removed `system` key

Cabin used to read `system = true` on a dependency entry, and asked pkg-config for a module of the
entry's name:

```toml
[dependencies]
zlib = { version = ">=1.2", system = true }
```

It no longer does.  A dependency entry that sets `system`, to either value, is an error
(`cabin::manifest::system_dependency_removed`): read past, the entry would name a package of the
registry.  Replace the entry with a dependency on a package that supplies the library, such as the
`zlib-sys` package [above](#writing-a-package-for-a-system-library), and add that package to the
`deps` of the targets that use the library.  That differs from the key:

- The module name and the version requirement move from the entry into the supplying package's
  build script.  A dependent no longer states a requirement on the system's copy.
- The library reaches the targets that list the package in `deps`, and their dependents by the
  ordinary [visibility rules](manifest.md#edge-visibility).  The key gave the flags to every target
  of the declaring package, and only when that package was one the command selected: a dependency's
  own `system = true` entries were never applied.
- A supplying package that comes from a registry is resolved, fetched, and recorded in
  `cabin.lock` like any other.  The system's copy is still not recorded.
- `cabin metadata` no longer has `system_dependencies` in a package, and neither it nor
  `cabin explain` asks pkg-config.  `cabin tidy` no longer adds pkg-config's flags, and refuses a
  library that lists no source.
- `cabin -v` no longer prints a line for each module.
- A build with an MSVC toolchain can now take a system library, from a package whose script
  reports the library's files.

A package's metadata no longer has a `system-dependencies` field.  A version published earlier
with one stays in the registry unchanged, and nothing supplies the modules it listed now.  Where
the registry's index carries the field, as the hosted registry's does, resolving to such a version
is an error that names it and those modules; require a version published without them.  A file
registry's index never carried the field, so there the version is refused when its own manifest is
read, by the error above.

## The removed `[system-provided]` table

Cabin used to read a `[system-provided]` table, in the workspace root manifest and in config files,
that made a build take a registry package from a pkg-config module.  It no longer does, and a
manifest or config file that still carries the table is an error.  Remove the table.

A package now offers the system's copy of its library itself, as
[A package for a system library](build-scripts.md#a-package-for-a-system-library) describes, and a
build asks for it the way that package documents, usually by a [feature](features.md).  That
differs from the table:

- Only a package written to offer the system's copy has one to offer.  A build can no longer name
  a pkg-config module to stand in for a package of its choosing, and a feature or a wrapper package
  does not do that for a package that was not written for it.  [`[patch]`](patch-overrides.md)
  still replaces a package with a copy of it that you keep, a whole package with its own manifest;
  it is not a table of system libraries.
- The package is fetched either way, since the build script that finds the system's copy is part
  of it.  The table skipped the fetch.
- The package's script names the pkg-config module and the version it accepts.  The table checked
  the module against the versions the dependents asked of the package.
- The choice is a feature, made in a manifest or with `--features`, not an entry of a config file.

## What Cabin does not do

Cabin does not install system packages.  Use your OS package manager (`apt`, `dnf`, `brew`,
`pacman`, ...), an external toolchain manager, or a manual install.  Cabin does not bundle or wrap
any third-party package manager.

Cabin does not integrate with vcpkg, Conan, or Cargo-style system probes beyond the `pkg-config`
query path described above.
