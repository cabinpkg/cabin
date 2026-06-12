# Language standards

Cabin treats C and C++ language standards as **typed build
metadata**: you declare them in `cabin.toml`, and Cabin lowers them
to the dialect-correct compiler flag, validates that the selected
toolchain supports them before the build, enforces library
interface requirements on consumers, folds them into the
build-configuration fingerprint, and reports them through
`cabin metadata` / `cabin explain build-config`.

This document is the canonical specification. The behavior
described here is what the manifest parser (`cabin-manifest`), the
typed model and resolution (`cabin-core::language_standard`), the
build planner and pre-build validation (`cabin-build`), the
dialect lowering (`cabin-driver`), the CLI (`cabin`), the
canonical package metadata (`cabin-package`), and the local /
sparse-HTTP index loaders (`cabin-index`, `cabin-index-http`) all
agree on.

## Manifest fields

Four kebab-case fields, accepted at both `[package]` and
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
```

- `c-standard` / `cxx-standard` — the **implementation standard**:
  how this package's (or target's) sources are compiled. `.c`
  sources use the effective C standard; `.cc` / `.cpp` / `.cxx` /
  `.c++` / `.C` sources use the effective C++ standard. A
  mixed-language target compiles each source with its language's
  standard.
- `interface-c-standard` / `interface-cxx-standard` — the
  **interface standard**: what consumers of the target's public
  headers need. Only meaningful on `library` / `header-only`
  targets; a target-level interface field on an `executable` /
  `test` / `example` target is a manifest error. Package-level
  interface fields are defaults consumed only by library-like
  targets (they are allowed, and inert, in packages without any).

Mental model: `c-standard` / `cxx-standard` set how the target is
compiled **and** double as its interface standard unless
`interface-*` overrides them. Declare `interface-*` only when the
public interface requires a different standard than the
implementation — for example a library compiled as C++20 whose
public headers only use C++17 (headers and implementation sources
are separate translation units, so the interface standard may also
*exceed* the implementation standard).

## Accepted values

Typed value sets; anything else is a manifest parse error listing
the valid spellings. There are no aliases and no GNU dialects
(`gnu11`, `gnu++20` — see the escape hatch below).

- C: `c89`, `c99`, `c11`, `c17`, `c23`
- C++: `c++98`, `c++03`, `c++11`, `c++14`, `c++17`, `c++20`,
  `c++23`

`c++26` is deferred until its toolchain-support thresholds are
audited.

## Precedence

Per language, per target:

- Effective implementation standard:
  `[target.<name>].c-standard` ▶ `[package].c-standard` ▶ built-in
  default (same chain for `cxx-standard`).
- Effective interface standard (library-like targets):
  `[target.<name>].interface-c-standard` ▶
  `[package].interface-c-standard` ▶ the target's effective
  implementation standard (same chain for C++).

The built-in defaults are **`c11`** and **`c++17`**. A project
that declares nothing builds with the same compile commands it
always has.

Registry and foundation-port packages keep their own declared
standards: unlike the raw `cflags` / `cxxflags` escape hatches
(dropped for registry packages during flag resolution), a typed
standard is a bounded correctness requirement, so a published
`c++20` library still compiles as C++20 inside your build.

## Flag lowering

The standard never appears in `[profile]` flags; the dialect layer
spells it:

| Dialect | Spelling |
| --- | --- |
| GCC / Clang | `-std=<value>` (e.g. `-std=c++20`) |
| MSVC (`cl` / `clang-cl`) | `/std:<value>` — only `c11`, `c17`, `c++14`, `c++17`, `c++20` have stable flags |

Standards without a stable MSVC flag (C89/C99/C23,
C++98/03/11/23) are rejected before the build on the MSVC
dialect. `compile_commands.json` records the same per-file
standard the build uses, so clangd and `cabin tidy` see exactly
how each translation unit compiles. Changing a standard changes
the lowered command, so Ninja rebuilds exactly the affected
translation units.

## Toolchain validation

Before any Ninja file is written, Cabin collects the set of
standards the selected packages' compiles will request and checks
each against the detected compiler — the whole set, not the
maximum, because MSVC support is non-monotonic (`/std:c++20`
exists, `/std:c++11` does not). The thresholds gate acceptance of
the exact flag spelling:

| C standard | GCC | Clang | Apple Clang | `clang-cl` | MSVC `cl` |
| --- | --- | --- | --- | --- | --- |
| `c89` / `c99` | always | always | always | — | — |
| `c11` | always | always | always | 13 | 19.28 |
| `c17` | 8 | 6 | 10 | 13 | 19.28 |
| `c23` | 14 | 18 | 17 | — | — |

| C++ standard | GCC | Clang | Apple Clang | `clang-cl` | MSVC `cl` |
| --- | --- | --- | --- | --- | --- |
| `c++98` / `c++03` / `c++11` | always | always | always | — | — |
| `c++14` | 5 | always | always | always | 19.10 |
| `c++17` | 5 | always | always | always | 19.11 |
| `c++20` | 10 | 11 | 13 | 13 | 19.29 |
| `c++23` | 11 | 17 | 16 | — | — |

“always” means any recognized version; “—” means no stable flag
exists and the request is rejected on that compiler with an
actionable error. A compiler whose version banner cannot be
parsed fails open (`assumed-default`), matching the rest of
capability detection. Dev-only targets (`test` / `example`)
contribute their standards only when the command activates them
(`cabin test`), mirroring dev-dependency activation; a planner-
level guard still rejects MSVC-incompatible standards on any
compile the plan actually emits.

## Interface enforcement

A library-like target imposes its effective interface standard on
every target that transitively depends on it, per language,
checked before the build:

- The consumer's effective implementation standard must be **at
  least** the dependency's interface standard (chronological
  comparison). This is a pragmatic compatibility policy, not a
  proof — it assumes headers valid under standard *N* stay valid
  under newer modes; Cabin does not verify header validity per
  standard.
- A language is relevant to a dependency only if the dependency
  has sources of that language, declares a target-level field for
  it, or is `header-only` while its package declares a
  package-level *interface* standard for it. A package-level
  *implementation* default alone never creates relevance — a
  pure-C library imposes no C++ requirement on C++ consumers.
- The check applies only to languages the consumer actually
  compiles.

Because an omitted interface standard defaults to the effective
implementation standard, an undeclared `c++20` library implicitly
requires C++20 from consumers; declare
`interface-cxx-standard = "c++17"` to relax that when the public
headers permit.

## Escape hatch and the conflict rule

`cflags` / `cxxflags` still accept raw standard flags, and they
come later in the argv, so for a package that declares **no**
first-class standard they keep winning over the built-in default —
this is the supported route to GNU dialects (`-std=gnu++20`) and
`/std:c++latest` today.

Declaring both is ambiguous and rejected: if a package declares a
first-class implementation standard for a language (package or
target level) and its manifest-derived `cflags` (C) / `cxxflags`
(C++) also contain a `-std=` / `--std=` / `/std:` token, the build
fails with `cabin::language::standard_flag_conflict`. Environment
`CPPFLAGS` / `CFLAGS` / `CXXFLAGS` and `pkg-config` output are
exempt — the check runs before those layers merge.

## Fingerprint

The effective standards (package level plus every target,
implementation and interface) are folded into
`BuildConfiguration::fingerprint` under a labeled
`language-standards` section — values only; provenance labels do
not move the fingerprint.

## Metadata

`cabin metadata` reports the effective standards with provenance
inside each declaring package's `configuration` block, and
`cabin explain build-config <package>` renders the same shape:

```json
"language": {
  "c":   { "standard": "c11",   "source": "builtin-default" },
  "cxx": { "standard": "c++17", "source": "package" },
  "targets": {
    "core": {
      "c":   { "standard": "c11",   "source": "builtin-default" },
      "cxx": { "standard": "c++20", "source": "target" },
      "interface_c":   { "standard": "c11",   "source": "compile-standard" },
      "interface_cxx": { "standard": "c++17", "source": "target" }
    }
  }
}
```

Sources are `builtin-default` / `package` / `target`, plus
`compile-standard` for an interface value defaulted from the
effective implementation standard. `interface_*` keys appear only
on `library` / `header-only` targets. The block is deterministic
and additive to the stable metadata contract.

`cabin package` / `cabin publish` preserve manifest-declared
standard fields in the canonical per-version metadata, and the
index loaders round-trip them opaquely (older index entries
without the field keep loading). This is round-trip preservation
only — the registry build honors the extracted manifest, and
resolver-side standard-compatibility filtering remains deferred.

## Deferred

- Workspace-level standard defaults (a `[workspace]` tier).
- Resolver standard-compatibility filtering.
- GNU dialects (`gnu11`, `gnu++20`) and `/std:c++latest` /
  `/std:clatest` mapping.
- `cfg(...)`-conditional or per-profile standards; CLI / env /
  config overrides.
- `c++26` (pending threshold audit).
- Duplicate build variants (one library compiled once per consumer
  standard).
