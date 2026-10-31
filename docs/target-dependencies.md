# Target / platform-specific dependencies

Cabin supports declaring a dependency only for a particular platform via
`[target.'cfg(...)'.<kind>]` tables in `cabin.toml`.  The shape mirrors Cargo's syntax so the
manifest stays familiar, but the supported predicates and evaluation context are pinned down to a
small, deterministic set.

This document is the canonical specification.  The behavior described here is what the manifest
parser, the workspace loader, the resolver, the feature resolver, the fetch / build pipeline, the
canonical package metadata, the local and sparse-HTTP index loaders, and the `cabin metadata` JSON
view all agree on.

## Manifest syntax

A target-conditional dependency table is a TOML table whose key is the literal string `target`
followed by a TOML key that is a quoted `cfg(...)` expression.  The dependency-kind sub-tables
inside are the same dependency kinds:

```toml
[target.'cfg(os = "linux")'.dependencies]
fmt = ">=10"

[target.'cfg(arch = "x86_64")'.dev-dependencies]
gtest = "^1.14"

[target.'cfg(os = "linux")'.build-dependencies]
gen = { path = "tools/gen" }

[target.'cfg(env = "musl")'.dependencies]
zlib = { version = ">=1.2", system = true }
```

Each dependency entry inside a conditional table accepts the same fields as the top-level kind table
(per-edge `features`, `optional`, `default-features` for Cabin package deps; `system = true` with
`version` for system deps).  Every declared system dependency is required.

The same `[target.'cfg(...)']` machinery also applies to the `[toolchain]` and `[profile]` tables
introduced for explicit toolchain selection and conditional build flags:

```toml
[target.'cfg(os = "linux")'.toolchain]
ar = "llvm-ar-18"

[target.'cfg(os = "linux")'.profile]
defines = ["USE_EPOLL"]

[target.'cfg(os = "linux")'.profile.release]
ldflags = ["-static"]
```

A `[target.'cfg(...)'.sources]` table adds sources to a named target on matching hosts; see
[Platform-conditional sources](targets.md#platform-conditional-sources).

Conditional `[toolchain]` tables follow the same workspace-root- only rule as the unconditional
`[toolchain]` table.  Conditional profile flag tables are per-package, like the rest of
`[profile]`.  `[target.'cfg(...)'.profile]` applies to every selected profile when the predicate
matches.  `[target.'cfg(...)'.profile.<name>]` additionally requires the selected profile's
resolved inheritance chain to contain `<name>`; it is a flag overlay and does not define a
selectable profile.  Full protocol in [`profiles.md`](profiles.md) and
[`toolchains.md`](toolchains.md).

`workspace = true` is **not** allowed inside a conditional table.  Workspace inheritance only flows
through the unconditional `[workspace.<kind>-dependencies]` tables; mixing the two would allow a
single workspace key to silently mean different things on different hosts.  Use a workspace dep
without a condition, or declare the dep directly inside the conditional table.

## Supported `cfg` grammar

A predicate is one of:

- `<key> = "<value>"` - a key/value test;
- `all(<expr>, <expr>, ...)` - every nested predicate must hold (at least one predicate is required;
  empty `all()` is rejected);
- `any(<expr>, <expr>, ...)` - at least one nested predicate must hold (at least one predicate is
  required; empty `any()` is rejected);
- `not(<expr>)` - exactly one nested predicate, negated.

Expressions nest at most 32 levels deep, counting the innermost predicate as one level - so
`os = "linux"` is one level and `not(os = "linux")` is two.  Nesting *width* is unbounded: `all(...)`
and `any(...)` accept as many sibling predicates as you like.  Real predicates nest two or three
levels; the cap exists so that a hostile manifest key or registry index entry cannot exhaust the
parser's stack.  A deeper expression is rejected at parse time with an error naming the cap.

Keys are bare identifiers; values are double-quoted strings.  Unknown keys, unquoted values, missing
parentheses, and wrong arity in `not(...)` are rejected at parse time with a clear error that names
the offending input.

The supported keys are fixed:

| Key      | Source                                                         | Examples                              |
| -------- | -------------------------------------------------------------- | ------------------------------------- |
| `os`     | `std::env::consts::OS`                                         | `"linux"`, `"macos"`, `"windows"`     |
| `arch`   | `std::env::consts::ARCH`                                       | `"x86_64"`, `"aarch64"`               |
| `family` | `std::env::consts::FAMILY`                                     | `"unix"`, `"windows"`                 |
| `env`    | mapped from the host OS (`"unknown"` for an unsupported OS)    | `"gnu"`, `"apple"`, `"msvc"`, `"unknown"` |
| `abi`    | always `"unknown"` (the host ABI is not detected today)        | `"unknown"`                           |
| `target` | constructed as `arch-family-os` (not a standard target triple) | `"x86_64-unix-linux"`, `"aarch64-unix-macos"` |

The table describes the build machine.  Under `--target <triple>` the six values come from the
triple instead: `arch` and `os` from its components (`arm64` reads as `"aarch64"`, `darwin` as
`"macos"`), `env` and `abi` from its environment component (`gnueabihf` reads as `"gnu"` plus
`"eabihf"`), and `target` is the triple as given.

Those six are the **platform** keys.  Five additional keys are recognized for flag tables only:

| Key           | Source                                              | Examples            |
| ------------- | --------------------------------------------------- | ------------------- |
| `feature`     | the enabled-feature set of the owning package       | `"simd"`, `"single-threaded"` |
| `cc`          | detected C compiler family                          | `"gcc"`, `"clang"`, `"msvc"` |
| `cxx`         | detected C++ compiler family                        | `"clang"`, `"apple-clang"` |
| `cc_version`  | detected C compiler version (SemVer requirement)    | `">=12"`, `"=14"`   |
| `cxx_version` | detected C++ compiler version (SemVer requirement)  | `">=18"`, `">=16, <19"` |

`feature = "<name>"` evaluates against the package's resolved [features](features.md) rather than
the platform, so it can be combined with platform keys - `cfg(all(feature = "simd", arch =
"x86_64"))` - to gate, say, an AVX translation unit's defines.  There is no `cfg(feature = ...)` to
C/C++ `#if` bridge: a feature condition gates Cabin's own build-flag layers (`defines`, `cflags`,
`link-libs`, ...), and you map a feature to the library's own macro explicitly, e.g.
`[target.'cfg(feature = "single-threaded")'.profile] defines = ["SQLITE_THREADSAFE=0"]`.

**`feature` is accepted on flag tables only** - i.e. inside `[target.'cfg(...)'.profile]`.  A
`cfg(...)` that references `feature` is **rejected** when it gates a `dependencies`,
`dev-dependencies`, `toolchain`, or `sources` table, because feature resolution itself walks the
dependency graph: a feature-gated dependency would be circular, the dependency/toolchain evaluation
paths run before features are known, and a target's sources must not depend on its features.  Use `[features]` with `dep:<name>` / `<dep>/<feature>` entries to
gate optional dependencies on features instead.

### Compiler conditions

`cc` / `cxx` match the **detected family id** of the resolved C / C++ compiler - the same ids `cabin
metadata` reports under `toolchain.detected.<slot>.identity.kind`: `clang`, `apple-clang`,
`clang-cl`, `gcc`, `msvc`, and `unknown`.  Any other value (an executable name like `"clang++"`, a
different casing) is rejected at manifest parse time, because a typo here would otherwise silently
never match.  `unknown` is matchable and covers an unrecognized `--version` banner, the fail-soft
commands where detection could not run at all, and (for `cc`) an unresolved C compiler slot.

`cc_version` / `cxx_version` take a SemVer **requirement** - the same grammar as dependency
versions, including comma/space comparator lists - matched against the detected version zero-padded
to `major.minor.patch`:

```toml
[target.'cfg(all(cxx = "clang", cxx_version = ">=18"))'.profile]
cxxflags = ["-stdlib=libc++"]
```

Partial-version semantics are SemVer's: `">=18"` accepts 18.0.0 and newer; `">18"` accepts 19.0.0
and newer (it excludes every 18.x); bare `"18"` and `"=18"` accept any 18.x.  A compiler whose
version could not be parsed never satisfies a version requirement.

Version numbers live in **per-family spaces** (Apple clang 15 is not LLVM clang 15, and MSVC
versions are `19.x`), so a standalone `cxx_version` condition is legal but almost always wants an
`all(...)` with the matching family check.

**Compiler conditions are accepted on flag tables only** - the same placement rule as `feature`.
They are **rejected** when they gate `dependencies`, `dev-dependencies`, `toolchain`, or `sources`:
dependency resolution and toolchain selection run before any compiler has been detected, the
toolchain case would be circular, and a target's sources must not depend on the detected compiler. Compiler-wrapper selection is unconditional manifest build configuration under
`[build]`; there is no target-conditional wrapper form.

Adding more keys requires a spec-level decision because it widens the public manifest grammar and
the canonical metadata schema.

`profile` is not a `cfg(...)` key.  Profile selection remains a separate build-configuration axis;
use `[target.'cfg(...)'.profile.<name>]` to combine a target predicate with a selected-profile
constraint.  Dependencies and toolchain selection remain profile-independent.

## Evaluation context

Cabin evaluates predicates against the platform a command builds for: the build machine, derived
via `cabin_core::TargetPlatform::current()`, or the platform `--target <triple>` names (see
[Building for another platform](toolchains.md#building-for-another-platform)).  The evaluation
context is deterministic for a given machine and flags and is reported back to the user under
`target_platform` in `cabin metadata` so dependency filtering is auditable.  The flag-table-only
keys read two further inputs: `feature` reads the owning package's resolved feature set, and the
compiler keys read the resolved toolchain's detection report (the same one `cabin metadata` shows
under `toolchain.detected`).

A package built as a [build dependency](dependency-kinds.md#build-dependencies) runs on the build
machine, so its own predicates - dependencies, sources, and flag tables - evaluate for the build
machine even under `--target`.  The `[target.'cfg(...)'.build-dependencies]` predicate itself
belongs to the declaring package and evaluates for that package's platform.

The evaluation rules are:

- `key = "value"` matches when the platform's value for `key` equals `value` exactly (string
  equality, case-sensitive).
- `cc_version` / `cxx_version` match when the detected version satisfies the SemVer requirement (no
  detected version - => no match).
- `all` / `any` short-circuit and recurse.
- `not` inverts.
- Unknown keys never reach evaluation - the parser rejects them.

Dependency resolution does not evaluate predicates: the [lockfile](#lockfile) serves every
platform, so every declaration reaches the workspace loader (a conditional path dependency loads
everywhere), the resolver input, the resolver's walk over registry metadata, and therefore the
artifact fetch path and `cabin vendor`.  A dependency whose predicate fails for the platform is left
out of the build:

- the build's feature resolution (`cabin_feature::resolve_features` skips it when computing `dep:`
  entries and per-edge requests);
- the build planner (a target dependency resolves only through a matching edge);
- `pkg-config` probing of conditional system dependencies;
- `cabin tree`, which follows only the edges active on the build machine, and `cabin explain`, which
  follows those active on the build machine or the `--target` platform;
- the `cabin metadata` JSON view's "active" flag.

The `condition` is preserved on `Dependency::condition` and `DependencyEdge::condition` so the JSON
view can still surface it as `target` plus an `active: false` marker.

## Round-trip through publishing

`cabin package` and `cabin publish` carry the predicate through the canonical
[`PackageMetadata`](package-format.md) document under a `target` field on each dependency table
entry, and the local-file plus sparse-HTTP index loaders parse the same field back into
`IndexPackageDependency::condition` / `IndexSystemDependency::condition`.  Older metadata that omits
`target` continues to load - the field is optional in every schema.

The on-disk encoding of a `Condition` is its canonical inner-expression form (`os = "linux"`,
`all(os = "linux", arch = "x86_64")`, ...).  The wrapping `cfg(...)` is implicit because the field
name already carries that meaning in the schema.

## Lockfile

The lockfile serves every platform.  Resolution treats every conditional declaration as active, so
the same manifests lock the same packages whichever machine resolves them, and `--locked`,
`--offline`, `cabin fetch`, and `cabin vendor` have every platform's dependencies in hand.  One
package name still resolves to one version for all platforms; declarations gated to different
platforms that cannot agree on a version fail resolution, as two unconditional ones would.  The
`links` check and the workspace loader's dependency-cycle check run over the same every-platform
graph, so two packages that claim one native identity collide, and two path dependencies that point
at each other form a cycle, even when gated to different platforms.

An older lockfile can lack the dependencies gated to other platforms; see
[`lockfile.md`](lockfile.md#format).

## CLI behavior

`cabin metadata` reports two new pieces of information:

- a top-level `target_platform` block listing the values used to evaluate predicates (`os`, `arch`,
  `family`, `env`, `abi`, `target`);
- per-dependency `target` (when present) and `active` flags so consumers can decide whether to
  surface or filter inactive declarations without re-evaluating the predicate.

`cabin resolve`, `cabin fetch`, `cabin vendor`, and `cabin update` resolve every platform's
declarations; `cabin build`, `cabin run`, and `cabin test` build for the build machine, or for the
platform `--target <triple>` names.

## Errors

Errors are rendered without referring to internal step numbers.  Examples (the precise wording lives
in the manifest / index error types):

- "invalid `cfg(...)` expression in `[target.' ...'.dependencies]`: unknown key `host_endian`"
- "`cfg(...)` expressions do not accept `workspace = true` - declare the dependency in
  `[workspace.dependencies]` and use `dep = { workspace = true }` outside any `target.cfg` table"
- "expected double-quoted string after `os =`"

The CLI surfaces them through the same channel as ordinary manifest errors: `cabin metadata`, `cabin
resolve`, and `cabin build` all stop early and report the offending line.
