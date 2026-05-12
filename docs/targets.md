# Targets

A *target* is one buildable unit declared inside a Cabin
package. A package may declare any number of targets; each
target has a `[target.<name>]` table with at least a `type` and
usually a `sources` list. Header-only libraries are the main
exception.

This document covers the C, C++, and Rust target kinds Cabin
supports today, when each one is built, and where their outputs
land. Cabin treats C and C++ as related but distinct source
languages: a single `cpp_library` / `cpp_executable` /
`cpp_test` / `cpp_example` target may carry both `.c` and `.cc`
(or `.cpp` / `.cxx` / `.c++` / `.C`) sources, and
the planner picks the language-appropriate compiler driver per
source. The `cpp_*` prefix on the kinds reflects "the C/C++
target family", not "C++ only".

## Supported target kinds

| Type             | Output                | Built by `cabin build` (default) | Run by `cabin test` |
| ---------------- | --------------------- | -------------------------------- | ------------------- |
| `cpp_library`    | static archive (`.a`) | yes                              | no                  |
| `cpp_header_only` | none                 | yes — graph/interface only       | no                  |
| `cpp_executable` | linked executable     | yes                              | no                  |
| `cpp_test`       | linked executable     | no — only when explicit         | yes                 |
| `cpp_example`    | linked executable     | no — only when explicit         | no                  |
| `rust_library`   | Cargo-built staticlib | reachable via target deps        | no                  |

Rust executable targets are not accepted in this release.

### C and C++ source languages

Within any `cpp_*` target, every source file is classified by
its filename extension:

| Extension                               | Language |
| --------------------------------------- | -------- |
| `.c`                                    | C        |
| `.cc`, `.cpp`, `.cxx`, `.c++`, `.C`    | C++      |

The planner then:

- compiles `.c` sources with the **C compiler driver**
  (`cc` / `clang` / `gcc`) and `-std=c11`;
- compiles C++ sources with the **C++ compiler driver**
  (`c++` / `clang++` / `g++`) and `-std=c++17`;
- chooses the **link driver** by walking the target's own
  objects plus every transitively reachable library object: if
  any object came from a C++ source, link with the C++ driver;
  otherwise link with the C driver. Pure-C executables therefore
  stay off the C++ runtime; mixed targets inherit the C++
  runtime as required.

Sources whose extension is not recognised produce an explicit
`unrecognised extension` build error so a misnamed file never
silently picks the wrong compiler.

`[package]` does not carry a language field — declaring one is
rejected as an unknown field. Per-target build behaviour is
decided by per-source classification (`.c` → C, `.cc` / `.cpp` /
`.cxx` → C++) plus the target kind, not by package-level
metadata.

## Manifest syntax

Every target is a `[target.<name>]` table. The `type` field
selects the kind; the rest of the fields apply to all kinds.

```toml
[target.demo]
type = "cpp_library"
sources = ["src/lib.cc"]
include_dirs = ["include"]

[target.demo_test]
type = "cpp_test"
sources = ["tests/lib_test.cc"]
deps = ["demo"]

[target.hello_example]
type = "cpp_example"
sources = ["examples/hello.cc"]
deps = ["demo"]
```

Common fields (apply to C/C++ target kinds):

- `sources` — source files relative to the package root.
- `include_dirs` — public include directories. Consumers of
  this target inherit them through `deps`.
- `defines` — preprocessor defines applied to this target's
  compile actions.
- `deps` — references to other targets:
  - same-package by bare name: `deps = ["lib"]`;
  - cross-package by name (resolves to the dep package's default
    library target): `deps = ["fmt"]`;
  - qualified `package:target`: `deps = ["fmt:fmt"]`.

Cross-package deps must reach the consumer through a `[dependencies]`
edge. `[build-dependencies]` and `[dev-dependencies]` are not
auto-linked into ordinary targets.

## Default-build vs. explicit selection

`cabin build` enumerates `cpp_library`, `cpp_header_only`, and
`cpp_executable` targets in the selected packages. Header-only
targets participate in dependency/interface propagation but emit no
compile, archive, or link action. Dev-only kinds (`cpp_test`,
`cpp_example`) are excluded from the default enumeration; they
reach the build graph in two ways:

- `cabin test` selects every `cpp_test` target in the selected
  packages, builds the chosen test executables, and runs them;
- a `cpp_test` or `cpp_example` may appear in another target's
  `target.<X>.deps`, in which case it is pulled into the build
  closure as a transitive dependency.

Cabin does not currently expose a single-target selector flag on
`cabin build` or `cabin test`. The historic `--target <name>`
selector has been removed because the flag name is reserved for
a future platform / toolchain target. To narrow the build or
test scope today, narrow the package selection with `--package`
/ `--workspace` / `--exclude`. A future explicit-kind selector
(e.g. `--test <name>`, `--example <name>`) may add a narrower
selector under a distinct flag name.

This keeps `cabin build` predictable: a package can ship tests
and examples without forcing every consumer's CI to build them.

Rust libraries are not enumerated by the default selection
either. They are reached via target dependencies from a default-
included C++ target, or via explicit `--target`.

## Output paths

Cabin lays test / example executables out the same way as
ordinary `cpp_executable` targets:

```
<build-dir>/<profile>/packages/<pkg>/<target>
```

`<build-dir>` defaults to `build/`; `<profile>` is the resolved
profile name (`dev` by default, `release` for `--release`, or
any custom profile declared in `[profile.<name>]`).

Two targets with the same `<target>` name in the same package
would collide here; the planner rejects duplicate target names
within a package, so this is a static guarantee.

## Dependency-kind policy summary

| Kind                 | `cabin build` | `cabin test` |
| -------------------- | ------------- | ------------ |
| `[dependencies]`     | included      | included     |
| `[build-dependencies]` | included     | included     |
| `[dev-dependencies]`   | declaration-only | included for selected packages |
| ``system = true` deps` | active normal / build declarations are probed with `pkg-config`; flags merge into build configuration | same, plus selected packages' dev-kind system declarations |

`cabin test` activates the selected packages' `[dev-dependencies]`
as real graph edges. The activation never propagates: a transitive
dep's own dev-deps stay declaration-only. `cabin build` continues
to ignore all dev-deps so ordinary builds are unaffected.

## Packaging behaviour

`cabin package` includes every declared source file in the
deterministic source archive — including `cpp_test` and
`cpp_example` sources. Consumers of the published package keep
the right to rebuild them locally.

The published canonical metadata records package-level surfaces
such as dependencies, features, options, variants, profiles,
toolchain/build settings, checksum, and source location. It does
not contain a target list; target declarations remain in the
archived `cabin.toml` and are visible to local tooling through
`cabin metadata`.

## Limitations

This release intentionally keeps the test surface small:

- no test discovery inside binaries (no GoogleTest / Catch2 /
  doctest output parsing);
- no XML / JUnit output;
- no `cabin run --example`, and no single-example selector on
  `cabin build` — `cpp_example` targets only reach the build
  graph as a transitive dep of another selected target until a
  future explicit-kind selector is added;
- no benchmark target kind or harness;
- no automatic `tests/` / `examples/` discovery;
- no parallel test execution;
- no coverage / sanitizer integration.

Future work may relax these. The current model is designed so
none of those features require breaking changes to the target
kind enum or the `cabin test` CLI.
