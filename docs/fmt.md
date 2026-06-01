# Formatting C/C++ sources (`cabin fmt`)

`cabin fmt` is Cabin's wrapper around `clang-format`.  It walks
the selected package(s) for C/C++ source and header files,
hands them to `clang-format`, and either rewrites the files in
place or verifies that they are already formatted.

## Usage

```text
cabin fmt [--check] [--exclude <PATH>]... [--no-ignore-vcs] [SELECTION]
```

`cabin fmt --help` lists every flag with its short description.

### Write mode (default)

```text
cabin fmt
```

`clang-format -i --style=file` is invoked on every discovered
source.  Status output reports how many files were processed:

```text
   Formatted 17 files
```

### Check mode

```text
cabin fmt --check
```

`clang-format --dry-run -Werror --style=file` is invoked.  The
command exits with a non-zero status if any file would be
reformatted, and never modifies files on disk:

```text
src/main.cc:5:1: warning: code should be clang-formatted [-Wclang-format-violations]
   Failed `cabin fmt --check`: 2 files would be reformatted (re-run without --check to apply)
```

`clang-format`'s per-file warnings are forwarded on stderr (the
same shape `cargo fmt --check` uses to forward rustfmt's diff)
and the Cabin-owned summary banner reports the file count on
stdout.  Use this in CI to enforce a clean tree.

### Excluding paths

```text
cabin fmt --exclude src/generated.cc --exclude vendored/
```

`--exclude` may be repeated.  Each argument is a file or
directory path resolved against the current working directory;
a directory entry skips every descendant.

### Including VCS-ignored files

By default `cabin fmt` honors `.gitignore`, `.ignore`,
parent-directory ignore files, and global git excludes.  Pass
`--no-ignore-vcs` to force `clang-format` to run on files that
are normally hidden by those rules:

```text
cabin fmt --no-ignore-vcs
```

`--no-ignore-vcs` disables *only* the VCS ignore layer.  The
walker still skips:

- Cabin's built-in build / cache / vendor / VCS-state
  directories (see "What gets formatted" below);
- top-level hidden directories that are not on the built-in
  exclude list (`.cache`, `.tools`, etc. — anything starting
  with a dot is treated as tool state, not developer-edited
  source).

This shape matches how `clang-format` users typically want the
flag to behave: it brings back files a project's
`.gitignore` chose to hide, not files whose location signals
that they aren't user-authored at all.

### Workspace selection

`cabin fmt` honors Cabin's standard workspace selection flags:

| Flag | Behavior |
|---|---|
| `--workspace` | Format every workspace member |
| `--package <name>`, `-p <name>` | Format the named workspace package; repeat for multiple |
| `--default-members` | Format `[workspace.default-members]` |

Without any of these flags, `cabin fmt` formats the *current
package*: the package the manifest discovery walk lands on,
which is consistent with `cabin build`'s default behavior.

## What gets formatted

Source discovery only emits files whose extension is one of:

- C sources: `.c`
- C++ sources: `.cc`, `.cpp`, `.cxx`, `.c++`, `.C`
- C/C++ headers: `.h`, `.hh`, `.hpp`, `.hxx`

The walk skips:

- VCS-ignored files (unless `--no-ignore-vcs` is in effect);
- Cabin's build directory (the path resolved by `--build-dir` /
  `CABIN_BUILD_DIR` / `[paths] build-dir`, or the built-in
  default `build`);
- Conventional output / cache / dependency directories
  (`target`, `dist`, `out`, `.cabin`, `node_modules`, `.venv`,
  `__pycache__`);
- VCS metadata directories (`.git`, `.hg`, `.svn`, `.jj`,
  `.pijul`);
- The manifest directories of *unselected* workspace members,
  so walking one package never spills into another.

## Style discovery

`cabin fmt` always passes `--style=file` to `clang-format`,
which makes `clang-format` walk upward from each file to find
the nearest `.clang-format` (or `_clang-format`) style file.
Cabin does **not** generate a `.clang-format` for you; commit
one to your repository if you want a project-wide style.  When
no `.clang-format` is found anywhere, `clang-format` falls back
to its built-in LLVM style — the same behavior you would see
invoking `clang-format` directly.

## Choosing the formatter executable

Cabin spawns `clang-format` from `PATH` by default.  Override
the executable by setting [`CABIN_FMT`](environment-variables.md):

```text
CABIN_FMT=/opt/llvm/bin/clang-format cabin fmt --check
```

`CABIN_FMT` is taken verbatim — typically an absolute path, but
a bare command name works too (it is then resolved against
`PATH`).  When the executable cannot be found Cabin emits an
actionable error:

```text
error: clang-format was not found on PATH.
  install `clang-format` (LLVM toolchain) and re-run, or set `CABIN_FMT=/path/to/clang-format` to a specific binary
```

## Verbosity

`cabin fmt` honors Cabin's standard verbosity flags
(see [`environment-variables.md`](environment-variables.md)):

- `-q` / `--quiet` suppresses normal status output.  Errors
  are still reported.
- `-v` / `--verbose` adds a one-liner describing the selected
  package(s) and the number of files being formatted.
- `-vv` (or `-v -v`) additionally echoes the formatter command
  line and the list of files that would be passed to it.
