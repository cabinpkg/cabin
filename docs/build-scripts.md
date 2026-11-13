# Build scripts

A package can carry a **build script**: a program Cabin builds and runs before it compiles the
package, to generate headers and sources, to compute preprocessor definitions, or to take a library
from an SDK on the machine instead of building it.  The script is the file `build.c` or `build.cpp`
in the package's root, beside `cabin.toml`.  No manifest key declares it; the file is the
declaration.

```c
// build.c
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    const char *out_dir = getenv("CABIN_OUT_DIR");
    char path[4096];
    snprintf(path, sizeof path, "%s/build_info.h", out_dir);
    FILE *header = fopen(path, "w");
    fprintf(header, "#define BUILD_PROFILE \"%s\"\n", getenv("CABIN_PROFILE"));
    fclose(header);

    FILE *result = fopen(getenv("CABIN_RESULT_FILE"), "w");
    fprintf(result, "include-dir hello %s\n", out_dir);
    return fclose(result) != 0;
}
```

With this file in its root, the package's `hello` target can `#include "build_info.h"`.  Four
runnable examples are under `examples/`: `build-script-header` (a C script that generates a
header), `build-script-helper-lib` (a C++ script that links a helper library),
`build-script-generator` (a script that runs a code generator the package declares), and
`build-script-sdk` (a script that takes a library from the system, or leaves Cabin to build it).

A root holds `build.c` or `build.cpp`, not both: Cabin refuses a package with both.  The names are
exact, so `build.cc` or `Build.c` is an ordinary file.

## Which build scripts run

Cabin runs the build scripts of the packages the build reaches.  A package the build builds no
target of, as another workspace member under `cabin run --bin app`, has its script neither built nor
run.

- A package on your machine - one of the workspace's own, a `path` dependency, or a `[patch]`ed
  package - has a build script when its root holds one.
- A package taken from a registry has a build script when its
  [index entry](package-index.md#package-file-shape) declares one (`build-script`).  Depending on such a
  package runs its script on your machine, [confined](#confinement) like your own.  A `build.c` in
  an archive whose entry declares none is an ordinary file: a version published before Cabin
  recorded the key never runs one.  An entry declaring a script its archive does not hold is an
  error.

## Publishing a package with a build script

`cabin package` and `cabin publish` archive the root build script whatever
[`[package] include`](manifest.md#package) lists, as they do `cabin.toml`, and record it in the
package's metadata as `build-script`.  What the script reads from the package is archived like any
other file: list it in `include` when the package has one.  Neither command builds or runs the
script, and neither does the registry.

The hosted registry's [verifier](remote-registry.md#verification-lifecycle) requires the declared
script to be the one in the archive.  For a package with
[`[package.upstream]`](manifest.md#packageupstream) it also refuses a script that is the upstream
archive's own file at that path, unchanged (`upstream_build_script`): a build script there is one a
declared copy step or patch puts there or changes, and a patch deletes an upstream `build.c` that is
not meant to be one.

## Which commands run build scripts

`cabin build`, `cabin run`, `cabin test`, `cabin check`, and `cabin install` build and run build
scripts.  No other command does: `cabin metadata`, `cabin tree`, `cabin fetch`, `cabin package`, and
the rest neither compile nor run one.  [`cabin tidy`](tidy.md) analyzes a package without what its
build script reports, so a source that includes a generated header does not compile there.

## How a script is built

A build script runs on the machine doing the build, so Cabin builds it in the
[build context](dependency-kinds.md#build-dependencies), with the build machine's toolchain: the
build's own compilers, or under `--target` the ones `HOST_CC`, `HOST_CXX`, and `HOST_AR` choose.

- `build.c` compiles as C11 and `build.cpp` as C++17, without GNU extensions, or as the nearest
  standard the libraries it links accept.
- The package's `[profile]` flags and language standards describe its targets and do not reach the
  script.  The environment's `CFLAGS` and the like reach the script of a workspace member, unless
  the build is for another platform (`--target`).
- The script links the libraries of the package's `[build-dependencies]`, and may run their
  executables.  Without `targets` on the entry, those are the dependency's executables, and its
  library when it has exactly one; a dependency with several libraries fails the build until
  `targets` names the ones the script uses.
- A `build.cpp` does not make its package a C++ package: a C program whose build script is C++
  still compiles and links as C.

## What a script is given

A script takes no arguments.  It starts in the package's root with this environment and nothing
else, whatever the platform:

| Variable | Value |
|---|---|
| `CABIN_OUT_DIR` | Absolute path of the directory the script generates files into. |
| `CABIN_RESULT_FILE` | Absolute path of the file the script writes its [result](#what-a-script-reports) to. |
| `CABIN_MANIFEST_DIR` | Absolute path of the package's root. |
| `CABIN_PACKAGE_NAME`, `CABIN_PACKAGE_VERSION` | The package's name and version. |
| `CABIN_PROFILE` | The active profile (`dev`, `release`, ...). |
| `CABIN_TARGET_OS`, `CABIN_TARGET_ARCH`, `CABIN_TARGET_FAMILY`, `CABIN_TARGET_ENV` | The platform the script prepares the package for, as a `cfg(...)` condition spells each; empty when the platform leaves one unset. |
| `CABIN_TARGET_DIALECT` | `gnu` or `msvc`: the command dialect of the compiler that builds the package. |
| `CABIN_FEATURE_<NAME>` | Set to `1` for each feature of the package the build enables.  `<NAME>` is the feature's name in upper case, with every character other than an ASCII letter or digit as `_`. |
| `CABIN_BIN_<NAME>` | Absolute path of each build-dependency executable the script may run, named after its target the same way. |
| `HOME`, `USERPROFILE` | An empty directory of the script's own. |
| `TMPDIR`, `TMP`, `TEMP` | A directory of the script's own for temporary files. |
| `CABIN_SDK_*` | Every variable of yours whose name starts with `CABIN_SDK_`, unchanged. |
| `PATH`, `LANG`, `LC_ALL`, `LC_CTYPE`, `DEVELOPER_DIR`, `SDKROOT` | Yours, unchanged. |
| `SystemRoot`, `windir`, `SystemDrive`, `ComSpec`, `PATHEXT`, `NUMBER_OF_PROCESSORS`, `PROCESSOR_ARCHITECTURE`, `INCLUDE`, `LIB`, `LIBPATH` | Yours, unchanged: what Windows and its compilers read. |
| `LOCALAPPDATA` | On Windows only, a directory Windows makes for the run. |

A package built in both contexts, as a dependency of the program and as a build dependency, runs
its script once for each, with that context's features and platform.  Under `--target`, the run
that prepares the program's copy is told the target platform, and the run that prepares the build
machine's copy is told the build machine.

Two features, or two executables, whose names give the same variable fail the build.

## What a script reports

A script reports by writing lines to the file `CABIN_RESULT_FILE` names.  A script that writes none
reports nothing.  The file is UTF-8; an empty line, a line starting with `#`, and a line repeating
an earlier one are skipped.  A directive's fields are separated by one space, and the last field is
the rest of the line, so a path may contain spaces.

| Directive | Meaning |
|---|---|
| `include-dir <target> <directory>` | A further include directory of the target, as an entry of its `include-dirs` is.  It is under `CABIN_OUT_DIR`, or under a directory a `CABIN_SDK_*` variable names. |
| `define <target> <NAME[=value]>` | A further entry of the target's `defines`. |
| `interface-define <target> <NAME[=value]>` | A further entry of a library's `interface-defines`, for a macro it does not define yet. |
| `source <target> <file>` | A source file under `CABIN_OUT_DIR` that the target compiles after its own. |
| `external <target>` | The script supplies this library, and Cabin builds nothing of it.  See [Supplying a library](#supplying-a-library). |
| `link-file <target> <file>` | A library file an `external` target is linked from, in the order of the lines.  It is under `CABIN_OUT_DIR`, or under a directory a `CABIN_SDK_*` variable names. |
| `pkg-config <target> <module> [<requirement>]` | A pkg-config module an `external` target's compile and link flags come from, with the version it has to have.  See [Supplying a library](#supplying-a-library). |
| `rerun-if-changed <path>` | A file or directory of the package, or under a directory a `CABIN_SDK_*` variable names, that the result depends on.  See [When a script runs again](#when-a-script-runs-again). |

Every path is absolute, starts with a directory the script was given (`CABIN_OUT_DIR`,
`CABIN_MANIFEST_DIR`, a `CABIN_SDK_*` value) with no `..` after it, and names something that
exists.  A target is one the package declares and the build enables.

Cabin refuses a result file at its first line that breaks a rule, names the line, and fails the
build; nothing of a refused file is applied.  The same holds for `CABIN_OUT_DIR` itself: a symbolic
link or a special file there fails the build, and so does, except on Windows, a second name for a
file (a hard link).

Cabin decides which languages a target is written in from the sources its manifest lists.  So a
library that takes every source from its build script declares the interface it offers
(`interface-c-standard`, `interface-cxx-standard`; see
[language standards](language-standards.md)), and a generated source in a language none of the
listed sources is in needs that language's standard declared.  A target built for the build
machine, as part of a build dependency, lists at least one source of its own.

### Supplying a library

A script can take a library from outside the package, such as an SDK you point it at, in place of
the one the manifest describes.  It reports `external <target>` before any other line that names
the target, and then what stands for the library:

```text
external z
include-dir z /opt/zlib/include
link-file z /opt/zlib/lib/libz.a
```

- Cabin compiles and archives nothing of an external target.  What depends on it compiles with the
  `include-dir` and `interface-define` lines the script reports, and with none of the manifest's
  `include-dirs` and `interface-defines`, which describe the library Cabin would have built: the
  script may report an `interface-define` for a macro the manifest defines.  A link takes the
  `link-file` files where the target's archive would go.
- The rest of the target stands: its `deps`, its declared interface standards, its
  [`links`](manifest.md#links) claim, and the `link-libs` and `frameworks` of its package's
  profile.  A link uses the C++ driver when one of the manifest's sources is C++.
- An external target takes at least one `link-file` or `pkg-config` line, and no `define` or
  `source` line.
- Cabin plans the build from the manifest before the script has run, so the manifest's description
  of the target has to plan without error on its own, even where the script replaces it.
- With `CABIN_TARGET_DIALECT=msvc`, a `link-file` is a `.lib` file.  A file of another name there
  is a GNU toolchain's library, or a DLL, which is linked through its import library.
- Cabin refuses a `link-file` built for another machine than the build is for, where the file
  says which: an object archive, an import library, a shared library, or a universal binary with
  no slice for it.  The build is for the machine `--target` names, and without `--target` for
  the one its compiler reports, the C++ compiler where there is one.  A GCC reports none and is
  taken to build for the build machine, and a flag that makes a compiler build for another
  machine (`-m32`, `-arch`) is not seen: use `--target` for such a build.  Cabin does not check
  anything else about the file: not the C runtime it was built against (`/MD`, `/MT`), a debug or
  release configuration, or a C++ standard library.
- A link takes the file by its path.  Finding a shared library when the program runs is left to
  the platform's loader, and on Windows to `PATH`.
- A link runs again when a `link-file` changes.  The script itself runs again for a change in an
  SDK only where it names the file with `rerun-if-changed`.

A script can also leave the search to pkg-config, by naming a module in place of the files:

```text
external z
pkg-config z zlib >=1.2.11
```

- Cabin asks pkg-config itself, outside the script's confinement.
  [System dependencies](system-dependencies.md) describes the executable it runs
  (`CABIN_PKG_CONFIG`), the [version requirements](system-dependencies.md#version-requirements),
  and the diagnostics.  What depends on the target compiles and links with the flags pkg-config
  answers, after the script's own `include-dir` and `link-file` lines.  Several lines for one
  target are asked in their order.
- A link names what pkg-config answers after every library file, and after that, except on Apple
  platforms, the libraries the target's `deps` reach once more, where a static library from
  pkg-config finds them.  A symbol that both that library and one of them define comes from the
  latter.
- A module's name keeps its upstream spelling (`glib-2.0`, `SDL2`): ASCII letters, digits, `_`,
  `-`, and `.`, not starting with `.` or `-`.  It is not the path of a `.pc` file.
- Cabin asks on every build, whether or not the script runs again, so a module that is gone or
  answers other flags is seen at once.  A library that changes behind the same flags links
  nothing again.
- With `CABIN_TARGET_DIALECT=msvc` a `pkg-config` line is refused: the flags pkg-config reports
  are a GNU toolchain's.  Report `include-dir` and `link-file` lines there.
- In a `--target` build, pkg-config has to
  [describe the target's libraries](system-dependencies.md#building-for-another-machine).  A library
  a build dependency links there is built for the build machine, and cannot come from pkg-config.

### A package for a system library

A package can offer a library both ways: the system's copy, and one Cabin builds from sources the
package carries.  The manifest describes the library built from the sources, a
[feature](features.md) of the package chooses, and the script reports `external` only when the
system's copy is wanted.  The `build-script-sdk` example is such a package for zlib, with a
feature it names `vendored`; Cabin gives no feature name a meaning of its own.

- The target and its [`links`](manifest.md#links) claim are the same both ways, so dependents name
  one target, and a program links one copy of the library.
- A build enables a feature for every dependent of a package at once.  When any of them asks for
  the sources, all of them link the library built from the sources.
- A build dependency is resolved by itself: a build script or a tool of the build machine can link
  the library built from the sources while the program links the system's.
- The script decides what a usable system copy is, and fails when there is none.  Switching to the
  sources by itself would hide which library a program got.
- The version the script asks of the system's copy is a requirement of its own, not the
  package's `version`.  `cabin.lock` records nothing of the system's copy: that is whatever the
  machine holds when the build runs.

## When a script runs

Cabin runs a package's script before it compiles the package, and after the scripts of the
package's build dependencies.  Packages whose build scripts would need each other are already a
dependency cycle, which Cabin reports as one.

Standard output and standard error of a run go to the files `stdout` and `stderr` in the package's
`.build-script` directory under the build directory
(`<build-dir>/<profile>/packages/<pkg>/.build-script/`, or under `build-machine/` for the build
machine's copy).  When a script fails, Cabin shows the end of its standard error and the path of
the file.  A script fails by exiting with a status other than zero or by being killed.  A failed
run leaves no result, and the next build runs the script again.

### When a script runs again

Once a script has succeeded, later builds reuse its result until something it depended on changes:

- Cabin itself, the script's executable (so its source, and the libraries it links), or an
  executable it may run;
- any variable of its environment, which covers the package's enabled features, the profile, the
  platform, and every `CABIN_SDK_*` variable;
- the files it wrote into `CABIN_OUT_DIR`;
- its inputs.  A script that reports no `rerun-if-changed` line depends on every file of the
  package it may read, compared by size and modification time.  One that reports such lines depends
  only on what they name, compared by content; a directory stands for every file beneath it, and
  for where each symbolic link beneath it points, not for what the link leads to.

A file a script writes again with the same content keeps its modification time, so nothing that
includes it compiles again.

## Confinement

A build script is code a package supplies, so Cabin runs it confined, and only confined: where the
platform cannot confine it, the build fails instead of running the script.  A registry package's
script runs under the same confinement, with the same limits, as one of your own.

| Platform | Mechanism |
|---|---|
| Linux (64-bit x86 and Arm) | Landlock and a seccomp filter; needs Linux 6.2 or newer with Landlock enabled |
| macOS | `sandbox-exec` |
| Windows | an AppContainer in a job object |

On any other platform a package with a build script does not build.

A running script:

- reads its own executable, the build-dependency executables it may run, and the files and
  directories in the package's root.  It does not read an entry of the root named `.git`, `.hg`,
  `.svn`, `.cabin`, `build`, `dist`, or `node_modules`, or the one that holds the build directory,
  and it follows a symbolic link in the root only to what it may read anyway.  The system's own
  files stay readable;
- reads each directory a `CABIN_SDK_*` variable names, unless that directory holds the package's
  root or the build directory, or its path leads through one of them;
- writes only `CABIN_OUT_DIR`, the directory of `CABIN_RESULT_FILE`, and its home and temporary
  directories, each emptied before a run;
- reaches no network;
- is started with no variable beyond [those listed](#what-a-script-is-given): no registry token,
  no `GITHUB_TOKEN`, no `SSH_AUTH_SOCK`, no `CC` or `CFLAGS`;
- starts programs that are confined the same way, and any still running when it exits are killed.
  On Windows, where a killed program keeps its files open a moment longer, Cabin reads what the
  script wrote only once each of them is gone, and fails the build if it cannot wait for one.

Limits to know:

- **Only the run is confined.**  What a script generates is compiled, linked, and run like the
  package's own sources, with your access.  Read a build script before you build a package you do
  not trust, as you would its sources.
- **The limits below are within reach of every dependency that declares a build script.**  A
  registry package's script runs the first time you build what depends on it, without asking.
- Each platform leaves some locations readable whatever Cabin grants.  macOS denies reads only
  where user data lives (`/Users`, `/Volumes`, the temporary directories, and your home), so a
  script of a package kept elsewhere can read the package's neighbors and the build directory.
  The root's entries named above stay unreadable on every platform; a `.git` or `build` deeper in
  the package is read like any other directory.
- **macOS does not hide other processes' environments.**  A script there can read the command
  line and environment every process of yours was started with, Cabin's included, so a token in
  the environment you run Cabin with is within its reach.  A program a script starts that leaves
  the script's process group outlives the run, still confined, and can write `CABIN_OUT_DIR` after
  Cabin has checked it.
- On Linux a script can see whether any path exists, with its size and times, change the mode,
  times, and extended attributes of any file you own, and read every process's command line.
  Before Linux 6.12 it can signal any process of yours.  On Windows it can see whether any path
  exists.
- **Every build script of a build reads every directory a `CABIN_SDK_*` variable names**, not
  only the script the variable is meant for.  Name the SDK's own directory, not one that holds
  anything else.  A variable that names a file opens nothing to a script, and neither does one
  that names a directory holding the package's root or the build directory, or reached through
  one of them, a symbolic link there included: a directory in the package's root is read as the
  root is.  A script may still point Cabin at what such a variable names (`include-dir`,
  `link-file`, `rerun-if-changed`).  On macOS a script cannot follow a symbolic link kept where
  it reads nothing, such as your home: name the SDK's directory itself there, not such a link to
  it.
- On Windows, Cabin opens a `CABIN_SDK_*` directory to a script by changing its access-control
  list for the run, which takes longer the more the directory holds.  A directory whose list you
  may not change, as one under `Program Files`, fails the build: copy the SDK to a directory of
  yours.
- A script cannot list the package's root, only what is in it, and on macOS `getcwd` fails there:
  use `CABIN_MANIFEST_DIR`.
- A script that opens its standard output or standard error again by name (`/dev/stderr`) is
  refused.
- On Windows, `TEMP` and `TMP` name the directory Windows makes for the run, as `LOCALAPPDATA`
  does, not the one `TMPDIR` names.
- On Windows, a script runs a `CABIN_BIN_*` program directly (`_spawnl`, `CreateProcess`), not
  through `system`: the command interpreter answers `Access is denied`.
- On Windows, a program a script starts with `_spawnl` can fail as it exits (`0xC0000005`), its
  work done, once it has closed its standard input, output, or error.  Windows applies strict
  handle checks to confined processes, and Cabin, which does not start that program, cannot turn
  them off for it.  Start the program with `CreateProcess`, or have it flush those streams and
  leave them open.
- Cabin is not meant to run as root or as an administrator: a script run with those privileges can
  reach past the confinement.
- The build directory must not hold the package: a build script does not read the build directory,
  so Cabin refuses a package with a build script whose root lies inside it.
