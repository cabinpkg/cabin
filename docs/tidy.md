# Static analysis with `clang-tidy` (`cabin tidy`)

`cabin tidy` is Cabin's wrapper around
[`run-clang-tidy`](https://clang.llvm.org/extra/clang-tidy/index.html#using-clang-tidy),
the standard LLVM driver that fans `clang-tidy` invocations out
across a Clang JSON compilation database.  Cabin walks the
selected package(s) for C/C++ source files, generates the
compilation database from its build graph, and hands both to
`run-clang-tidy`.  Source discovery shares its rules with
[`cabin fmt`](fmt.md), so a file `cabin fmt` would touch is
a file `cabin tidy` will analyse — provided it has a compile
entry in the database.

## Usage

```text
cabin tidy [--fix] [--exclude <PATH>]... [--no-ignore-vcs] [-j <N>] [SELECTION]
```

`cabin tidy --help` lists every flag with its short
description.

### Default invocation

```text
cabin tidy
```

Cabin generates `build/<profile>/compile_commands.json` for the
selected packages and runs
`run-clang-tidy -p build/<profile> -quiet <files>` over every
recognised C/C++ source.  The `-quiet` argument hides
`run-clang-tidy`'s per-file progress chatter so only real
diagnostics reach stderr; pass `-v` / `--verbose` to drop
`-quiet` and see the driver's full progress output.

`cabin tidy` exits non-zero whenever `run-clang-tidy` exits
non-zero, which is the standard way clang-tidy signals that at
least one file produced a diagnostic.

### Applying fixes

```text
cabin tidy --fix
```

`--fix` enables `run-clang-tidy -fix`, which applies clang-tidy's
suggested rewrites back to disk.  Cabin never enables this
implicitly; the rewrites are off unless you pass the flag.  In
fix mode Cabin clamps the effective parallelism to one
clang-tidy instance so concurrent rewrites cannot race; verbose
mode reports the override when `--jobs <N>` was supplied with
`N > 1`.

### Parallel jobs

```text
cabin tidy -j 8
cabin tidy --jobs 8
```

`-j` / `--jobs` controls how many `clang-tidy` instances
`run-clang-tidy` runs in parallel.  Same precedence chain as
`cabin build`:

1. **`-j` / `--jobs <N>`** on the command line.
2. **`CABIN_BUILD_JOBS=<N>`** environment variable.
3. **`[build] jobs = <N>`** in a config file.
4. **Default** — `run-clang-tidy`'s own default (today the host
   CPU count).

`<N>` must be a positive integer.  `0`, negatives, and
non-numeric values are rejected at parse time, the same way
`cabin build` rejects them.

### Excluding paths

```text
cabin tidy --exclude src/generated.cc --exclude vendored/
```

`--exclude` may be repeated.  Each argument is a file or
directory path resolved against the current working directory; a
directory entry skips every descendant.

### Including VCS-ignored files

```text
cabin tidy --no-ignore-vcs
```

By default `cabin tidy` honours `.gitignore`, `.ignore`,
parent-directory ignore files, and global git excludes.
`--no-ignore-vcs` disables only the VCS ignore layer; Cabin's
built-in build / cache / vendor exclusions still apply.

### Workspace selection

`cabin tidy` accepts Cabin's standard workspace selection flags:

| Flag | Behaviour |
|---|---|
| `--workspace` | Analyse every workspace member |
| `--package <name>`, `-p <name>` | Analyse the named workspace package; repeat for multiple |
| `--default-members` | Analyse `[workspace.default-members]` |

Without any of these flags, `cabin tidy` operates on the current
package.

## Compile database generation

`cabin tidy` always generates (or refreshes) a
`compile_commands.json` before invoking the tidy driver:

- the file is written to `build/<profile>/compile_commands.json`
  using Cabin's existing build directory and per-profile layout —
  the same path `cabin build` produces;
- the database is generated *without* invoking Ninja: tidy is
  read-only analysis and a build is unnecessary.

The build directory honours the same precedence chain as `cabin
build`: `--build-dir` > `CABIN_BUILD_DIR` > `[paths] build-dir`
config setting > built-in default `build`.

## Choosing the tidy driver executable

Cabin spawns `run-clang-tidy` from `PATH` by default.  Override
the executable by setting
[`CABIN_TIDY`](environment-variables.md):

```text
CABIN_TIDY=/opt/llvm/bin/run-clang-tidy cabin tidy
```

`CABIN_TIDY` is taken verbatim — typically an absolute path, but
a bare command name works too (it is then resolved against
`PATH`).  When the executable cannot be found Cabin emits an
actionable error:

```text
error: run-clang-tidy was not found on PATH.
  install `clang-tidy` (LLVM toolchain) and re-run, or set `CABIN_TIDY=/path/to/run-clang-tidy` to a specific binary
```

## `.clang-tidy` discovery

`clang-tidy` has its own file-based configuration mechanism
called `.clang-tidy`.  Cabin does not generate or modify these
files and does not pass any `-config=`, `-checks=`, or related
flags to the tidy driver: it lets `clang-tidy` walk upward from
each translation unit's directory to find the nearest
`.clang-tidy`, exactly as it would in any other invocation.

Commit a `.clang-tidy` to your repository if you want a
project-wide configuration.  When no `.clang-tidy` is found
anywhere, `clang-tidy` falls back to its built-in default
checks — the same behaviour you would see invoking `clang-tidy`
directly.

## What gets analysed

Source discovery applies the same rules as `cabin fmt` — see
[`fmt.md`](fmt.md#what-gets-formatted).  `cabin tidy` then
narrows the discovered set to files that have a compile entry in
the generated `compile_commands.json`; headers and undeclared
sources are skipped because clang-tidy cannot meaningfully
analyse a translation unit it has no compile command for.

The walk skips the same directories `cabin fmt` skips:

- VCS-ignored files (unless `--no-ignore-vcs` is in effect);
- Cabin's resolved build directory;
- `target`, `dist`, `out`, `.cabin`, `node_modules`, `.venv`,
  `__pycache__`;
- VCS metadata (`.git`, `.hg`, `.svn`, `.jj`, `.pijul`);
- the manifest directories of *unselected* workspace members.

## Verbosity

`cabin tidy` honours Cabin's standard verbosity flags:

- `-q` / `--quiet` suppresses Cabin-owned status output but does
  not suppress clang-tidy diagnostics — `run-clang-tidy`
  inherits stderr directly.
- `-v` / `--verbose` adds a per-invocation summary describing
  the selected packages, the file count, the compile database
  path, and the resolved jobs setting; it also drops `-quiet`
  from the spawned `run-clang-tidy` command so the driver's own
  progress output appears.
- `-vv` (or `-v -v`) additionally echoes the spawned tidy
  command line.

## Versioned dependencies

`cabin tidy` does not run Cabin's artifact pipeline.  When the
selected package closure declares a versioned registry dependency
(`dep = "1.2"`), Cabin refuses to plan a tidy run and surfaces a
clear diagnostic:

```text
error: package `hello` declares versioned registry dependencies; `cabin tidy` does not run the artifact pipeline, so registry-backed selections are not supported
```

`cabin build` and `cabin fetch` can still materialise those
dependencies for build workflows, but they do not make a
registry-backed selection usable with `cabin tidy`.
