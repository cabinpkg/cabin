# Compiler-cache wrappers

Cabin can prefix every C++ compile command with a compiler-cache
wrapper such as `ccache` or `sccache`. The wrapper sits *on top* of
the C++ compiler driver — it does not replace it, and it never
intercepts link, archive, or Cargo invocations.

This document is the canonical specification for the wrapper
selection model and how it interacts with the rest of the
toolchain. The behavior described here is what the manifest
parser (`cabin-manifest`), the typed model
(`cabin-core::compiler_wrapper`), the resolver
(`cabin-toolchain::wrapper`), the build planner (`cabin-build`),
the CLI (`cabin`), and the canonical package metadata
(`cabin-package`) all agree on.

## What gets wrapped

| Action                                | Wrapped? |
| ------------------------------------- | -------- |
| C++ compile (`cxx -c <src> -o <obj>`) | **Yes**  |
| Static-library archive (`ar crs …`)   | No       |
| Link (`cxx <objs> -o <exe>`)          | No       |

Only compile commands benefit from caching. Wrapping link or
archive would make every cache miss include the linker work and
defeat the purpose. The split is deliberate and documented.

### `compile_commands.json` vs. `build.ninja`

Both files are written for the same compile actions, but they
record different argument lists when a wrapper is selected:

- `build.ninja` — the **wrapped** command (`ccache cxx -std=c++17
  …`). Ninja invokes this verbatim, so caching takes effect.
- `compile_commands.json` — the **unwrapped** command (`cxx
  -std=c++17 …`). IDE / clangd tooling sees the underlying
  compiler, which is what they expect.

There is no flag to flip this behavior today.

## Supported wrappers

Cabin recognizes two wrappers by name today:

| Kind      | CLI / manifest value | Bare command |
| --------- | -------------------- | ------------ |
| `ccache`  | `"ccache"`           | `ccache`     |
| `sccache` | `"sccache"`          | `sccache`    |

The conservative initial surface accepts only these two named
wrappers plus the special value `"none"` (explicit opt-out).
Path-shaped values (`/usr/local/bin/ccache`) and unknown names
(`fastcache`) are rejected with a clear error so a typo never
silently disables caching. A future revision may extend the
surface.

## Precedence

The resolver walks the layers below in order and keeps the first
that yields a value:

1. **CLI flag** — `--compiler-wrapper <name>` /
   `--no-compiler-wrapper`. Highest precedence; the two flags are
   mutually exclusive.
2. **Environment variable** — `CABIN_COMPILER_WRAPPER`. Empty
   values are treated as unset.
3. **`[build.cache]` config-file layer** — `[build.cache]` in
   `<root>/.cabin/config.toml` (workspace or project) or
   `~/.config/cabin/config.toml` (user). See
   [`config.md`](config.md) for the full file-discovery rules.
4. **`[target.'cfg(...)'.profile.cache]`** matching the host
   platform. Multiple matches settle in declaration order — the
   last match wins, mirroring the build-flag merger.
5. **`[profile.cache]`** on the workspace root manifest.
6. **Default** — no wrapper.

A layer that says `"none"` is a *hard* opt-out: any lower-priority
layer that requested a wrapper is ignored.

The resolver records which layer won under
`toolchain.compiler_wrapper.source` in `cabin metadata` so users
can audit the resolved selection without re-running anything.

## CLI surface

```sh
cabin build --compiler-wrapper ccache
cabin build --no-compiler-wrapper          # disable for one run
cabin metadata --compiler-wrapper sccache  # report what a build would pick
```

`--compiler-wrapper` accepts `none`, `ccache`, or `sccache`.
`--no-compiler-wrapper` is shorthand for `--compiler-wrapper none`
and is mutually exclusive with `--compiler-wrapper`. The flags
apply only to the current invocation; nothing is written back to
the manifest.

`cabin build` and `cabin metadata` accept the flags. `cabin
resolve`, `cabin update`, `cabin fetch`, `cabin package`, and
`cabin publish` deliberately do **not** accept them — wrapper
selection has no effect on dependency resolution, the lockfile,
or the published archive.

## Environment variables

```
CABIN_COMPILER_WRAPPER=ccache cabin build
```

The variable accepts the same values as the CLI flag. Empty
values look identical to no variable at all.

## Manifest syntax

The workspace root manifest may declare a general `[profile.cache]`
table plus any number of conditional overlays. Member or path-dep
manifests that declare any cache settings are rejected with the
error
`compiler-cache wrapper settings may only appear in the workspace root manifest`.

```toml
[profile.cache]
compiler-wrapper = "ccache"

[target.'cfg(os = "linux")'.profile.cache]
compiler-wrapper = "sccache"
```

The schema is `compiler-wrapper = "<name>"`. Unknown sub-keys are
rejected at parse time. The cache table is intentionally a
sub-table of `[profile]` so it composes with the existing `[profile]`
flag layers, but it does **not** merge into the per-package
`ProfileFlags`: cache settings are workspace-wide while build
flags remain per-package.

## Detection

Once the resolver has located the wrapper executable on `PATH` it
runs `<wrapper> --version`, captures the first non-empty output
line, and parses a `major[.minor[.patch]]` substring. The detected
identity becomes available to `cabin metadata`'s
`toolchain.compiler_wrapper.{kind, spec, source, version, raw_version_line}`
block.

`cabin metadata` is fail-soft: if the wrapper executable is
missing or its `--version` exits non-zero, the JSON view falls
back to a `null` `toolchain.compiler_wrapper` block and emits a
warning to stderr. `cabin build` is strict: a missing or
unparsable wrapper executable surfaces a typed error
(`compiler-cache wrapper '<kind>' was requested by <source> but
could not be found on PATH`) before any Ninja file is written, so
a regression that silently disabled caching never reaches a real
build.

## Build configuration fingerprint

`BuildConfiguration::fingerprint` (the SHA-256 already hashed
across features / profile / toolchain / build flags) now also folds
in the resolved wrapper kind, spec,
and detected version. Switching from no wrapper to `ccache`,
upgrading `ccache` to a new version, or flipping
`--no-compiler-wrapper` all produce a different fingerprint by
design — a future cache layer keys on the same value.

## Build-script environment

```text
CABIN_COMPILER_WRAPPER          # "ccache" | "sccache" — unset when no wrapper
CABIN_COMPILER_WRAPPER_PATH     # absolute path to the resolved wrapper
CABIN_COMPILER_WRAPPER_VERSION  # "4.10.2" — set only when version was parsed
```

Build scripts only see the workspace-wide wrapper selection; the
wrapper applies uniformly to every package's C++ compile commands,
so there is no per-package variant of these variables.

## Package + index metadata

`cabin package` writes the manifest's declared `[profile.cache]`
and `[target.'cfg(...)'.profile.cache]` tables into the canonical
`<name>-<version>.json` so consumers who rebuild from source can
reproduce the same wrapper preferences.
**Environment- or CLI-derived selections are never written.** A
user's `CABIN_COMPILER_WRAPPER=...` or `--compiler-wrapper ...`
only affects the local invocation; it never leaks into a published
archive.

The local file registry and the sparse-HTTP index round-trip the
same fields opaquely. Older registries that omit the new fields
continue to load. The resolver itself does not consult any of
these values — registry resolution remains wrapper-independent.

## Deferred / out of scope

- distcc / icecc / other compile-server wrappers.
- Per-language wrappers (C-only / C++-only). The current model
  applies one wrapper to every C++ compile command in the build.
- Wrapper-specific configuration (e.g. `ccache --max-size`). Use
  the wrapper's own configuration mechanism (`~/.config/ccache/ccache.conf`,
  `SCCACHE_*` env vars).
- Path-shaped CLI / manifest values
  (`--compiler-wrapper /opt/cache/bin/ccache`). Today only the
  named kinds are accepted.
- Wrapping link / archive / Cargo invocations.
- A `--show-compiler-cache-stats`-style command. Use the
  wrapper's own stats command (`ccache -s`, `sccache -s`).
