# CLI distribution artifacts

Cabin ships two CLI subcommands that emit artifacts package
managers usually want:

- `cabin compgen` — shell completion scripts (bash, zsh, fish,
  powershell, elvish);
- `cabin mangen` — man pages (`cabin(1)` plus one
  `cabin-<sub>(1)` per top-level subcommand).

Both commands derive every byte of output from the canonical
`clap::Command` tree returned by `Cli::command()`.  There are
no hand-written completion scripts and no hand-written man
pages.

`compgen` and `mangen` are aimed at downstream packagers, so
they are hidden from `cabin --help` to keep the day-to-day
listing short.  They are still listed by `cabin --list`, still
parse normally, still appear in generated shell completions,
and still ship per-command man pages — only the curated help
view omits them.

The two commands bound themselves to completion and man-page
generation: they do not write a Homebrew formula, publish to a
registry, attach release binaries, or perform deployment
integration.  Release binary packaging is handled separately by
the repository's tag-triggered workflow.

## Shell completions

```sh
cabin compgen <shell> [--output-dir <dir>]
cabin compgen --all --output-dir <dir>
```

| Shell | Identifier | Default filename in `--output-dir` |
| --- | --- | --- |
| Bash | `bash` | `cabin.bash` |
| Zsh | `zsh` | `_cabin` |
| Fish | `fish` | `cabin.fish` |
| PowerShell | `powershell` | `cabin.ps1` |
| Elvish | `elvish` | `cabin.elv` |

Behavior:

- Without `--output-dir`, `cabin compgen <shell>` writes the
  script to stdout.  This is the normal "install one shell"
  entry point (`cabin compgen bash > cabin.bash`).
- With `--output-dir <dir>`, the script is written into the
  directory using the table above.  The directory is created
  if it does not already exist; existing files are
  overwritten.
- `cabin compgen --all --output-dir <dir>` writes one file
  per supported shell into the directory.  `--all` requires
  `--output-dir`; multiple files cannot be written to stdout
  cleanly, so omitting it produces a clear error.
- An unknown shell name fails with clap's standard validation
  error.

Examples:

```sh
# Bash
cabin compgen bash > cabin.bash

# Zsh — drop into a directory on $fpath, e.g. ~/.zfunc/
cabin compgen zsh > _cabin

# Fish — into the user completions directory
cabin compgen fish > ~/.config/fish/completions/cabin.fish

# PowerShell
cabin compgen powershell > cabin.ps1

# Elvish
cabin compgen elvish > cabin.elv

# Every supported shell, into one folder
cabin compgen --all --output-dir completions
```

## Man pages

```sh
cabin mangen [--output-dir <dir>]
```

Behavior:

- Without `--output-dir`, `cabin mangen` writes the root
  `cabin(1)` man page (ROFF) to stdout.
- With `--output-dir <dir>`, the directory is created if
  needed and populated with `cabin.1` (the root page) plus
  `cabin-<sub>.1` for every top-level subcommand.  Hidden
  subcommands such as `compgen` and `mangen` still receive
  their own per-command pages; the root `cabin.1` page mirrors `cabin
  --help` and therefore omits the hidden subcommands from its
  SUBCOMMANDS section.
- Aliases do not get separate pages.
- Existing files in the output directory are overwritten.

Examples:

```sh
# Root cabin(1) to stdout
cabin mangen > cabin.1

# Full set into ./man
cabin mangen --output-dir man

# Install root + per-subcommand pages into a system man path
cabin mangen --output-dir /usr/local/share/man/man1
```

## Notes for package-manager integrators

- The generated artifacts are derived from the clap CLI
  definition, so they stay in sync with whatever subcommands
  and flags currently exist.
- Refresh both completions and man pages on every release
  that changes the CLI surface — adding or renaming a
  subcommand, flag, or value enum changes the output.
- Both commands are deterministic for the same `cabin`
  binary: running them twice in a row produces identical
  output.
- Bundling the generated completion and man-page artifacts into
  a Homebrew formula, deb / rpm package, or release tarball is a
  downstream concern. The repository's tag-triggered release
  workflow packages the Cabin binary archives and checksums, but
  it does not run `cabin compgen` / `cabin mangen` or attach
  those generated files.
