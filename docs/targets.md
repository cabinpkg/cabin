# Targets

A *target* is one buildable unit declared inside a Cabin package.  A package may declare any number
of targets; each target has a `[target.<name>]` table with at least a `type` and usually a `sources`
list.  Header-only libraries are the main exception.

This document covers the target kinds Cabin supports today, when each one is built, and where their
outputs land.

Target kinds describe **artifact role only**.  They do not encode a source-language contract: a
single target may freely mix C and C++ sources.  The compile driver is selected per source file
based on its extension, and the link driver is selected from the direct and transitive
source-language closure.  If you want to keep a target C-only, rely on review, convention, or
external linting.  Cabin accepts C++ sources in any target kind, including `executable`.

## Supported target kinds

| Type          | Output                | Built by `cabin build` (default) | Run by `cabin test` |
| ------------- | --------------------- | -------------------------------- | ------------------- |
| `library`     | static archive (`.a`) | yes                              | no                  |
| `header-only` | none                  | yes, graph/interface only        | no                  |
| `executable`  | linked executable     | yes                              | no                  |
| `test`        | linked executable     | no, only when explicit           | yes                 |
| `example`     | linked executable     | no, only when explicit           | no                  |

`header-only` libraries declare `include-dirs` instead of `sources`; declaring `sources` on a
`header-only` target is rejected at manifest-load time.  The other kinds all carry a `sources` list
of `.c` and/or C++ source files, optionally with [assembly sources](#assembly-sources).

### C/C++ source languages

Cabin treats C/C++ as related but distinct source languages.  Every source file is classified by its
filename extension:

| Extension                               | Language |
| --------------------------------------- | -------- |
| `.c`                                    | C        |
| `.cc`, `.cpp`, `.cxx`, `.c++`, `.C`     | C++      |

The planner then:

- compiles `.c` sources with the **C compiler driver** (`cc` / `clang` / `gcc`) and the target's
  effective C standard (required; see [Language standards](language-standards.md));
- compiles C++ sources with the **C++ compiler driver** (`c++` / `clang++` / `g++`) and the target's
  effective C++ standard (required);
- chooses the **link driver** by walking the target's own objects plus every transitively reachable
  library object: if any object came from a C++ source, or none came from a C or C++ source
  (assembly only), link with the C++ driver; otherwise link with the C driver.  Pure-C executables
  therefore stay off the C++ runtime; mixed targets inherit the C++ runtime as required.

Sources whose extension is not recognized produce an explicit `unrecognized extension` build error
so a misnamed file never silently picks the wrong compiler.

### Assembly sources

A target may also list assembly sources, which Cabin assembles rather than compiles:

| Extension | Syntax                     | Assembler                                         |
| --------- | -------------------------- | ------------------------------------------------- |
| `.S`      | GNU assembly, preprocessed | the GCC or Clang driver that compiles C++         |
| `.asm`    | MASM                       | `ml64.exe` from the directory holding `cl.exe` (x64 MSVC; not `clang-cl`) |

Only the target's defines and include dirs reach the assembler: standards, optimization, and the
escape-hatch flags (`cflags` / `cxxflags`, `CPPFLAGS` / `CFLAGS` / `CXXFLAGS`) are compiler flags.
Each syntax assembles only on its own toolchain, so a portable target lists its assembly in
[platform-conditional sources](#platform-conditional-sources), and a source the active toolchain
cannot assemble is a build error.  `cabin check` skips assembly sources, and `compile_commands.json` lists only C and
C++ translation units.  `ml64` reports no included files, so editing a file a MASM source pulls in
with `INCLUDE` does not re-assemble it; the GNU driver tracks `#include`s like a C compile.

There is no `language = "c"` / `language = "cpp"` switch on a package or target.  Per-target build
behavior is decided by per-source classification (`.c` -> C, `.cc` / `.cpp` / `.cxx` -> C++) plus
the target kind.  The *standard* each language compiles with is declared separately via `c-standard`
/ `cxx-standard` (package or target level); see [Language standards](language-standards.md).

## Manifest syntax

Every target is a `[target.<name>]` table.  The `type` field selects the kind; the rest of the
fields apply to all kinds.

```toml
[target.demo]
type = "library"
sources = ["src/lib.cc"]
include-dirs = ["include"]

[target.demo_test]
type = "test"
sources = ["tests/lib_test.cc"]
deps = ["demo"]

[target.hello_example]
type = "example"
sources = ["examples/hello.cc"]
deps = ["demo"]
```

Common fields:

- `sources`: source files relative to the package root.
- `include-dirs`: public include directories, relative to the package root.  Absolute paths and
  `..` traversal are rejected when the manifest loads: a package must not aim its consumers'
  include search outside itself.  Consumers of this target inherit them through `deps`, and
  their consumers only through public edges; see [Edge visibility](manifest.md#edge-visibility).
  When the providing package is third-party code (an extracted registry package or a foundation
  port), consumers compile with the inherited dirs marked as *system* search paths (`-isystem` /
  MSVC `/external:I`) so warnings inside dependency headers do not fail a strict warning profile;
  see [System include directories](toolchains.md#system-include-directories).
- `defines`: preprocessor defines applied to this target's compile actions only.
- `interface-defines`: preprocessor defines applied to the compiles of this target's consumers,
  with the same reach as `include-dirs` (`library` / `header-only` only).
- `deps`: explicit references to the linked targets:
  - same-package by bare name: `deps = ["lib"]`;
  - cross-package by bare package name: `deps = ["cabin-ports/zlib"]` resolves to the
    `cabin-ports/zlib` package's sole `library` (or `header-only`) target, whatever it is named -
    `cabin-ports/zlib:z` when `z` is the only one;
  - qualified `package:target`: `deps = ["fmt:fmt"]` - the only spelling for a non-library
    dependency target (`deps = ["foo:opt"]`), and the required one when the dependency declares
    several library / header-only targets.

  A bare name resolves to a local target first, then as the dependency shorthand.  A dependency
  with several library / header-only targets makes the bare name ambiguous - a hard error naming
  the qualified candidates - and one with none has nothing a bare name could link; both cases
  require the qualified spelling.

  `[dependencies]` only makes a package available; declaring it does not add it to any target's
  dependency graph.  When a target's compile fails on a missing header, or its link fails, the
  build names the package's declared dependencies that the target does not list in a `help:`
  note.

  Any of these references may be wrapped in the table form `{ name = "fmt", public = true }` to
  declare the edge **public** - meaning this target's public headers include headers of that
  dependency.  String entries are private edges: the dependency's include dirs and interface
  defines reach this target's compiles but not its consumers'.  Linking ignores visibility; see
  [Edge visibility](manifest.md#edge-visibility).
- `required-features`: package features that must all be enabled for this target to be built or
  used; see [Feature-gated targets](features.md#feature-gated-targets).  Entries must name features
  declared in this package's `[features]` table.
- `links`: the native-library identity a `library` target claims (`links = "z"`), enforced unique
  across a resolution graph; see [`links`](manifest.md#links).
- `instruction-sets`: x86 instruction sets one of the target's C or C++ sources may use beyond the
  platform baseline; see [Per-source instruction sets](#per-source-instruction-sets).

Cross-package deps must reach the consumer through a `[dependencies]` edge.  `[dev-dependencies]`
are never linked into ordinary targets; the dev-only kinds (`test`, `example`) may additionally
reference the owning package's `[dev-dependencies]`, which `cabin test` activates for the selected
packages (see [`testing.md`](testing.md)).  An ordinary target referencing a dev dependency - or a
dev-only target referencing one outside `cabin test`'s activation - fails with a diagnostic naming
the `[dev-dependencies]` policy.

### Platform-conditional sources

A target compiles some sources only on particular hosts through `[target.'cfg(...)'.sources]`
tables.  Each key names a target of the same package, and its value lists the sources that target
adds when the predicate matches the host:

```toml
[target.thread]
type = "library"
sources = ["src/future.cc"]

[target.'cfg(family = "windows")'.sources]
thread = ["src/win32/thread.cc"]

[target.'cfg(family = "unix")'.sources]
thread = ["src/pthread/thread.cc"]
```

The build compiles the target's `sources` plus the list of every matching table, in manifest
order.  Predicates may test only the platform keys (`os`, `arch`, `family`, `env`, `abi`, `target`;
see [the `cfg` grammar](target-dependencies.md#supported-cfg-grammar)), because a target's sources
must not depend on its features or the detected compiler.  A table that names an unknown target, or
a `header-only` one, is a manifest error.

Only the build selects by host.  Everything else treats every conditional list as declared:
`cabin package` archives the files, the registry verifier requires them, and the language-standard
checks count them.  A manifest therefore loads, packages, and verifies identically on every host.

### Per-source instruction sets

A source that uses SIMD intrinsics past the platform baseline names the instruction sets in its
target's `instruction-sets` table, keyed by the source's path:

```toml
[target.sum]
type = "library"
sources = ["src/sum.c"]

[target.'cfg(any(arch = "x86", arch = "x86_64"))'.sources]
sum = ["src/sum_sse41.c", "src/sum_avx2.c"]

[target.sum.instruction-sets]
"src/sum_sse41.c" = ["sse4.1"]
"src/sum_avx2.c" = ["avx2"]
```

The names are `sse2`, `sse3`, `ssse3`, `sse4.1`, `sse4.2`, `avx`, and `avx2`, and each key must be
one of the target's C or C++ sources, from `sources` or a conditional list.  The instruction sets
reach that source's compile only, never the target's other sources or its consumers, so code built
with them must run only after a runtime CPU check: here `src/sum.c` would pick a variant by the CPU
it runs on.  In C++, keep such a source out of inline functions and templates that other sources
also use: the linker keeps one copy of each, and it may be the one built with the instruction set.
GCC, Clang, and `clang-cl` compile the source with `-m<name>` (`-msse4.1`); `cl.exe` gets no flag,
since it exposes every intrinsic without one.  Off x86 (an `arch` other than `x86`
and `x86_64`) the table is ignored, like a non-matching `cfg`, so a source that guards its
intrinsics with `#ifdef` builds on every platform.  The names are typed, so registry packages keep
them, unlike raw `cflags`.

## Default-build vs. explicit selection

`cabin build` enumerates every `library`, `header-only`, and `executable` target in the selected
packages.  A target whose [`required-features`](features.md#feature-gated-targets) are not all
enabled is skipped by this enumeration (a note-worthy exception: when *every* default-buildable
target is gated off, the build fails with an error naming the missing features instead of silently
building nothing).  Naming a gated target explicitly - through a `deps` entry, `cabin test --test`,
or `cabin run --bin` - is always a hard error that spells out the missing features.

Header-only targets participate in dependency/interface propagation but emit no compile,
archive, or link action.  Dev-only kinds (`test`, `example`) are excluded from the default
enumeration; they reach the build graph in two ways:

- `cabin test` selects every `test` target in the selected packages, or only the named ones when
  `--test <NAME>` is given, builds the chosen test executables, and runs them;
- any `test` or `example` target may appear in another target's `target.<X>.deps`, in which case it
  is pulled into the build closure as a transitive dependency.

Cabin does not expose a single-target selector flag on `cabin build`; narrow the build scope by
narrowing the package selection with `--package` / `--workspace` / `--exclude`.  On `cabin test`,
`--test <NAME>` selects individual `test` targets within the selected packages (see
[`testing.md`](testing.md)).

This keeps `cabin build` predictable: a package can ship tests and examples without forcing every
consumer's CI to build them.

## Output paths

Cabin lays test / example executables out the same way as ordinary `executable` targets:

```
<build-dir>/<profile>/packages/<pkg>/<target>
```

`<build-dir>` defaults to `build/`; `<profile>` is the resolved profile name (`dev` by default,
`release` for `--release`, or any custom profile declared in `[profile.<name>]`).

Two targets with the same `<target>` name in the same package would collide here; the planner
rejects duplicate target names within a package, so this is a static guarantee.

## Dependency-kind policy summary

| Kind                 | `cabin build` | `cabin test` |
| -------------------- | ------------- | ------------ |
| `[dependencies]`     | included      | included     |
| `[dev-dependencies]`   | declaration-only | included for selected packages |
| ``system = true` deps` | active normal declarations are probed with `pkg-config`; flags merge into build configuration | same, plus selected packages' dev-kind system declarations |

`cabin test` activates the selected packages' `[dev-dependencies]` as real graph edges.  The
activation never propagates: a transitive dep's own dev-deps stay declaration-only.  `cabin build`
continues to ignore all dev-deps so ordinary builds are unaffected.

## Packaging behavior

`cabin package` includes every declared source file in the deterministic source archive, including
`test` and `example` sources.  Consumers of the published package keep the right to rebuild them
locally.

The published canonical metadata records package-level surfaces such as dependencies, features,
profiles, toolchain/build settings, checksum, and source location.  It does not contain a target
list; target declarations remain in the archived `cabin.toml` and are visible to local tooling
through `cabin metadata`.  The two per-target exceptions are the derived
[`standards` table](package-index.md#standard-metadata) and the declared
[`links` claims](package-index.md#links-metadata), which index consumers must read without
downloading the archive.

## Limitations

The test surface is intentionally small:

- no test discovery inside binaries (no GoogleTest / Catch2 / doctest output parsing);
- no XML / JUnit output;
- no `cabin run --example`, and no single-example selector on `cabin build`; `example` targets only
  reach the build graph as a transitive dep of another selected target;
- no automatic `tests/` / `examples/` discovery;
- no parallel test execution.
