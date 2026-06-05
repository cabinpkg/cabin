# AGENTS.md

Guidance for contributors working on this repository.

## Project goal

Cabin is a **package manager and build system for C/C++**,
distributed as the public local OSS core.  The repository is the
Rust implementation.  Cabin is *Cargo-inspired*, not
*Cargo-compatible*: it borrows Cargo's vocabulary where the
semantics line up and diverges where C/C++ semantics demand it.

The local core is **pre-1.0**.
Capabilities already in this repository:

- C/C++ multi-package builds via Ninja, with
  a typed `BuildGraph` and a Clang-compatible
  `compile_commands.json`.
- `cabin run` (build + execute an `executable` with `--`
  arg forwarding and a `CABIN_*` env overlay).
- `cabin test` for `test` targets, with a deterministic
  per-test `CABIN_*` env.
- Two dependency kinds (`normal`, `dev`) plus a
  `system = true` sourcing flag, with documented activation
  rules.
- Workspace semantics: member globs, `exclude`,
  `default-members`, shared `[workspace.<kind>-dependencies]`,
  selection-aware loading.
- PubGrub-backed dependency resolver with Cabin-owned inputs,
  outputs, and miette diagnostics; `cabin.lock`; artifact
  pipeline (fetch / verify / extract).
- `cabin package` + local file-registry `cabin publish`
  (no remote registry protocol).
- Sparse HTTP index read path.
- Features + cross-package feature resolver.
- `[target.'cfg(...)'.<kind>]` dependency conditions.
- Build profiles, toolchain selection, capability detection,
  and `ccache` / `sccache` compiler-cache wrappers.
- Typed `.cabin/config.toml` system with documented precedence.
- Patch / override / source-replacement.
- `cabin vendor` + `--offline` / `CABIN_NET_OFFLINE`.
- `cabin metadata` / `cabin tree` / `cabin explain`.
- The Cargo-inspired interface foundation: `--build-dir <dir>`
  is the build-output flag (default `build/`), `--target` is
  reserved for the future platform / toolchain target flag and
  is *not* a manifest-target selector on any command,
  `cabin run` / `cabin test` get a deterministic `CABIN_*`
  overlay.
- Developer tooling: `cabin fmt` (clang-format) and
  `cabin tidy` (run-clang-tidy) — both sharing the
  `cabin-source-discovery` walker, an env-override for the
  underlying tool, and the same workspace-selection flags.
- C/C++ environment-flag ingestion: `CPPFLAGS` / `CFLAGS` /
  `CXXFLAGS` / `LDFLAGS` parse with shell-style quoting and
  route to the matching compile / link commands.
- `system = true` dependency probing via `pkg-config`
  (`CABIN_PKG_CONFIG` overrides the executable); cflags / libs
  flow into the build planner and `compile_commands.json`.
- `-j` / `--jobs <N>` build / run / tidy parallelism with a typed
  validated model and a documented precedence chain.
- `cabin new <path>` / `cabin init` with `--bin` / `--lib`
  scaffold parity.
- `cabin version` (concise + verbose forms) and `cabin --list`
  (full subcommand directory; `cabin --help` stays curated).
- Windows / MSVC support: the `cabin-driver` crate lowers the
  toolchain-independent build IR to either the GCC/Clang or the
  MSVC (`cl.exe` / `lib.exe`) command-line dialect, selected from
  the detected compiler. The dialect governs artifact naming
  (`.o` / `.obj`, `lib<x>.a` / `<x>.lib`, `<x>` / `<x>.exe`) and
  Ninja's header-dependency mode (`deps = gcc` depfiles vs.
  `cl /showIncludes` + `deps = msvc`); `cfg(...)` conditions and
  the user config / cache homes resolve per platform.

Probe compilations beyond `--version`, distcc / icecc compile-
server wrappers, cross-compilation,
SARIF / structured-diagnostic frameworks, sanitizer frameworks,
coverage instrumentation and reporting, a benchmark target kind
or harness, broad CMake / Meson compatibility, and any remote
build cache are explicitly deferred — see
[`docs/architecture.md`](docs/architecture.md).

## Where compiler / tool detection work belongs

- `cabin-core::compiler` owns `CompilerKind`, `ArchiverKind`,
  `CompilerVersion`, `CompilerIdentity`, `ArchiverIdentity`,
  `CompilerCapabilities`, `ArchiverCapabilities`, `Capability`,
  `CapabilitySource`, `ToolDetection`,
  `ToolchainDetectionReport`, and `ToolDetectionError`. It also
  holds the pure parsers (`parse_cxx_version_output`,
  `parse_ar_version_output`), the capability-derivation
  functions (`derive_*_capabilities`), and the backend
  validators (`validate_*_for_backend`).
- `cabin-toolchain::detect` owns subprocess spawning. The
  `ToolRunner` trait abstracts `tool --version` so detection is
  testable without real binaries; `ProcessRunner` is the
  production implementation. Detection never touches the
  network and never compiles probe sources.
- `cabin-build::validate_toolchain_for_backend` consumes the
  detection report and rejects compilers / archivers that
  cannot run the GCC/Clang-style commands the planner emits.
  Validation runs *before* any Ninja file is written.
- `cabin` runs detection after toolchain resolution,
  validates before planning, and surfaces the report under
  `toolchain.detected` in `cabin metadata`.
- `cabin-package`, `cabin-index`, and `cabin-registry-file`
  must **never** serialize detection results into package /
  index metadata — the report is local-environment state.

**Do not** put version-output parsing, process probing,
capability decisions, or backend support policy in `cabin`.
The CLI calls the typed APIs above and renders the result; new
detection logic belongs in the owning crate, not in
`cabin/src/cli.rs`.

## Where build-profile work belongs

- `cabin-core::profile` owns `ProfileName`, `OptLevel`,
  `BuiltinProfile`, `ProfileDefinition`, `ProfileSelection`,
  `ResolvedProfile`, `ProfileSource`, and `resolve_profile`.
  These are the typed values every other crate consumes.
- `cabin-manifest` parses `[profile.*]` tables and rejects
  unsupported fields. Raw serde structs stay private to the
  crate; the public surface returns
  `cabin_core::ProfileDefinition` values.
- `cabin-workspace` rejects member / path-dep manifests that
  declare `[profile.*]` tables, because only the entry-point
  manifest's profile tables count.
- `cabin-build` consumes `ResolvedProfile` to derive C++ compile
  flags and the per-profile output directory. It does not parse
  CLI flags or manifests.
- `cabin-package` preserves manifest `[profile.*]` declarations
  in the canonical metadata; `cabin-index` round-trips them
  opaquely. Index resolution remains profile-independent.

**Do not** put profile parsing, inheritance resolution, build-
graph policy, or compiler-flag mapping in `cabin`. The CLI
parses `--profile` / `--release` and converts them into
`cabin_core::ProfileSelection`; everything else lives behind a
typed API in the owning crate.

## Where toolchain / build-flag work belongs

- `cabin-core::toolchain` owns `ToolKind`, `ToolSpec`,
  `ToolSource`, `ToolSelection`, `ResolvedTool`, `ResolvedToolchain`,
  `ToolchainSettings`, `ConditionalToolchainDecl`, `ToolchainDecl`,
  and `ToolchainResolutionError`.
- `cabin-core::build_flags` owns `ProfileFlags`,
  `ConditionalProfileFlags`, `ProfileSettings`,
  `ResolvedProfileFlags`, the `resolve_build_flags` merge function,
  and `BuildFlagsValidationError`.
- `cabin-toolchain::resolve` owns the precedence walk
  (CLI ▶ env ▶ matching `[target.'cfg(...)'.toolchain]` ▶
  `[toolchain]` ▶ default fallback list), `PATH` search, the
  per-OS default fallback list (`cl` / `lib` on Windows, `cc` /
  `c++` / `ar` elsewhere), and the rejection of the linker
  (`link`) or archiver (`lib`) named for a compiler slot.
- `cabin-manifest` parses `[toolchain]`, `[profile]`,
  `[profile.<name>]`, `[target.'cfg(...)'.toolchain]`, and
  `[target.'cfg(...)'.profile]` tables and rejects unknown fields.
  Raw serde structs stay private.
- `cabin-workspace` rejects member / path-dep manifests that
  declare `[toolchain]` tables, mirroring the existing
  `[profile.*]` rule.
- `cabin-build` consumes the `ResolvedToolchain` and the
  per-package `ResolvedProfileFlags` map. It maps semantic flags
  onto the existing compile / link / archive commands without
  parsing CLI flags or manifests.
- `cabin-package`, `cabin-index`, and `cabin-registry-file`
  round-trip *manifest-declared* `[toolchain]`, `[profile]`,
  and `[target.'cfg(...)'.profile]` declarations only. CLI- or
  env-derived selections must never flow into the canonical
  metadata document or the file registry.

## Where patch / override / source-replacement work belongs

- `cabin-core::patch` owns `PatchSource`, `PatchSourceKind`,
  `PatchProvenance`, `DeclaredPatch`, `PatchManifestSettings`
  (root-only manifest model), and the typed
  `PatchValidationError`. Pure data + small parsers only.
- `cabin-core::source_replacement` owns `SourceLocator`,
  `SourceReplacementEntry`, `SourceReplacementSettings`, the
  cycle-detecting `resolve()`, and the typed
  `SourceReplacementError`.
- `cabin-manifest` parses the `[patch]` table on root manifests
  with stable rejection messages for `git` / `url` / `version`.
  Member manifests with `[patch]` are rejected by
  `cabin-workspace`.
- `cabin-config` parses `[patch]` and `[source-replacement]`
  tables, rejects credentials in URLs and unsupported source
  kinds, and threads patches through into `EffectiveConfig`.
- `cabin-workspace::patch` resolves the merged manifest +
  config patch policy, validates each entry (path, manifest,
  name, version), and exposes `ActivePatchSet` for downstream
  consumers. `load_workspace_with_registry_and_patches`
  stitches each active patch as a `kind = Local` package.
- `cabin`'s `patch_glue` module orchestrates: it converts
  `EffectiveConfig` into `cabin-workspace`-shaped inputs,
  applies source replacement to the resolved index source,
  threads patches into the artifact pipeline / lockfile /
  metadata view, and renders the deterministic JSON / lockfile
  records.
- `cabin-package` rejects manifests with a non-empty `[patch]`
  table to keep local override policy out of published
  archives.
- `cabin-lockfile` exposes top-level `[[patch]]` and
  `[[source-replacement]]` arrays for stale-detection under
  `--locked`. Old lockfiles without these arrays remain valid.

**Do not** put patch parsing, config merging, source
replacement, resolver candidate modification, lockfile patch
state, or publish validation in `cabin/src/cli.rs`. The
typed surfaces above own the policy; the CLI layer only
threads typed values through. New patch source kinds extend
[`cabin_core::PatchSource`] explicitly — never as stringly
typed strings — and add a matching parser in
`cabin-manifest` / `cabin-config`. The patch / override layer
explicitly does not implement Git sources, vendoring, registry
authentication, credentials handling, new registry protocols,
HTTP publish, or registry-server work — those are tracked
separately in [`docs/architecture.md`](docs/architecture.md).

## Where config-file work belongs

- `cabin-config` owns config discovery, raw
  TOML deserialisation (private serde types behind
  `deny_unknown_fields`), validation, merging, and the typed
  [`EffectiveConfig`] consumed by the rest of the workspace.
  Reuses typed models from `cabin-core` (`ToolSpec`,
  `CompilerWrapperRequest`, `ConfigValueSource`) so the config
  layer never invents parallel grammars.
- `cabin` only *orchestrates* config: it loads the
  effective config via the typed API, threads it into existing
  resolvers and into the metadata view, and exposes the
  documented env vars (`CABIN_NO_CONFIG`, `CABIN_CONFIG`,
  `CABIN_CONFIG_HOME`). Discovery, parsing, merging, validation,
  and precedence policy do **not** belong in
  `cabin/src/cli.rs`. The thin glue helpers live in
  `cabin/src/config_glue.rs`.
- `cabin-core::config_source` owns the cross-cutting
  `ConfigValueSource` enum used by metadata reporting for paths,
  profile, and registry settings. Tool/wrapper-specific source
  enums (`ToolSource`, `CompilerWrapperSource`) gain matching
  `*Config` variants so the existing precedence walkers can
  attribute a value to the exact config file.
- `cabin-toolchain::resolve` and `cabin-toolchain::wrapper`
  accept an optional config layer (`ConfigToolchainLayer` /
  `ConfigWrapperLayer`) that slots between the env variable and
  the manifest. The resolvers do not parse config TOML or know
  about file discovery — they just consume the typed layer.
- `cabin-package`, `cabin-index`, `cabin-index-http`, and
  `cabin-publish` must **never** serialize effective config into
  package or index metadata. Local config files (`.cabin/`) are
  excluded from deterministic source archives by the existing
  `EXCLUDED_DIR_NAMES` policy.

**Do not** put config discovery, parsing, merging, precedence
policy, validation, secrets handling, source replacement, or
vendoring in `cabin`. The config layer's public surface is
intentionally narrow: `[registry]`, `[paths]`, `[build]`,
`[build.cache]`, `[toolchain]`, `[patch]`, and
`[source-replacement]` tables — nothing else, no auth, no
tokens, no `[target.'cfg(...)']`-conditioned config tables.

## Where compiler-cache wrapper work belongs

- `cabin-core::compiler_wrapper` owns `CompilerWrapperKind`,
  `CompilerWrapperRequest`, `CompilerWrapperManifestSettings`,
  `ConditionalCompilerWrapperDecl`, `CompilerWrapperSource`,
  `CompilerWrapperIdentity`, `ResolvedCompilerWrapper`,
  `CompilerWrapperSummary`, and `CompilerWrapperParseError`.
- `cabin-toolchain::wrapper` owns the precedence walk
  (CLI ▶ `CABIN_COMPILER_WRAPPER` env ▶ config
  `[build.cache]` ▶ matching
  `[target.'cfg(...)'.profile.cache]` ▶ workspace-root
  manifest `[profile.cache]` ▶ no wrapper), `PATH` search via the same
  `EnvLookup` / `ExecutableProbe` callbacks the toolchain
  resolver uses, and an optional `--version` probe through
  `ToolRunner`.
- `cabin-manifest` parses `[profile.cache]` (and the
  target-conditioned `[target.'cfg(...)'.profile.cache]` variant)
  into the typed
  `CompilerWrapperManifestSettings`. Member / path-dep manifests
  with non-empty cache settings are rejected via the workspace
  loader's new `MemberDeclaresCompilerWrapper` error.
- `cabin-build`'s planner accepts `Option<&ResolvedCompilerWrapper>`
  on `PlanRequest` and prepends the wrapper to every C++ compile
  command on the Ninja path *only*. Link and archive commands are
  deliberately never wrapped, and `compile_commands.json` keeps
  the underlying compiler so clangd / IDE tooling stays accurate.
- `cabin-package`, `cabin-index`, and `cabin-registry-file`
  round-trip *manifest-declared* `[profile.cache]` settings only.
  CLI / env-derived selections must never flow into canonical
  metadata.

**Do not** put wrapper parsing, precedence walking, `PATH`
search, version probing, or planner integration in `cabin`.
The CLI parses `--compiler-wrapper` / `--no-compiler-wrapper`,
calls the typed APIs above, and threads the result through
`PlanRequest` and `MetadataView`.

**Do not** put toolchain resolution, condition evaluation, flag
merging, or build-graph policy in `cabin`. The CLI parses
`--cc` / `--cxx` / `--ar` and converts them into
`cabin_core::ToolchainSelection`; everything else lives behind a
typed API in the owning crate.

## Where dependency-kind work belongs

- `cabin-core` owns `DependencyKind`, `Dependency`,
  `SystemDependency`, and the per-kind `Project` collections.
  Add new kind-related types here only when they are needed by
  more than one downstream crate.
- `cabin-manifest` is the only crate allowed to parse
  `[dependencies]` and `[dev-dependencies]` text (including
  `system = true` entries). Raw serde structs stay private.
- `cabin-workspace` owns kind-specific workspace inheritance and
  the package-graph edge model (`DependencyEdge` carries
  `(index, kind)`).
- `cabin-resolver` only ever sees the resolvable kinds (normal).
  System deps must never reach it; dev deps are excluded by
  default.
- `cabin-build` only links normal-kind edges into ordinary
  targets. Build / dev deps must not auto-link.
- `cabin-package`, `cabin-index`, and `cabin-registry-file`
  preserve every kind end-to-end through canonical metadata.

**Do not** put dependency parsing, dependency-kind policy,
dependency-graph algorithms, or resolver-input construction
logic in `cabin`. The CLI translates clap inputs into the
typed APIs above and renders the result; new dependency-kind
behavior belongs in the owning crate, not in
`cabin/src/cli.rs`.

**Do not** implement future dependency features
opportunistically. Cross-compilation remains explicitly
deferred — manifest fields that gesture at it must stay rejected
with clear errors.

## Where system-dependency probing work belongs

- `cabin-system-deps` owns `PkgConfigTool`, the typed
  `SystemDependencyProbeRequest` / `SystemDependencyProbeReport`
  / `SystemDependencyFlags` model, the
  `probe_system_dependency` entry point, the
  `cabin::system_deps::*` `PkgConfigError` diagnostic family,
  the pkg-config argv builder (including the
  SemVer-comparator → pkg-config-operator translation), and the
  minimal-quoting splitter used to parse `--cflags` / `--libs`
  output.  Must not parse manifests, walk the workspace graph,
  or mutate `ResolvedProfileFlags`.
- `cabin::system_deps_glue` is the orchestration shell: it
  collects active system dependencies from
  `cabin_workspace::PackageGraph::primary_packages`, applies
  conditional declarations against the host platform, calls
  `probe_system_dependency` once per active dep, and merges
  the resulting flags into the per-package
  `HashMap<usize, ResolvedProfileFlags>` that flows through the
  build pipeline.  The single helper
  `augment_build_flags_with_system_deps` is called from every
  command that constructs a build configuration —
  `cabin build`, `cabin run`, `cabin test`, `cabin tidy`,
  `cabin metadata`, and `cabin explain build-config` — after
  `resolve_per_package_build_flags` and before
  `resolve_build_configurations` so the
  `BuildConfiguration::fingerprint` observes the discovered
  flags. The other `cabin explain` subcommands (`package`,
  `target`, `source`, `feature`) do not build a configuration
  and therefore skip probing.
- `cabin-env` exposes `CABIN_PKG_CONFIG` alongside the other
  read-side env var constants.  No new env-handling logic
  belongs in `cabin`.

**Do not** add `pkg-config` invocation code, flag-classifier
logic, version-comparator translation, or executable-resolution
policy to `cabin/src/cli.rs`.  The CLI threads the typed
report into the existing build-configuration pipeline; the
probing layer stays in `cabin-system-deps`.

**Do not** route discovered flags into canonical package
metadata (`cabin-package`, `cabin-index`, `cabin-registry-file`)
or into the lockfile.  `system = true` declarations
round-trip end-to-end; pkg-config probe results are local
build-time state.

**Do not** expand system dependency probing into a broader
package-manager integration (vcpkg / Conan / Homebrew / apt).
Cabin queries `pkg-config` and nothing else.

## Where dev / test / example target work belongs

- `cabin-core` owns `TargetKind` and the per-kind classifier
  predicates (`is_default_buildable`, `is_dev_only`, `is_test`,
  `produces_executable`). Add new kinds and classifiers here only
  when more than one downstream crate needs them.
- `cabin-manifest` parses the artifact-role target-kind strings
  (`library`, `header_only`, `executable`, `test`, `example`)
  into `TargetKind` variants. Raw serde structs stay private.
- `cabin-workspace` thread an `include_dev_for: &BTreeSet<String>`
  set through `WorkspaceLoadOptions` and the `_with_dev` closure
  helpers so `cabin test` activates dev-deps for the *selected*
  packages without affecting `cabin build`. Dev-dep activation
  never propagates to transitive deps.
- `cabin-build` knows that `test` / `example` link as
  executables and excludes them from the default-target
  enumeration. `select_targets_of_kind` is the typed "all
  `test` selectors in selected packages" convenience for
  `cabin test`.
- `cabin-test` owns the test execution plan (`TestPlan`,
  `TestExecutable`), the sequential runner (`run_tests`), the
  output sink trait, and the typed summary (`TestSummary`,
  `TestRunStatus`). It does not parse manifests, build
  dependency graphs, generate Ninja, or know about config /
  patches.
- `cabin/src/test_glue.rs` is the orchestration shell for
  `cabin test`: it parses CLI args, drives the existing
  build pipeline, hands the resulting `BuildGraph` to
  `cabin-test`, and renders the summary. It must not own test
  discovery, build-graph target-kind policy, or test execution
  business logic.

**Do not** put `test` / `example` policy, test
discovery, test runner business logic, or build-graph
target-kind policy in `cabin/src/cli.rs`.

**Do not** implement test framework integration
(GoogleTest / Catch2 / doctest output parsing, XML / JUnit
output, in-binary test discovery), sanitizer frameworks,
benchmark target kinds / harnesses, coverage instrumentation,
or `cabin run --example` commands here. Those remain tracked
separately in [`docs/architecture.md`](docs/architecture.md).
`cabin tidy` (run-clang-tidy) already ships — see its owning
crate and docs ([`docs/tidy.md`](docs/tidy.md),
[`docs/testing.md`](docs/testing.md)).

## Where C/C++ language work belongs

Cabin treats C/C++ as related but distinct source
languages. Future changes must keep C support first-class; do not
let C++ assumptions leak back in. The owning crates are:

- `cabin-core` owns the typed `SourceLanguage` enum, the
  per-source classifier (`classify_source`), the link-driver
  predicate (`link_driver_language`), and the `validate_cc_for_backend`
  / `validate_cxx_for_backend` capability validators. Add
  language-related typed concepts here, not in downstream crates.
- `cabin-manifest` parses `cflags`, `cxxflags`, and `ldflags`
  separately; raw serde structs stay private. Keep the C-only,
  C++-only, and link argument buckets distinct.
- `cabin-toolchain` resolves CC and CXX as separate slots and
  tries the documented C-compiler fallback list opportunistically
  so a C compiler is populated by default.
- `cabin-build` classifies every source per-file, dispatches
  compiles through the language-appropriate driver and standard
  flag (`-std=c11` for C, `-std=c++17` for C++), keeps the
  CFLAGS / CXXFLAGS argv spaces strictly separate, and selects
  the link driver by walking the target's own objects plus
  every transitively reachable library object.
- `cabin-ninja` declares `c_compile` and `cxx_compile` rules
  separately and a single language-neutral `link_executable`
  rule; the link driver lives in the action's `command`, not in
  the rule name.

Acceptance guidance for *every* future change:

- Add or update C-specific fixtures alongside C++ fixtures
  whenever changes touch the build planner, the manifest
  parser, the build flags, the toolchain layer, the build
  graph, the Ninja generator, the package archive, the lockfile,
  the artifact pipeline, or the metadata view.
- Keep CFLAGS and CXXFLAGS separate. A new escape-hatch field
  must be classed as language-neutral, C-only, or C++-only at
  declaration time.
- Keep C/C++ standard flags separate. Do not hardcode
  `-std=c++NN` for a C compile and do not hardcode `-std=cNN`
  for a C++ compile.
- Keep CC capability detection separate from CXX capability
  detection. A C-only feature must not require the CXX side to
  support it.
- Document the link-driver pick when adding any new linking
  surface (e.g. shared libraries, future plugin targets) — the
  rule is "C++ if any reachable C++ object, C otherwise" and
  any deviation must be justified in `docs/architecture.md`.
  The build planner exposes the predicate as
  `cabin_core::link_driver_language(&[SourceLanguage])`; do
  not reimplement the rule in another crate.
- Treat the typed dispatch surfaces as the contract: prefer
  extending `cabin_core::SourceLanguage`,
  `cabin_core::classify_source`, the planner's
  `CompileDispatch` (internal to `cabin-build`), and
  `cabin_build::flags_for_profile` over scattering language
  conditionals across the planner. New language-related work
  should land at one of those points.
- Keep public C/C++ headers and private headers separate. The
  existing `include_dirs` propagation is the public path; do
  not collapse it into a private-also concept without an
  explicit language change.
- Keep system dependencies usable for C system libraries
  (glibc / libm / libpthread / etc.) the same way they are for
  C++ system libraries.

## Test portability rules

These rules apply to every test that lives under `cabin`,
the planner, the toolchain layer, and any future crate that
exercises the build / test pipeline. They are normative —
adding a test that violates them is a review-blocking change.

### 1. Tool gating must be explicit

Tests that compile real C or C++ sources gate on a tool
availability helper from `crates/cabin/tests/cli.rs`:

| Helper                              | Required tools                          |
| ----------------------------------- | --------------------------------------- |
| `ninja_available`                   | `ninja`                                 |
| `c_compiler_available`              | one of `cc` / `clang` / `gcc`           |
| `cxx_compiler_available`            | one of `c++` / `clang++` / `g++`        |
| `build_tools_available`             | `ninja` + a C++ compiler                |
| `c_and_cxx_build_tools_available`   | `ninja` + a C compiler + a C++ compiler |

A test that compiles `.c` sources **must** gate on
`c_and_cxx_build_tools_available`, not on
`build_tools_available`. Without that, the test would silently
fall through to a planner-time `MissingCCompiler` error on a
runner that has only `c++` / `clang++` / `g++` installed. A
test that compiles only C++ sources gates on
`build_tools_available`.

Pure data-model tests that never spawn a compiler (planner
unit tests, lockfile / metadata round-trips) do not need to
gate on tool availability.

The CLI suite also has external-tool smoke tests for the tools
Cabin shells out to directly: `ninja`, `clang-format`,
`run-clang-tidy`, and `pkg-config`. These tests intentionally
fail by default when the real tools are absent, so CI catches a
missing package instead of silently exercising only fake helpers.
Set `CABIN_SKIP_EXTERNAL_TOOL_TESTS=1` only when you deliberately
want those smoke tests to use the bundled fake-tool binaries.

### 2. Environment isolation

Integration tests use the shared `cabin()` helper, which clears
or pins the read-side environment that commonly affects test
output and tool selection:

- `CABIN_NO_CONFIG`, `CABIN_CONFIG`, `CABIN_CONFIG_HOME`
  (config discovery);
- `CC`, `CXX`, `AR` (toolchain selection);
- `NINJA` (backend lookup);
- `CPPFLAGS`, `CFLAGS`, `CXXFLAGS`, `LDFLAGS` (build-flag
  ingestion);
- `CABIN_NET_OFFLINE` (offline override);
- `CABIN_COMPILER_WRAPPER` (compiler-cache wrapper);
- `CABIN_CACHE_DIR` (artifact cache override);
- `CABIN_FMT`, `CABIN_TIDY`, `CABIN_PKG_CONFIG`
  (developer-tool and system-dependency executable overrides);
- standard pkg-config lookup variables (`PKG_CONFIG_PATH`,
  `PKG_CONFIG_LIBDIR`, `PKG_CONFIG_SYSROOT_DIR`);
- terminal-color controls (`NO_COLOR`, `CLICOLOR`,
  `CLICOLOR_FORCE`), while pinning `CABIN_TERM_COLOR=never`.

The helper does not remove every read-side Cabin variable. Tests
whose assertions depend on build directory, job count, or
verbosity must set or remove `CABIN_BUILD_DIR`,
`CABIN_BUILD_JOBS`, `CABIN_TERM_VERBOSE`, and
`CABIN_TERM_QUIET` explicitly.

Tests that intentionally exercise env precedence (e.g. "CXX
env wins over the manifest's `[toolchain]`") opt back in with a
plain `.env(KEY, VALUE)` after `cabin()` returns — `assert_cmd`
applies env mutations in declaration order, so a later
`.env(...)` overrides the earlier `.env_remove(...)`.

The shared `cabin_with_config()` helper in the patches module
keeps the same scrubbing rules but additionally re-enables
config discovery for tests that exercise config files; consult
that module for the documented opt-in pattern.

### 3. No host-specific absolute paths

Integration tests must not use hardcoded host-specific
absolute paths (`/tmp/...`, `/usr/bin/...`, `/this/path/does/not/exist/...`).
Construct paths under `assert_fs::TempDir` instead —
`dir.child("missing-cc").path()` is the canonical
"non-existent path" idiom for tests that need a path that will
fail to resolve.

The planner unit tests use fake POSIX-shaped paths (`/abs/proj`,
`/usr/bin/g++`) but never *execute* them — those tests are pure
data-model assertions on the build graph that happens to take a
`PathBuf` as input. That is the only place absolute fake paths
are acceptable.

### 4. Driver-name assertions

Tests that need to verify the link-driver pick should prefer
*structural* assertions over driver-name substring matching:

- assert on the rule name (`c_compile`, `cxx_compile`,
  `link_executable`) by walking generated `build.ninja` edges
  rather than grepping for `c++` / `g++` / `clang++`;
- when the test must check the actual driver path, ask
  `cabin metadata --format json` for the resolved
  `toolchain.tools.cc.path` / `toolchain.tools.cxx.path` and
  compare the link command's first argument against that.

Substring checks are acceptable as a *belt-and-suspenders*
sanity check, never as the primary assertion.

### 5. Generated output normalization

Golden / fixture tests compare generated output (Ninja file,
metadata JSON, package archive contents) against a snapshot.
Output that contains absolute paths must be normalized before
comparison so the snapshot does not bake in the developer's
temp-directory prefix. The lockfile renderer is the canonical
example: every value sorts deterministically, paths are stored
relative to a documented anchor, and the tests assert on
byte-equal output.

### 6. Test filesystem fixtures

Use [`assert_fs`](https://docs.rs/assert_fs) for temporary
filesystem fixtures and filesystem assertions in Rust tests.
The canonical pattern is:

```rust
use assert_fs::TempDir;
use assert_fs::prelude::*;
use predicates::prelude::*;

let dir = TempDir::new().unwrap();
dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
dir.child("src/main.cc").write_str(MAIN_CC).unwrap();
let out = dir.child("dist");

// Pass `&Path` across the production boundary:
cabin().args(["build", "--manifest-path"])
    .arg(dir.child("cabin.toml").path())
    .arg("--build-dir").arg(out.path())
    .assert().success();

// Predicate-based filesystem assertions:
out.child("dev/build.ninja").assert(predicate::path::is_file());
```

`ChildPath` is a test fixture type — never expose it from
production crates. Pass `child.path()` or `child.to_path_buf()`
across the production API boundary so Cabin's library code
keeps accepting `&Path` / `PathBuf` / `OsStr`.

Keep command execution through the shared `cabin()` helper so
environment isolation remains consistent (see § 2). Pair
`assert_fs` for fixture setup with `assert_cmd` for command
invocation and `predicates` for stdout / stderr / path
assertions.

Normalize absolute temp paths before comparing generated output
against a golden snapshot (see § 5). The fixture path printed
by `assert_fs::TempDir` is a host-specific temp directory and
must not leak into expected text.

## CI portability boundary

The Rust CI job in `.github/workflows/rust.yml` runs on both
`ubuntu-latest` and `macos-latest`. Linux installs
`ninja-build`, `gcc`, and `g++`; macOS installs Ninja and LLVM
through Homebrew and uses the platform Clang drivers. The
`cabin()` env scrubbing and the
`c_and_cxx_build_tools_available` gating keep tests portable
across both runners without silently masking C coverage.

## Where vendoring / offline-mode work belongs

`cabin vendor` materializes the selected dependency closure
into a deterministic local file-registry directory (default
`vendor/`). The owning crates are:

- `cabin-vendor` owns the typed `VendorPlan` /
  `VendorEntry` / `VendorOptions` model, the deterministic
  write logic (`materialize`), the `cabin-vendor.json`
  summary, and the path-traversal-safe archive copy. It
  re-uses `cabin_registry_file::FileRegistry` so the on-disk
  layout is byte-equivalent to what `cabin publish
  --registry-dir` writes.
- `cabin/src/vendor_glue.rs` is the orchestration shell
  for `cabin vendor`: it parses CLI args, drives the existing
  `run_artifact_pipeline`, reads each per-package index entry
  from the source `--index-path`, builds a `VendorPlan`, and
  hands it to `cabin-vendor`. It must not own vendor-write
  logic, plan ordering, or checksum verification — all that
  lives in `cabin-vendor`.

`--offline` is the cross-cutting flag that forbids network
access. The single enforcement point is
`crate::config_glue::enforce_offline_index_source`, called
from every command that resolves an index source. New
commands that touch the network must thread `args.offline`
through that helper.

Future changes must keep these invariants:

- `cabin vendor`'s output is a Cabin file registry, byte-
  equivalent to `cabin publish --registry-dir`. Do not
  introduce a parallel on-disk format.
- `--offline` enforcement lives in one place; do not
  duplicate the URL-rejection check across crates.
- Local path dependencies and patched packages are *not*
  vendored. Document any change to that policy in
  `docs/vendoring-offline.md` first.
- Vendoring re-verifies every archive checksum before
  writing. Do not weaken that check.
- HTTP-source vendoring is intentionally deferred. If a
  future change adds it, the per-package JSON re-fetch belongs
  in `cabin-vendor`'s plan-construction layer (or a new
  helper), not in `cabin`.

## Where metadata / tree / explain work belongs

`cabin metadata`, `cabin tree`, and `cabin explain` form one
observability surface over the resolved project state. The
owning crates are:

- `cabin-explain` owns the typed model: `TreeNode`,
  `SourceProvenance`, the `Explanation` tagged union
  (`Package`, `Target`, `Source`, `Feature`), the
  `BuildConfig` query helper, the `ExplainError` family,
  and the deterministic renderers
  (`render_tree_human` / `render_tree_json` /
  `render_explanation_human` / `render_explanation_json`).
  This crate must never run the resolver, parse manifests,
  plan builds, or perform I/O — it consumes the typed values
  the orchestration layer hands it.
- `cabin/src/tree_glue.rs` and
  `cabin/src/explain_glue.rs` orchestrate the workspace /
  config / patch / lockfile / feature-resolution preamble
  (the same preamble `cabin metadata` runs) and hand the
  typed values to `cabin-explain`. They must not own tree
  rendering, explanation chains, or provenance labeling —
  all that lives in `cabin-explain`.
- `cabin metadata` itself stays in `cabin/src/cli.rs`
  for now; future moves of the metadata view into a dedicated
  crate would go alongside `cabin-explain`, not into it.

Future changes must keep these invariants:

- The `cabin metadata` JSON contract is stable. New fields
  may be added (and must be deterministic); existing fields
  must keep their shape.
- `cabin tree --format json` and every `cabin explain ...
  --format json` document is byte-stable across runs given
  the same workspace + lockfile + config inputs.
- Tree children sort by `(dependency_kind, name, version)`;
  explanation paths sort by `(length, joined name sequence)`.
  Do not introduce alternate orderings.
- Provenance labeling lives in `cabin-explain`. Adding a new
  source kind (e.g. `git`, `oci`) is one variant addition to
  `SourceProvenance` plus matching arms in renderers and
  explain queries — do not push the kind detection into the
  CLI glue.
- New `cabin explain` subcommands extend the
  `ExplainCommand` enum and the typed `Explanation` model. The
  glue dispatches; the domain logic stays in `cabin-explain`.
- Renaming a serialized field requires updating
  `docs/metadata-tree-explain.md` in the same commit.

## Where diagnostic / error-rendering work belongs

User-facing diagnostics are produced through a single
presentation layer:

- **Domain crates own typed errors.** Every `cabin-*` crate
  exposes a `thiserror`-derived `Error` enum that carries the
  load-bearing field values in its variants. Rich diagnostics
  that need source snippets or variant-specific help derive
  `miette::Diagnostic` with a stable
  `#[diagnostic(code(cabin::<area>::<symbol>))]`; simpler
  user-facing domain errors are registered with an area-level
  stable code and adapted through `cabin_diagnostics::CodedError`
  / `CodedMessage`.
- **`cabin-diagnostics` is the single renderer.** It owns the
  byte-stable formatter, the source-snippet boundary
  (`annotate-snippets` is reachable only through this crate),
  and the path-normalization helpers golden tests use. New
  source-annotated diagnostics expose `#[source_code]` /
  `#[label]` on the diagnostic-bearing struct; the renderer
  then emits a Cargo-style snippet automatically.
- **`cabin` does not own error construction.** The
  dispatcher walks `anyhow::Error`'s source chain, downcasts
  to the deepest typed diagnostic or coded domain error, and
  routes it through `cabin_diagnostics::render`. Adding a new
  diagnostic-bearing type or coded domain error is a small
  addition to `crates/cabin/src/lib.rs::downcast_diagnostic`
  plus, for a new code, `cabin_diagnostics::code`.

Future changes must keep these invariants:

- **Avoid duplicative `with_context("failed to load X at <path>")`
  wrappers around typed domain errors.** Typed domain errors
  already include the path / operation in their own `Display`;
  wrapping them produces the duplicated chain Cabin used to emit
  (`failed to read X: failed to read X: No such file or
  directory: No such file or directory`). Generic filesystem /
  subprocess calls may still add local context, but when a domain
  error already carries the operation and path, use `?` and let
  the typed error flow up.
- **Codes are stable user-facing API.** Renaming
  `cabin::workspace::manifest_not_found` is a breaking change
  for any tooling that grep-matches Cabin's stderr. Bump
  documentation alongside any rename.
- **Help text means actionable next action.** If there is no
  fix the user can take, omit `help(...)`. Don't write filler
  help.
- **Source-snippet diagnostics live in the owning crate.**
  `cabin-manifest::ManifestParseError` carries
  `#[source_code]` + `#[label]`. `cabin-diagnostics::render`
  picks them up. New parse / validation errors that have a
  source span must follow this pattern; do not construct
  `annotate-snippets` snippets in `cabin`.
- **Machine-readable stdout stays clean.** Diagnostics go to
  stderr through `render_error`; stdout remains parseable
  JSON for `cabin metadata`, `cabin tree --format json`,
  `cabin explain --format json`, etc.

## Where Cargo-inspired interface work belongs

Cabin's surface — subcommands, flags, config keys, env vars,
manifest names, help text — is *Cargo-inspired*, not
*Cargo-compatible*. Two pages are the social contract:

- [`docs/cargo-inspired-interface.md`](docs/cargo-inspired-interface.md)
  enumerates what is adopted, what is renamed for C/C++
  clarity, and what is intentionally not adopted.
- [`docs/environment-variables.md`](docs/environment-variables.md)
  is the single source of truth for read-side / run / test
  `CABIN_*` env vars.

Future changes must keep these invariants:

- Every `CABIN_*` env var name lives as a `pub const &str` in
  [`cabin-env`](crates/cabin-env/src/lib.rs). Adding a new var
  is a one-liner there plus a row in
  `environment-variables.md`. Do not introduce a `CABIN_*`
  string literal anywhere else.
- Read-side env-var precedence is `CLI flag > env > config >
  built-in default`. The single helpers
  `crate::config_glue::resolve_build_dir_with_env` and
  `crate::config_glue::effective_offline` are the only places
  the env layer is consulted; commands threading these flags
  must reuse them.
- `--target <triple>` is reserved for the future
  **platform / toolchain target** flag and is not accepted on
  any current command. Manifest-target selection is *not*
  exposed under a single flag: `cabin run` uses `--bin <name>`
  for `executable` targets, `cabin test` builds every
  `test` in the selected packages, and `cabin build` builds
  every default-buildable target in the selected packages.
  Users narrow the build / test scope by narrowing the package
  selection (`--package` / `--workspace` / `--exclude`). Do not
  re-introduce a manifest-target overload of `--target`; any
  future explicit-kind selector (`--example`, `--test <name>`,
  etc.) must use a distinct flag name so `--target` stays free
  for the platform-triple meaning.
- `--build-dir <dir>` is the primary build-output flag; the
  config key is `[paths] build-dir`; the env var is
  `CABIN_BUILD_DIR`. `--target-dir` does *not* exist as a
  Cabin alias.
- Default build directory is `build/`. Renaming the default
  requires updating the manpages, completions golden tests,
  README, and every doc that references it.
- Compile commands do **not** receive automatic
  `-DCABIN_PACKAGE_*` macros. Run / test executables receive
  the metadata as env vars instead. If a future change adds
  opt-in macro injection, it must (a) distinguish private
  target compile-defines from public usage-requirements, and
  (b) thread the macro values through the build-configuration
  fingerprint so a change invalidates the cache.
- Cargo / Rust commands explicitly *not* adopted (`cabin doc`,
  `cabin install`, `cabin search`, `cabin login` / `logout` /
  `owner` / `yank`, `cabin rustc` / `rustdoc` / `fix`,
  `cabin check`) require their own change before landing.

## Build-configuration fingerprint rules

`cabin_core::BuildConfiguration::fingerprint` is the canonical
hash over every build-affecting input. It is surfaced through
`cabin metadata` / `cabin explain` and is the value any future
on-disk artifact cache will key on. Future changes must
keep the fingerprint complete:

- A new build-affecting input must be folded into
  `compute_fingerprint` in the same commit that introduces it.
  Adding a field to `ResolvedProfileFlags` without extending the
  fingerprint is a regression the unit tests in
  `crates/cabin-core/src/config.rs::tests` are wired to catch
  (one `fingerprint_differs_when_*` test per field).
- The fingerprint must move when a flag changes language slot
  (`cflags` ↔ `cxxflags`), even when the argv string is
  identical. The `fingerprint_distinguishes_c_only_from_cxx_only_extra_args`
  test pins this contract.
- Inputs Cabin does not consume must not appear in the
  fingerprint. `CFLAGS`, `CXXFLAGS`, `LDFLAGS`, and `LD` are
  intentionally not consumed; the local absolute path to a
  config file is not an input either (only the resolved values
  the file contributed are). See `docs/toolchains.md` for the
  full table.
- Direct `ninja` invocation does not reload Cabin inputs;
  documentation must continue to direct users to `cabin build`
  after manifest / config / toolchain edits so the fingerprint
  and the generated commands stay in sync.

## Currently in scope

Anything that does not change the behavior of any existing
shipped feature is fair game inside the current scope's spec.
Anything that does change behavior, or that adds a new
feature, must be scoped to the explicit scope it belongs to and
must follow the architecture rules below.

The current canonical scope is documented in
[`docs/architecture.md`](docs/architecture.md) and must not be
duplicated here.

The deliberately-deferred list — items that are out of scope
until specifically scoped or until they are moved out of the
deferred band — is:

- cross-compilation (`--target <triple>` for the C/C++ build) —
  Cabin still evaluates `[target.'cfg(...)']` predicates against
  the host platform only;
- probe compilations beyond `--version`, distcc / icecc
  compile-server wrappers, and any remote build cache;
- SARIF / structured-diagnostic frameworks, sanitizer
  frameworks, coverage instrumentation and reporting, benchmark
  target kinds / harnesses, and broad CMake / Meson
  compatibility;
- Rust binary, test, or proc-macro targets, Rust-to-C++ target
  dependencies, and header generation (`cxx`, `autocxx`,
  `bindgen`);
- C++ modules;
- network-backed publish, package upload APIs, registry storage
  schema, non-local account / ownership / policy / control-plane
  / quota logic, and registry authentication;
- a Git repo index (intentionally never planned);
- exposing the underlying solver type from `cabin-resolver`.

Workspace graph algorithms must stay in `cabin-workspace`; CLI
flag parsing stays in `cabin`. Do **not** put workspace
discovery, member expansion, or selection resolution into
`cabin`.

See [`docs/architecture.md`](docs/architecture.md) for the full sequence
sequence and [`docs/architecture.md`](docs/architecture.md) for
the seams that future work must not cross prematurely.

## Implemented behavior (foundational capabilities)

The list below covers the foundational local surface that later
capabilities build on. Dependency kinds, optional dependencies,
features resolution, target conditions, profiles,
toolchain selection, capability detection, compiler-cache
wrappers, the typed config system, patch / source-replacement,
dev / test / example targets, vendoring + offline
mode, `cabin metadata` / `cabin tree` / `cabin explain`,
`cabin run`, and the Cargo-inspired `CABIN_*` env-var
foundation) are documented in their dedicated pages under
[`docs/`](docs/) and summarized in
[`docs/architecture.md`](docs/architecture.md).


- CLI commands `cabin init`, `cabin metadata`, `cabin build`,
  `cabin resolve`, `cabin update`, `cabin fetch`, `cabin package`,
  `cabin publish [--dry-run] [--registry-dir <path>]`,
  `cabin compgen`, `cabin mangen`. `resolve` / `fetch` / `build` /
  `update` accept either `--index-path <path>` (local file index)
  or `--index-url <url>` (sparse HTTP index).
- `cabin.toml` parsing with serde + `toml`, including string- and
  table-form versioned dependencies.
- Stable internal `Project` / `Dependency` model with
  `DependencySource::{Path, Version}`.
- C++ compiler / archiver / Ninja detection.
- Local workspace + path-dep loader producing a topologically-sorted
  `PackageGraph`, with optional registry-package stitching via
  `cabin-workspace::load_workspace_with_registry`.
- Backend-independent build graph IR with cycle detection.
- Cross-package target dependency resolution (works the same way for
  local and registry packages).
- `build.ninja` and `compile_commands.json` generation.
- Local C++ build execution via Ninja, including registry packages
  whose sources have been extracted into the artifact cache.
- Local JSON package index loader (`<package>.json`, schema 1) with
  optional `source = { type = "archive", path, format = "tar.gz" }`.
- Backtracking dependency resolver with deterministic output, yanked
  filtering, conflict diagnostics, and four `ResolveMode` variants:
  `PreferLocked`, `Locked`, `UpdateAll`, `UpdatePackage`.
- `cabin.lock` reader / writer / validator (schema `version = 1`,
  alphabetical package ordering, `deny_unknown_fields`, deterministic
  formatter).
- `cabin resolve --locked` / `--frozen` for non-mutating CI runs.
- `cabin update [--package <name>]` for refreshing the lockfile.
- `cabin metadata` includes lockfile contents when `cabin.lock` exists.
- `cabin fetch`: resolve, write/update the lockfile, verify SHA-256
  checksums, copy archives into the cache, and safely extract source
  trees, with `--cache-dir`, `--locked`, `--frozen`, and `--format`.
- `cabin build --index-path <path> [--cache-dir <path>] [--locked\|--frozen]`:
  same fetch pipeline plus a unified plan + Ninja invocation.
- `cabin package [--manifest-path <path>] [--output-dir <path>] [--format human\|json]`:
  validate the package, build a deterministic `.tar.gz`, hash it,
  generate canonical per-version metadata, and write both files into
  `--output-dir` (default `dist/`). Re-running with identical input
  succeeds silently; existing on-disk artifacts with different bytes
  fail loudly.
- `cabin publish --dry-run [--manifest-path <path>] [--output-dir <path>] [--format human\|json]`:
  same pipeline, plus a "no registry was modified" report.
- `cabin publish --registry-dir <path> [--manifest-path <path>] [--format human\|json]`:
  publish the staged package into a local file registry. Initializes
  the layout (`config.json`, `packages/`, `artifacts/`) on first
  use; rejects duplicate versions and orphaned artifacts. The
  registry is then consumable by `cabin resolve`, `cabin fetch`,
  and `cabin build --index-path <path>`.
- `cabin publish --dry-run --registry-dir <path>`: validate every
  pre-write check against the registry without mutating it.
- `cabin publish` without `--dry-run` and without `--registry-dir`:
  exits with a clear error.
- `cabin <resolve|fetch|build|update> --index-url <url>`: read
  the registry over static HTTP. Mutually exclusive with
  `--index-path`. `--frozen --index-url` fails with a documented
  error because there is no persistent HTTP metadata cache.
- `cabin compgen <shell> [--output-dir <path>]` /
  `cabin compgen --all --output-dir <path>`: emit shell completion
  scripts (bash / zsh / fish / powershell / elvish) derived from
  the clap command tree.
- `cabin mangen [--output-dir <path>]`: emit `cabin(1)` plus one
  `cabin-<sub>(1)` per top-level subcommand, including hidden
  distribution / machine-interface commands. The root `cabin(1)`
  page mirrors normal help and omits hidden commands. Output is
  ROFF produced by `clap_mangen` — no hand-written man pages.
- Features foundation: `[features]` manifest table;
  `BuildConfiguration` selection model with deterministic SHA-256
  fingerprint; `--features` / `--all-features` /
  `--no-default-features` on `cabin build` and `cabin metadata`;
  declarations preserved in `cabin package` metadata, file-registry
  publish, and HTTP / file index round-trips. Older index entries
  that omit the field keep loading. Full protocol in
  [`docs/features.md`](docs/features.md).
- Advanced workspace semantics: `[workspace]` with `members`
  (paths or trailing-`*` globs), `exclude`, `default-members`, and
  `[workspace.dependencies]` shared by `dep = { workspace = true }`
  member entries. Cabin walks upward from the current directory to
  discover workspace roots; nested workspaces are rejected. The
  `--workspace` / `-p / --package` / `--default-members` /
  `--exclude` selection-flag bundle works on every workspace-aware
  command. `cabin metadata` reports `workspace.members`,
  `default_members`, `excluded_members`, and `selected_packages`
  (all sorted). `cabin package` and `cabin publish` against a
  workspace root require exactly one `--package <name>` selection.
  Full protocol in [`docs/workspaces.md`](docs/workspaces.md).

## Workspace layout

```
crates/
  cabin-artifact/             source-archive cache, checksum verifier, extractor
  cabin-build/                backend-independent build graph planner
  cabin/                      `cabin` binary, command dispatch
  cabin-config/               typed `.cabin/config.toml` discovery + merge
  cabin-core/                 stable internal data model
  cabin-diagnostics/          user-facing diagnostic presentation + annotate-snippets boundary
  cabin-env/                  CABIN_* env-var names + run/test env builder
  cabin-explain/              typed model for `cabin tree` / `cabin explain`
  cabin-feature/              cross-package feature resolver
  cabin-fmt/                  clang-format runner used by `cabin fmt`
  cabin-fs/                   shared low-level filesystem helpers
  cabin-index/                local JSON package index loader
  cabin-index-http/           sparse HTTP index client (read-only)
  cabin-lockfile/             cabin.lock reader / writer / validator
  cabin-manifest/             cabin.toml parsing
  cabin-ninja/                build.ninja + compile_commands.json writers
  cabin-package/              deterministic source-archive + canonical metadata writer
  cabin-port/                 foundation-port recipe parser + preparation pipeline
  cabin-publish/              publish-workflow orchestration
  cabin-registry-file/        local file-registry layout, atomic writes, lock
  cabin-resolver/             dependency resolver with lockfile-aware modes
  cabin-source-discovery/     shared C / C++ source walker for fmt / tidy
  cabin-system-deps/          pkg-config probing for `system = true` deps
  cabin-test/                 test-target plan + sequential runner
  cabin-tidy/                 run-clang-tidy runner used by `cabin tidy`
  cabin-toolchain/            C/C++ compiler / archiver / Ninja detection + wrappers
  cabin-vendor/               typed VendorPlan + file-registry materialiser
  cabin-workspace/            local + registry package graph loader, patches, selection
docs/
  architecture.md              crates, data flow, current direction
  artifacts.md                 source archive + cache layout
  cargo-inspired-interface.md  Cabin-vs-Cargo audit / classification
  compiler-cache.md            ccache / sccache wrappers
  config.md                    .cabin/config.toml schema and discovery
  dependency-kinds.md          two dependency kinds + activation rules
  distribution.md              shell completions + man pages
  environment-variables.md     CABIN_* read / run / test env vars
  features.md                  features foundation
  index.md                     local JSON index format
  lockfile.md                  cabin.lock format reference
  manifest.md                  cabin.toml schema reference
  metadata-tree-explain.md     `cabin metadata` / `cabin tree` / `cabin explain`
  package-format.md            package archive + canonical metadata schema
  patch-overrides.md           patch / override + source-replacement layer
  profiles.md                  build profile model
  registry-design.md           design-only registry direction
  system-dependencies.md       `system = true` deps + pkg-config probing
  target-dependencies.md       target/platform-specific dependency conditions
  targets.md                   target kinds + manifest target model
  testing.md                   cabin test runner + workflow
  toolchains.md                C/C++ tool selection + capability detection
  vendoring-offline.md         cabin vendor + offline mode
  workspaces.md                workspace discovery, member selection, inheritance
```

This repository is the **public local OSS core only**. Non-local
registry, account, ownership, policy, control-plane, and
infrastructure surfaces do not live here, and no code, fixture, or
test in this repository should add them.

## Crate boundaries to preserve

- `cabin-core` owns the stable domain model: `Project`,
  `Target`, `Dependency`, and the build-configuration model
  (`Features`, `SelectionRequest`,
  `BuildConfiguration` with deterministic SHA-256 fingerprint).
  Must not depend on `clap`, parse TOML, know about Ninja, know
  about resolver internals, know about lockfile TOML, invoke
  processes, or read / write registry index files directly.
  Generic filesystem helper policy lives in `cabin-fs`.
- `cabin-fs` owns small filesystem helpers shared by Cabin's
  production crates: atomic file replacement and lexical
  path-safety predicates. Intentionally narrow rather than a
  broad filesystem abstraction. Must not own manifest parsing,
  config-file discovery, XDG base-directory resolution, registry
  layout, the package archive format, archive extraction policy
  (that lives in `cabin-artifact`), resolver behavior, CLI
  behavior, diagnostics rendering, or shell / Ninja escaping.
  The helpers do not canonicalize, follow symlinks, read the
  filesystem, or create parent directories; callers own
  parent-directory creation and domain-specific error mapping so
  the destination path stays visible in the surfaced diagnostic.
- `cabin-manifest` owns `cabin.toml` parsing. Raw serde structs stay
  private. Must not load workspaces, run resolution, write Ninja, or
  read / write `cabin.lock`.
- `cabin-workspace` owns local package and path-dep loading,
  workspace root discovery (upward walk from cwd that errors when
  two or more `[workspace]`-bearing manifests stack above the
  start path), member globbing + exclude filtering,
  default-member validation, workspace dependency inheritance,
  nested-workspace rejection, the `PackageSelection` model, the
  `ResolvedSelection::closure(graph)` walk over local
  path-dependency edges, `collect_closure_versioned_deps`, and
  selection-aware registry materialization
  (`load_workspace_with_registry_for_selection`).
  Versioned dependencies are preserved on each `Project` for the
  resolver but not traversed here. Must not run the resolver,
  write Ninja, fetch artifacts, or parse CLI flags directly.
  Workspace graph algorithms — closure walks, versioned-dep
  aggregation, nested-workspace detection — must stay in
  `cabin-workspace` rather than `cabin`.
- `cabin-index` owns the local JSON index loader. Must not run
  the resolver, fetch artifacts, or read / write `cabin.lock`.
  The HTTP sibling lives in `cabin-index-http`.
- `cabin-resolver` owns dependency resolution. Cabin's resolver uses
  PubGrub internally, while exposing Cabin-owned resolver inputs,
  outputs, and diagnostics. A private adapter translates
  `semver::VersionReq` into PubGrub's `Ranges<semver::Version>`,
  implements `DependencyProvider` against `cabin_index::PackageIndex`,
  and handles yanked filtering, locked-mode preferences, optional /
  conditional edges, and candidate ordering. `ResolveError` implements
  `miette::Diagnostic` directly; conflict failures collapse PubGrub's
  derivation tree into a deterministic, human-readable explanation
  embedded in `ResolveError::Conflict`. Lockfile errors stay specific
  so users can tell whether to update the lockfile, fix constraints,
  or investigate a checksum mismatch. Must not expose PubGrub types in
  its public API, read / write `cabin.lock` directly, fetch artifacts,
  or render diagnostics itself.
- `cabin-lockfile` owns the `cabin.lock` model and I/O. Must not run
  the resolver, load indexes, parse `cabin.toml`, or fetch artifacts.
- `cabin-artifact` owns the source-archive cache. SHA-256
  verification, fail-closed `.tar.gz` extraction, and the
  checksum-addressed cache layout. Must not run the resolver, write
  Ninja, invoke C++ compilers, implement networking, or implement
  publishing.
- `cabin-package` owns deterministic source-archive creation and
  canonical per-version metadata generation. Must not mutate any
  registry, run the resolver, fetch artifacts, invoke C++ compilers,
  or implement networking.
- `cabin-publish` owns publish-workflow orchestration. It calls
  `cabin-package` for staging and `cabin-registry-file` for the
  actual file-registry mutation. HTTP / OCI publish and any
  server-side functionality stay out of scope.
- `cabin-registry-file` owns the local file-registry layout
  (`config.json`, `packages/`, `artifacts/`), the per-package index
  file format, atomic artifact + index writes (via the
  `atomic-write-file` crate's sibling-temp + rename), and the
  simple `.cabin-registry.lock` lock file. It must not parse
  arbitrary `cabin.toml`s, run the resolver, build packages, or
  implement networking.
- `cabin-index-http` owns the read-only sparse HTTP index client.
  Wraps `ureq::Agent` for blocking `GET` requests; validates
  `<base>/config.json`; fetches `<base>/packages/<name>.json`;
  resolves `source.path` values into absolute URLs against the
  package metadata URL; downloads source archives. Must not
  publish, authenticate, follow redirects to alternate registries,
  or persist a metadata cache. The artifact bytes it downloads are
  handed to `cabin-artifact` as in-memory bytes so the artifact
  layer stays HTTP-free.
- `cabin-toolchain` owns C++ toolchain detection. Must not parse
  TOML, run resolution, write lockfiles, or invoke the tools it
  locates.
- `cabin-build` owns backend-independent build graph planning.
  Must not write Ninja syntax, invoke Ninja, or parse TOML
  directly.
- `cabin-ninja` owns Ninja file generation and
  `compile_commands.json` generation. Must not parse TOML, resolve
  packages, or know about the resolver or the lockfile.
- `cabin` owns CLI parsing and command orchestration. May
  call any other crate. Must not contain business logic that
  belongs in a reusable crate; keep argument parsing separate
  from command execution where practical. The `compgen` (via
  `clap_complete`) and `mangen` (via `clap_mangen`) generators
  under `crates/cabin/src/{completions.rs,manpages.rs}`
  consume `Cli::command()` directly; do not duplicate command
  names, flags, or descriptions in either generator.

  **`cabin/src/cli.rs` must not grow further with new
  business logic.** When a future change adds new behavior, the
  implementation belongs in the owning crate (e.g.
  `cabin-workspace`, `cabin-resolver`, `cabin-build`,
  `cabin-publish`), exposed through a typed API; the CLI layer
  should only translate clap inputs into that API and render
  the result. New top-level commands or any non-trivial command
  logic should land in a per-command module under
  `cabin/src/cli/` rather than in `cli.rs`. A small,
  behavior-preserving split of view structs or dispatch
  helpers is acceptable inside a routine PR; a broad rewrite of
  `cli.rs` is not in scope for a routine change.

The architecture rules above mirror those in
[`docs/architecture.md`](docs/architecture.md). When the two ever
disagree, the architecture document is canonical.

## Repository content policy

This repository is a project-level technical codebase. Contributors
must:

- keep all repository content (documentation, code, comments, tests,
  test fixtures, examples, commit messages) at the project level;
- not implement network-backed publish, package upload over the
  network, OCI / GHCR transports,
  release-packaging workflows, Homebrew formulas, package yanking,
  ownership, account handling, or persistent HTTP metadata caches
  in this repository — those surfaces belong outside the public
  local OSS core;
- not add tests that depend on external internet access —
  sparse-HTTP tests must boot a local `tiny_http` server on
  `127.0.0.1:0`;
- not add non-local account, ownership, policy, quota,
  control-plane, or infrastructure logic, fixtures, or
  documentation. These surfaces are out of scope for this
  repository regardless of the current scope;
- keep artifact fetching (`cabin-artifact`) separate from resolver
  internals (`cabin-resolver`) and from the index loader
  (`cabin-index`);
- keep package archive logic in `cabin-package`, publish workflow
  orchestration in `cabin-publish`, and file-registry mutation in
  `cabin-registry-file`; the CLI must not contain archive,
  metadata, or registry-write logic;
- keep publish dry-run separate from any actual registry mutation —
  the dry-run path must remain a no-op against any registry;
- treat the clap command tree as the single source of truth for
  shell completions and man pages; `cabin compgen` must use
  `clap_complete::generate` against `Cli::command()` and
  `cabin mangen` must use `clap_mangen::Man` against the same
  tree;
- keep archive extraction safe: reject absolute paths, `..`
  components, symlinks, hard links, and any tar entry type that is
  not a regular file or directory;
- keep package archive creation deterministic: sorted file
  enumeration, zeroed mtimes / uid / gid / uname / gname, and a
  gzip header with `mtime = 0` plus an `os = 0xff` (unknown) byte;
- treat future external-service work as outside this repository
  unless architecture explicitly moves it into scope.

## Do not implement "not implemented" features

Cabin's product surface is what is actually implemented today.
If a feature is not currently supported, it must not exist in
public syntax, CLI flags, domain models, docs, fixtures, or
tests merely to say that it is unsupported.

This rule is normative: future agents must read it before
extending the parser, the CLI, the docs, or the test suite.

- **Prefer generic unknown-syntax diagnostics.** Unknown manifest
  tables, unknown manifest fields, unknown target kinds, unknown
  CLI subcommands, and unknown CLI flags must reach the generic
  `deny_unknown_fields` / clap unknown-flag path. Do not add a
  feature-named arm in a parser or CLI just to emit a
  feature-named error.

- **Feature-specific rejection only protects current invariants.**
  A feature-specific rejection is acceptable only when it
  protects a current invariant, a security property, a
  reproducibility guarantee, or an implemented public contract.
  Examples that pass this bar: rejecting `system = true` combined
  with `path` on the same dependency table (current dependency
  grammar invariant); rejecting an unknown `compiler-wrapper`
  value (a typed enum on a supported field); refusing a URL
  index source under `cabin vendor` (byte-stable output
  invariant). Examples that do **not** pass this bar: rejecting
  `[variants]`, `[options]`, `[build-dependencies]`,
  `[tool-dependencies]`, `[lint.*]`, `cpp_bench`, `rust_*`
  targets, `--coverage`, `--option`, `--variant`, `git` /
  `registry` / `source` dependency keys, `git` / `url` /
  `version` patch keys, weak-feature `dep?/feature` syntax, or
  any other speculative future surface.

- **Current behavior docs describe implemented behavior only.**
  Manifest, command, and example docs must not document removed
  or speculative future features as "not yet supported", "future
  work", "reserved", or similar. Brief roadmap-style mentions
  are confined to architecture / roadmap documents and must not
  define public syntax for unimplemented features.

- **No reserved public schema.** Manifest fields, CLI flags,
  metadata fields, registry schema, and lockfile fields must not
  exist solely to be parsed and rejected.

- **No placeholder / stub / TODO surface.** Do not add public
  syntax, enum variants, struct fields, golden outputs, or
  fixtures for a feature that is not currently implemented. The
  internal model must not carry a variant whose only consumer is
  a feature-named rejection arm.

- **Removing a feature removes its bloat.** When a feature is
  removed, also remove its dedicated unsupported diagnostics,
  feature-named negative tests, docs, examples, fixtures, golden
  outputs, metadata fields, and internal-model variants — unless
  a generic validation test still needs to cover the shape.

- **Replace feature-specific negative tests with generic ones.**
  Coverage for unknown manifest tables, unknown fields, unknown
  target kinds, and unknown CLI flags should use one
  representative sentinel value (e.g.\ `not-a-real-table`,
  `not-a-real-key`, `wasm_executable`), not a real
  removed-or-future feature name.

If a contributor or AI agent finds themselves writing a test or
diagnostic whose name is `*_is_rejected` and whose body is
"`<future-feature-name>` is unsupported", stop. Use the generic
unknown-syntax path instead. If the feature is genuinely needed,
implement it; if not, do not surface it.

## Required checks

Before submitting any change, run:

```sh
cargo fmt --all --verbose -- --check
taplo fmt --check
typos
cargo clippy --workspace --all-targets --all-features --locked --verbose -- -D warnings
cargo check --workspace --all-targets --locked --verbose
cargo test --workspace --all-targets --all-features --locked --verbose -- --show-output
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked --verbose

# Conventional-commit lint of the commits this branch adds.
# Mirrors CI's @commitlint/config-conventional gate; every commit
# header must be a valid conventional commit and stay <= 100 chars.
npx --yes --package @commitlint/cli --package @commitlint/config-conventional \
  commitlint --extends @commitlint/config-conventional --from origin/main --to HEAD --verbose
```

CI runs the Rust commands above and treats warnings as errors.
A separate CI job runs the `commitlint` command above (via
`@commitlint/config-conventional`) and rejects any commit whose
message is not a valid conventional commit.
Mirror the flags verbatim — in particular `--all-features` on
both `cargo clippy` and `cargo doc` (cabin gates several
modules behind features, and dropping the flag hides lints and
broken intra-doc links that CI still fires on), the trailing
`-- -D warnings` on `cargo clippy` (the `clippy::pedantic` group
is denied workspace-wide via `[workspace.lints]` in the root
`Cargo.toml`, so it no longer needs a command-line flag), and the
`RUSTDOCFLAGS="-D warnings"` environment variable on
`cargo doc`. Skipping any of those locally lets PRs fail in CI
on lints or doc warnings that did not appear in the local run.

The repository's `typos.toml` pins the project locale to American
English; do not modify it (including adding new `extend-words`
entries) unless a reviewer explicitly asks for the change. If
`typos` flags a spelling, fix the offending occurrence instead of
allowlisting it.
CI installs `ninja`, C/C++ compilers, `clang-format`,
`run-clang-tidy`, and `pkg-config` so the real external tool
smoke tests run by default. Set
`CABIN_SKIP_EXTERNAL_TOOL_TESTS=1` only for local runs that
should exercise the bundled fake-tool fallback.

## Commit messages

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs
`commitlint` with `@commitlint/config-conventional` against every
commit on a PR (and against `HEAD` on `main`). Each commit subject
must therefore:

- follow [Conventional Commits](https://www.conventionalcommits.org/)
  (`<type>(<scope>)?: <subject>`), where `<type>` is one of `build`,
  `chore`, `ci`, `docs`, `feat`, `fix`, `perf`, `refactor`, `revert`,
  `style`, or `test`;
- keep the subject in lower case (the conventional ruleset rejects
  `sentence-case`, `start-case`, `pascal-case`, and `upper-case`
  subjects);
- stay at or under 100 characters total (header-max-length).

Body and footer lines, if any, must also stay at or under 100
characters per line. Run a quick `git log -1 --format=%s | wc -c`
before pushing — commitlint failures block CI and there is no opt-out.

## Keeping docs, AGENTS.md, and the website in sync

Cabin's user-facing surface lives in three places that drift apart
quickly if a change updates only one of them: the per-area pages under
[`docs/`](docs/), this file, and the website under
[`website/`](website/) (deployed to `cabinpkg.com`), which also
renders the `docs/` pages at `cabinpkg.com/docs/`. The website is not
auto-regenerated from the Rust crates, so a change that shifts
Cabin's positioning, supported languages, supported platforms, or
top-level command surface must update the website in the same PR.

Before opening a PR, walk the checklist below and update every page
the change touches:

- **Language scope** (e.g. adding or removing a target language,
  changing the C/C++ standard story, dropping a platform): update
  `website/src/pages/index.astro`, `website/src/components/Footer.astro`,
  `website/src/layouts/BaseLayout.astro`,
  `website/src/lib/constants.ts`, and `website/README.md`, plus
  any `docs/` page that names the scope.
- **Marketing copy** (taglines, feature-card descriptions, hero
  text, "Why Cabin" bullets): only lives under `website/src/`.
- **Top-level command surface or feature names** visible on
  `cabinpkg.com` (install instructions, dependency-declaration
  snippet, package-detail badges): update
  `website/src/components/package/InstallSnippet.astro` and the
  `website/src/components/package/` neighbors alongside the
  corresponding `docs/` page.
- **Architecture or behavior** that contradicts an existing
  rule in this file: update the rule in `AGENTS.md` and the
  matching `docs/` page in the same commit.

If you cannot reach into `website/` (e.g. the website lives in a
separate deploy pipeline you do not own), still call out the
website-touching scope in the PR description so the website
maintainer can land a follow-up before the change reaches users.

## Docs CI

The canonical docs live under [`docs/`](docs/) and render through the
Astro website at `cabinpkg.com/docs/` — there is no separate MkDocs
build. [`.github/workflows/website.yml`](.github/workflows/website.yml)
builds the whole site (docs included) on every push and PR. Before
submitting a change that touches `docs/`, build the site locally from
`website/`:

```sh
cd website
yarn build   # typecheck, build, CSP check, and docs-link check
```

Two rules the docs build enforces:

- **Every page in `docs/` must be listed in the sidebar nav**
  (`website/src/lib/docsNav.ts`). The build's `assertDocsNavMatches`
  guard fails if a `docs/*.md` page is missing from the nav, or a nav
  entry has no matching page.
- **Cross-repo file references must use absolute GitHub URLs.**
  Relative `*.md` links are rewritten to `/docs/<slug>/`, so a `.md`
  target that is not a sibling docs page (e.g. `../crates/...`) does
  not resolve and is flagged by the `verify:docs-links` check. Use
  `https://github.com/cabinpkg/cabin/blob/main/<path>` (or
  `tree/main/<path>` for a directory) — see
  [`docs/environment-variables.md`](docs/environment-variables.md)
  for an example.

Docs render via an Astro content collection (`docs` in
`website/src/content.config.ts`) and the
`website/src/pages/docs/[...slug].astro` route. See
[`website/AGENTS.md`](website/AGENTS.md) for details.

## Where to extend later

Future crates should depend on `cabin-core` (plus other lower-level
crates) rather than reaching across layers. The
[`docs/architecture.md`](docs/architecture.md) "Future monorepo
direction" section sketches the intended shape; new crates appear
when needed.
