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
of `.c` and/or C++ source files.

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
  library object: if any object came from a C++ source, link with the C++ driver; otherwise link
  with the C driver.  Pure-C executables therefore stay off the C++ runtime; mixed targets inherit
  the C++ runtime as required.

Sources whose extension is not recognized produce an explicit `unrecognized extension` build error
so a misnamed file never silently picks the wrong compiler.

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
- `include-dirs`: public include directories.  Consumers of this target inherit them through `deps`.
  When the providing package is third-party code (an extracted registry package or a foundation
  port), consumers compile with the inherited dirs marked as *system* search paths (`-isystem` /
  MSVC `/external:I`) so warnings inside dependency headers do not fail a strict warning profile;
  see [System include directories](toolchains.md#system-include-directories).
- `defines`: preprocessor defines applied to this target's compile actions.
- `deps`: references to other targets:
  - same-package by bare name: `deps = ["lib"]`;
  - cross-package by name (resolves to the dep package's default `library` or `header-only` target):
    `deps = ["fmt"]`;
  - qualified `package:target`: `deps = ["fmt:fmt"]`.

Cross-package deps must reach the consumer through a `[dependencies]` edge.  `[dev-dependencies]`
are never linked into ordinary targets; the dev-only kinds (`test`, `example`) may additionally
reference the owning package's `[dev-dependencies]`, which `cabin test` activates for the selected
packages (see [`testing.md`](testing.md)).  An ordinary target referencing a dev dependency - or a
dev-only target referencing one outside `cabin test`'s activation - fails with a diagnostic naming
the `[dev-dependencies]` policy.

## Default-build vs. explicit selection

`cabin build` enumerates every `library`, `header-only`, and `executable` target in the selected
packages.  Header-only targets participate in dependency/interface propagation but emit no compile,
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
through `cabin metadata`.

## Limitations

The test surface is intentionally small:

- no test discovery inside binaries (no GoogleTest / Catch2 / doctest output parsing);
- no XML / JUnit output;
- no `cabin run --example`, and no single-example selector on `cabin build`; `example` targets only
  reach the build graph as a transitive dep of another selected target;
- no automatic `tests/` / `examples/` discovery;
- no parallel test execution.
