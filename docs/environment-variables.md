# Environment variables

This page lists every runtime `CABIN_*` environment variable Cabin reads, the small fixed set Cabin
sets on a child process (`cabin run` / `cabin test`), and the precedence chain for each.

The single source of truth for runtime and child-process names lives in the
[`cabin-env`](https://github.com/cabinpkg/cabin/blob/main/crates/cabin-env/src/lib.rs) crate as `pub
const ... : &str = ...` constants.

## Read-side variables

| Name | Default | Meaning |
|---|---|---|
| `CABIN_CONFIG` | unset | Path to one explicit config file.  Disables the normal config-discovery walk. |
| `CABIN_CONFIG_HOME` | platform user config home with `cabin` suffix | Override for the per-user config home.  Used verbatim (no extra `cabin` segment).  When unset, Cabin resolves the user config home via the [`etcetera`](https://crates.io/crates/etcetera) crate (`$XDG_CONFIG_HOME/cabin` / `$HOME/.config/cabin` on Linux and macOS, `%APPDATA%\cabin` on Windows). |
| `CABIN_NO_CONFIG` | unset | When truthy, no config files load at all |
| `CABIN_BUILD_DIR` | `build` | Build output directory |
| `CABIN_CACHE_DIR` | unset | Artifact cache directory for this invocation.  Wins over `CABIN_CACHE_HOME` and the platform fallback. |
| `CABIN_CACHE_HOME` | platform user cache home with `cabin` suffix | Per-user cache home (the directory the global cache lives under).  Used verbatim (no extra `cabin` segment).  When unset, Cabin resolves the user cache home via the `etcetera` crate (`$XDG_CACHE_HOME/cabin` / `$HOME/.cache/cabin` on Linux and macOS, `%LOCALAPPDATA%\cabin` on Windows). |
| `CABIN_NET_OFFLINE` | unset | Forbid network access this invocation |
| `CABIN_REGISTRY_TOKEN` | unset | Bearer token for the experimental remote-registry client (`-Z remote-registry`).  When set and non-empty it wins over every `credentials.toml` entry for this invocation.  See [`remote-registry.md`](remote-registry.md#client-side-token-handling). |
| `CABIN_COMPILER_WRAPPER` | unset | Compiler-wrapper executable name or path. `none` (aliases `off`, `disabled`) disables wrapping. |
| `CABIN_TERM_COLOR` | unset | Terminal-color choice (`auto` / `always` / `never`) |
| `CABIN_TERM_VERBOSE` | unset | Enable verbose Cabin-owned status output when truthy |
| `CABIN_TERM_QUIET` | unset | Suppress Cabin-owned status output when truthy |
| `CABIN_FMT` | unset | Override for the `clang-format` executable `cabin fmt` spawns |
| `CABIN_TIDY` | unset | Override for the `run-clang-tidy` executable `cabin tidy` spawns |
| `CABIN_PKG_CONFIG` | unset | Override for the `pkg-config` executable Cabin spawns when probing ``system = true` deps` |
| `CABIN_BUILD_JOBS` | unset | Number of parallel jobs the build backend should use |
| `CABIN_RESOLVER_INCOMPATIBLE_STANDARDS` | unset | Standard-aware version preference (`allow` / `fallback`) |
| `CPPFLAGS` | unset | Conventional preprocessor flags appended to **both** C/C++ compile commands |
| `CFLAGS` | unset | Conventional flags appended only to C compile commands |
| `CXXFLAGS` | unset | Conventional flags appended only to C++ compile commands |
| `LDFLAGS` | unset | Conventional flags appended only to link commands |

### Precedence

Read-side settings use a consistent shape when they have more than one source:

1. **CLI flag** (e.g.  `--build-dir`, `--offline`)
2. **Environment variable** (e.g.  `CABIN_BUILD_DIR= ...`)
3. **Config file** (e.g.  `[paths] build-dir = ...`)
4. **Built-in default** (e.g.  `build`)

Not every environment variable has all four layers.  For example, `CPPFLAGS` / `CFLAGS` / `CXXFLAGS`
/ `LDFLAGS` have no Cabin CLI or config equivalent, and tool executable overrides such as
`CABIN_FMT` are environment-only.  Where a setting does have a config or CLI counterpart, precedence
labels are surfaced through `cabin metadata` so users can audit which layer supplied each effective
value.

### Truthy / falsy spellings

For boolean env vars (`CABIN_NET_OFFLINE`, `CABIN_NO_CONFIG`, `CABIN_TERM_VERBOSE`,
`CABIN_TERM_QUIET`, ...) Cabin recognizes:

- truthy: `1`, `true`, `yes`, `on` (case-insensitive)
- falsy: empty string, `0`, `false`, `no`, `off`

Anything else is an error.

### Terminal color (`CABIN_TERM_COLOR` and `--color`)

Cabin emits ANSI color in human-readable diagnostic output.  The choice follows the same precedence
chain as every other read-side variable:

1. **`--color <when>`** on the command line.
2. **`CABIN_TERM_COLOR=<when>`** environment variable.
3. **`[term] color = "<when>"`** in a config file.
4. **Default**: `auto`.

`<when>` is one of:

| Value | Meaning |
|---|---|
| `auto` | Emit color only when stderr is a terminal and the environment does not opt out (e.g.  `NO_COLOR`). |
| `always` | Always emit color, even when output is redirected. |
| `never` | Never emit color, regardless of terminal detection. |

Invalid values produce a clear error.  Example:

```text
$ CABIN_TERM_COLOR=sometimes cabin build
error: invalid CABIN_TERM_COLOR value 'sometimes'; expected one of: auto, always, never
```

Machine-readable output (`cabin metadata --format json`, `cabin tree --format json`, etc.) is always
emitted without color regardless of the selected mode, so piping into `jq` stays deterministic.

### Terminal verbosity (`CABIN_TERM_VERBOSE`, `CABIN_TERM_QUIET`, `-v`, and `-q`)

Cabin-owned status output follows this precedence chain:

1. **`-q` / `--quiet`** or **`-v` / `--verbose`** on the command line.
2. **`CABIN_TERM_QUIET=<bool>`** or **`CABIN_TERM_VERBOSE=<bool>`**.
3. **`[term] quiet = <bool>`** or **`[term] verbose = <bool>`** in a config file.
4. **Default**: normal status output.

The two environment variables use the truthy / falsy spellings listed above.  If both are truthy in
the same invocation, Cabin rejects the conflict instead of guessing which level the user intended.
CLI flags still take precedence, so `cabin -q build` does not inspect a conflicting
`CABIN_TERM_VERBOSE`.

### Build jobs (`CABIN_BUILD_JOBS` and `--jobs`)

Cabin lets the user cap the number of parallel jobs the build backend runs.  The choice follows the
standard precedence chain:

1. **`-j` / `--jobs <N>`** on the command line (supported by `cabin build`, `cabin run`, and `cabin
   tidy`).
2. **`CABIN_BUILD_JOBS=<N>`** environment variable.
3. **`[build] jobs = <N>`** in a config file.
4. **Default** - the build backend's own default (Ninja picks a value derived from the host's CPU
   count).

`<N>` must be a positive integer.  Cabin rejects `0`, negatives, and non-numeric values with a clear
error before spawning anything:

```text
$ cabin build --jobs 0
error: invalid value '0' for '--jobs <N>': expected a positive integer, got 0
$ CABIN_BUILD_JOBS=many cabin build
error: invalid CABIN_BUILD_JOBS value "many": invalid jobs value "many"; expected a positive integer
```

Cabin passes the resolved value to Ninja as `-jN` and does not otherwise modify the parallelism
story for compilers or linkers.  `cabin test` does not expose `--jobs`: the test runner is
sequential, and `CABIN_BUILD_JOBS` is ignored when `cabin test` invokes Ninja for the build phase.

`cabin tidy` honors the same precedence chain and forwards the resolved value to `run-clang-tidy` as
`-j N`.  In `--fix` mode the effective parallelism is clamped to `1` so concurrent clang-tidy
instances cannot race while applying overlapping fixes; verbose mode reports the override when a
higher count was requested.

### Standard-aware version preference (`CABIN_RESOLVER_INCOMPATIBLE_STANDARDS`)

Controls whether the resolver orders candidate versions by language-standard compatibility.  The
value vocabulary is **Cargo's `resolver.incompatible-rust-versions` verbatim**:

1. **`CABIN_RESOLVER_INCOMPATIBLE_STANDARDS=<mode>`** environment variable.
2. **`[resolver] incompatible-standards = "<mode>"`** in a config file.
3. **Default** - `fallback`.

`<mode>` is `allow` or `fallback`; any other value is rejected before resolution:

```text
$ CABIN_RESOLVER_INCOMPATIBLE_STANDARDS=warn cabin update
error: invalid CABIN_RESOLVER_INCOMPATIBLE_STANDARDS value "warn": invalid incompatible-standards value "warn"; expected one of: allow, fallback
```

`fallback` prefers standard-compatible versions and reports any version held back; `allow` makes
selection a pure function of semver constraints.  See
[`language-standards.md`](language-standards.md#version-selection) and
[`config.md`](config.md#resolver) for the full policy.

### `CPPFLAGS` / `CFLAGS` / `CXXFLAGS` / `LDFLAGS`

Cabin honors the conventional C/C++ build-flag environment variables alongside its typed manifest
`[profile]` fields.  Each variable is read at command start, parsed once, and merged into the
per-package compile / link command-lines.

| Variable | Routes to | Reaches |
|---|---|---|
| `CPPFLAGS` | language-neutral compile bucket | every C and every C++ compile command |
| `CFLAGS` | C-only compile bucket | every C compile command |
| `CXXFLAGS` | C++-only compile bucket | every C++ compile command |
| `LDFLAGS` | link bucket | every link command |

Cabin keeps the four buckets strictly separated.  A `CFLAGS` token never reaches a C++ compile line,
a `CXXFLAGS` token never reaches a C compile line, and neither reaches the link command.  `CPPFLAGS`
is preprocessor-shaped so it lands on both compile languages, but is never treated as a linker flag.

#### Parsing

Each variable's value is split into argv tokens using POSIX shell-style word splitting via the
[`shlex`] crate.  No shell process is invoked.  Quoted runs (`' ...'` and `" ..."`), backslash
escapes (`\<char>`), and whitespace separation all behave as a POSIX shell would when reading a
single command line.

Two adjustments make the parser fit the flag-env-var role rather than a shell command line:

- an unquoted `#` is preserved as a literal character (it does not start a comment), so a value like
  `CFLAGS="-DFOO=1 #r1 -O2"` reaches the compiler with every token intact;
- `\r` outside quotes is treated as a token separator, so CRLF-contaminated values from
  Windows-formatted tooling do not carry a stray `\r` into an argument.

Empty and whitespace-only variables are no-ops.  Malformed shell words - for example, an
unterminated quote or a trailing backslash - are rejected with a clear error that names the
offending variable:

```text
$ CXXFLAGS="'oops" cabin build
error: invalid CXXFLAGS: could not parse shell words
```

```text
$ LDFLAGS='-L/lib\' cabin build
error: invalid LDFLAGS: could not parse shell words
```

[`shlex`]: https://crates.io/crates/shlex

#### Layer order

Environment flags merge with Cabin's existing build-flag sources in this documented order (later
layers append to earlier ones; nothing replaces or erases an earlier layer):

1. Built-in backend defaults (the effective per-target standard flag from the manifest-declared
   standards, see [Language standards](language-standards.md) - and the profile's `-O ...` /
   `-g`, ...).
2. The package's own `[profile]` table.
3. The package's matching `[target.'cfg(...)'.profile]` blocks.
4. The selected profile's `[profile.<name>]` block.
5. `pkg-config` contributions from ``system = true` deps`.
6. `CPPFLAGS`, `CFLAGS`, `CXXFLAGS`, `LDFLAGS` - the environment layer described above.

Environment flags only contribute to **primary** packages - the workspace members the user owns.
Registry and path dependencies still observe their own `[profile]` declarations but are not
augmented by the user's environment, so a `-Werror` in `CXXFLAGS` cannot break a third-party dep.

#### Commands and reproducibility

`cabin build`, `cabin run`, `cabin test`, and `cabin tidy` read the four variables and pass the
parsed tokens through to the generated `build.ninja` and `compile_commands.json`.  The build
configuration fingerprint folds in the per-bucket values, so changing a relevant variable moves the
fingerprint exactly the way changing the matching `[profile]` field would; Ninja rebuilds are driven
by the changed command lines.

`cabin fmt`, `cabin clean`, `cabin new`, and `cabin init` do **not** participate.  These commands
either do not invoke the C/C++ build at all (`fmt`, `new`, `init`) or only touch the on-disk build
directory (`clean`), so the four environment variables have no effect on them.

#### Output policy

- Quiet and normal modes do not print the env flags in Cabin-owned status output.
- Verbose mode prints one Cabin-owned line per active variable on stderr with arg counts only (e.g.
  `cabin: applying CPPFLAGS (2 args)`).
- Commands that invoke the Ninja backend (`cabin build`, `cabin run`, and `cabin test`) also pass
  `-v` to Ninja when Cabin verbosity is verbose or higher.  Ninja then prints the full compile,
  archive, and link commands on stdout, including any tokens contributed by these environment
  variables.
- Very-verbose mode additionally prints the parsed argv tokens on stderr before the build backend
  runs.
- Be careful: environment flag values can contain local include paths or sensitive tokens.  Treat
  verbose and very-verbose logs as command-line output.
- Machine-readable stdout (`cabin metadata --format json`, `cabin tree --format json`, ...) stays
  clean: all chatter routes to stderr.  The metadata JSON view reflects the applied flags through
  the per-package `toolchain.build_flags_per_package` block.

#### `LD` is not honored

The conventional `LD` environment variable selects a linker binary.  Cabin does not honor `LD` -
linker selection would require a linker-command abstraction the C++ backend does not expose.  Use
`LDFLAGS` to extend the link command line (via the C/C++ driver Cabin already picked); the driver is
what controls whether the C++ runtime is pulled in.

## Variables Cabin sets for `cabin run` and `cabin test`

`cabin run` spawns the selected `executable`, and `cabin test` spawns each `test` executable, with
the user's environment **plus** a small, deterministic, identical `CABIN_*` package-execution
overlay:

| Name | Meaning |
|---|---|
| `CABIN_MANIFEST_DIR` | Absolute path to the owning package's manifest directory |
| `CABIN_MANIFEST_PATH` | Absolute path to the owning package's `cabin.toml` |
| `CABIN_PACKAGE_NAME` | Package name as the manifest declares it |
| `CABIN_PACKAGE_VERSION` | Resolved package version |
| `CABIN_PROFILE` | Active profile (`dev`, `release`, ...) |
| `CABIN_BUILD_DIR` | Resolved build directory |

This is the entire injected contract: the overlay is the same for `cabin run` and `cabin test` and
does not depend on the target's name or kind.  The user's `PATH`, `LANG`, etc. are inherited
unchanged, with one subtraction: `CABIN_REGISTRY_TOKEN` is removed from the child environment -
the registry credential is Cabin's input, and spawned code must not be able to read it.  The same
scrub applies to the other tools Cabin spawns (Ninja and the compile / wrapper commands it runs,
the compiler / archiver / wrapper detection probes, `clang-format`, `run-clang-tidy`,
`pkg-config`).  `cabin run`'s working directory is the user's invoking cwd (matching `cargo run`);
`cabin test` runs each executable in its owning package's manifest directory so tests can reach
repository-relative fixture data deterministically.

## Why Cabin does not auto-inject `-DCABIN_PACKAGE_*` macros

Cargo's `CARGO_PKG_*` env vars are compile-time constants in Rust because `env!()` reads them during
compilation.  C/C++ has no equivalent: turning package metadata into compiler `-D` flags would
change every translation unit's preprocessor state, leak into public headers if surfaced via
`usage-requirements`, and churn every affected object file whenever the version bumps.

The default is therefore: **run/test executables receive the metadata as env vars; compile commands
do not receive `-DCABIN_PACKAGE_*` macros**.  Users who want the macro form can add explicit
`defines` to their target in the manifest.

## See also

- [`cargo-inspired-interface.md`](cargo-inspired-interface.md) - full Cabin-vs-Cargo audit.
- [`config.md`](config.md) - config-file precedence and `[paths] build-dir` syntax.
- [`toolchains.md`](toolchains.md) - toolchain inputs that participate in the build-configuration
  fingerprint.
