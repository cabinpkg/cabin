# Installing programs with `cabin install`

`cabin install` builds a package's `executable` targets and copies them into a directory you run
programs from.  `cabin uninstall` removes them again.

It is for programs you use yourself.  A package that needs a program to build - a code generator,
say - declares it under `[build-dependencies]` and is built with exactly that one, whatever is
installed (see [`build-scripts.md`](build-scripts.md)).

## Usage

```text
cabin install <PACKAGE>[@<REQ>] [--bin <NAME>]... [--root <DIR>] [OPTIONS]
cabin install --path <DIR> [--bin <NAME>]... [--root <DIR>] [OPTIONS]
cabin install --list [--root <DIR>]
cabin uninstall <PACKAGE> [--bin <NAME>]... [--root <DIR>]
```

`<PACKAGE>` names a package in the registry your [configuration](config.md#registry) names.
`cabin install acme/tool` installs the newest version, and `cabin install acme/tool@1.2` the newest
one that [version requirement](manifest.md#version-requirement-syntax) allows, read as a manifest
reads it: `acme/tool@=1.2.0` installs that version only.  A yanked version is never installed, and
a pre-release only when the requirement names one.  The package and its dependencies are fetched
and built as a project that depends on them builds them: each archive is checked against the
checksum its index entry records, and a package's [build script](build-scripts.md) runs when its
index entry declares one.

`--path` names the directory that holds the package's `cabin.toml`.  A member of a
[workspace](workspaces.md) is built as its workspace builds it: with what it inherits from the
workspace root, and the root's `[patch]` and `[profile]` tables.

## What is installed, and where

Every `executable` target of the package that its enabled features allow is installed, or only
those named with `--bin`, which can be repeated.  Libraries, headers, and `test` targets are never
installed, and a package without an `executable` target is an error.

The files go into `<root>/bin`.  The root is `--root <DIR>`, or `.cabin` in your home directory:
`~/.cabin/bin`, or `%USERPROFILE%\.cabin\bin` on Windows.  Cabin does not edit `PATH` or a shell
profile.  When the directory is not on `PATH`, it warns and says what to add: the line for the
startup file of your login shell (`SHELL`) when that is zsh, bash, or fish, and on Windows the
entry for your user `Path` variable.

Cabin keeps its record of what it installed in `<root>/.cabin-install/`.  The directory is Cabin's
own: its files are not an interface.

## How the package is built

As `cabin build` builds it, build scripts confined the same way, with these differences:

- The profile is `release`, unless `--profile <NAME>` names another.
- The build directory is a temporary one, deleted afterwards.  Nothing is written into the
  package's directory, and the directory the command runs from plays no part.
- Only your user-level [configuration](config.md) applies.  No project's `.cabin/config.toml` is
  read, the package's own included, so the package index is the one your configuration names and
  a `[patch]` entry in such a file does not apply.
- The build is for the machine it runs on.

`--features` and `--no-default-features` choose the package's features, and so does
`--all-features` with `--path`.  `--offline` forbids network access: a registry package then
installs only when your configuration names its registry by `index-path`, such as a
[vendored](vendoring-offline.md) one, and not by URL.

A registry package is built as a dependency is, so less of its manifest applies than with `--path`:

- A package that declares `[profile.<name>]` tables, a `[toolchain]` table, or
  `[build] compiler-wrapper` is refused: those belong to the root of a build.
- The raw `cflags`, `cxxflags`, and `ldflags` it declares are dropped, and the
  [environment's flags](environment-variables.md#cppflags--cflags--cxxflags--ldflags) do not reach
  it.
- Its `system = true` dependencies are not probed, so a program that needs one does not link.
- `--profile` names `dev` or `release`.

Install such a package from a copy of its sources with `--path`.

Only the executable is copied.  Cabin links the libraries of the packages it builds statically, so
the program does not need its build.  A shared library a package names through raw `ldflags` is
not copied: the program finds it only where those flags said.

## The lockfile

With `--path`, the `cabin.lock` next to the package's `cabin.toml`, or next to its workspace
root's for a workspace member, is read and never written.  The versions it names are used where
they still satisfy the manifest; the rest are resolved as if there were no lockfile.

A registry package's archive holds no lockfile, so the versions of its dependencies are resolved at
each installation, and `--locked` is for `--path` only.

`--locked` takes every version from the lockfile.  It fails when the package has registry
dependencies and the file is missing or no longer matches the manifest, and when the file was
written with a `[patch]` or source replacement from a project's `.cabin/config.toml`, which an
installation does not read.

The command reports how many registry dependencies it resolved and how many of them at the
versions the lockfile names.  The record keeps each dependency's version and the checksum of its
archive, and whether the lockfile chose it.

## Installing again, and files in the way

Installing a package again replaces what was installed from it: its executables are replaced,
whatever is at their names now, and those the new installation does not have are removed.  One of
those that changed since Cabin installed it is left in place with a warning, and Cabin no longer
tracks it.

A file name in `bin` that belongs to another installed package, or to a file Cabin did not
install, stops the command before it changes anything.  `--force` replaces those files and makes
them the package's.

Two executables whose names differ only in the case of their letters do not go into one root, on
any file system: uninstall the package that has the other one first.

The build finishes before the root is touched, so a failed build changes nothing.  One command at
a time changes a root, and a second one waits.  When changing the root fails partway, Cabin puts
the files back.  When the command itself is interrupted, run it again; when it then reports files
of the package's first installation as in the way, add `--force`.  A directory in the way is never
replaced.

On Windows, a running program's file cannot be overwritten or deleted.  Cabin moves it aside, and
deletes it during a later `cabin install` or `cabin uninstall` once the program has exited.

## Listing

```text
$ cabin install --list
acme/tool v1.2.0 (sha256:9f2c...):
    tool
hello v0.1.0 (/home/me/src/hello):
    hello
```

Each installed package is listed with its version, its executables, and the directory it was
installed from or, for a registry package, the checksum of its archive.

## Uninstalling

`cabin uninstall <PACKAGE>` removes the package's executables, or only those named with `--bin`.

It removes only files that are still the ones Cabin installed.  When one has changed, the command
names it and removes nothing: delete or restore that file, then run the command again.  A file
that is already gone is not an error.
