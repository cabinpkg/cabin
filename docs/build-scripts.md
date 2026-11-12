# Build scripts

A package can carry a **build script**: a program Cabin builds and runs before it compiles the
package, to generate headers and sources or to compute preprocessor definitions.  The script is the
file `build.c` or `build.cpp` in the package's root, beside `cabin.toml`.  No manifest key declares
it; the file is the declaration.

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

With this file in its root, the package's `hello` target can `#include "build_info.h"`.  Three
runnable examples are under `examples/`: `build-script-header` (a C script that generates a
header), `build-script-helper-lib` (a C++ script that links a helper library), and
`build-script-generator` (a script that runs a code generator the package declares).

A root holds `build.c` or `build.cpp`, not both: Cabin refuses a package with both.  The names are
exact, so `build.cc` or `Build.c` is an ordinary file.

## Which build scripts run

Cabin runs the build scripts of the packages on your machine that the build reaches: the workspace's
own packages, `path` dependencies, and `[patch]`ed packages.  A package the build builds no target
of, as another workspace member under `cabin run --bin app`, has its script neither built nor run.

**The build script of a package taken from a registry is not run.**  `cabin package` treats the
file like any other, but a build that takes the package from a registry compiles the package
without running it.  Do not publish a package that needs its build script.

`cabin build`, `cabin run`, `cabin test`, and `cabin check` build and run build scripts.  No other
command does: `cabin metadata`, `cabin tree`, `cabin fetch`, `cabin package`, and the rest neither
compile nor run one.  [`cabin tidy`](tidy.md) analyzes a package without what its build script
reports, so a source that includes a generated header does not compile there.

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
platform cannot confine it, the build fails instead of running the script.

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
- A directory a `CABIN_SDK_*` variable names is one the script may point Cabin at
  (`include-dir`, `rerun-if-changed`).  Cabin does not grant the script itself access to it.
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
