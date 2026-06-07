# Target / platform-specific dependencies

Cabin supports declaring a dependency only for a particular host
platform via `[target.'cfg(...)'.<kind>]` tables in `cabin.toml`.
The shape mirrors Cargo's syntax so the manifest stays familiar,
but the supported predicates and evaluation context are pinned
down to a small, deterministic set.

This document is the canonical specification. The behavior
described here is what the manifest parser, the workspace loader,
the resolver, the feature resolver, the fetch / build pipeline,
the canonical package metadata, the local and sparse-HTTP index
loaders, and the `cabin metadata` JSON view all agree on.

## Manifest syntax

A target-conditional dependency table is a TOML table whose key is
the literal string `target` followed by a TOML key that is a
quoted `cfg(...)` expression. The dependency-kind sub-tables
inside are the same dependency kinds:

```toml
[target.'cfg(os = "linux")'.dependencies]
fmt = ">=10"

[target.'cfg(arch = "x86_64")'.dev-dependencies]
gtest = "^1.14"

[target.'cfg(env = "musl")'.dependencies]
zlib = { version = ">=1.2", system = true }
```

Each dependency entry inside a conditional table accepts the same
fields as the top-level kind table (per-edge `features`,
`optional`, `default-features` for Cabin package deps; `system =
true` with `version` for system deps). Every declared system
dependency is required.

The same `[target.'cfg(...)']` machinery also applies to the
`[toolchain]` and `[profile]` tables introduced for explicit
toolchain selection and conditional build flags:

```toml
[target.'cfg(os = "linux")'.toolchain]
ar = "llvm-ar-18"

[target.'cfg(os = "linux")'.profile]
defines = ["USE_EPOLL"]
```

Conditional `[toolchain]` tables follow the same workspace-root-
only rule as the unconditional `[toolchain]` table. Conditional
`[profile]` tables are per-package, like the rest of `[profile]`.
Full protocol in [`toolchains.md`](toolchains.md).

`workspace = true` is **not** allowed inside a conditional table.
Workspace inheritance only flows through the unconditional
`[workspace.<kind>-dependencies]` tables; mixing the two would
allow a single workspace key to silently mean different things
on different hosts. Use a workspace dep without a condition, or
declare the dep directly inside the conditional table.

## Supported `cfg` grammar

A predicate is one of:

- `<key> = "<value>"` — a key/value test;
- `all(<expr>, <expr>, …)` — every nested predicate must hold
  (at least one predicate is required; empty `all()` is rejected);
- `any(<expr>, <expr>, …)` — at least one nested predicate must
  hold (at least one predicate is required; empty `any()` is rejected);
- `not(<expr>)` — exactly one nested predicate, negated.

Keys are bare identifiers; values are double-quoted strings.
Unknown keys, unquoted values, missing parentheses, and wrong
arity in `not(...)` are rejected at parse time with a clear
error that names the offending input.

The supported keys are fixed:

| Key      | Source                                                         | Examples                              |
| -------- | -------------------------------------------------------------- | ------------------------------------- |
| `os`     | `std::env::consts::OS`                                         | `"linux"`, `"macos"`, `"windows"`     |
| `arch`   | `std::env::consts::ARCH`                                       | `"x86_64"`, `"aarch64"`               |
| `family` | `std::env::consts::FAMILY`                                     | `"unix"`, `"windows"`                 |
| `env`    | mapped from the host OS (`"unknown"` for an unsupported OS)    | `"gnu"`, `"apple"`, `"msvc"`, `"unknown"` |
| `abi`    | always `"unknown"` (the host ABI is not detected today)        | `"unknown"`                           |
| `target` | constructed as `arch-family-os` (not a standard target triple) | `"x86_64-unix-linux"`, `"aarch64-unix-macos"` |

Those six are the **platform** keys. One additional key is
recognized for flag tables only:

| Key       | Source                                          | Examples            |
| --------- | ----------------------------------------------- | ------------------- |
| `feature` | the enabled-feature set of the owning package   | `"simd"`, `"single-threaded"` |

`feature = "<name>"` evaluates against the package's resolved
[features](features.md) rather than the host platform, so it can
be combined with platform keys —
`cfg(all(feature = "simd", arch = "x86_64"))` — to gate, say, an
AVX translation unit's defines. There is no `cfg(feature = ...)`
to C/C++ `#if` bridge: a feature condition gates Cabin's own
build-flag layers (`defines`, `cflags`, `link-libs`, …), and you
map a feature to the library's own macro explicitly, e.g.
`[target.'cfg(feature = "single-threaded")'.profile] defines =
["SQLITE_THREADSAFE=0"]`.

**`feature` is accepted on flag tables only** — i.e. inside
`[target.'cfg(...)'.profile]`. A `cfg(...)` that references
`feature` is **rejected** when it gates a `dependencies`,
`dev-dependencies`, or `toolchain` table, because feature
resolution itself walks the dependency graph: a feature-gated
dependency would be circular, and the dependency/toolchain
evaluation paths run before features are known. Use `[features]`
with `dep:<name>` / `<dep>/<feature>` entries to gate optional
dependencies on features instead.

Adding more keys requires a spec-level decision because it widens
the public manifest grammar and the canonical metadata schema.

## Evaluation context

Cabin evaluates predicates against the **host** platform, derived
once via `cabin_core::TargetPlatform::current()`. There is no
cross-compilation in this step, so the evaluation context is
deterministic for a given machine and is reported back to the
user under `target_platform` in `cabin metadata` so dependency
filtering is auditable.

The evaluation rules are:

- `key = "value"` matches when the host's value for `key`
  equals `value` exactly (string equality, case-sensitive).
- `all` / `any` short-circuit and recurse.
- `not` inverts.
- Unknown keys never reach evaluation — the parser rejects them.

A dependency whose predicate fails on the host is filtered out
**before** it reaches:

- the workspace closure walker
  (`cabin_workspace::collect_closure_versioned_deps_filtered`);
- the resolver input (no constraint reaches the solver);
- the feature resolver (`cabin_feature::resolve_features` skips
  it when computing `dep:` entries);
- the artifact fetch path;
- the build planner;
- the canonical metadata document and the `cabin metadata` JSON
  view's "active" flag.

The `condition` is preserved on `Dependency::condition` and
`DependencyEdge::condition` so the JSON view can still surface it
as `target` plus an `active: false` marker; the dependency
itself does not participate in resolution or build.

## Round-trip through publishing

`cabin package` and `cabin publish` carry the predicate through
the canonical [`PackageMetadata`](package-format.md) document
under a `target` field on each dependency table entry, and the
local-file plus sparse-HTTP index loaders parse the same field
back into `IndexPackageDependency::condition` /
`IndexSystemDependency::condition`. Older metadata that omits
`target` continues to load — the field is optional in every
schema.

The on-disk encoding of a `Condition` is its canonical
inner-expression form (`os = "linux"`,
`all(os = "linux", arch = "x86_64")`, …). The wrapping
`cfg(...)` is implicit because the field name already carries
that meaning in the schema.

## Lockfile

The lockfile records the resolved closure for the host platform,
exactly as before. Because filtering happens before resolution,
the lockfile always describes a valid, host-evaluated set; it is
not partitioned per-platform. If the host platform changes, the
closure may change and the lockfile is updated by re-running
`cabin update` or by removing and regenerating it. Cross-platform
lockfile partitioning is deliberately out of scope for this step.

## CLI behavior

`cabin metadata` reports two new pieces of information:

- a top-level `target_platform` block listing the values used to
  evaluate predicates (`os`, `arch`, `family`, `env`, `abi`,
  `target`);
- per-dependency `target` (when present) and `active` flags so
  consumers can decide whether to surface or filter inactive
  declarations without re-evaluating the predicate.

`cabin resolve`, `cabin fetch`, `cabin build`, and
`cabin update` apply the host-platform filter implicitly. They do
not gain new flags — cross-compilation lives outside this step.

## Errors

Errors are rendered without referring to internal step numbers.
Examples (the precise wording lives in the manifest / index error
types):

- "invalid `cfg(...)` expression in `[target.'…'.dependencies]`:
  unknown key `host_endian`"
- "`cfg(...)` expressions do not accept `workspace = true` —
  declare the dependency in `[workspace.dependencies]` and use
  `dep = { workspace = true }` outside any `target.cfg` table"
- "expected double-quoted string after `os =`"

The CLI surfaces them through the same channel as ordinary
manifest errors: `cabin metadata`, `cabin resolve`, and
`cabin build` all stop early and report the offending line.
