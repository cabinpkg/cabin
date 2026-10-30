# Capability roadmap: ledger

This is the working ledger for the capability roadmap: ten requested areas taken from design to
practical completion through stages 01-14, against the [shared
contracts](design/capability-roadmap/contracts.md).  It records, per area, the current source
evidence, a workflow users cannot complete today, the necessary changes, dependencies, acceptance
criteria, and status; then the documentation discrepancies found on the way and the progress
record.  Reconcile it against the repository and open pull requests whenever the work resumes, and
update it in the same change as the work it describes.

Research baseline: `main` at `62f46d84523b04bc0e4d4e8d7506753659aed239` (2026-10-29).  Evidence
cites a file and the symbol in it, since line numbers drift.  Evidence comes from reading the source;
where nothing ran, the entry says so.

## Status vocabulary

`not started` -> `in progress` -> `implemented` (merged with docs and tests) -> `validated` (the
stage's real-project demonstration ran on the platforms recorded in the entry).  `blocked` names
its blocker.  A scope document changing is never `implemented`.  `adopted` is the finished state of
design-only work (stage 00): its documents are on `main`, and it implements nothing.

## Stages

| Stage | Outcome | Areas | Main owners | Needs | Status |
| --- | --- | --- | --- | --- | --- |
| 00 | Shared contracts, this ledger, scope alignment | all | `docs/` | - | adopted |
| 01 | Registry optional dependencies and transitive features in one active graph | 1 | `cabin-feature`, `cabin-resolver`, `cabin-index`, `cabin-index-http`, `cabin-workspace`, `cabin` | 00 | implemented |
| 02 | Interface definitions, visibility-driven propagation, final-link closure, per-source options | 2 | `cabin-core`, `cabin-manifest`, `cabin-build`, `cabin-driver`, `cabin-package` | 00; integrates 01 | in progress |
| 03 | Per-revision resolution metadata; old pins keep their meaning | 1 (metadata) | `cabin-index`, `cabin-index-http`, `cabin-resolver`, `cabin-lockfile`, `cabin-registry-file`, `cabin-publish`, `cabin-registry-verify`, `registry/` | 00, 01; 02's metadata | not started |
| 04 | Build and target contexts, tool dependencies, `--target <triple>` | 5, 4 (tool foundation), 9 (platform), 1 (contexts) | `cabin-core`, `cabin-manifest`, `cabin-workspace`, `cabin-feature`, `cabin-resolver`, `cabin-lockfile`, `cabin-build`, `cabin-toolchain`, `cabin` | 00-03 | not started |
| 05 | Compile/link probes, sysroots and SDKs, MinGW on evidence | 9, 5 | `cabin-toolchain`, `cabin-core::compiler`, `cabin-driver` | 02, 04 | not started |
| 06 | Root-selected system providers | 3 | `cabin-system-deps`, `cabin-workspace`, `cabin-manifest`, `cabin-config`, `cabin-build` | 01-05 | not started |
| 07 | Generated actions and build-machine tools | 4 | `cabin-build`, `cabin-driver`, `cabin-ninja`, `cabin-manifest`, `cabin-core` | 02, 04-06; 03's metadata | not started |
| 08 | Static, shared, and loadable artifacts; PIC, DLLs, runtime lookup | 6 | `cabin-core`, `cabin-build`, `cabin-driver`, `cabin-ninja`, `cabin` | 02, 04-07 | not started |
| 09 | Install, CMake Config and pkg-config export, relocation | 7 | owner chosen in 09; `cabin-build` | 02, 06-08 | not started |
| 10 | C++20 modules | 8 | `cabin-core`, `cabin-build`, `cabin-driver`, `cabin-ninja`, `cabin-toolchain` | 02, 04, 05, 07-09 | not started |
| 11 | Everyday CLI: test argv, test concurrency, JUnit, examples, target selection | 10 | `cabin`, `cabin-test` | 00 for independent parts; 04, 08, 10 | not started |
| 12 | OpenSSL and OpenCL packages; Boost TLS and Compute | validation of 1-7, 9-10 | `ports/`, [Boost ledger](design/boost-ports.md) | 01-09; 11 where needed | not started |
| 13 | Python and MPI packages; remaining Boost gaps | validation of 1-10 | `ports/`, [Boost ledger](design/boost-ports.md) | 01-12 | not started |
| 14 | Integrated acceptance across the ten areas | 1-10 | all | 00-13 | not started |

A stage is not one pull request: each stage picks cohesive PR boundaries, and dependent work starts
only after what it needs is merged on `main`.  Independent CLI work from stage 11 may land early.

## Areas

### 1. Dependency resolution

Stages 01 and 03; contexts in 04; validated in 12-14.

**Today.**

- PubGrub's private packages are the root, each package name, and each (name, feature) pair
  (`cabin-resolver/src/provider.rs`, `Node`), so one version per package name and one feature set
  per package; lockfile modes are prefer-locked, locked, update-all, and update-package
  (`cabin-resolver/src/input.rs`).
- Since stage 01, features resolve with versions: a (name, feature) pair's candidates are the
  versions declaring the feature, an enabled feature adds its entries, and registry optional
  dependencies activate exactly when a feature enables them (`provider.rs`, `constraints`).
  `cabin-feature` resolves the local packages and hands the requests on registry edges to the
  resolver (`FeatureResolution::versioned_requests`); local packages, patch forks included, are
  single-version candidates (`cabin/src/cli/seed.rs`, `local_packages`).  Index metadata loads
  lazily over sparse HTTP (`ResolveError::MetadataNeeded`); the eager prefetch still walks
  non-optional edges only (`IndexPackageDependency::is_active_for`).
- Feature resolution keeps one feature set per package for the whole graph
  (`cabin-feature`, `FeatureResolution`) and propagates dependency-edge feature requests into the
  fetched graph's compiles and target gates (`cabin-ports/sqlite3`'s `single-threaded` reaches
  compilation, `tests/cli/registry_ports.rs`); the building commands refuse a final feature pass
  that enables a dependency resolution did not select.  The configuration report
  (`cabin/src/cli/build_prep.rs`, `resolve_build_configurations`) reads the local feature
  resolution.
- `cabin.lock` stores `name`, `version`, `checksum`, and resolved direct dependencies per registry
  package (`cabin-lockfile/src/model.rs`, `LockedPackage`); the checksum pins the packaging revision;
  path packages are not locked.
- Index revisions carry only `checksum`, `published-at`, and `source`
  (`cabin-index/src/model.rs`, `RevisionMetadata`); dependencies, features, standards, and `links`
  are version-level, which is why a respin may not change them.

**Blocked workflow.**  Before stage 01, a consumer enabling a published package's feature that
pulls in an optional registry dependency (`feature = ["dep:cabin-ports/zlib"]`) got nothing:
resolution skipped the dependency.  Stage 01 unblocked that, so Boost integrations can be offered
as opt-in features; `cabin-ports/boost-iostreams` still leaves out its bzip2/lzma/zstd filters
until those libraries have ports.  A port that needs a dependency added at the same upstream
version still cannot be corrected (stage 03).

The port publisher adds a constraint of its own: `xtask-port-publish` refuses `cfg(...)` dependency
tables and orders publication by each port's own default feature closure, not by feature requests
on its dependency edges (`xtask-port-publish/src/plan.rs`;
[foundation ports](foundation-ports.md#anatomy-of-a-foundation-port)).  The first port that needs
either lifts it, in stage 01 or in the port stage that needs it (12 or 13).

**Necessary changes.**  Stage 01: feature closure that agrees with candidate selection and
backtracking; metadata acquisition separated from archive fetching; one active graph for
`resolve`, `update`, `fetch`, `build`, `test`, `metadata`, `tree`, `explain`, and `vendor`; patches
and source replacement on the same path.  Stage 03: resolution metadata per revision, revision
selection for every lockfile mode, client/server schema negotiation.  Stage 04: per-context
closures.

**Acceptance.**  Stage 01's scenarios (registry A->B->C feature chain, default-features diamond,
older candidate chosen for a feature, optional-edge backtracking, non-matching `cfg` edge,
registry-reached path patch with an optional versioned dependency, unknown feature, locked replay,
offline rebuild) and stage 03's decisive regression (r1 without B, r2 with B: an r1 lock keeps r1's
graph and bytes through file, HTTP, and vendor paths; `cabin update` moves to r2's).

**Representative projects.**  Boost TLS features (stage 12); `cabin-ports/sqlite3`'s
`single-threaded` feature as the regression guard for edge feature requests that already work; `cabin-ports/boost-iostreams` compression filters once their libraries
are ported.

### 2. Usage requirements

Stage 02; consumed by 06, 08, 09, 10; validated in 12-14.

**Today.**

- `public` per `deps` entry (`cabin-core/src/model.rs`, `TargetDep`) decides which compiles see a
  dependency's consumer interface: its `include-dirs` and `interface-defines` reach the depending
  target's compiles, and that target's consumers only through public edges
  (`cabin-build/src/planner/lowering.rs`, `collect_interface`).  The post-resolution standard check
  (`cabin-workspace/src/standards.rs`) and the build-time interface-standard check use the same
  visibility.  Registry packages' directories become system includes.
- `defines` and package flag layers apply to the declaring package's own compiles, and `ldflags`
  to its own link steps, only (`cabin-build/src/planner/mod.rs`); target `interface-defines` reach
  consumers only, and a target's own `defines` override an incoming one.  Two visible dependencies
  defining one macro differently fail the build with both paths, recorded per compile so
  `cabin check` prunes a dependency's conflict.  `defines` on a `header-only` target is a manifest
  error.
- The final link takes the archives of the whole transitive closure, private edges included
  (`collect_link_libs`), plus package-level `link-libs` (`collect_link_lib_names`).
- Registry packages' raw `cflags` / `cxxflags` / `ldflags` are dropped
  (`cabin-core/src/build_flags.rs`, `apply_package_layer`).
- Implemented and not missing: C and C++ as separate languages with separate standards; target
  `public` edges; interface standard ranges; `cfg`-conditional sources and flag layers;
  compiler-conditioned flag layers (`cc`, `cxx`, `cc_version`, `cxx_version`); assembly sources.
- Package flag layers' `include-dirs` reach only the declaring package's own compiles: the
  producer-only include form (contracts section 2); there is no per-target one.

**Blocked workflow.**  A Boost.Stacktrace consumer must define `BOOST_STACKTRACE_LINK` itself
(`ports/boost-stacktrace/1.92.0/cabin.toml`), Boost.Contract ships header-only because consumers
cannot receive `BOOST_CONTRACT_STATIC_LINK`, and Boost.Atomic and Boost.Log drop their SSE/AVX
sources because no per-source ISA option exists.

**Necessary changes.**  Interface definitions; a producer-only include form; propagation driven by
edge visibility; the final-link closure kept separate from compile visibility with order
preserved; per-source options; conflicting-definition diagnostics; the same interface carried
through package metadata, planning, lowering, and `compile_commands.json`.  Migration of the
private-edge include leak (contracts section 2).

**Acceptance.**  C and C++, header-only and static libraries, public/private diamonds, frameworks,
static private link requirements, definition conflicts; a Stacktrace consumer that links without
defining any configuration macro.

**Representative projects.**  `boost-stacktrace`, `boost-contract`, `boost-atomic`, `boost-log`,
`libpng` -> `zlib`, `spdlog` -> `fmt`.

### 3. System providers

Stage 06; validated in 12-14.

**Today.**

- `{ version = "...", system = true }` names a pkg-config module (`cabin-manifest/src/parse/dependency.rs`);
  pkg-config is the only probe (`cabin-system-deps`).
- Only primary packages are probed (`cabin/src/cli/system_deps.rs`,
  `collect_active_system_deps`); results merge into the declaring package's own flags, and `--libs`
  lands in that package's `ldflags`, which reaches only its own executable links.
- Probing is refused on MSVC (`ensure_dialect_supports_system_deps`).
- `[patch]` accepts only a path source (`cabin-core/src/patch.rs`, `PatchSource`); nothing lets a
  consumer swap a Cabin package for an installed copy.
- Ports may not declare `system = true` (`xtask-port-publish`, enforced and documented in
  [foundation ports](foundation-ports.md#anatomy-of-a-foundation-port)); no port has a
  system/vendored feature.

**Blocked workflow.**  An application that depends on `cabin-ports/zlib` cannot build against the
distribution's zlib without editing every manifest on the path.  Boost.MySQL, Boost.Redis,
Boost.Compute, Boost.Python, and Boost.MPI are blocked because their SDKs can be neither ports nor
declared system dependencies ([Boost ledger](design/boost-ports.md#cabin-gaps-and-blockers)).

**Necessary changes.**  Root-owned provider policy and its precedence against `[patch]`, source
replacement, and config; per-domain provider consistency; interface modeling (version, targets,
components, usage requirements); transitive application; static/shared pkg-config semantics;
ABI-appropriate MSVC discovery; a decided migration for `system = true`.

**Acceptance.**  One unchanged consumer switched between source and system providers by root
policy alone; transitive diamond; absent and mismatched versions; multiple components; a cross
sysroot; an MSVC-compatible Windows library; no silent fallback.

**Representative projects.**  `zlib`, then OpenSSL (stage 12), Python and MPI (stage 13).

### 4. Code generation

Stage 07; build-tool foundation in 04; validated in 08, 10, 12-14.

**Today.**

- The action set is closed: compile, syntax check, assemble, archive, link
  (`cabin-driver/src/action.rs`, `BuildAction`; `cabin-driver/src/lower.rs`).  A compile edge's
  only explicit input is its source, and the Ninja writer emits no order-only or implicit inputs
  (`cabin-ninja/src/writer.rs`).
- `build.ninja` is rewritten on every build; incremental correctness rests on that, Ninja's
  command-line tracking, and compiler depfiles.  `BuildConfiguration::fingerprint`
  (`cabin-core/src/config.rs`) is computed but not used by the planner.
- Dependency kinds are only `normal` and `dev` (`cabin-core/src/model.rs`, `DependencyKind`);
  `[build-dependencies]` is an unknown key.
- Ports avoid generation: `libpng` copies a prebuilt `pnglibconf.h`, `catch2`, `sqlite3`, and
  `miniz` use amalgamations, `boost-log` drops the Windows event log (needs `mc.exe`).

**Blocked workflow.**  A project that uses Protocol Buffers cannot run `protoc` from a dependency
and compile its output; no configure-style header can be generated; flex/bison, Qt `moc`, and
`mc.exe` workflows are impossible.

**Necessary changes.**  Tool dependencies and build-context units (stage 04); generated-file
actions with declared inputs, outputs, environment, and ordering; configuration templates and
compile checks (with stage 05 probes); root execution policy (contracts section 4); generated
public headers in the usage interface.

**Acceptance.**  A small C generator, then real `protoc` -> generated C++ -> runtime -> run, with
tool/runtime version agreement; multiple outputs and discovered inputs; Windows `mc.exe`; clean,
parallel, no-op, input-change, tool-change, and failure-recovery builds.

**Representative projects.**  Protocol Buffers, a flex/bison fixture, `boost-log` event log, Qt
`moc` (with stage 08).

### 5. Cross-compilation

Stages 04 and 05; validated in 06-10, 12-14.

**Today.**

- `cfg(...)` predicates evaluate against `TargetPlatform::current()`
  (`cabin-core/src/condition.rs`), built from `std::env::consts`: the platform Cabin itself was
  compiled for, not the compiler's target.  `env` follows the OS, `abi` is always `unknown`, and
  `target` is `<arch>-<family>-<os>`, not a real triple.
- `--target` has no definition and fails as an unknown argument; tests pin that
  (`cabin/tests/cli.rs`, `cabin/tests/cli/test_targets.rs`).
- Clang's `Target:` line is parsed into `CompilerIdentity.target` and only reported in metadata.
- MSVC discovery uses Cabin's own architecture (`cabin-toolchain/src/msvc.rs`); MASM is always
  `ml64.exe`, x64 only (`cabin-driver/src/dialect.rs`, `Dialect::assembler`).

**Blocked workflow.**  Building an aarch64 Linux binary from an x86-64 machine, or a Windows ARM64
binary from x64, with the build tools running natively.

**Necessary changes.**  `--target <triple>`; per-context platform and toolchain; diagnostics for a
requested target that contradicts the compiler's actual target or the sysroot; tool dependencies on
the build machine; per-unit output separation; never running target binaries implicitly; host
pkg-config never contaminating a target sysroot.

**Acceptance.**  A build-machine C generator whose output compiles into a program for a different
target; one source used in both contexts with different features; native behavior unchanged;
verified cross combinations recorded.

**Representative projects.**  A generator fixture, then Protocol Buffers (stage 07) and a cross
Abseil/protobuf build (stage 14).

### 6. Library variants

Stage 08; validated in 09, 12-14.

**Today.**

- `TargetKind` is `library` (static archive), `header-only`, `executable`, `test`, `example`
  (`cabin-core/src/model.rs`); static library names follow the dialect, `lib<name>.a` or
  `<name>.lib` (`cabin-driver/src/dialect.rs`).
- Profiles select optimization, debug information, and assertions (`BuiltinProfile::defaults`,
  lowered by the driver) and separate output directories; nothing selects `-fPIC`, `/MD` or `/MT`,
  or a library variant, and there is no shared or module kind.
- Features are additive and cannot change a target's kind ([features](features.md)).

**Blocked workflow.**  Producing a shared library or a Python extension module; linking a C++
shared library that embeds a private static dependency; consuming a DLL from a Windows executable.

**Necessary changes.**  Static/shared selection with a compatible default for `library`; typed
ELF, Mach-O, and Windows artifact behavior (SONAME, install names, rpaths, import libraries,
exports); PIC; runtime and CRT settings; loadable-module naming; runtime discovery for `cabin run`
and `cabin test`.

**Acceptance.**  Stage 08's demonstration: a C shared library used from C and C++, a C++ shared
library with a private static dependency, static/shared switching, a Windows DLL consumer, ELF and
Mach-O runtime lookup, a loaded module, a small QtCore program with `moc`.

**Representative projects.**  `zlib` shared and static, QtCore, Boost.Python extension (stage 13).

### 7. Installation and interoperability

Stage 09; validated in 10 and 14.

**Today.**  No `install` command (`cabin/src/cli/mod.rs`, `Command`); nothing writes `.pc` or CMake
package files; `compile_commands.json` is written beside `build.ninja`; `cabin metadata` reports no
artifact paths; the build-directory layout is described but not declared stable.

**Blocked workflow.**  A CMake project cannot `find_package` a library built by Cabin, and a
pkg-config consumer has no `.pc` file.

**Necessary changes.**  Install selection, prefixes, and staging; CMake Config/Targets/version files
with IMPORTED targets; pkg-config files with `Requires.private` / `Libs.private`; build-tree versus
install-tree exports; relocation; safe handling of destination escapes and collisions; diagnostics
for requirements an export cannot represent.

**Acceptance.**  Install C and C++ libraries, then build and run a separate CMake consumer through
`find_package` without hand-written flags; move the prefix and repeat with the source tree gone;
pkg-config consumers for static and shared configurations.

**Representative projects.**  `zlib`, `fmt`, a generated-header library from stage 07.

### 8. C++ modules

Stage 10; validated in 14.

**Today.**  No module support: source classification accepts `.c`, `.cc`, `.cpp`, `.cxx`, `.c++`,
`.C`, `.S`, and `.asm` (`cabin-core/src/source_language.rs`); no scanner, dyndep, or BMI handling;
`build.ninja` already requires Ninja 1.10, which has dyndep.  The `fmt` port leaves out its module
unit.  The standard-compatibility spec explicitly does not model BMI compatibility.  Issue #1505
asks for module support.

**Blocked workflow.**  A package exporting a named module that another package imports.

**Necessary changes.**  Module source classification; scanning with the compilers' own scanners
(P1689 where supported) under the same configuration as compilation; dyndep ordering; BMIs as
configuration-sensitive products; module visibility through the usage contract; `import std` and
header units as separate capabilities; scaffolding and a support matrix.

**Acceptance.**  Real modular projects with cross-package imports, partitions, generated
interfaces, mixed translation units, incremental import changes, and installed consumption, on the
compiler versions recorded as verified.

**Representative projects.**  The `fmt` module unit, a scaffolded module package.

### 9. Toolchains

Stages 05 and 04; used by 06-14.

**Today.**

- Implemented and not missing: GCC, Clang, Apple Clang, MSVC, and clang-cl detection
  (`cabin-core/src/compiler/identity.rs`); MSVC environment discovery
  (`cabin-toolchain/src/msvc.rs`); MASM and GNU assembly; per-slot precedence CLI > env > config >
  `cfg` table > manifest > default (`cabin-toolchain/src/resolve.rs`); compiler wrappers;
  compiler-conditioned flag layers; a C or C++ link driver chosen from the linked objects
  (`link_driver_language`).
- Detection runs `--version` only.  The fingerprint hashes tool specs, not resolved paths,
  versions, or targets.  MinGW is best-effort.  No sysroot or SDK model.

**Blocked workflow.**  Generating a configuration header from a compile check; building against an
explicit sysroot; a supported MinGW toolchain.

**Necessary changes.**  Typed compile and link probes whose results record their inputs; sysroot
and SDK selection with explicit choices over discovery; configured choices distinguished from
detected facts in diagnostics and metadata; MinGW promoted only with CI evidence.

**Acceptance.**  C-only, C++, and mixed targets; compiler mismatch; a sysroot that compiles but
cannot link; SDK change and probe invalidation; MinGW/MSVC ABI mismatch; paths with spaces.

**Representative projects.**  The stage 04 cross fixture; OpenSSL's configuration (stage 12).

### 10. Developer-facing CLI workflows

Stage 11; integrated with 04, 08, 10; validated in 14.

**Today.**

- Visible commands: `init`, `new`, `add`, `remove`, `build`, `check`, `clean`, `run`, `test`,
  `update`, `publish`, `fmt`, `tidy`, `version`; hidden: `metadata`, `resolve`, `fetch`, `vendor`,
  `tree`, `explain`, `package`, `login`, `logout`, `yank`, `compgen`, `mangen`
  (`cabin/src/cli/mod.rs`).
- `cabin run --bin` and `cabin test --test` select targets; nothing selects an example or a single
  build target; `cabin test` runs executables sequentially with no forwarded arguments
  (`cabin-test`, `run_tests`; `cabin/src/cli/test.rs` passes no job count).

**Blocked workflow.**  `cabin test -- --gtest_filter=Foo.*`; running one example; building one
target; a JUnit report for CI.

**Necessary changes.**  Exact argv forwarding after `--`; test-process concurrency separate from
build jobs; truthful JUnit; example and target selection kept distinct from `--target`; dev
dependencies activated only for the selected workflow; shared-runtime and cross-runner integration;
help, completions, man pages, and docs.

**Acceptance.**  Stage 11's validation list (argv edge cases, parallel success and failure, large and
invalid-UTF-8 output, timeouts and cleanup, valid XML, unknown/gated/ambiguous targets, dev
dependency selection, examples, cross runners).

**Representative projects.**  `examples/unit-test-gtest`, `examples/library-with-tests`.

## Scope documents aligned in stage 00

Stage 00 moved the requested work out of the deferred scope without marking anything implemented:
`docs/architecture.md` ("Scope and limitations" and the invariants note), the root `AGENTS.md`
deferred-feature rule, the deferred lists in [toolchains](toolchains.md#deferred--out-of-scope) and
[profiles](profiles.md#limitations), the port policy in [foundation ports](foundation-ports.md) and
`ports/README.md`, and the Boost ledger's gap table.

Statements that stay true until a stage changes the behavior, and that stage must update:

- `--target` rejection tests in `cabin/tests/cli.rs` and `cabin/tests/cli/test_targets.rs`
  (stage 04).
- The `docs/architecture.md` C++ semantic invariant on link interfaces (stage 02; the edge
  visibility and include-directory statements were updated in stage 02).
- [Target dependencies](target-dependencies.md#evaluation-context) on host-only evaluation and the
  lockfile (stage 04).
- [Dependency kinds](dependency-kinds.md) listing two kinds (stage 04).
- [Remote registry](remote-registry.md) and [foundation ports](foundation-ports.md#packaging-revisions)
  on revision-invariant resolution metadata (stage 03).
- [Cargo-inspired interface](cargo-inspired-interface.md) on examples and target selection
  (stage 11).
- [System dependencies](system-dependencies.md) on no probe path beyond `pkg-config` (stage 06).

## Documentation discrepancies

Found while gathering evidence.  Fixed means fixed in stage 00; the others name the stage or
change expected to fix them.

| Where | Says | Code | Disposition |
| --- | --- | --- | --- |
| [Toolchains](toolchains.md#deferred--out-of-scope) deferred list | compiler-conditional flags are deferred | `cc`, `cxx`, `cc_version`, `cxx_version` flag-table keys exist | fixed |
| [Toolchains](toolchains.md#deferred--out-of-scope) deferred list | SARIF/JSON capabilities "are detected" | no such capability field | fixed |
| `docs/architecture.md` C++ semantic invariants | system-dependency flags "are wired into consumers that link the producing target"; a header-only link interface is "include-dir + system deps" | primary packages only; `--libs` reaches the declaring package's own links; `link-libs` is what propagates | fixed (wording); behavior is stage 06 |
| [Manifest](manifest.md#edge-visibility), `cabin-core` `TargetDep` doc | `public` "does not change how anything builds"; consumed by nothing | it drives the post-resolution standard check | fixed in stage 02 |
| [Standard-compatibility spec](design/standard-compatibility/spec.md) D5 | a private edge does not expose the dependency to the consumer's own consumers | include directories follow private edges to every transitive consumer | fixed in stage 02 (the code now follows D5) |
| [Profiles](profiles.md), `build_flags.rs` | `link-libs` belong to a target's objects | collected per package | stage 02 |
| [Targets](targets.md), `TargetKind` doc | a `library` is a `.a` archive | `<name>.lib` on MSVC | stage 08 |
| [Target dependencies](target-dependencies.md) | `cfg(env = "musl")` example; `target` documented as a triple in `condition.rs` | `env` always follows the OS; `target` is `<arch>-<family>-<os>` | stage 04 |
| [Toolchains](toolchains.md), [compiler cache](compiler-cache.md), architecture | the wrapper's `source` is fingerprinted | only kind, spec, and version are hashed | stage 04 (fingerprint completeness) |
| [Toolchains](toolchains.md) | the fingerprint reaches `cabin run` and `cabin test` | they discard it | stage 04 |
| [Profiles](profiles.md) | output layout `packages/<package>/<target>/...` | artifacts in `packages/<pkg>/`, objects in `packages/<pkg>/obj/<target>/` | stage 09 (layout contract) |
| [Metadata, tree, explain](metadata-tree-explain.md) | metadata is byte-identical across machines | it includes resolved tool paths and pkg-config output | stage 05 (configured versus detected) |
| [System dependencies](system-dependencies.md), `docs/architecture.md` (`cabin-system-deps`) | only the selected members are primary; probes fire only for a selected primary package | every primary package is probed; only the MSVC gate narrows to the selection (not run) | stage 06 |
| [Lockfile](lockfile.md#limitations) | HTTP index, authentication, registry server, and publishing are unsupported; workspace lockfiles without a root package are unsupported | all exist | fixed in stage 01 |
| [Lockfile](lockfile.md) format | `dependencies` are transitive names; `source` includes local provenance | direct resolved index dependencies; only `"index"` parses | fixed in stage 01 |
| [Package index](package-index.md#packaging-revisions) | resolution never reads revisions | locked validation reads `revisions` | stage 03 |
| `cabin resolve` help | selection only narrows diagnostics; resolves "against a local JSON package index" | selection scopes resolution ([workspaces](workspaces.md)); `--index-url` works | stage 11 |
| [Cargo-inspired interface](cargo-inspired-interface.md) | subcommand table; `-j` "identical" | omits `login`, `logout`, `yank`, `resolve`, `compgen`, `mangen`; `test` has no `-j` | stage 11 |
| `docs/architecture.md` | `SystemDependencyProbeReport` | the type is `SystemDependencyResolution` | stage 06 |

## Port corrections waiting on stage 03

None recorded yet.  A stage that needs a published port to change its dependencies, features,
standards, or an existing `links` claim records it here instead of respinning.

## Progress record

**Adopted contracts.**  Stage 00 adopted the [shared contracts](design/capability-roadmap/contracts.md):
package versus build-unit identity, usage-requirement kinds and edge rules, root-selected providers,
generated actions with root execution policy, and per-revision metadata as stage 03's target.

**Changes.**  Stage 00: this ledger, the contracts page, and the scope alignment listed above.
Stage 00 counts as adopted once this ledger is on `main`.

Stage 01: feature-aware dependency resolution (area 1, "Today").  Every command that resolves -
`resolve`, `update`, `fetch`, `vendor`, `build`, `run`, `test` - shares one selection over file and
sparse-HTTP indexes; `metadata` and `explain build-config` report configurations from the local
feature resolution, and `tree` and `explain package` leave out optional dependencies no enabled
feature turns on.  Patch forks reached through registry edges are ordinary single-version
candidates, which replaced the activation loop, its residue packages, and the build-time-only
collision check for fork optional dependencies; the transitive fork version check is now a
resolution conflict.  The lockfile format is unchanged; [lockfile](lockfile.md#features-and-the-selected-closure)
documents how features meet its contents.  Contracts section 1's seam for stage 04: the resolver's feature
packages (`Node::Feature`) and its single `platform` are what per-context resolution splits, while
`Node::Package` stays shared, keeping one version per name across contexts.

Stage 02, first change: edge visibility governs compile interfaces (area 2, "Today"), with target
`interface-defines`, header-only `defines` rejected, and definition conflicts diagnosed.  The
migration audit found one private port edge whose consumers relied on the old leak through their
own sources, not through the producer's headers: `libpng` -> `zlib` stays private (`png.h` does
not include `zlib.h`), and the `libpng-usage` example now declares the `zlib` it includes;
`boost-iostreams` -> `zlib` is the only other private port edge.  No port changed.

**Validation.**  Stage 00 changes documentation only; the website gate covers it.  Stage 01:
resolver unit tests for each acceptance scenario (feature chain, defaults diamond, older candidate
for a feature, optional-edge backtracking, non-matching `cfg` edge, unknown feature, locked replay,
lazy metadata), CLI tests for the registry chain built and replayed `--frozen`, an inactive
optional dependency absent from file and HTTP registries, a registry-reached path patch enabling
its optional versioned dependency, and dependency configurations in `metadata`; `cargo ci` on
macOS.  Not run: Linux and Windows locally (CI covers them), and no real-project demonstration -
stage 12 validates.  Stage 02, first change: planner tests for private and public edges, public
chains and diamonds, definition conflicts (both paths, escaping, check pruning), own-define
override, and the visible set feeding the build-time standard check; manifest tests for the
`interface-defines` grammar and kinds; the `libpng-usage` and `png-info` examples built and run;
`cargo ci` on macOS.

**Unfinished.**  Stages 02-14.  Follow-ups found in stage 01, outside its scope:
`xtask-port-publish` still orders publication by each port's own default feature closure (area 1,
"Blocked workflow"); a transitively requested unknown feature is reported as a resolution conflict
naming `<package>[<feature>]` rather than with the direct-request diagnostic; `metadata` and
`explain build-config` see local edges only, so a registry edge's request on a member or fork is
missing from their configuration report; `tree` and `explain package` filter by included package,
not by edge, so a disabled optional edge onto a package another edge includes still shows; over
sparse HTTP, each optional dependency a feature enables costs one fetch round and re-solve.

Stage 02 remaining, in order: graph-ordered `link-libs` with repetition kept and typed
`frameworks`; per-source instruction sets; the Boost ports (`stacktrace` and `contract` interface
definitions, `atomic` and `log` ISA sources).  Follow-ups found so far: a missing-header link
diagnostic does not suggest a public edge or a direct dependency; the `libpng` port's comment
claims `png.h` includes `zlib.h`, a comment-only packaging-revision correction for a later respin.

## Handoff to stage 02

Stage 02 starts from contracts section 2 and area 2 above.  From stage 01 it inherits one enabled
feature set per package (`FeatureResolution` locally, `ResolvedPackage::features` for registry
packages), which is what feature-gated interface requirements must read.
