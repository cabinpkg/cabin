# Language standards

Cabin treats C and C++ language standards as **typed build metadata**: you declare them in
`cabin.toml`, and Cabin lowers them to the dialect-correct compiler flag, validates that the
selected toolchain supports them before the build, enforces library interface requirements on
consumers, folds them into the build-configuration fingerprint, and reports them through `cabin
metadata` / `cabin explain build-config`.

This document is the canonical specification.  The behavior described here is what the manifest
parser (`cabin-manifest`), the typed model and resolution (`cabin-core::language_standard`), the
workspace loader (`cabin-workspace`), the build planner and pre-build validation (`cabin-build`),
the dialect lowering (`cabin-driver`), the CLI (`cabin`), the canonical package metadata
(`cabin-package`), and the local / sparse-HTTP index loaders (`cabin-index`, `cabin-index-http`) all
agree on.

## Manifest fields

Four kebab-case standard fields plus the `gnu-extensions` boolean, accepted at both `[package]` and
`[target.<name>]` level:

```toml
[package]
name = "foo"
version = "0.1.0"
c-standard = "c11"
cxx-standard = "c++17"
interface-cxx-standard = "c++17"  # optional package-wide default

[target.core]
type = "library"
sources = ["src/core.cc"]
include-dirs = ["include"]
cxx-standard = "c++20"            # implementation standard override
interface-cxx-standard = "c++17"  # consumers only need C++17
gnu-extensions = true             # GNU-extension dialect for this target
```

- `c-standard` / `cxx-standard` - the **implementation standard**: how this package's (or target's)
  sources are compiled.  `.c` sources use the effective C standard; `.cc` / `.cpp` / `.cxx` / `.c++`
  / `.C` sources use the effective C++ standard.  A mixed-language target compiles each source with
  its language's standard.
- `interface-c-standard` / `interface-cxx-standard` - the **interface requirement**: the minimum
  standard consumers of the target's public headers need.  Only meaningful on `library` /
  `header-only` targets; a target-level interface field on an `executable` / `test` / `example`
  target is a manifest error.  Package-level interface fields are defaults consumed only by
  library-like targets (they are allowed, and inert, in packages without any).  The special value
  `"none"` declares that the target's headers are not consumable from that language at all; it is
  valid only on these two fields.
- `gnu-extensions` - whether the target's sources rely on GNU language extensions.  A plain boolean
  (no `{ workspace = true }` form), defaulting to `false`; a target-level value overrides the
  package-level one.  It affects only which compiler flag spelling the standard lowers to
  (`-std=gnu++20` instead of `-std=c++20` on GCC/Clang; see [Flag lowering](#flag-lowering)) and
  never participates in interface compatibility.  On the MSVC dialect it is rejected at planning
  time - `cl.exe` has no GNU dialect mode - never silently ignored.

Mental model: `c-standard` / `cxx-standard` set how the target is compiled **and** double as its
interface requirement unless `interface-*` overrides them.  Declare `interface-*` only when the
public interface requires an *older* standard than the implementation - for example a library
compiled as C++20 whose public headers only use C++17.  An interface minimum *newer* than the
implementation standard is rejected as a contradiction (see
[Precedence](#precedence)).

### Workspace defaults

A workspace root's `[workspace]` table accepts the same four fields as shared defaults; member
packages opt in per field:

```toml
[workspace]
members = ["packages/*"]
cxx-standard = "c++20"
```

```toml
# member cabin.toml
[package]
name = "core"
version = "0.1.0"
cxx-standard = { workspace = true }   # inherits c++20
```

- The `[workspace]` fields take **literal values only** - the same typed value sets as the
  `[package]` fields, with the same unknown-value error.  The opt-in marker is not a legal value
  there.
- A member opts in **per field** with `<field> = { workspace = true }`, at `[package]` level only -
  a marker on a `[target.<name>]` field is rejected.  The workspace root's own `[package]` may opt
  into its own `[workspace]` values.
- The workspace loader resolves the marker at load time, and the inherited value lands in the
  member's **package tier** - the precedence chain below is unchanged, and `[target.<name>]` fields
  still override an inherited value.
- Opting in counts as declaring: the escape-hatch conflict rule fires for an inherited standard
  exactly as for a literal, and interface relevance / enforcement treat inherited values like
  literals.
- Opting into a field the workspace root does not declare fails at load with an error naming the
  package, the field, and the manifest path (" ... but the workspace root does not declare `<field>`
  under `[workspace]`").  The same error fires for a marker in a standalone package with no
  workspace.
- `workspace = false` is rejected: either remove the field or declare a literal standard value.

## Accepted values

Typed value sets of ISO levels; anything else is a manifest parse error listing the valid
identifiers.

- C: `c89`, `c99`, `c11`, `c17`, `c23`.  `c90` is a parse alias of `c89`, normalized immediately.
- C++: `c++98`, `c++11`, `c++14`, `c++17`, `c++20`, `c++23`, `c++26`.  `c++03` is a parse alias of
  `c++98`, normalized immediately.

Ordering is the plain chronological chain per language (in particular `c11 < c17`).  GNU dialect
spellings (`gnu11`, `gnu++20`, …) are not standard values - GNU extensions are the orthogonal
per-target `gnu-extensions` boolean - so they are rejected as ordinary unknown values.

Two further shapes get dedicated diagnostics instead of the generic unknown-value error:

- Range-like inputs (anything containing `>=`, `<=`, `>`, `<`, or `,`) are rejected with an error
  saying range requirements are reserved for a future version.  Internally each interface
  requirement is already a `{ min, max }` pair whose `max` slot is reserved for that future range
  support and never populated today, but the slot stays in every serialized form so the wire shape
  will not change when ranges land.
- `"none"` is valid only on `interface-c-standard` / `interface-cxx-standard`; on `c-standard` /
  `cxx-standard` it is rejected with an error naming the interface fields, because compiled sources
  always need a concrete standard.

## Precedence

Per language, per target:

- Effective implementation standard: `[target.<name>].c-standard` - > `[package].c-standard`
  (same chain for `cxx-standard`).
- Effective interface standard (library-like targets): `[target.<name>].interface-c-standard` - >
  `[package].interface-c-standard` - > the target's effective implementation standard (same chain
  for C++).

There is **no built-in default**.  Standards are required where they matter:

- A target that compiles C sources must have an effective `c-standard`; one that compiles C++
  sources must have an effective `cxx-standard`.  A manifest that violates this is rejected at load
  with an error naming the target, the missing field, and the `{ workspace = true }` opt-in.
- A `header-only` target must declare at least one interface standard
  (`interface-c-standard` / `interface-cxx-standard`, at `[target.<name>]` or `[package]` level) so
  consumers know what its headers require; a header-only target without one is rejected at load.
- A library that compiles sources may still omit its interface fields: the interface defaults to
  the (explicitly declared) effective implementation standard.  A declared `"none"` occupies the
  same slot in the chain, so it also suppresses that fallback.

One combination is rejected outright, after workspace markers resolve: for the same language, an
effective interface minimum **newer** than the target's effective implementation standard is a
contradiction - the target's own translation units could not include its own public headers.  The
load fails with `cabin::language::interface_standard_contradiction` naming the target, both values,
and that reason.  The check applies per library-like target and only to languages the target
actually compiles (a header-only target has no translation units, so it is exempt).

A workspace-inherited value (see "Workspace defaults" above) occupies the `[package]` slot of the
chain - inheritance adds no new tier, and opting in with `{ workspace = true }` counts as
declaring.

Registry and foundation-port packages keep their own declared standards: unlike the raw `cflags` /
`cxxflags` escape hatches (dropped for registry packages during flag resolution), a typed standard
is a bounded correctness requirement, so a published `c++20` library still compiles as C++20 inside
your build.

## Flag lowering

The standard never appears in `[profile]` flags; the dialect layer spells it:

| Dialect | Spelling |
| --- | --- |
| GCC / Clang | `-std=<value>` (e.g. `-std=c++20`); with `gnu-extensions = true`, the GNU spelling of the same ISO level (`-std=gnu++20`, `-std=gnu17`) |
| MSVC (`cl` / `clang-cl`) | `/std:<value>` - only `c11`, `c17`, `c++14`, `c++17`, `c++20` have stable flags |

The GNU spelling is produced by the dialect layer at lowering time from the target's ISO level and
its effective `gnu-extensions` value - strictly per target, so two targets in one build may differ
in both.  Standards without a stable MSVC flag (C89/C99/C23, C++98/11/23/26) are rejected before
the build on the MSVC dialect, as is any target with `gnu-extensions = true` (`cl.exe` has no GNU
dialect mode; remove the field or build with GCC/Clang).  `compile_commands.json` records the same
per-file standard the build uses, so
clangd and `cabin tidy` see exactly how each translation unit compiles.  Changing a standard changes
the lowered command, so Ninja rebuilds exactly the affected translation units.

## Toolchain validation

After planning and before any Ninja file is written, Cabin checks every standard the planned
compiles request against the detected compiler - the whole set, not the maximum, because MSVC
support is non-monotonic (`/std:c++20` exists, `/std:c++11` does not).  Because the set comes from
the final planned graph, only compiles the command runs participate: a sibling target that `cabin
run --bin <name>` never plans cannot gate the toolchain, and the dependency compiles `cabin check`
drops do not gate the check.  The thresholds gate acceptance of the exact flag spelling:

| C standard | GCC | Clang | Apple Clang | `clang-cl` | MSVC `cl` |
| --- | --- | --- | --- | --- | --- |
| `c89` / `c99` | always | always | always | n/a | n/a |
| `c11` | always | always | always | 13 | 19.28 |
| `c17` | 8 | 6 | 10 | 13 | 19.28 |
| `c23` | 14 | 18 | 17 | n/a | n/a |

| C++ standard | GCC | Clang | Apple Clang | `clang-cl` | MSVC `cl` |
| --- | --- | --- | --- | --- | --- |
| `c++98` / `c++11` | always | always | always | n/a | n/a |
| `c++14` | 5 | always | always | always | 19.10 |
| `c++17` | 5 | always | always | always | 19.11 |
| `c++20` | 10 | 10 | 12 | 13 | 19.29 |
| `c++23` | 11 | 17 | 16 | n/a | n/a |
| `c++26` | 14 | 17 | 16 | n/a | n/a |

`always` means any recognized version; `n/a` means no stable flag exists and the request is rejected
on that compiler with an actionable error.  A compiler whose version banner cannot be parsed fails
open (`assumed-default`), matching the rest of capability detection.  The planner additionally
records any MSVC-dialect compile whose standard has no stable `/std:` flag (no compile-commands
entry will be generated); the build is rejected if that compile survives into the final graph - so a
dependency compile `cabin check` drops never gates the check, while `cabin build` / `run` / `test` /
`tidy` still fail fast on real violations.

## Interface enforcement

A library-like target imposes its effective interface standard on every target that transitively
depends on it, per language, checked after planning and before any Ninja file is written:

- The consumer's effective implementation standard must be **at least** the dependency's interface
  minimum (chronological comparison).  This is a pragmatic compatibility policy, not a proof - it
  assumes headers valid under standard *N* stay valid under newer modes; Cabin does not verify
  header validity per standard.  `gnu-extensions` never participates in this comparison.
- An explicit `"none"` requirement carries no minimum, so it imposes nothing on consumers today;
  rejecting consumers of not-consumable headers is deferred alongside range support.
- A language is relevant to a dependency only if the dependency has sources of that language,
  declares a target-level field for it, or is `header-only` while its package declares a
  package-level *interface* standard for it.  A package-level *implementation* default alone never
  creates relevance - a pure-C library imposes no C++ requirement on C++ consumers.
- The check applies only to languages the consumer compiles.

Because an omitted interface standard defaults to the effective implementation standard, a library
declaring `cxx-standard = "c++20"` and nothing else implicitly requires C++20 from consumers;
declare `interface-cxx-standard = "c++17"` to relax that when the public headers permit.

Like the other standards checks, enforcement is scoped to the final planned graph: the planner
records each incompatibility on the consumer's compiles, so a pair whose compiles `cabin check`
drops - a dependency built below another dependency's interface requirement - never gates the
syntax-only check, while `cabin build` / `run` / `test` / `tidy` still fail before anything is
compiled.

## The conflict rule

Declaring a first-class standard alongside a raw standard flag is ambiguous and rejected: if a
planned compile carries both a first-class implementation standard declaration for its language
(package level, or a target-level field on the compiled target) and a `-std=` / `--std=` / `/std:`
token in the manifest-derived `cflags` (C) / `cxxflags` (C++), the build fails with
`cabin::language::standard_flag_conflict`.  The conflict is scoped to the compiles the declaration
covers - an unbuilt sibling target's declaration never gates a command that does not compile it -
and environment `CPPFLAGS` / `CFLAGS` / `CXXFLAGS` and `pkg-config` output are exempt (candidates
are detected before those layers merge).  A workspace-inherited standard counts as a declared
standard for this rule - opting in is declaring.

Because every compiled language must declare a standard, a manifest-level raw standard flag always
sits next to some covering declaration for the compiles it reaches: the raw-flag route through
`cflags` / `cxxflags` is effectively closed for standard selection.  GNU extensions do not need it
(`gnu-extensions = true` is the first-class knob), and the environment variables above remain the
deliberately unvalidated injection point (see [Not supported](#not-supported)).

## Fingerprint

The effective standards (package level plus every target, implementation and interface) are folded
into `BuildConfiguration::fingerprint` under a labeled `language-standards` section - values only;
provenance labels do not move the fingerprint.  A target's effective `gnu-extensions` contributes a
line only when `true`, so the default moves nothing.

## Metadata

`cabin metadata` reports the effective standards with provenance inside each declaring package's
`configuration` block, and `cabin explain build-config <package>` renders the same shape:

```json
"language": {
  "c":   { "standard": "c11",   "source": "package" },
  "cxx": { "standard": "c++17", "source": "package" },
  "targets": {
    "core": {
      "c":   { "standard": "c11",   "source": "package" },
      "cxx": { "standard": "c++20", "source": "target" },
      "interface_c":   { "requirement": { "min": "c11",   "max": null }, "source": "compile-standard" },
      "interface_cxx": { "requirement": { "min": "c++17", "max": null }, "source": "target" },
      "gnu_extensions": true
    }
  }
}
```

Sources are `package` / `target` / `workspace`, plus `compile-standard` for an interface value
defaulted from the effective implementation standard.  A language with no declaration anywhere
reports no entry at all - there is no built-in default.  A workspace-inherited value reports
`"source": "workspace"` - for implementation standards and for package-level inherited interface
standards alike.  `interface_*` keys appear only on `library` / `header-only` targets; their
`requirement` is the `{ min, max }` pair (an explicit `"none"` reports as the string `"none"`),
with `max` present-but-null while range support stays reserved.  `gnu_extensions` is the target's
effective value and appears only when `true`.  The block is deterministic and additive to the
stable metadata contract.

`cabin package` / `cabin publish` preserve manifest-declared standard fields in the canonical
per-version metadata, and the index loaders round-trip them opaquely (older index entries without
the field keep loading).  Implementation standards serialize as bare strings; interface fields
serialize as their `{ min, max }` requirement (or `"none"`), keeping the reserved `max` slot on the
wire.  A workspace-inherited value is baked in as a literal, and the archived
`cabin.toml` is normalized: a targeted, format-preserving rewrite replaces the marker-bearing
standard fields with their resolved literals (the dependency-marker rewrite shares the same pass -
see [`package-format.md`](package-format.md)), so packaging an inherited member produces an archive
byte-identical to a literal-declaring twin.  Standalone `cabin package` on a marker-bearing manifest
fails with a clear error directing the user to package from inside the workspace, and registry /
foundation-port manifests that nonetheless carry markers are rejected at load - an external
package's compile standard is never chosen by the consuming workspace.  This is round-trip
preservation only - the registry build honors the extracted manifest, and resolver-side
standard-compatibility filtering remains deferred.

## Post-resolution compatibility errors

`cabin build` / `check` / `run` / `test` always run a check that evaluates the edge-compatibility model of
[`design/standard-compatibility/spec.md`](design/standard-compatibility/spec.md) over the resolved
target graph - after resolution, whether fresh or lockfile-seeded - and reports every violated
dependency edge as an error, one per violated language.  Any violation fails the command with exit
code 1 after all diagnostics have rendered.  Each error renders the provenance chain with manifest
`path:line` references - for example:

```text
`app:app` (c++17, app/cabin.toml:12) -> `foo:bar` requires C++ consumers at `c++20`
or newer via public dependency `baz:baz` (`interface-cxx-standard`, baz/cabin.toml:8)
```

naming the consuming target and its effective standard (with the declaring manifest and line), the
dependency and its effective requirement - marked as inferred for header-only inference, or noting
that consumption was disabled by an interface `"none"` - and the origin declaration, hop by hop
when the requirement arrives over more than one public edge.  The remedies, in order: raise the
consumer's standard; pin an older version of a registry dependency; and, as a last resort, exempt
the edge with `ignore-interface-standard = true` (below).  The exemption is only offered for the
forbidden classes (an interface `"none"`, the strict cross-language default) - exactly the ones the
always-on build-time enforcement deliberately accepts; a minimum violation is independently
rejected by that enforcement, so exempting it here could not unblock the command.  A registry
dependency whose resolved
version came out of an existing `cabin.lock` additionally notes that the lockfile records version
pins only - so the likely cause is a standard declaration that changed in a manifest after the
lockfile was generated - and suggests `cabin update` to re-resolve (see
[`lockfile.md`](lockfile.md)).  The check never influences version selection.  The resolver-level defaults apply (see [Version selection](#version-selection)
below): no implementation-standard fallback for compiled libraries, and `"none"` is unsatisfiable -
so the check can disagree with the build-time enforcement above by design, in both directions.

A single escape hatch exists, deliberately narrow and per-edge:

- **Per-edge override.**  `ignore-interface-standard = true` on a `[dependencies]` /
  `[dev-dependencies]` table entry (see
  [`manifest.md`](manifest.md#ignore-interface-standard)) exempts exactly that edge.  The check
  still evaluates the edge and prints a downgraded note that the edge is unchecked - the override
  suppresses the failure, not the evaluation.  It is deliberately narrow: there is no
  package-wide or global variant.

## Version selection

The resolver consults standard compatibility when it orders candidate versions, controlled by
`[resolver] incompatible-standards` in `.cabin/config.toml` (see
[`config.md`](config.md#resolver)) or the `CABIN_RESOLVER_INCOMPATIBLE_STANDARDS` environment
variable.  The value vocabulary is Cargo's `resolver.incompatible-rust-versions` verbatim:

- `fallback` (the default) prefers versions whose declared interface standards the workspace
  satisfies, deprioritizes versions that declare nothing relevant, and ranks a
  declared-incompatible version last - newest-first within each tier, and never filtered.  When it
  passes a newer version over for a standard reason, `cabin update` / `cabin resolve` name the
  selected version, the newest available, and the requirement that held it back.
- `allow` makes selection a pure function of semver constraints: standards never move a lockfile,
  and incompatibilities surface only through the post-resolution enforcement above.  This is the
  strict / deterministic mode, not a legacy one.

The workspace consumer standard used for the check is the Cargo-style approximation: per language,
the minimum effective implementation standard declared across workspace member targets.
Preference is an ordering heuristic, never a hard constraint - it never introduces a resolution
failure `allow` would not also produce, and the always-on build-time enforcement (and the
post-resolution check above) remain the correctness authority.  The full policy is
recorded in
[`design/standard-compatibility/preference-mode.md`](design/standard-compatibility/preference-mode.md).
Its defaults deliberately differ from the build-time enforcement above, which is unchanged and
still runs after resolution: at resolve time a compiled library with no declared interface field
imposes no constraint (no implementation-standard fallback), while an explicit `"none"` makes the
dependency unsatisfiable from that language instead of imposing nothing.

## Deferred

- Hard in-resolver standard filtering is permanently out of scope (see
  [`design/standard-compatibility/preference-mode.md`](design/standard-compatibility/preference-mode.md)
  section 4); only the soft preference above and the post-resolution checks refuse a resolution.
- `cfg(...)`-conditional or per-profile standards; per-command CLI overrides of the preference
  mode.
- Range interface requirements (populating the reserved `max`), and enforcement of `"none"`
  against consumers that compile the language.
- Duplicate build variants (one library compiled once per consumer standard).

## Not supported

The MSVC `/std:c++latest` and `/std:clatest` spellings are intentionally **not** mapped as
first-class standards, and there is no plan to add them.  They float to the compiler's newest
in-progress draft rather than naming a concrete standard, so they cannot participate in Cabin's
typed value set, per-standard toolchain validation, interface enforcement, or the reproducible
build-configuration fingerprint.  If you need them, inject them through the environment
(`CXXFLAGS` / `CFLAGS`), which merges after the manifest layers and is exempt from the conflict
rule; that route is deliberately unvalidated and unpinned.
