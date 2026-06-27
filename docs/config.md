# Configuration files

Cabin reads typed TOML configuration files for *local policy* - defaults the user, the workspace, or
a single project want to apply across many invocations of `cabin build`, `cabin metadata`, and the
resolution / fetch family.  Config is **not** package source spec: it never enters the manifest, the
lockfile, the published package metadata, or the registry index.

This document is the canonical specification for config discovery, parsing, merging, precedence, and
validation.  The behavior described here is what `cabin-config`, `cabin-toolchain`, `cabin`, the
metadata view, and the package archiver all agree on.

## File locations

A single command consults at most three files.  The discovery order is intentionally short so the
precedence stays auditable.

| Source     | Location                                                                        |
| ---------- | ------------------------------------------------------------------------------- |
| User       | `$CABIN_CONFIG_HOME/config.toml`; otherwise the platform user config home with the `cabin` suffix - Linux: `$XDG_CONFIG_HOME/cabin/config.toml` (fallback `$HOME/.config/cabin/config.toml`); macOS: `~/Library/Application Support/cabin/config.toml`; Windows: `%APPDATA%\cabin\config.toml` |
| Workspace  | `<workspace-root>/.cabin/config.toml` when the entry-point manifest declares `[workspace]` |
| Package    | `<package-root>/.cabin/config.toml` when the entry-point manifest is a single-package project |
| Explicit   | `$CABIN_CONFIG=<path>` - exactly one file, missing files are a hard error      |

When `CABIN_CONFIG_HOME` is unset, Cabin computes the user config home via the
[`directories`](https://crates.io/crates/directories) crate - the platform's user config directory
with a `cabin` suffix - and looks for `config.toml` there.  Cargo's `$CARGO_HOME` model is not used.
On Linux the lookup follows the XDG Base Directory specification (`$XDG_CONFIG_HOME`, falling back
to `$HOME/.config`); empty or relative values are treated as unset.  Cabin does not read system-wide
XDG config directories (`/etc/xdg`, `$XDG_CONFIG_DIRS`).

Discovery does **not** walk arbitrary parent directories beyond the workspace root.  Member-local
`.cabin/config.toml` files inside a workspace are ignored - only the workspace-root file applies.

If `CABIN_CONFIG` is set to a non-empty value, it short-circuits discovery: only that one file
loads, and a missing file is an explicit `config file <path> was requested explicitly but could not
be read` error rather than a silent fallback.

If `CABIN_NO_CONFIG=1` is set, no files load at all.  The metadata view still reports an empty
`config.loaded_files = []` block so consumers can distinguish "config absent" from "config silent".

## Precedence

For every setting Cabin's config layer can supply (registry source, paths, build defaults,
toolchain, compiler wrapper) the precedence order is, highest to lowest:

1. **CLI flag** - e.g., `--profile`, `--cxx`, `--compiler-wrapper`, `--index-path`, `--cache-dir`.
2. **Environment variable** - e.g., `CXX`, `AR`, `CABIN_COMPILER_WRAPPER`.  Empty values are treated
   as unset.
3. **Package-local or workspace config** - `<root>/.cabin/config.toml`.  Workspace files when
   `[workspace]` is declared, project files otherwise.
4. **User config** - the platform user config home (on Linux `$XDG_CONFIG_HOME/cabin/config.toml` or
   its `$HOME/.config/` fallback) unless `CABIN_CONFIG_HOME` overrides it.
5. **Manifest-declared package defaults** - e.g., `[toolchain]`, `[profile]`, `[profile.<name>]`,
   and `[build] compiler-wrapper` on the entry-point manifest.
6. **Built-in defaults** - Cabin's documented fallbacks (`dev` profile, `c++`/`clang++`/`g++` for
   the C++ compiler, `ar` for the archiver).

A `CABIN_CONFIG` file slots in at the same precedence as a workspace / project config - Cabin treats
the explicit file as the sole config layer.  The metadata view reports its provenance label as
`explicit-config`.

`cabin metadata` reports every effective config value paired with its `value_source` (one of `cli`,
`env`, `user-config`, `workspace-config`, `project-config`, `explicit-config`, `manifest`,
`builtin-default`) so the precedence is auditable without re-deriving it.

## Schema

Config files use TOML.  The supported tables are:

```toml
[registry]
index-path = "registry"
index-url = "https://example.com/index"

[paths]
cache-dir = ".cabin/cache"
build-dir = "build"

[build]
profile = "release"
compiler-wrapper = "ccache"

[toolchain]
cc = "clang"
cxx = "clang++"
ar = "llvm-ar"

[term]
color = "auto"

[patch]
fmt = { path = "../forks/fmt" }

[source-replacement]
"https://example.com/index" = { index-path = "../mirror" }
```

Every other top-level table - and every unknown field inside a known table - is rejected at parse
time with a clear error.

### `[registry]`

Selects a default index source for the resolver / fetch family when neither `--index-path` nor
`--index-url` is supplied on the CLI.

| Key          | Type    | Notes                                                                 |
| ------------ | ------- | --------------------------------------------------------------------- |
| `index-path` | path    | Local-filesystem index.  Relative paths resolve against the config file's directory. |
| `index-url`  | URL     | Sparse HTTP index URL.  Used as-is.                                    |

A single config file may declare *either* `index-path` or `index-url`, never both - the parser
rejects the combination with ``config key `registry.index-path` conflicts with `registry.index-url`;
choose only one at the same precedence level``.  Across precedence levels, the higher-priority
file's variant replaces the lower file's value entirely (so a workspace-level `index-url` overrides
a user-level `index-path`).

`cabin metadata` does not contact the configured `index-url`; network access still happens only when
a command (like `cabin resolve --index-url ...` or a build with versioned dependencies) needs the
index.

### `[paths]`

Defaults for local filesystem paths Cabin already accepts as CLI flags.  CLI flags win; otherwise
the highest-priority config setting applies.  Relative paths resolve against the config file's
directory.

| Key         | Type | Notes                                                              |
| ----------- | ---- | ------------------------------------------------------------------ |
| `cache-dir` | path | Override `--cache-dir`.  Used by the artifact pipeline (`cabin fetch`, `cabin build` with versioned deps). |
| `build-dir` | path | Override the build-output directory for commands that plan, run, or remove build outputs (`build`, `clean`, `run`, `test`, `tidy`, `fmt`).  The clap default `build` still applies when no flag and no config is set. |

Absolute paths pass through unchanged.  Cabin never serializes absolute local paths into package or
index metadata.

### `[build]`

Persistent defaults for build-time selectors that Cabin already exposes as CLI flags.

| Key                | Type    | Notes                                                                 |
| ------------------ | ------- | --------------------------------------------------------------------- |
| `profile`          | string  | Default profile. Overridden by `--profile <name>` and `--release`. Must reference a built-in (`dev`, `release`) or a custom profile declared in the workspace root manifest. |
| `jobs`             | integer | Default number of parallel jobs for the build backend. Must be a positive integer; `0` and negative values are rejected at parse time. |
| `compiler-wrapper` | string  | Executable name or path that prefixes C and C++ compile commands. Empty and whitespace-only values are rejected. |

`cabin build`'s profile precedence is `--profile` - > `--release` - > `build.profile` config - >
built-in `dev`.

`cabin build` / `cabin run` jobs precedence is `-j` / `--jobs <N>` - > `CABIN_BUILD_JOBS` - >
`build.jobs` config - > build backend default.  `cabin test` does not honor any jobs source: the
test runner is sequential.

`compiler-wrapper` accepts any single executable name or path and is not
shell-split. Precedence is `--compiler-wrapper` / `--no-compiler-wrapper` →
`CABIN_COMPILER_WRAPPER` → config `[build] compiler-wrapper` → workspace-root
manifest `[build] compiler-wrapper` → no wrapper. See
[`compiler-cache.md`](compiler-cache.md).

### `[patch]` and `[source-replacement]`

Local-development override policy.  The `[patch]` table replaces a registry-resolved package
candidate with a local working copy; the `[source-replacement]` table redirects one supported index
source to another supported index source.  Both tables are *config-only* surfaces (manifest
`[patch]` tables are also supported but live in `cabin.toml`); both follow the same
config-precedence ladder as the rest of this file.

```toml
[patch]
fmt = { path = "../forks/fmt" }

[source-replacement]
"https://example.com/index" = { index-path = "../mirror" }
```

The full schema, validation rules, and lockfile / metadata behavior live in
[`patch-overrides.md`](patch-overrides.md).  Important guarantees:

- `git`, `url`, and `version` patch source kinds are rejected at parse time.
- URLs containing `userinfo` (e.g., `https://user:pw@example.com/index`) are rejected so credentials
  never leak.
- Replacement chains are walked once with cycle detection.
- Member manifests cannot declare `[patch]` tables.
- `cabin package` rejects manifests with a non-empty `[patch]` table; config patches and source
  replacements are excluded from package archives by `.cabin/`'s existing exclusion rule.

### `[term]`

Persistent default for the terminal-color choice.

| Key     | Type   | Notes                                                       |
| ------- | ------ | ----------------------------------------------------------- |
| `color` | string | One of `auto`, `always`, `never`.  Other values are rejected at parse time. |

Precedence: `--color <when>` - > `CABIN_TERM_COLOR=<when>` - > `[term].color` config - > default
`auto`.  See [`environment-variables.md`](environment-variables.md) for the full table of values and
their behavior.

### `[toolchain]`

Persistent defaults for the C/C++ tool selection from [`toolchains.md`](toolchains.md).

| Key   | Type   | Notes                                                              |
| ----- | ------ | ------------------------------------------------------------------ |
| `cc`  | string | Bare command name searched on `PATH`, or an explicit filesystem path. |
| `cxx` | string | Same shape as `cc`, for the C++ compiler driver.                   |
| `ar`  | string | Same shape as `cc`, for the static-library archiver.               |

Per-tool precedence: `--cc` / `--cxx` / `--ar` - > `CC` / `CXX` / `AR` env - > `[toolchain]` config
- > workspace-root manifest `[target.'cfg(...)'.toolchain]` overlays - > workspace-root manifest
`[toolchain]` - > Cabin's documented fallback list.

The same `--cxx` / `CXX` / `[toolchain].cxx` value is read at every layer; the resolver's source
label distinguishes which layer ultimately won (`user-config`, `workspace-config`, `project-config`,
or `explicit-config` for the config layer).

## Environment variables

Cabin's config layer recognizes three environment variables.

| Variable              | Purpose                                                             |
| --------------------- | ------------------------------------------------------------------- |
| `CABIN_NO_CONFIG=1`   | Disable config discovery entirely.                                  |
| `CABIN_CONFIG=<path>` | Load exactly one config file from `<path>`.  Missing files are a hard error. |
| `CABIN_CONFIG_HOME=<dir>` | Override the user config directory.  Useful for tests and controlled environments.  Cabin reads `<dir>/config.toml` directly. |

`CABIN_CONFIG_HOME` is a Cabin-specific override and is **not** treated as an XDG variable: when set
to a non-empty value, Cabin reads `<value>/config.toml` directly with no `cabin` application prefix
appended.  When `CABIN_CONFIG_HOME` is unset, Cabin uses the platform user config home computed by
the `directories` crate (which on Linux honors `XDG_CONFIG_HOME` and `HOME` per the XDG Base
Directory specification) and looks for `cabin/config.toml` below it.

Existing env variables Cabin already honors (`CC`, `CXX`, `AR`, `CABIN_COMPILER_WRAPPER`,
`CABIN_CACHE_DIR`) are unchanged.  They take precedence over the config layer per the precedence
ladder above.

## What config does **not** do

These items are explicitly out of scope for the config layer and will not be added here.

- **No credentials, tokens, registry authentication, or credential-helper integration.** Cabin's
  config file is not a secrets store.  Tables named `auth`, `credentials`, `tokens`, `token`, or
  `registries` are rejected with a dedicated error so a typo never silently smuggles a credential
  into a published archive.
- **No vendoring policy table.** `cabin vendor` may consume the configured registry/path defaults,
  but config does not declare vendored entries or a `[vendor]` table.
- **No offline config key.** Offline mode is controlled by `--offline` / `CABIN_NET_OFFLINE`, not by
  a persistent config setting.
- **No new registry protocols.** `index-path` and `index-url` remain the only supported flavors.
- **No remote-cache configuration.** Compiler-cache server settings (`SCCACHE_*`, `CCACHE_DIR`,
  `CCACHE_MAXSIZE`, ...) belong to the wrapper's own configuration mechanism.
- **No target-conditioned config tables.** Encountering `[target.'cfg(...)'.<...>]` in a config file
  produces `target-conditioned config tables are not supported`.  The equivalent feature lives in
  the package manifest.
- **No new package semantics.** The config file may not declare dependencies, features,
  target-conditioned build flags, or any other field the package manifest already owns.
- **No network publish, account, ownership, or website support.**

## Package + index metadata

Config is local policy.  Cabin never:

- writes the contents of `.cabin/config.toml` into the canonical per-version package metadata
  (`<name>-<version>.json`);
- writes config-derived effective values into `cabin.lock` or any registry index file;
- includes `.cabin/` in deterministic source archives (the directory is in `EXCLUDED_DIR_NAMES` for
  `cabin package`).

Manifest declarations remain the only source-spec surface for published packages.  `cabin
metadata`'s `config` block is intended for human / tool consumption of *the local environment* and
is not serialized to the registry.

## Examples

### Minimal user config - pick a fixed compiler

```toml
# ~/.config/cabin/config.toml
[toolchain]
cxx = "clang++"
ar = "llvm-ar"
```

Every `cabin build` and `cabin metadata` invocation picks `clang++` and `llvm-ar` unless overridden
by `CXX` / `AR` / `--cxx` / `--ar`.

### Workspace-level cache and wrapper defaults

```toml
# <workspace-root>/.cabin/config.toml
[paths]
cache-dir = ".cabin/cache"

[build]
compiler-wrapper = "ccache"
```

Every contributor's local `cabin build` writes artifacts to `<workspace-root>/.cabin/cache` and
prefixes compile commands with `ccache`.  CI pipelines can override either by passing the matching
CLI flag.

### Workspace-level registry default

```toml
# <workspace-root>/.cabin/config.toml
[registry]
index-path = "vendor/index"
```

Resolves the index path against the config file's directory (`.cabin/vendor/index` here).  `cabin
resolve --index-path ...` overrides the default; `--index-url ...` is rejected against the
configured `index-path` only when both are passed on the same command line.

### Build profile default

```toml
# .cabin/config.toml
[build]
profile = "release"
```

`cabin build` defaults to `--profile release`.  CLI `--profile dev` or `--release` still wins.

### Disabling all config for a single command

```sh
CABIN_NO_CONFIG=1 cabin build --release
```

Useful in CI when a developer's workspace config might mask the intended behavior.
