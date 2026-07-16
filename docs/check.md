# Checking for errors with `cabin check`

`cabin check` type-checks a package's C/C++ sources **without producing object files or binaries**.
It is Cabin's analogue of `cargo check`: it reuses the exact same build graph as
[`cabin build`](index.md#quickstart) - the same toolchain, profile, include paths, defines, and
per-package flags - but compiles each translation unit in the compiler's syntax-only mode
(`-fsyntax-only` for GCC/Clang-family drivers, `/Zs` for MSVC `cl`) and skips archiving and linking
entirely.

Because it shares the build pipeline (and the Ninja backend), a check is **incremental and
parallel**: a second run with no source changes does nothing, and editing a header re-checks only
the translation units that include it.  It is the fast inner-loop command for catching compile
errors before paying for a full build.

## Usage

```text
cabin check [OPTIONS]
```

`cabin check` accepts the same options as `cabin build` - manifest and build-directory selection,
profile selection, workspace selection, and the parallel-jobs flag.  `cabin check --help` lists them
all.

### Default invocation

```text
cabin check
```

Cabin plans the build for the selected package(s), rewrites every compile into an `-fsyntax-only`
check, and runs them through Ninja.  No `.o` objects, no `.a` archives, and no executables are
written; each successful check records a small stamp file under
`build/<profile>/packages/<pkg>/obj/` (a scoped package nests as `packages/<scope>/<name>/`) so
Ninja can skip unchanged translation units next time.  The
`build.ninja` and `compile_commands.json` files are written to `build/<profile>/`, the same path
`cabin build` uses.

`cabin check` exits non-zero as soon as any translation unit fails to compile; the compiler's
diagnostics are streamed through unchanged.

### Build profiles

```text
cabin check --release
cabin check --profile <name>
```

A check honors the same profile selection as `cabin build`, defaulting to `dev`.  The profile
matters for a syntax check: its flags (for example `-O3 -DNDEBUG` under `release`) can change which
`#if` branches compile.  See [Build profiles](profiles.md).

### Parallel jobs

```text
cabin check -j 8
cabin check --jobs 8
```

`-j` / `--jobs` caps how many checks run concurrently, with the same precedence chain as `cabin
build`: the flag, then `CABIN_BUILD_JOBS`, then `[build] jobs` in a config file, then Ninja's own
default (the host CPU count).

### Workspace selection

`cabin check` accepts Cabin's standard workspace selection flags:

| Flag | Behavior |
|---|---|
| `--workspace` | Check every workspace member |
| `--package <name>`, `-p <name>` | Check the named workspace package; repeat for multiple |
| `--default-members` | Check `[workspace.default-members]` |

Without any of these flags, `cabin check` operates on the current package, checking its libraries
and executables - the same default target set [`cabin build`](index.md#quickstart) builds.

## What gets checked

`cabin check` syntax-checks the **selected workspace package(s)' own translation units**.  It
deliberately does *not* separately check the implementation (`.c` / `.cc`) files of dependencies or
[foundation ports](foundation-ports.md): your code never compiles a dependency's implementation
files, only its headers - and those headers *are* checked transitively wherever your translation
units `#include` them.  Skipping third-party implementation files keeps the check fast and focused
on code you can fix, matching `cargo check`'s model.

Header-only targets declare no translation units, so they emit no check of their own; their headers
are validated through the targets that include them.

## Relationship to `cabin build`

`cabin check` and `cabin build` are two modes of one pipeline:

- **`cabin build`** compiles each translation unit to an object, archives libraries, and links
  executables - producing runnable artifacts.
- **`cabin check`** compiles each translation unit with `-fsyntax-only` (parse plus semantic
  analysis, no code generation) and stops there - producing only diagnostics.

Both write the same `compile_commands.json`, so editor tooling (clangd) sees a consistent database
regardless of which you run.  For the related source-tooling commands, see [`cabin fmt`](fmt.md)
(formatting) and [`cabin tidy`](tidy.md) (static analysis).

## Toolchain support

`cabin check` validates the resolved toolchain the same way `cabin build` does and lowers each check
through the detected compiler's command-line dialect: GCC/Clang-family drivers (GCC, Clang, Apple
Clang) get `-fsyntax-only`, MSVC `cl` gets `/Zs`.  See [Toolchains](toolchains.md).
