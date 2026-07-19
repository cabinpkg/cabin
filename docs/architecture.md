# Architecture

This document describes the Cabin workspace, the responsibilities of each crate, the data flow for
the currently implemented behavior, and the planned shape of deferred layers.  The codebase is
organized as small crates with narrow ownership boundaries; the notes below describe which crate
owns each implemented surface and where deferred work should land.

The currently implemented surface, layered briefly: first-class dependency kinds (`normal` / `dev`),
advanced workspace semantics, the local C/C++ build, the Cabin-owned resolver layered on PubGrub,
the lockfile, the content-addressed source-archive cache, the local file registry, the read-only
sparse-HTTP index client, features with a cross-package feature resolver and the documented
foundation limits, target / platform-specific dependencies, build profiles, typed toolchain
selection with capability detection, `ccache` / `sccache` wrapper integration, the typed
`.cabin/config.toml` system, patch / override and source replacement, the dev / test / example
target kinds plus `cabin test`, vendoring + `--offline`, `cabin metadata` / `cabin tree` / `cabin
explain`, the Cargo-inspired interface foundation (`cabin run`, the `cabin-env` crate), `cabin
check`, `cabin fmt` / `cabin tidy`, `pkg-config`-driven ``system = true` deps`, `CPPFLAGS` /
`CFLAGS` / `CXXFLAGS` / `LDFLAGS` ingestion, `-j` / `--jobs` build / run / tidy parallelism, `cabin
new --bin` / `--lib` scaffold parity, `cabin version` plus `cabin --list`, `cabin add` / `cabin
remove` manifest editing, `cabin clean`, and the curated foundation-port layer - bundled,
version-pinned recipes for upstream C/C++ libraries under
[`crates/cabin-port/ports/`](https://github.com/cabinpkg/cabin/tree/main/crates/cabin-port/ports/)
(see [`foundation-ports.md`](foundation-ports.md)).

See [`dependency-kinds.md`](dependency-kinds.md) for the dependency-kind protocol and command
behavior, [`registry-design.md`](registry-design.md) for the registry direction (including the
file-registry layout that the sparse HTTP client consumes), [`artifacts.md`](artifacts.md) for the
source-archive layout, [`package-format.md`](package-format.md) for the package archive + canonical
metadata schema, [`distribution.md`](distribution.md) for the shell-completion and man-page
surfaces.

## Repository shape today

```
crates/
  cabin-core/        stable internal data model
  cabin-manifest/    cabin.toml parsing
  cabin-config/      typed `.cabin/config.toml` discovery + merge
  cabin-toolchain/   C/C++ compiler / archiver / Ninja detection + wrappers
  cabin-workspace/   local + registry package graph loader, patches, selection
  cabin-feature/     cross-package feature resolver
  cabin-build/       backend-independent build graph planner
  cabin-driver/      compiler-dialect lowering of the build IR (GCC/Clang vs MSVC)
  cabin-ninja/       build.ninja + compile_commands.json writers
  cabin-index/       local JSON package index loader
  cabin-resolver/    dependency resolver (PubGrub-backed) with lockfile-aware modes
  cabin-lockfile/    cabin.lock reader / writer / validator
  cabin-artifact/    source-archive cache, checksum verifier, extractor
  cabin-package/     deterministic source-archive + canonical metadata writer
  cabin-port/        foundation-port recipe parser + preparation pipeline
    ports/           curated foundation-port recipes, embedded at build time
      README.md      foundation-port policy + retirement plan
      <name>/<version>/  one recipe directory per bundled port
  cabin-publish/     publish-workflow orchestration
  cabin-registry-file/ local file-registry layout, atomic writes, lock
  cabin-index-http/  sparse HTTP index client (read-only)
  cabin-credentials/ registry token storage (credentials.toml, -Z remote-registry)
  cabin-registry-api/ remote registry API client (publish / yank, -Z remote-registry)
  cabin-registry-verify/ hosted-registry archive verifier (verification lifecycle)
  cabin-vendor/      typed VendorPlan + file-registry materialiser
  cabin-test/        test-target plan + sequential runner
  cabin-explain/     typed model for `cabin tree` / `cabin explain`
  cabin-fs/          shared low-level filesystem helpers
  cabin-diagnostics/ user-facing diagnostic presentation + miette rendering boundary
  cabin-env/         CABIN_* env-var names + run/test env builder
  cabin-source-discovery/ shared C/C++ source walker for fmt / tidy
  cabin-fmt/         clang-format runner used by `cabin fmt`
  cabin-tidy/        run-clang-tidy runner used by `cabin tidy`
  cabin-system-deps/ pkg-config runner used by ``system = true` deps`
  cabin/             `cabin` binary, command dispatch
docs/
  architecture.md    this file
  manifest.md        cabin.toml schema reference
  index.md           local JSON index format
  lockfile.md        cabin.lock format reference
  artifacts.md       source archive + cache layout
  package-format.md  package archive + canonical metadata schema
  distribution.md    shell completions + man pages
  installation.md    supported platforms + install methods
  registry-design.md local registry interface boundary
  remote-registry.md experimental remote-registry protocol contract (behind -Z remote-registry)
  features.md        features foundation
  workspaces.md      workspace root discovery, member selection, inheritance
  metadata-tree-explain.md  `cabin metadata` / `cabin tree` / `cabin explain`
  cargo-inspired-interface.md  Cabin-vs-Cargo audit / classification
  environment-variables.md  CABIN_* read-side / run / test env vars
  check.md           `cabin check` (syntax-only type-check)
  fmt.md             `cabin fmt` (clang-format)
  tidy.md            `cabin tidy` (run-clang-tidy)
  system-dependencies.md  ``system = true` deps` and pkg-config
  new-and-init.md    scaffold semantics for `cabin new` / `cabin init`
  testing.md         `cabin test` runner
  targets.md         target kinds, `test` / `example`
  language-standards.md  per-target C/C++ standard + interface declarations
  toolchains.md      typed toolchain selection, capability detection
  config.md          `.cabin/config.toml` schema, discovery, precedence
  profiles.md        build profile model, inheritance, fingerprint inputs
  compiler-cache.md  compiler-wrapper integration
  vendoring-offline.md  `cabin vendor` and `--offline` semantics
  dependency-kinds.md  two dependency kinds (normal/dev)
  target-dependencies.md  `[target.'cfg(...)'.<kind>]` predicates
  patch-overrides.md  patch / override / source replacement
  package-index.md   package index schema
  foundation-ports.md  curated foundation-port recipes + `cabin port list`
  design/
    standard-compatibility/
      spec.md            normative resolver-level standard-compatibility model
      registry-index.md  standard metadata in the package index (design)
      publish-lints.md   publish-time standard-compatibility lints (design)
      preference-mode.md standard-aware version preference (implemented)
```

## Crate responsibilities and rules

The split is by **responsibility**, not by feature.  Each crate has a narrow public surface; future
work adds new crates rather than widening existing ones.

### `cabin-core`

Stable, format-agnostic types: `Package`, `Target`, `TargetKind`, `PackageName`, `TargetName`,
`Dependency`, `DependencySource::{Path, Version}`, plus the build-configuration model - `Features`,
`SelectionRequest`, and `BuildConfiguration` (with a deterministic SHA-256 fingerprint that now also
includes the selected profile's relevant fields and the effective language standards).  The cfg /
target-condition AST also lives here as `Condition`, `ConditionKey`, and `TargetPlatform`, the
build-profile model lives here as `ProfileName`, `OptLevel`, `BuiltinProfile`, `ProfileDefinition`,
`ProfileSelection`, `ResolvedProfile`, `ProfileSource`, and `resolve_profile`, and the typed
language-standard model lives here as `cabin_core::language_standard` (`CStandard`, `CxxStandard`,
`LanguageStandard`, the `gnu-extensions` boolean, the `{ min, max }` interface-requirement types,
the resolution / interface / relevance helpers, the conflict and contradiction detectors, and the
per-standard compiler capability tables in `cabin_core::compiler`).  The resolver-level
standard-compatibility core of `docs/design/standard-compatibility/spec.md` lives next to it as
`cabin_core::standard_compatibility` (the requirement chain and join, `ReqOf`, edge compatibility,
and package-version viability, each item citing the spec identifier it implements; the
graph-composition recursion `R_L` is a graph algorithm and therefore lives in
`cabin_workspace::standards`, not here).  Manifest, index, lockfile,
resolver, build, and feature crates all share these typed values without depending on each other.
The crate must:

- not depend on `clap`;
- not parse TOML or any other on-disk format;
- not know about Ninja, the build graph, the resolver, the lockfile, or any registry / index
  transport;
- not invoke processes;
- stay reusable by client / server / shared tooling alike.

Generic filesystem helper policy lives in `cabin-fs`; `cabin-core` stays focused on typed domain
models and pure logic.

### `cabin-manifest`

Owns `cabin.toml` parsing.  Raw serde structs are private to the crate and converted to `cabin-core`
domain types at the boundary.  The crate must:

- not load workspaces or follow path dependencies;
- not run dependency resolution;
- not write Ninja;
- not read or write `cabin.lock`.

### `cabin-workspace`

Owns local package and workspace loading: workspace member globbing, recursive local path-dep
traversal, dedup-by-canonical-path, duplicate name detection, package cycle detection, and
topological ordering.  Versioned dependencies are preserved on each `Package` for the resolver but
are intentionally not traversed here.  Current invariants:

- Workspace discovery walks upward from the start path looking for a `cabin.toml` whose root
  declares a `[workspace]` table.  With zero or one such manifest the walk returns it (or `None`);
  with two or more stacked roots the walk errors with a `nested workspace detected` diagnostic so
  the caller is forced to disambiguate via `--manifest-path`.
- `[workspace]` expansion supports `members`, `exclude`, and `default-members`, plus workspace
  dependency inheritance via `dep = { workspace = true }`.
- A `PackageSelection` model turns CLI flags into a deterministic list of selected packages.
  `ResolvedSelection::closure(graph)` and `collect_closure_versioned_deps(graph, closure)` extend
  that selection over local path-dep edges so commands scoped to one member can still see the
  registry deps of the path-deps they pull in.
- Workspace loading exposes two registry-aware entry points: `load_workspace_with_registry` (strict
  - every versioned dep in the workspace must be resolved) and
  `load_workspace_with_registry_for_selection(manifest, registry, strict_packages)`.  The
  selection-aware variant is what the CLI calls when the user has scoped a command to a subset of
  the workspace: registry entries are required only for packages reachable from the selected
  closure, so unrelated workspace members' versioned deps are silently skipped during loading rather
  than being materialized into the package graph.
- `PackageName` accepts a bare `name` or a scoped `<scope>/<name>` with exactly one `/`.
  `cabin_core::is_path_safe_package_name` is the single authoritative *component* grammar: ASCII
  alphanumerics plus `_-.`, non-empty, not `.`/`..`, no leading dot.  It covers filesystem path
  components, sparse-HTTP URL path segments, and Windows-reserved filename characters in one rule,
  guarding bare names, the package part of a scoped name, `TargetName`, and - one URL segment at a
  time - the remote URL boundaries in `cabin-index-http` / `cabin-registry-api`.  The scope
  part uses `cabin_core::is_valid_package_scope`, the registry's GitHub-login-compatible scope
  grammar (lowercase alphanumerics plus interior `-`, at most 39 bytes) - a strict subset of the
  component grammar.  Both are enforced by `PackageName::new` so URL-reserved characters cannot
  reach `Url::join` through any code path.  The full name string is the one canonical identity
  (manifest, index, lockfile, resolver) and is never used as a single filesystem path component:
  path sinks fold `PackageName::path_components` (scope dir + name dir for scoped names), and
  filename/pkg-config/linker sinks use `base_name()` or the flattened `artifact_stem()`.  The
  diagnostic emitted on rejection echoes the offending name and describes the grammar.
- `cabin_workspace::standards` implements the effective-requirement recursion `R_L` of
  `docs/design/standard-compatibility/spec.md` (D10) over an already-resolved target graph: one
  topological pass per language over the public-edge subgraph, `O(|V| + |E|)` per language (spec
  T3(2)), folding the `ReqOf` values of `cabin_core::standard_compatibility`.  Alongside each value
  it records provenance - the chain of public edges to the declaration, header-only inference, or
  cross-language default that attains the join, carrying manifest paths and optional `miette` spans
  for later diagnostics.  The module does not resolve target references itself: `cabin-build` owns
  turning raw manifest `deps` into a concrete target graph, and `cabin-resolver` owns enumerating
  candidates; both hand this module resolved nodes and edges.  Its first consumer is
  `cabin_build::standard_compat`, the post-resolution compatibility check.

The crate must:

- not run the resolver or any other resolver algorithm;
- not write Ninja;
- not fetch artifacts;
- not parse CLI flags (the CLI builds `PackageSelection` values);
- own every workspace graph algorithm (closure walks, versioned-dep aggregation, nested-workspace
  detection) - none of these may live in `cabin`.

### `cabin-index`

Owns the local-filesystem JSON package index format and its loader.  The crate must:

- not run the resolver;
- not fetch artifacts;
- not read or write `cabin.lock`.

The sparse HTTP read path lives in `cabin-index-http`; `cabin-index` holds the local filesystem
loader.  Both feed the same typed index model, so downstream crates consume one shape regardless of
transport.

### `cabin-resolver`

Owns dependency resolution.  Cabin's resolver uses PubGrub internally, while exposing Cabin-owned
resolver inputs, outputs, and diagnostics (`ResolveInput`, `ResolveOutput`, `ResolvedPackage`,
`ResolvedSource`, `LockedVersion`, `ResolveMode`, `ResolveError`, `ResolverConstraint`); the PubGrub
crate is an implementation detail and never appears in the crate's public types.  A private adapter
translates `semver::VersionReq` into PubGrub's `Ranges<semver::Version>`, implements
`DependencyProvider` against `cabin_index::PackageIndex`, and handles yanked filtering, locked-mode
preferences, optional / conditional edges, and candidate ordering.

`ResolveError` implements `miette::Diagnostic` directly so dependency resolution failures are
rendered through Cabin's miette-based diagnostics layer.  Lockfile errors stay specific - the
resolver preserves `LockfileMissingPackage`, `LockedVersionMissing`, `LockedVersionYanked`,
`LockedVersionViolatesConstraint`, and `LockedChecksumMismatch` so users can tell whether to update
the lockfile, fix constraints, or investigate a checksum mismatch.  Conflict cases collapse
PubGrub's derivation tree into a human-readable explanation embedded in `ResolveError::Conflict {
package, detail }`.  The stable diagnostic code [`cabin_diagnostics::code::RESOLVER_ERROR`] is
attached to every variant.

The crate must:

- not expose PubGrub types in its public API;
- not read or write `cabin.lock` directly (the CLI bridges `cabin-lockfile` and `cabin-resolver`);
- not fetch artifacts;
- not render diagnostics itself (rendering lives in `cabin-diagnostics`).

### `cabin-lockfile`

Owns the `cabin.lock` model and I/O: TOML serialization, deterministic ordering, schema validation.
The crate must:

- not run the resolver;
- not load indexes;
- not parse `cabin.toml`;
- not fetch artifacts;
- not write Ninja.

### `cabin-artifact`

Owns the source-archive cache.  Given a checksum-and-path-bearing fetch plan, it copies archives
into a checksum-addressed cache, verifies SHA-256 along the way, safely extracts them into the same
cache, and validates that each extracted package's `cabin.toml` matches the resolved name and
version.  Registry package archives are `.zip`, extracted through `safe_extract_zip`; the crate also
owns a safe `.tar.gz` extractor (`safe_extract_tar_gz`) that the foundation-port layer uses for
upstreams whose release artifact is a tarball (zip upstreams reuse `safe_extract_zip`).  Both
extractors enforce the same fail-closed rules (decompression-bomb
caps, path-traversal protection, and symlink rejection - with an opt-in `skip_symlinks` mode that
skips symlink entries without materializing anything, used by the foundation-port layer for
upstream tarballs that carry convenience symlinks).  The crate must:

- not run the resolver;
- not write Ninja;
- not invoke C/C++ compilers;
- not implement networking;
- not implement publishing;
- reject every tar entry that is not a regular file or directory, every entry with `..` components
  or absolute paths, and every entry whose joined destination escapes the cache target;
- bound every extraction by explicit named limits - entry count, entry path length, per-entry and
  aggregate decompressed bytes, a whole-stream cap derived from the compressed size, and a separate
  budget for the tar framing and metadata records the reader buffers in memory;
- leave no partial state behind: source trees are built in a sibling scratch directory and renamed
  into place only after they extract and validate.

The lexical path-safety predicates that back the rejection above come from `cabin-fs`.
Archive-specific extraction policy - allowed tar entry types, GNU/PAX metadata handling, declared
`strip_prefix` matching, decompressed-size caps, and partial-file cleanup - stays in this crate.
The client-facing statement of these rules, and the reason the client may never delegate them to a
registry's own verifier, is [`package-format.md`](package-format.md#extraction-safety-contract).

### `cabin-package`

Owns deterministic source-archive creation and canonical per-version metadata generation.  Given a
single-package manifest, it validates the source tree, walks it under a fixed include / exclude
policy, writes a byte-deterministic `.zip`, hashes it with SHA-256, and emits a JSON metadata
document shaped like a future registry's `<package>.json` version entry.  The crate must:

- not mutate any registry;
- not run the resolver;
- not fetch artifacts;
- not invoke C/C++ compilers;
- not implement networking;
- reject path dependencies (path deps are not publishable);
- refuse to overwrite an on-disk archive whose bytes differ from what the current run would produce
  - identical bytes succeed silently.

### `cabin-index-http`

Owns the read-only sparse HTTP index client.  Wraps `ureq::Agent` for blocking `GET` requests; its
public surface is intentionally small:

- [`cabin_index_http::HttpClient`] - `get_bytes` and `download` helpers that map HTTP statuses
  (`404`, `401`/`403` for the experimental authenticated path, `402` for the hosted registry's
  read-side budget breaker, `5xx`) and transport errors to `IndexHttpError` variants;
- [`cabin_index_http::HttpIndex`] - opens a registry by fetching `<base>/config.json`, validates it,
  exposes `fetch_package(name) -> IndexEntry` and a transitive walker `load_package_index(roots) ->
  PackageIndex` that returns the same shape as the local file loader;
- [`cabin_index_http::RegistryAuth`] - a caller-supplied bearer credential scoped to one normalized
  origin (part of the experimental `-Z remote-registry` client, see
  [`remote-registry.md`](remote-registry.md)).  The token is attached only to requests on that exact
  origin and never over cleartext `http` beyond loopback hosts;
- [`cabin_index_http::fetch_login_url`] - `cabin login`'s advisory, always-unauthenticated probe of
  `config.json` for the `WWW-Authenticate` `Cabin login_url` challenge; every failure degrades to
  `None` so the probe can never block a login.

The crate must:

- not POST, PUT, or otherwise mutate a remote registry;
- not resolve credentials itself - it receives an optional `RegistryAuth` from the caller
  (`cabin`'s orchestration layer reads `cabin-credentials`), and must not implement
  alternate-server redirect handling;
- reject HTTP artifact URLs that resolve outside the package metadata origin or contain `userinfo`
  credentials;
- not persist a metadata cache (`--frozen` with an effective HTTP index URL therefore fails with a
  documented error message - there is no offline HTTP path);
- never reach into HTTP from the artifact layer - downloaded archive bytes are handed to
  `cabin-artifact` via [`FetchSource::InMemoryArchive`] so checksum verification + safe extraction
  stay HTTP-free.

### `cabin-credentials`

Owns registry credential storage for the experimental remote-registry client
([`remote-registry.md`](remote-registry.md)): the `credentials.toml` file in the user config home
(the same `CABIN_CONFIG_HOME` / `etcetera` resolution as the user-level `config.toml`), keyed by
normalized index origins, plus the `CABIN_REGISTRY_TOKEN` environment override and the redacting
`Token` newtype.  The crate must:

- never let token bytes surface through `Debug` / `Display` output or its own error messages;
- keep credentials out of `cabin-config` - the config parser continues to reject
  credential-shaped tables, and this crate reads only `credentials.toml`;
- write atomically (sibling temp file + rename) and, on Unix, create the file with mode `0600`,
  surfacing a warning (not an error) for an existing group/world-readable file;
- not perform HTTP; the client crates receive tokens as typed values from the orchestration layer.

### `cabin-registry-api`

Owns the typed HTTP client for the experimental remote-registry *mutations*
([`remote-registry.md`](remote-registry.md)): `PUT /api/v1/packages/<scope>/<name>/<version>`
with the crates.io-style length-prefixed metadata + archive frame, and
`PATCH /api/v1/packages/<scope>/<name>/<version>/yank`; the routes address scoped names only.
Requests target the API origin a registry's
`config.json` declares (`api`), carry the caller-supplied bearer token, and map the protocol's
status codes (`200` no-op, `201` created, `400`, `401`, `403`, `404`, `409`) plus the
`{"errors":[{"detail":"..."}]}` envelope into typed errors, degrading to the raw status when the
envelope is malformed.  The crate must:

- not stage, validate, or lint packages - it frames and ships bytes produced by `cabin-package` /
  `cabin-publish`;
- not implement read routes; `config.json`, package metadata, and artifact downloads stay in
  `cabin-index-http`;
- not resolve credentials itself - it receives an optional typed token from the orchestration
  layer, refuses cleartext `http` beyond loopback hosts, and never follows redirects;
- never let token bytes surface through errors or `Debug` output.

### `cabin-registry-verify`

Owns the hosted registry's external verifier: the hostile-archive inspection behind the
verification lifecycle ([`remote-registry.md`](remote-registry.md), "The verifier's checks").
Given a downloaded archive and the canonical metadata the admin listing reported, it hand-parses
the zip container and decompresses each entry under decompression caps, checks the structure
`cabin package` emits (the strict zip profile in `registry/docs/archive-format.md`), parses the
embedded manifest with `cabin-manifest`, and compares it against the metadata - rendering a
verdict plus machine-readable reason codes.  It also owns the pure name advisories the
workflow runs before any download, whose findings make it abstain - render no verdict at
all - rather than reject (`registry/docs/architecture.md`, "Name fidelity").  It lives in
the root workspace precisely to reuse
the real manifest parser and the real `cabin-package` metadata seams, but it is a client of the
hosted service: the `cabin-registry-verify` binary is invoked by the `registry-verify` GitHub
Actions workflow, never by users.  The crate must:

- never appear in the `cabin` binary's dependency graph;
- never extract an archive to disk; every check streams under the decompression caps (memory
  stays within a small constant factor of the cap; only the manifest entry is retained), and a
  cap violation is a rejection, never an OOM;
- reuse `cabin-package`'s seams instead of duplicating them: the publishability rules
  (`validate_publishable`) and the canonical-metadata derivation (`canonical_metadata`) are
  the same code publish runs, so producer and verifier cannot drift;
- not perform HTTP - the workflow downloads archives and PATCHes verdicts; the binary is pure
  local inspection (files in, JSON verdict out);
- treat archive-caused failures as verdicts and environment-caused failures as errors that
  leave the version pending (fail safe).

### `cabin-port`

Owns the foundation-port recipe layer: parsing `port.toml`, the checksum-addressed port cache, and
the source-preparation pipeline that turns a pinned upstream archive plus an overlay manifest into a
directory the workspace loader treats as a normal path dependency.  The crate must:

- never reach into HTTP - like `cabin-artifact`, it accepts archive bytes via a typed
  `PortFetchSource` (LocalArchive / InMemoryArchive); the HTTP path lives in `cabin`'s orchestration
  layer;
- never reimplement extraction safety.  Decompression-bomb caps, symlink handling, and
  path-traversal protection belong to `cabin-artifact::safe_extract_tar_gz` /
  `cabin-artifact::safe_extract_zip`; `cabin-port` picks the extractor from the `[source].url` path
  extension (`.zip` opts in, everything else is `.tar.gz`) and calls it with the declared
  `strip_prefix`, but does not duplicate the security rules.  Port extraction opts into skipping
  symlink entries (upstream tarballs commonly carry convenience symlinks; nothing is materialized
  for a skipped entry), while package archives keep the strict reject-symlinks default.

Foundation ports are local development policy, not published metadata: `cabin-package` rejects port
deps in its validator and `cabin-publish` never archives them.  See
[`foundation-ports.md`](foundation-ports.md) for the policy, the schema, and the zlib milestone.

### `cabin-publish`

Owns publish-workflow orchestration.  Combines `cabin-package`'s [`stage`] entry point with
`cabin-registry-file`'s atomic writers to publish a single-package source tree into a local file
registry.  The experimental remote publish path (behind `-Z remote-registry`) reuses the same
staging step and the shared lint seam (`staged_lint_warnings`) with a baseline fetched over the
sparse HTTP read path; its transport lives in `cabin-registry-api` and its orchestration in
`cabin`.  The crate must:

- not implement HTTP / sparse / OCI publish;
- not implement server-side functionality;
- keep registry mutation in `cabin-registry-file`;
- keep dry-run distinct from actual mutation - the dry-run path is a no-op against any registry;
- return a clear error when invoked without `--dry-run` and without `--registry-dir`.

### `cabin-registry-file`

Owns the local file-registry layout and the atomic writes that keep partially-written state from
sticking around.  Given a [`cabin_package::StagedPackage`] plus a registry root, it:

- creates the registry layout (`config.json`, `packages/`, `artifacts/`) on first publish;
- validates `config.json` (`schema = 1`, `kind = "file-registry"`, no `..` in `packages` /
  `artifacts`);
- detects duplicate versions and orphaned artifacts before any bytes are written;
- places the artifact and updates the per-package index file via atomic write + rename, rolling back
  the artifact if the index update fails;
- guards concurrent runs with a simple `<registry>/.cabin-registry.lock` lock file (best-effort -
  recovery from a crashed publisher is out of scope today);
- never parses arbitrary `cabin.toml`s, runs the resolver, builds packages, or implements
  networking.

### `cabin-toolchain`

Owns toolchain resolution, subprocess-based compiler / archiver detection, compiler-wrapper
resolution, and Ninja lookup.  The crate must:

- not parse TOML;
- not run dependency resolution;
- not read or write `cabin.lock`;
- not compile probe sources or write build plans.

### `cabin-build`

Owns backend-independent build planning: `PackageGraph` plus `ResolvedToolchain`, per-package build
flags, per-package language standards, and build settings become a `BuildGraph` of compile / archive
/ link actions, with pre-Ninja validation of requested standards against the detected toolchain and
interface-standard enforcement across the target dependency closure.  The crate must:

- not write Ninja syntax or any other backend's syntax;
- not invoke Ninja;
- not parse TOML directly.

### `cabin-driver`

Owns compiler-dialect lowering: the single boundary where `cabin-build`'s toolchain-independent
`BuildAction` IR becomes concrete command lines.  A `Dialect` names a command-line family -
`GnuLike` (the GCC / Clang driver: `-std=c++17`, `-D` / `-I` / `-isystem`, `-MD -MF` depfiles) or
`Msvc` (the `cl.exe` / `lib.exe` driver: `/std:c++17`, `/D` / `/I` / `/external:I`,
`/showIncludes`) - and owns every platform- and toolchain-specific spelling: artifact naming, Ninja
header-dependency discovery, and how each action is lowered.  The planner and the Ninja writer stay
dialect-agnostic.  The crate must:

- stay pure and deterministic - no I/O, no process invocation;
- not parse TOML;
- not plan builds or write Ninja syntax (it lowers actions; `cabin-ninja` serializes them).

### `cabin-test`

Owns the test plan and the sequential test runner used by `cabin test`.  Given a finished
[`cabin_build::BuildGraph`] and the originating [`cabin_workspace::PackageGraph`], it:

- builds a deterministic [`cabin_test::TestPlan`] of every `test` target whose linked executable
  appears in the graph's default outputs;
- runs each executable sequentially via [`cabin_test::run_tests`], capturing stdout / stderr through
  a [`cabin_test::TestOutputSink`] trait;
- returns a typed [`cabin_test::TestSummary`] (totals, per-test status) plus stable rendering
  helpers (`render_summary_line`, `render_result_line`, `render_running_line`).

The crate must:

- not parse manifests, plan builds, or resolve dependencies;
- not generate Ninja or invoke `ninja`;
- not know about config / patches / source replacement;
- not introduce parallel test execution, in-binary test discovery, or test-framework output parsing
  - those are documented limitations of the current model.

`cabin/src/cli/test.rs` orchestrates `cabin test` by driving the existing build pipeline and handing
the resulting `BuildGraph` to this crate.

### `cabin-ninja`

Owns Ninja file generation and Clang-compatible `compile_commands.json` generation.  The crate must:

- not parse TOML;
- not resolve packages;
- not know about the resolver or the lockfile.

### `cabin-explain`

Typed model for `cabin tree` and `cabin explain`.  Consumes the already-loaded `PackageGraph`,
optional `Lockfile`, optional `ActivePatchSet`, and the merged `SourceReplacementSettings`, and
produces:

- `TreeNode` forests (with `SourceProvenance`-tagged nodes, edge-kind labels, deduplicated repeats,
  deterministic sorting), rendered either as a Unicode-drawing tree or a structured JSON document;
- `Explanation` values (`Package`, `Target`, `Source`, `Feature`) plus a typed entry point for
  `BuildConfig` that reuses `BuildConfiguration::as_json` so metadata and explain agree on the same
  shape.

The crate must:

- not run the resolver, parse manifests, or plan builds;
- not perform I/O - the orchestration layer hands it typed inputs;
- not invent new identity for packages: provenance comes from `PackageKind`, the lockfile, the
  active patch set, and the source-replacement table.

`cabin`'s `cli/tree.rs` and `cli/explain.rs` modules are the orchestration layer that loads
workspace + lockfile + patches + source-replacements + (for `build-config`) the full profile /
toolchain / build-flags preamble, then hands the typed values to `cabin-explain`.  Domain logic
lives in `cabin-explain`; CLI glue stays thin.

### `cabin-env`

Single home for every `CABIN_*` environment variable name.  Read- side names are `pub const &str`;
the run/test side provides one typed builder, `package_env`, returning the deterministic six- key
overlay (`CABIN_MANIFEST_DIR`, `CABIN_MANIFEST_PATH`, `CABIN_PACKAGE_NAME`, `CABIN_PACKAGE_VERSION`,
`CABIN_PROFILE`, `CABIN_BUILD_DIR`) that `cabin run` and `cabin test` apply on top of the user's
environment, plus `parse_bool` for read-side boolean env vars.

The crate must:

- not run processes;
- not read configuration files or touch the filesystem;
- not depend on `cabin`, `cabin-build`, or other higher- level crates that would create cyclic
  dependencies.

Adding a new `CABIN_*` env var requires extending this crate's constants list (and the doc page) so
every consumer of the name agrees byte-for-byte.

### `cabin-source-discovery`

Shared C/C++ source / header walker used by `cabin fmt` and `cabin tidy`.  Consumes a typed
`SourceDiscoveryRequest` (roots, excluded paths, excluded directories, VCS-ignore policy), honors
`.gitignore` / `.ignore` via the `ignore` crate, skips a fixed built-in exclude list (`.git`, build
/ cache / vendor directories), and returns `DiscoveredSourceFile` values sorted by absolute path so
output is byte-stable across platforms.

The crate must:

- not own command construction or executable resolution - that belongs to the matching tool runner
  crate;
- not read Cabin's configuration files - the orchestration layer threads any config-derived inputs
  through the typed request;
- not classify Cabin's notion of "compilable source" beyond the per-extension grammar documented in
  the module head.

### `cabin-fmt`

`clang-format` runner consumed by `cabin fmt`.  Owns formatter executable resolution (`CABIN_FMT`
override, otherwise `clang-format` on `PATH`), the `clang-format` command-line shape, and the typed
`FormatRequest` / `FormatReport` boundary.  Modes: `Write` (`-i` in place) and `Check` (`--dry-run
-Werror`, no rewrites).

The crate must:

- not walk the filesystem looking for sources - that is `cabin-source-discovery`'s job;
- not read Cabin's configuration files - the orchestration layer threads any config-derived inputs
  through the typed `FormatRequest`.

### `cabin-tidy`

`run-clang-tidy` runner consumed by `cabin tidy`.  Owns tidy executable resolution (`CABIN_TIDY`),
the `run-clang-tidy` command-line shape, typed jobs forwarding (`-j` from `cabin-core::BuildJobs`),
and the fix-mode safety clamp (`--fix` forces jobs to 1 to avoid concurrent rewrites).  The compile
database the tool consumes is produced by `cabin build` through `cabin-ninja::compile_commands`;
this crate never generates one.

The crate must:

- not walk the filesystem - `cabin-source-discovery` does that;
- not plan builds or generate compile databases - those are `cabin-build`'s and `cabin-ninja`'s
  jobs;
- not read Cabin's configuration files - `.clang-tidy` discovery remains clang-tidy's
  responsibility.

### `cabin-system-deps`

`pkg-config` runner consumed when a workspace declares ``system = true` deps`.  Owns executable
resolution (`CABIN_PKG_CONFIG`), `pkg-config` command-line construction, and the typed
`SystemDependencyProbeRequest` / `SystemDependencyProbeReport` boundary.  Probes only fire when at
least one selected primary package declares an active system dependency; dependency manifests
outside that primary set preserve declarations but do not spawn `pkg-config`.  The orchestration
layer merges the resolved cflags / libs into `ResolvedProfileFlags` so they reach `build.ninja` and
`compile_commands.json` in deterministic order.

The crate must:

- not read Cabin's configuration files;
- not walk the filesystem or generate the build graph;
- not assume any specific `pkg-config` implementation (the POSIX-style command-line surface is the
  contract).

### `cabin-fs`

Small filesystem helpers shared by Cabin's production crates.  Currently provides atomic file
replacement and lexical path-safety predicates; intentionally narrow rather than a broad filesystem
abstraction.

- Atomic replacement stages bytes in a sibling temporary file and commits with a rename only after
  the write succeeds, so an interrupted run leaves any previous contents of the destination intact.
- The lexical path-safety predicates reason over path components only - they reject absolute paths,
  `..` traversal, root components, and Windows path prefixes - and are safe to call on paths that do
  not yet exist.
- The helpers do not canonicalize, follow symlinks, read the filesystem, create parent directories,
  or enforce archive-, registry-, or config-specific policy.  Callers own parent-directory creation.
- Domain-specific error mapping stays with each consumer so the destination path and user-facing
  context remain in the surfaced diagnostic.  `cabin-lockfile` maps write failures to
  `LockfileError`, `cabin-ninja` to `NinjaError`, `cabin-package` scaffold writes to
  `ScaffoldError`, `cabin-artifact` extraction and path-safety failures to `ArtifactError`, and
  `cabin-port` unsafe recipe paths to `PortError`.

The crate must not own:

- manifest parsing;
- config-file discovery;
- XDG base-directory resolution;
- registry layout;
- the package archive format;
- archive extraction policy (that lives in `cabin-artifact`);
- resolver behavior;
- CLI behavior;
- diagnostics rendering;
- shell or Ninja escaping;
- recursive copy / sync abstractions unless a future focused change justifies one.

### `cabin-diagnostics`

User-facing diagnostic presentation for Cabin's typed domain errors.  Owns the deterministic
formatter, the `miette` rendering boundary used to draw source-annotated snippets for parse /
validation errors, and the stable-code constants the CLI attaches to rendered error-chain messages
(`cabin::config::load_failed`, `cabin::resolver::error`, ...).  The owning crates'
`#[diagnostic(code(...))]` attributes are the canonical source of every stable code; a constant
exists only for the codes the CLI needs as a `&str` value.

Depends only on `miette`, `termcolor`, and `thiserror`.  Domain crates that own source spans
(today: `cabin-manifest`'s `ManifestParseError`) depend on `miette` for the `Diagnostic` derive and
pass the typed value up; the CLI orchestrator (`cabin`) routes it through
`cabin_diagnostics::render` so the user sees a stable `error[code]: message` block plus optional
`help:` text and snippet.

The crate must:

- not depend on `cabin`, `cabin-build`, or other higher- level crates that would create cyclic
  dependencies;
- not run processes, read configuration files, or touch the filesystem - the renderer takes typed
  inputs and produces a string;
- emit byte-stable output (no terminal color, no Unicode-only flourishes that vary with terminal
  capabilities).

Adding a new diagnostic-bearing error is a three-step pattern:

1. derive `miette::Diagnostic` on the error type, attach a
   `#[diagnostic(code(cabin::<area>::<symbol>))]` attribute, add `help(...)` when there is an
   actionable next step;
2. for source-annotated cases, expose `#[source_code]` / `#[label]` so
   `cabin_diagnostics::render` picks the values up automatically;
3. extend `cabin/src/main.rs::downcast_diagnostic` so the typed error participates in the renderer.

### `cabin`

Owns CLI flags and user-facing command orchestration.  May call any other crate.  Should keep
clap-driven argument parsing separate from command execution where practical, and must not contain
business logic that belongs in a reusable crate.

**`cabin/src/cli/mod.rs` must not grow further with new business logic.** When new behavior lands,
the implementation belongs in the owning crate (e.g.  `cabin-workspace` for workspace algorithms,
`cabin-resolver` for resolution, `cabin-build` for build planning, `cabin-publish` for publish
orchestration), exposed through a typed API; the CLI layer should only translate clap inputs into
that API and render the result.  This invariant is enforced socially through review: PRs that add
non-trivial command logic, helpers, or types to `cli/mod.rs` must move them into either the owning
crate or a new per-command module under `cabin/src/cli/` (one file per top-level subcommand) before
they can land.  A small, behavior-preserving split of view structs or dispatch helpers into a
private module is acceptable inside a routine PR; a broad rewrite of `cli/mod.rs` is not in scope
for a routine change.

## Data flow - implemented today

### Dependency kinds end-to-end

Every Cabin dependency is classified into one of two kinds via `cabin_core::DependencyKind`:

```
Normal -> Dev          (Cabin package dependency kinds)
```

Any entry in those tables can additionally set `system = true` to mark it as externally provided;
system-flagged entries route to a separate `system_dependencies` collection and never enter the
resolver / fetcher / build pipeline.

The kind information flows through the system at each layer:

```
[dependencies]            ----+
[dev-dependencies]        ----+--> cabin_manifest      (typed BTreeMaps + system_dependencies
                                   -> ManifestError      vec; entries with `system = true`
                                                         route to the system vec, others to the
                                                         per-kind dep map. Both also fold
                                                         [target.'cfg(...)'.<kind>] in with an
                                                         optional Condition predicate.)
                                       |
                                       v
                            cabin_core::Package
                              dependencies: Vec<Dependency>      // kind on every entry
                              system_dependencies: Vec<SystemDependency>
                                       |
                                       v
                            cabin_workspace::Package
                              deps: Vec<DependencyEdge { index, kind }>
                                       |
                                       v
                  +------------------+--+--+----------------------+
                  |                     |                          |
                  v                     v                          v
            collect_closure        cabin-build target         cabin-package
            _versioned_deps        dep resolution             canonical metadata
            (Normal-only)          (Normal-only edges)        (per-kind tables +
            -> ResolveInput        for `target.<X>.deps`)      system-dependencies)
                  |
                  v
            cabin_resolver         (resolver never sees Dev or System)
                  |
                  v
            cabin.lock + artifact cache
            (kind metadata is not duplicated here; resolver re-decides
            included kinds on each run)
```

System dependencies branch off at the manifest layer and never enter the resolver / fetcher / cache.
Dev dependencies flow through `Package::dependencies` for metadata round-tripping but are filtered
out at the `collect_closure_versioned_deps` boundary and at the workspace path-dep traversal so they
do not affect ordinary builds.

Workspace inheritance is kind-specific: a member's `{ workspace = true }` opt-in under
`[<kind>-dependencies]` looks up the matching `[workspace.<kind>-dependencies]` table only - there
is no cross-kind fallback.  `workspace = true` is also rejected inside `[target.'cfg(...)'.<kind>]`
tables so a single workspace key cannot silently mean different things on different hosts.

Conditional dependencies declared via `[target.'cfg(...)'.<kind>]` travel the same path.  Each
`Dependency` / `SystemDependency` / `DependencyEdge` carries an optional `condition:
Option<Condition>` field.  The host `TargetPlatform` filters out non-matching declarations at three
boundaries: the workspace loader skips them when building path-dep edges, the closure walker skips
them in `collect_closure_versioned_deps_filtered`, and the feature resolver skips them when
expanding `dep:` and per-edge feature requests.  The resolver also skips conditional
`IndexPackageDependency` entries on registry packages.  The `condition` itself is preserved on
`Package::dependencies` and round-trips through `PackageMetadata` and the index loaders, so `cabin
metadata` can surface inactive declarations without losing the predicate text.  Full protocol in
[`target-dependencies.md`](target-dependencies.md).

### Manifest parsing

```
cabin.toml  --> cabin_manifest::parse_manifest_str / load_manifest
                   |
                   v
            ParsedManifest  (private serde structs already shed)
                   |
                   v
            cabin_core::Package + WorkspaceTable
```

### Workspace loading

```
cwd or --manifest-path  --> cabin_workspace::discover_workspace_root
                              |  upward walk for cabin.toml with [workspace]
                              |  (no walk when --manifest-path is explicit)
                              v
                            workspace root manifest
                              |
                              v
                    cabin_workspace::load_workspace
                      |  member globbing
                      |  exclude filtering
                      |  default-members validation
                      |  workspace.dependencies inheritance
                      |  nested-workspace rejection
                      |  recursive local path-dep traversal
                      |  dedup, cycle, name-collision checks
                      v
                   PackageGraph (topologically sorted)
                  - root_package: Option<usize>
                  - primary_packages: Vec<usize>
                  - default_members: Vec<usize>

                  - excluded_members: Vec<PathBuf>

                  - packages: Vec<WorkspacePackage { package, manifest_path }>
```

Versioned dependencies are kept on each `Package` but are not traversed here.  CLI workspace flags
(`--workspace`, `-p / --package`, `--exclude`, `--default-members`) flow through
`cabin_workspace::resolve_package_selection`, which validates the request against the loaded
`PackageGraph` and returns the deterministic ordered list of selected primary-package indices the
downstream commands (build / metadata / package / publish / fetch) operate on.

### Local build planning + Ninja generation

```
PackageGraph + ResolvedToolchain --> cabin_build::plan(PlanRequest)
+ build flags / settings     |  cycle detection
                                       |  cross-package target resolution
                                       |  language-specific compile dispatch
                                       v
                                    BuildGraph (Vec<Action>, CompileCommand[])
                                       |
                                       v
                          cabin_ninja::write_build_ninja        --> build.ninja
                          cabin_ninja::write_compile_commands   --> compile_commands.json
                                       |
                                       v
                               `ninja -C <build_dir>`           (cabin)
```

### Local index resolution

```
<index>/<package>.json files  --> cabin_index::load_index
                                     |  per-file schema validation
                                     |  filename / name agreement
                                     |  SemVer of every version
                                     v
                                  PackageIndex
                                     |
ResolveInput (root package + versioned deps + locked map + mode)
                                     |
                                     v
                                cabin_resolver::resolve
                                     |
                                     v
                                ResolveOutput { packages: [Root, Index, ...] }
```

### Lockfile-aware resolution

```
cabin.toml  --> PackageGraph
                  |
cabin.lock  --> cabin_lockfile::read_lockfile  --> Lockfile
                                                       |
                                                       v
                                                  LockedVersion entries
                                                       |
                                                       v
                                            ResolveInput { mode, locked }
                                                       |
                                                       v
                                            cabin_resolver::resolve
                                                       |
                                                       v
                                            ResolveOutput
                                                       |
                                                       v   PackageIndex meta
                                                       \  /
                                                        ||
                                            Lockfile (rebuilt)
                                                       |
                                                       +--  write to <manifest_dir>/cabin.lock
                                                       |    if the mode permits writing
                                                       v
                                            human / json output (cabin)
```

The resolver receives `LockedVersion` values constructed by the CLI from a `Lockfile`.  The resolver
never reads the lockfile itself; the lockfile crate never runs the solver.  They meet only inside
`cabin`.

| Mode | Locked map effect | Writes lockfile |
| --- | --- | --- |
| `PreferLocked` (default `cabin resolve`) | Tries the locked version first; falls back to newest compatible if locked no longer satisfies constraints. | yes |
| `Locked` (`cabin resolve --locked` / `--frozen`) | Restricts each candidate set to `[locked.version]`; surfaces precise errors when missing / yanked / constraint-violating / checksum-mismatched. | no |
| `UpdateAll` (`cabin update`) | Ignores the locked map entirely. | yes |
| `UpdatePackage(name)` (`cabin update --package <name>`) | Drops one entry from the locked map. | yes |

Once the artifact cache is involved, `--frozen` becomes operationally distinct from `--locked`: both
forbid writing the lockfile, but `--frozen` additionally forbids the artifact cache from being
populated.  Already-cached and already-extracted artifacts may still be reused.

### Artifact fetch + registry-aware build

```
ResolveOutput + PackageIndex
   |
   |  cabin builds a FetchPlan: per resolved registry package,
   |  pull `source.path` + `checksum` straight off the index entry.
   |
   v
cabin_artifact::fetch
   |  for each entry:
   |   - hash the cached archive; reuse if it already matches;
   |   - else (and not --frozen) copy from source.path while
   |      hashing, fail on checksum mismatch;
   |   - extract safely into <cache>/sources/sha256/<hex>/;
   |   - validate <source>/cabin.toml name + version.
   v
FetchResult { packages: [FetchedPackage { name, version, archive_path,
                                          source_dir, checksum }] }
   |
   v
cabin_workspace::load_workspace_with_registry(manifest, fetched)
   |  walk root + every extracted source manifest;
   |  versioned dependencies resolve via the registry map by name;
   |  return a unified PackageGraph (Local + Registry packages).
   v
cabin_build::plan + cabin_ninja::write_*  --> build.ninja + ninja
```

The artifact crate never runs the resolver or invokes the C/C++ toolchain.  The workspace crate
never verifies checksums.  The CLI is the only place where these layers meet.

### Package archive + canonical metadata

```
cabin.toml
   |
   |  cabin_manifest::load_manifest
   v
ParsedManifest -> Package
   |
   |  cabin_package::validate (no path deps, no escaping sources)
   v
ValidatedPackage
   |
   |  cabin_package::archive::collect_package_files
   |   - sorted, fixed include / exclude policy
   |   - regular files and directories only
   v
[PackageFile, ...]
   |
   |  cabin_package::archive::build_zip
   |   - entries sorted by path, deflated at level 6
   |   - fixed 1980-01-01 timestamp, System::Unix, no zip64
   v
archive bytes (Vec<u8>) ---> sha256_hex ---> sha256:<hex>
   |
   |  cabin_package::canonical_metadata
   v
PackageMetadata { schema, name, version, dependencies,
                  yanked, checksum, source }
   |
   |  cabin_package::package writes both files into --output-dir
   v
dist/<stem>-<version>.zip
dist/<stem>-<version>.json
```

The filename stem flattens a scoped name (`fmtlib/fmt` -> `fmtlib-fmt`) so the staged files stay
self-identifying and land flat in the output directory; a bare name is its own stem.

`cabin-publish::dry_run` calls into the same pipeline and returns a `DryRunReport` whose
`registry_modified` flag is always `false`.  No registry, no network, no server is involved in the
dry-run flow - though it does require a scoped name, because a dry run rehearses a publish.  The
canonical metadata's `source` block matches the existing index `source` shape
(`type = "archive"`, `format = "zip"`, `path = "../artifacts/<name>/<name>-<version>.zip"`
for a bare name, `path = "../../artifacts/<scope>/<name>/<scope>-<name>-<version>.zip"` for a
scoped one - the shape the hosted registry validates verbatim).

### Local file-registry publish

```
cabin.toml
   |
   |  cabin_package::stage  (no disk write)
   v
StagedPackage { name, version, archive_bytes, checksum, metadata }
   |
   |  cabin_publish::publish_to_file_registry
   v
cabin_registry_file::publish_to_registry
   |
   |  Require a scoped name: registry packages are always
   |  `<scope>/<name>`; bare names fail (`cabin-publish` already
   |  rejected them earlier with the rename diagnostic).
   |
   |  RegistryLock::acquire(<registry>/.cabin-registry.lock)
   |  FileRegistry::open_or_initialize (writes config.json on first run)
   |
   |  Read the existing package index (if any), reject duplicate
   |  versions and orphaned artifacts.
   |
   |  Phase 1: write artifact through `atomic-write-file` (sibling
   |           temp + rename)
   |  Phase 2: write the package index file the same way; on failure,
   |           delete the placed artifact so the registry
   |           never carries an orphan.
   |
   |  RegistryLock::drop  (lock file removed)
   v
RegistryPublishReport
   {
     registry_dir, package_index_path, artifact_path,
     checksum, source_path, registry_modified, registry_initialized
   }
```

`cabin_publish::dry_run_against_file_registry` runs the same validation (`FileRegistry::inspect` +
the duplicate / orphan checks) without acquiring a lock or writing anything; the `registry_modified`
flag in the returned report is always `false`.

The registry written by this flow lands at:

```
<registry>/
  config.json
  packages/<scope>/<name>.json
  artifacts/<scope>/<name>/<scope>-<name>-<version>.zip
```

Publishing requires scoped names, so new entries always nest one scope directory deep; legacy
bare-name registries (`packages/<name>.json`, `artifacts/<name>/<name>-<version>.zip`) stay
readable and vendorable.  `cabin-index::load_index` detects `config.json` and reads packages out of
the configured `packages/` subdirectory - flat `<name>.json` files as bare names, one-level
`<scope>/<name>.json` nesting as scoped names - so the same path that publish wrote to is consumable
by `cabin resolve`, `cabin fetch`, and `cabin build --index-path` without any repackaging step.

### Sparse HTTP index read path

```
--index-url http://host/registry
   |
   |  cabin_index_http::HttpIndex::open
   |    GET <base>/config.json   -> RegistryConfig
   v
HttpIndex { base, config, packages_base, client }
   |
   |  cabin_index_http::HttpIndex::load_package_index(roots)
   |    BFS over (root deps + transitive):
   |      GET <base>/<config.packages>/<name>.json
   |    Each `<name>.json` is parsed via
   |    `cabin_index::parse_package_entry` with a `SourceContext::HttpUrl`
   |    closure, so `source.path` resolves to an absolute URL using
   |    RFC 3986 relative resolution against the package metadata URL,
   |    then must remain on that package metadata origin.
   v
PackageIndex { packages: BTreeMap<PackageName, IndexEntry> }   (same shape as the local file loader)
   |
   v
cabin_resolver::resolve
   |
   v
ResolveOutput
   |
   |  cabin::build_fetch_plan(output, index, IndexAccess::Http(client))
   |    For each registry-source package:
   |     - LocalPath -> FetchSource::LocalArchive(path)  (file index)
   |     - HttpUrl -> http_client.download(url) -> FetchSource::InMemoryArchive(bytes)
   v
cabin_artifact::fetch
   |  Same checksum + cache + extraction as the local-file path:
   |  bytes are hashed against the index's sha256, written into
   |  <cache>/archives/sha256/<hex>.zip, and extracted into
   |  <cache>/sources/sha256/<hex>/.
   v
FetchedPackage { archive_path, source_dir, checksum }
```

The HTTP path is **read-only**.  There is no persistent metadata cache, so `--frozen` with an
effective HTTP index URL fails with a documented error message.  `--locked --index-url` works
because the lockfile is on disk locally and the resolver can validate fetched metadata against it.

## Architectural seams to preserve

- Raw TOML serde structs stay private to `cabin-manifest`.
- `clap` only appears in `cabin`.
- The stable domain model lives in `cabin-core`.
- Workspace loading and resolver are independent: the workspace loader emits unresolved versioned
  dependencies; the resolver consumes them.
- Build graph IR is backend-independent.  Ninja serialization lives in a separate crate.
- Index format and resolver are independent: the index crate produces data; the resolver consumes
  it.
- Lockfile I/O and the resolver are independent: `cabin-resolver` receives `LockedVersion` values,
  not `Lockfile` itself.
- The underlying solver type is never exposed from `cabin-resolver`.

## C++ semantic invariants

Cabin's resolver and lockfile are Cargo-shaped on purpose, but the build graph the resolver feeds
into is not.  The list below states the C++-specific invariants Cabin's build planning maintains
today, so future contributors do not silently regress them by porting more Cargo-like assumptions:

- **Public vs. private include directories.** Header reachability is part of a `library` target's
  interface, not a free-floating workspace property.  A target's `include-dirs` are *public*: every
  consumer of the target inherits them transitively.  Sources that exist only to compile the library
  must live under `sources` / internal subdirectories that the public include path does not expose.
  There is no `private-include-dirs` field today; adding one is a deliberate language change, not a
  build-graph fix-up.  Provenance decides the spelling on consumer compiles: dirs inherited from
  registry packages and foundation ports are marked as system search paths (`-isystem` / MSVC
  `/external:I`), while workspace members, plain `path` dependencies, and `[patch]`ed packages stay
  on plain `-I` (see [`docs/toolchains.md`](toolchains.md#system-include-directories)).

- **Link interface propagation.** A `library` target propagates its public link interface (the link
  line consumers must add) to every direct and transitive dependent automatically.  Build-time
  link-only deps (linker libraries that are not Cabin packages) are still represented as
  `system-dependencies`; active declarations are probed through `pkg-config`, and the resulting
  flags are wired into consumers that link the producing target.  Cabin does not model CMake's
  `INTERFACE` / `PUBLIC` / `PRIVATE` distinction at the package boundary, and the resolver
  intentionally does not re-implement the C++ link-order rules - Ninja + the linker do.

- **Header-only is its own kind.** Header-only libraries are modeled as the dedicated `header-only`
  kind: they declare `include-dirs` and no `sources`, so the build graph emits no compile or archive
  actions and the link interface stays purely include-dir + system deps.  Declaring `sources` on a
  `header-only` target is rejected at manifest-load time.

- **Patch/override targets a name, not a target inside it.** `[patch] foo = { path = "../foo" }`
  replaces the *entire* package named `foo`.  There is no per-target patch surface.  Consumers
  resolve targets the same way they would for the registry version of `foo`; the patched manifest
  must keep target names stable for consumers to keep building.

- **Dev-only targets are scoped to dev commands.** `test` and `example` link as ordinary executables
  but are excluded from the default `cabin build` enumeration.  `test` targets are built and run by
  `cabin test`, which selects every test target in the selected packages - or only the named ones
  when `--test <name>` is given.  `example` targets reach the build graph only as transitive deps of
  a selected target - Cabin does not yet expose a single-example selector flag, because the historic
  `--target` overload has been removed and the flag name is reserved for the future
  platform/toolchain target.  A future `--example <name>` selector would follow the same
  distinct-flag pattern as `cabin test --test <name>`.  Dependencies of dev-only targets follow the
  same `target.<X>.deps` rules as an `executable`: include and link interfaces propagate from the
  libraries they pull in, but the dev-only targets never contribute include or link interface back
  to ordinary production targets.

- **`[dev-dependencies]` activate per-package, not transitively.** `cabin test` activates the
  `[dev-dependencies]` of the *selected* primary packages so test executables can link against
  test-only packages.  The activation does not propagate: a transitive dep's own dev-deps stay
  declaration-only even under `cabin test`.  `cabin build` continues to ignore every dev-dep, so
  production builds are unaffected.

These invariants are normative: a change that breaks one of them is a language / build-system change
and needs an explicit design update, not an implementation tweak.

## Implemented layers - quick reference

The crate boundaries above stay aligned with the responsibilities listed here.  Each item names the
crate that owns the layer today; future transports / modes should be added to the named crate rather
than carved out into ad-hoc places.

### Artifact layer

A content-addressed cache and source / archive fetcher that turns a locked package set into actual
on-disk source trees, verifying checksums recorded in `cabin.lock`.  Implemented as `cabin-artifact`
for local filesystem source archives.  Future transports (OCI / Git) may be added without
changing the cache shape.

### Package / archive layer

Source-archive creation for publishable packages.  Pure local operation: take a package directory,
produce a deterministic archive plus a per-version metadata digest.  Implemented as `cabin-package`.
The archive contract matches the extractor: a `.zip` whose root contains `cabin.toml`, regular
files only (directories implied).

### File registry publish layer

Local file-registry publish path that drops a freshly created package archive plus updated
`<package>.json` index entries into a directory.  No network, no auth, no server.  Implemented as
`cabin-registry-file` with atomic rename writes via `atomic-write-file` and a simple
`.cabin-registry.lock` lock file.

### Sparse HTTP index / artifact client

Read-path client for fetching `<package>.json` and archives over HTTP from a static layout.
Implemented as `cabin-index-http`.  The on-disk index format and the transport stay separate by
design: local file reading lives in `cabin-index`, HTTP reading in `cabin-index-http`, and they emit
the same `cabin_index::PackageIndex` / `IndexEntry` shape so the resolver and lockfile layers stay
HTTP-free.

### Features - implemented (foundation)

Public additive named-boolean capabilities the user (or a downstream consumer) selects at build
time.

What ships today: manifest declarations (`[features]`), `cabin-core`'s `BuildConfiguration` value
with a deterministic SHA-256 fingerprint, CLI selection flags (`--features` / `--all-features` /
`--no-default-features`), and round-trip preservation through `cabin package` and `cabin publish
--registry-dir`.  Older index entries that omit the field continue to load.  Full protocol in
[`features.md`](features.md).

Optional dependencies and per-edge feature requests, target-cfg dependencies, and build profiles all
layer on top of the same surface; toolchain conditional flags are documented in
[`toolchains.md`](toolchains.md).  Target / platform-specific dependencies are documented in
[`target-dependencies.md`](target-dependencies.md); build profiles are documented in
[`profiles.md`](profiles.md).

### Build profiles - implemented

Named build-configuration presets (`dev`, `release`, plus any custom `[profile.<name>]` declarations
the manifest adds).  Resolution lives entirely in `cabin-core::profile`: `ProfileSelection` (the
user's pick) plus a typed definition table go through `resolve_profile`, which walks `inherits`
chains, detects cycles, applies built-in defaults under manifest overrides, and returns a
fully-typed `ResolvedProfile { name, debug, opt_level, assertions, source, inherits_chain }`.
Target-conditional named profile overlays remain separate typed flag layers: they do not define
profiles or alter scalar settings and are applied only when their target predicate matches and
their name appears in `inherits_chain`.

```
[profile.<name>]   ----> cabin_manifest        (typed ProfileDefinition;
                                                rejects unsupported fields)
                          |
                          v
ProfileSelection ----> cabin_core::resolve_profile
                          |                    (cycle / unknown-parent / built-in
                          v                     `inherits` rejection)
                       ResolvedProfile
                          |
       +------------------+----------------------+----------------------+
       v                                         v                      v
   cabin_build (compile flags,            BuildConfiguration       cabin_metadata
   per-profile output dir)                 fingerprint              JSON view
```

Profile selection does **not** affect dependency resolution, the lockfile, the package archive, the
index, or registry behavior; those remain profile-independent by design.  Output paths are
profile-segregated (`<build-dir>/<profile>/...`) so dev / release / custom builds never overwrite
each other; the build- configuration fingerprint includes the resolved profile so a cache layer can
key on it.  Full protocol in [`profiles.md`](profiles.md).

### Toolchain selection and build flags - implemented

Explicit, typed C/C++ toolchain selection plus a small set of semantic `[profile]` flags.
`cabin-core::toolchain` owns the data model (`ToolKind`, `ToolSpec`, `ToolSource`, `ToolSelection`,
`ResolvedTool`, `ResolvedToolchain`, `ToolchainSettings`); `cabin-core::build_flags` owns the
parallel flag model (`ProfileFlags`, `ConditionalProfileFlags`, `ProfileSettings`,
`ResolvedProfileFlags`, the `resolve_build_flags` merge); `cabin-toolchain::resolve` walks
precedence (CLI - > env - > config - > matching `[target.'cfg(...)'.toolchain]` - > `[toolchain]` -
> default fallback list) per kind, searches `PATH`, applies the per-OS default fallback list (`cl` /
`lib` on Windows, `cc` / `c++` / `ar` elsewhere), and rejects the linker (`link`) or archiver
(`lib`) named for a compiler slot.  The build planner consumes a `ResolvedToolchain` directly for
compile / link / archive commands and a per-package `ResolvedProfileFlags` map to layer `-D` / `-I`
/ extra args onto each target.

```
[toolchain]   ----+
                  +--> cabin_manifest      (typed ToolchainDecl /
[target.'cfg'.toolchain]                    ProfileFlags;
                                            unknown fields rejected)
[profile]                                |
[target.'cfg'.profile]   --------------+
[target.'cfg'.profile.<name>] ----------+
                          |
                          v
ToolchainSelection ----> cabin_toolchain::resolve_toolchain
                          (CLI > env > [target.'cfg(...)'.toolchain]
                           > [toolchain] > defaults; PATH search;
                           Windows cl/lib defaults; link/lib-as-
                           compiler rejection)
                          |
                          v
                       ResolvedToolchain  +  ResolvedProfileFlags
                          |                    (per-package)
                          v
       +------------------+----------------------+----------------------+
       v                                         v                      v
   cabin_build (compile flags,         BuildConfiguration         cabin_metadata
   per-package -D/-I,                  fingerprint                JSON view
   extra-{compile,link}-args,          (toolchain + flags)
   archive command)
```

Manifest-declared `[toolchain]`, `[profile]`, general target-profile, and named target-profile
overlay tables round-trip through `PackageMetadata` and the index loaders; environment- or
CLI-derived selections are never published.  Registry resolution remains toolchain- and
build-flag-independent.  Full protocol in [`toolchains.md`](toolchains.md).

### Compiler / tool capability detection - implemented

After `cabin-toolchain::resolve_toolchain` returns a `ResolvedToolchain`,
`cabin-toolchain::detect_toolchain` runs each picked tool with `--version`, captures the output, and
hands it to the pure parsers in `cabin-core::compiler`.  The result is a typed
`ToolchainDetectionReport` carrying a `CompilerIdentity` / `ArchiverIdentity` (kind, version, target
where reported) and a `CompilerCapabilities` / `ArchiverCapabilities` set per tool.  Capability
decisions record their source (`version`, `assumed-default`, `unsupported`) so consumers can audit
the inference chain.

```
ResolvedToolchain ----+
                                +----> cabin_toolchain::detect_toolchain
                                       (ToolRunner trait;
                                        ProcessRunner spawns
                                        `tool --version`;
                                        parsers in cabin_core::compiler)
                                            |
                                            v
                              ToolchainDetectionReport
                                            |
       +------------------------------------+
       v                                    v
cabin_build::validate_toolchain_for_backend  cabin MetadataView
(accepts an MSVC cl/lib toolchain or a       (toolchain.detected)
 GCC/Clang + ar toolchain; rejects unknown
 compilers and mixed-dialect toolchains)
```

Recognized compiler families: `clang`, `apple-clang`, `clang-cl`, `gcc`, `msvc`.  MSVC (`cl.exe`),
Clang's `clang-cl` driver, and the `lib.exe` archiver drive the MSVC command-line dialect (see
`cabin-driver`); `clang`, `apple-clang`, and `gcc` drive the GCC/Clang dialect.  Validation requires
the whole toolchain to speak one dialect - an MSVC compiler paired with a GNU `ar` (or the reverse)
is rejected up front rather than left to fail mid-build.  Unknown compilers are conservative:
capabilities default to `unsupported`, and the build flow rejects them when the planner needs
GCC-style flags.

Detection results are deliberately **not** serialized into package or index metadata.  They are
local-environment state and would create reproducibility problems if they leaked into a published
archive.  The build configuration fingerprint is unaffected because the planner still emits the same
fixed command shapes whether the detected compiler is Clang 17 or GCC 13.

Detection is parser-only by design: it reads `tool --version` output rather than staging probe
compilations, which keeps the step fast, deterministic, and free of temporary build artifacts.

Full protocol in [`toolchains.md`](toolchains.md).

### Patch / override / source replacement

A typed local-policy layer that swaps registry-resolved package candidates for local working copies
(*patches*) and redirects one supported index source to another (*source replacement*).

```
[patch] manifest table (workspace root only)
  fmt = ...
      |
      v
cabin_workspace::resolve_active_patches
 - walks user -> workspace -> project -> explicit config patches
 - overlays the manifest patch, with config taking precedence
 - validates path exists, cabin.toml, name match, and version match
 - canonicalizes paths
      |
      v
ActivePatchSet (sorted by name)
      |
      v
cabin_workspace::load_workspace_with_registry_and_patches
      |
      v
PackageGraph augmented with patched packages
      |
      v
Build / metadata / lockfile / publish
```

Source replacement is config-only and lives next to patches in `EffectiveConfig`:

```
[source-replacement]
"https://example.com/index" = { index-path = "../mirror" }
      |
      v
cabin_core::SourceReplacementSettings::resolve()
 - walks one hop at a time
 - detects cycles
 - rejects credentials in URLs
 - swaps only between existing IndexPath / IndexUrl kinds
      |
      v
resolved index source
      |
      v
artifact pipeline + lockfile [[source-replacement]] array
```

`cabin`'s `cli::patch` module owns the orchestration glue: typed inputs in, typed values out, no
business logic in `cli/mod.rs`.  The lockfile gains optional `[[patch]]` and
`[[source-replacement]]` arrays (default-empty so old lockfiles remain valid), and `--locked` errors
if the recorded arrays differ from the active policy.  `cabin metadata` adds two top-level arrays
(`patches`, `source_replacements`) for deterministic auditability.

Local override policy never enters published artifacts: `cabin-package` rejects manifests with a
non-empty `[patch]` table; `.cabin/config.toml` (which carries config patches + source replacement)
is already in `EXCLUDED_DIR_NAMES`.  Git sources and registry-server work remain deferred; the
authenticated remote-registry client - reads and publish - is an experimental track behind
`-Z remote-registry` ([`remote-registry.md`](remote-registry.md)).

Full protocol in [`patch-overrides.md`](patch-overrides.md).

### Configuration files

Cabin reads typed TOML configuration files for *local policy*: defaults the user, the workspace, or
a single project want to apply across many invocations.  Config sits between the manifest (which is
*package source spec*) and the CLI / environment so existing per-command flags keep their highest
precedence.

`cabin-config` (a new crate) owns the entire surface:

```
[user, workspace/project, explicit]
       config files
            |
            v
cabin_config::discover_config_files
 - env-driven discovery: CABIN_NO_CONFIG / CABIN_CONFIG /
    CABIN_CONFIG_HOME, falling back to the xdg-resolved user config
    home with the `cabin` application prefix
 - deny_unknown_fields parsing of [registry] / [paths] / [build] /
    [toolchain] / [term] (private serde shape)
 - reject [target.'cfg(...)'.<...>] tables, auth/token/credentials/
    registries tables, registry index-path/url conflicts, and empty
    or invalid values
   v
cabin_config::merge_loaded_files
 - lower-priority files first, higher overrides per field
 - attaches every effective value to its ConfigSource
   v
EffectiveConfig
 - registry source           (with file provenance)
 - paths.cache_dir/build_dir (resolved relative to the config file)
 - build.profile             (string, validated against the project's
                               profile definitions)
 - compiler_wrapper          (CompilerWrapperRequest)
 - toolchain.cc/cxx/ar       (ToolSpec)
```

`cabin` orchestrates only: `cabin/src/cli/config.rs` maps `EffectiveConfig` into the typed layers
the existing resolvers consume:

- `cabin_toolchain::ConfigToolchainLayer` slots between the env variable and the manifest in
  `cabin_toolchain::resolve_toolchain`.
- `cabin_toolchain::ConfigWrapperLayer` slots between the `CABIN_COMPILER_WRAPPER` env variable and
  the manifest in `cabin_toolchain::resolve_compiler_wrapper`.
- The build-side helpers (`profile_selection_for_build`, `resolve_index_source`,
  `resolve_cache_dir`, `resolve_build_dir`) consult the same `EffectiveConfig` and return a typed
  value plus its `ConfigValueSource`.

The metadata view emits a top-level `config` block with the loaded files plus every effective
config-derived setting, paired with its `value_source` so consumers can audit provenance without
re-running discovery.  `BuildConfiguration::fingerprint` already covers the per-tool spec + wrapper
kind / version, so a config that picks `clang++` or `ccache` produces a different fingerprint than
one that does not - without the config layer needing to emit anything new into the hash.

Local config never enters package archives, the canonical per-version metadata, the lockfile, or the
registry index:

- `cabin-package::archive::EXCLUDED_DIR_NAMES` already filters the `.cabin/` directory out of
  deterministic source archives.
- `cabin-package::metadata` and `cabin-index` consume `Package` values from `cabin-core`, which
  never contain config-derived fields.
- `cabin-publish` does not read config for authentication.

Auth tokens and new registry protocols are *deliberately* out of scope; `cabin-config`'s parser
rejects auth/credential/token keys with a dedicated error message so a typo cannot smuggle a secret
into a published archive.  Source replacement and vendoring are implemented local-policy/read-path
features; they remain excluded from package archives, canonical package metadata, and registry
authentication.  Full protocol in [`config.md`](config.md).

### Compiler wrappers

Cabin can prefix C and C++ compile commands with any executable name or path, including `ccache`,
`sccache`, and `icecc`. The wrapper sits *on top* of the resolved toolchain: it does not replace the
compiler driver, and it never wraps link or archive commands.

`cabin-core::compiler_wrapper` owns the typed model (`CompilerWrapperKind`,
`CompilerWrapperRequest`, `CompilerWrapperSource`, `CompilerWrapperIdentity`,
`ResolvedCompilerWrapper`, `CompilerWrapperSummary`) plus validation for non-empty executable
values.
`cabin-toolchain::wrapper::resolve_compiler_wrapper` walks the precedence ladder and returns an
`Option<ResolvedCompilerWrapper>`, reusing the same `EnvLookup` / `ExecutableProbe` / `ToolRunner`
abstractions as the rest of the toolchain layer:

```
--compiler-wrapper / --no-compiler-wrapper -+
CABIN_COMPILER_WRAPPER ---------------------+--> resolve_compiler_wrapper
config [build] compiler-wrapper ------------+    (first selected layer wins)
manifest [build] compiler-wrapper ----------+
                                                  |
                                                  v
                                    Option<ResolvedCompilerWrapper>
                                                  |
                                                  v
                              cabin_build::PlanRequest.compiler_wrapper
                                                  |
                              +-------------------+-------------------+
                              v                                       v
                build.ninja: wrapper cc/cxx ...       compile_commands.json:
                (Ninja invokes wrapped command)       unchanged cc/cxx ...
```

The request stores a `ToolSpec`; it is never shell-split. Bare names use `PATH` lookup and paths are
probed directly. Wrapper selection is unconditional and happens before compiler detection.

`cabin metadata` surfaces the resolved wrapper under `toolchain.compiler_wrapper`. The
build-configuration fingerprint folds in the wrapper kind, spec, source, and version. Workspace
member manifests that declare `[build] compiler-wrapper` are rejected with
`MemberDeclaresCompilerWrapper`, so a single invocation cannot silently apply different wrappers to
different packages. Manifest declarations round-trip through `cabin package`; CLI, environment, and
local config selections never do.

Full protocol in [`compiler-cache.md`](compiler-cache.md).

### Non-Local Registry Control Planes

This repository implements the local registry interface: local file registries, package archives,
and a read-only sparse HTTP client for a static layout.  Account systems, hosted write paths,
ownership workflows, package yanking, signing policy, and administrative control planes are outside
this local-core boundary.  See [`registry-design.md`](registry-design.md) for the concrete read-path
and file-registry shape this repository supports.  The *client* side of an authenticated remote
registry - bearer-token reads, publish, yank - is an experimental track gated behind
`-Z remote-registry` and specified in [`remote-registry.md`](remote-registry.md); the registry
service's hosted implementation lives under `registry/` in this repository, outside the OSS-core
boundary.

## Scope and limitations

Cabin is pre-1.0 and intentionally focused on the local OSS package-manager-and-build-system core.
The following are *not* part of this repository today:

- **No Git dependencies.** A Git-backed registry index is intentionally never planned; see
  [`registry-design.md`](registry-design.md).  Source registries are local file directories or
  sparse HTTP today.
- **No non-local registry control plane.** Every command that needs an index expects `--index-path
  <dir>` or `--index-url <url>`.  There is no default remote registry, and package upload over the
  network exists only behind the experimental `-Z remote-registry` client - protocol,
  `cabin login` / `cabin logout`, authenticated reads, `cabin publish` against an HTTP index
  source, and `cabin yank`; see [`remote-registry.md`](remote-registry.md).
- **No account / ownership workflows.** Ownership, signing, and restricted package access are out
  of scope.
- **No administrative policy surfaces.**
- **No remote / binary build cache.** The artifact cache stores source archives only.
- **No cross-compilation.** Cabin builds only for the host platform: `[target.'cfg(...)']`
  predicates evaluate against the host and `--target <triple>` is reserved for future use. (Windows
  / MSVC itself *is* supported - CI builds and tests on `windows-2025-vs2026`, and the
  `cabin-driver` crate lowers the build IR to the MSVC `cl.exe` / `lib.exe` dialect; see the
  detection and dialect sections above.)
- **No hard resolver-side language-standard filtering.** First-class C/C++ language standards are
  implemented locally (required manifest fields with no built-in default, `[workspace]` defaults,
  the per-target `gnu-extensions` boolean, dialect lowering, pre-build validation, interface
  enforcement, fingerprint / metadata - see [`language-standards.md`](language-standards.md)).
  Standards inform resolver *version preference* - the `[resolver] incompatible-standards` knob
  (default `fallback`) orders candidates by compatibility and reports hold-backs (see
  [`language-standards.md`](language-standards.md#version-selection) and
  [`design/standard-compatibility/preference-mode.md`](design/standard-compatibility/preference-mode.md)) -
  but they are **never** encoded as `PubGrub` constraints and never filter a version out of the
  solution, so preference never introduces a resolution failure `allow` would not also produce.
  Range interface requirements (the reserved `max` slot) belong to a future version.  The
  post-resolution standard-compatibility check (`cabin_build::standard_compat`) evaluates the spec's
  edge-compatibility model over the *resolved* graph and fails the command on violated edges (see
  [`language-standards.md`](language-standards.md)); it is the post-resolution correctness
  authority, distinct from the preference heuristic.  The MSVC
  `/std:c++latest` / `/std:clatest` spellings are intentionally never mapped as first-class
  standards - they float to the compiler's newest in-progress draft, so the unvalidated
  environment flag route (`CXXFLAGS` / `CFLAGS`) is their only supported injection point.
- **No workspace-level profile or toolchain overrides beyond the documented root-owned settings.**
  Member manifests cannot carry root-only build policy, and workspace-level profile/toolchain
  expansion beyond the current model is out of scope.
- **Not a CMake / Meson drop-in replacement.** Cabin does not consume `CMakeLists.txt` or
  `meson.build` files.  Existing CMake / Meson projects cannot be migrated without rewriting the
  build description as `cabin.toml`.
- **No shared-library linkage model.** The current build model is based on executables, static
  archives, header-only libraries, and system-library link flags; broad shared library generation /
  ABI policy is out of scope.
- **No lockfile capture of resolved build configurations.** The lockfile records dependency and
  local-override state, not profile / toolchain / environment-derived build configuration
  fingerprints.
- **No C++ modules, no generated-source bindings.** Header-generation tools (`cxx`, `autocxx`,
  `bindgen`) and the C++ modules build flow are out of scope.
- **No cross-compilation.** `--target <triple>` is reserved for the future cross-compilation flow.

Per-feature limitations live with each feature page (for example [`targets.md`](targets.md),
[`profiles.md`](profiles.md)).

## Contributor-facing architecture guardrails

The architecture document is the canonical source for crate boundaries, ownership rules, and scope
limits.  `CONTRIBUTING.md` points here rather than restating those rules.  If code moves across
crate boundaries, update this document plus the relevant `AGENTS.md` routing file in the same
change.

Architecture-sensitive behavior changes should add focused unit coverage in the owning crate and CLI
integration coverage when the behavior is user-facing.  Observable output used by tooling or tests
must stay deterministic: workspace selections, generated Ninja, `compile_commands.json`, metadata /
tree / explain JSON, package archives, lockfiles, and registry files should sort or normalize their
output explicitly.

Tests must not require external network access.  Network protocol tests boot an in-process server on
`127.0.0.1:0` and point Cabin at that server.  CLI integration tests use the shared `cabin()` helper
to scrub process environment variables Cabin reads; tests that exercise config discovery opt back in
through the documented `cabin_with_config()` helper.  The full portability rules live in
`crates/AGENTS.md`.

## Why a separate lockfile crate?

`cabin-lockfile` and `cabin-resolver` solve unrelated problems:

- **Lockfile I/O**: TOML serialization, deterministic ordering, schema validation.  Pure data, no
  algorithms.
- **Resolution**: constraint satisfaction over an index.  Algorithmic.

Keeping them apart means the artifact layer can hash into `cabin.lock` without churning the
resolver, and a future resolver algorithm change can land in `cabin-resolver` without touching the
lockfile crate.

## Why a separate build graph IR?

Same reasoning as the lockfile split: the build graph in `cabin-build` is a small, dumb data
structure on purpose so future backends (a direct in-process executor, a remote-cache hook, a
Bazel-style exporter) can consume the same shape without reaching into Ninja specifics.
