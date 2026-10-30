# Capability roadmap: shared contracts

This page fixes the contracts that the capability roadmap's implementation stages share.  The
[ledger](design/capability-roadmap/ledger.md) maps each requested area to its stages, records the
current source evidence, and tracks status.  Read both before starting a stage.

**Status.** Design only.  Nothing on this page is implemented by writing it down; a capability is
implemented when its stage lands code, documentation, and tests, and the ledger says so.  Where
this page shows syntax it is illustrative: the owning stage chooses the final spelling, and a
stage that finds a better fit revises this page in the same change.

The roadmap covers ten areas: dependency resolution, usage requirements, system providers, code
generation, cross-compilation, library variants, installation and interoperability, C++ modules,
toolchains, and developer-facing CLI workflows.  Remote or binary build caches and signing or
attestation products are not part of it.  Build identity, incremental correctness, execution trust
boundaries, and metadata consistency are part of it wherever those ten areas need them.

## Vocabulary

| Term | Meaning |
| --- | --- |
| Build machine | The machine that runs Cabin, Ninja, and every build action. |
| Target platform | The platform a build artifact is built to run on.  Today it is always the build machine's own platform. |
| Context | Either **target** (artifacts for the target platform; today's only context) or **build** (tools that execute on the build machine while the build runs, such as a code generator).  A native build has one platform but still two contexts. |
| Package identity | `(name, source, version, revision)` - what the resolver chooses and the lockfile pins. |
| Build unit | One target of one package built in one context with one unit configuration.  Build units are what the planner instantiates. |
| Link/runtime domain | The build units whose code ends up in one final artifact (an executable, shared library, or loadable module) or is loaded into one process. |
| Provider | What supplies a package in a context: Cabin (fetch or path, then build; the default) or the system (an installed copy the root chose explicitly). |
| Action | One build-graph step: compile, assemble, archive, link today; generated actions later. |

Cabin avoids the word *host* for new surfaces: Cargo uses it for the build machine and Conan for the
target platform.  Existing code and docs say *host* for the machine Cabin runs on
(`TargetPlatform::current()`); stage 04 renames what it touches.

## 1. Package identity versus build-unit identity

A **package identity** says which source is used.  A **build unit** says how and for what that
source is built.  The resolver and lockfile deal only in package identities; the planner deals in
build units.

**Package identity** is `(name, source, version, revision)`:

- *Registry* packages: the index origin, a SemVer version, and the packaging revision the lockfile
  pins through its `checksum` ([lockfile](lockfile.md#packaging-revisions)).
- *Path and workspace* packages: the manifest directory.  They are not locked.
- `[patch]` replaces the package of that name with a path source; source replacement and vendoring
  change only where registry bytes come from, never the identity - the pinned checksum must still
  match.

One package name resolves to one `(version, revision)` per lockfile, shared by every context, as
today (the resolver keys PubGrub packages by name).  A build tool and a target library built from
the same package therefore come from the same source.  A stage that needs two versions of one
name must bring evidence and revise this rule.

**Build-unit identity** is the package identity, the target name, the context, and the **unit
configuration**:

- the platform of the context (target platform or build machine);
- the toolchain identity used for that context;
- the profile;
- the package's enabled features in that context;
- the library variant (stage 08);
- the provider (stage 06; a system-provided package supplies prebuilt artifacts instead of build
  units).

Rules:

1. **The lockfile records package identities and resolution choices, not unit configuration.**  No
   toolchain, flag, profile, provider probe result, or fingerprint enters `cabin.lock`, as the
   [lockfile](lockfile.md#version-pins-only) promises today.  Context-specific edges come from
   manifests and index metadata; stage 04 decides what the lockfile must add so that `--locked`,
   `--offline`, `fetch`, and `vendor` reproduce both contexts' closures.
2. **Features unify per package within one context, never across contexts.**  Stage 01 completes
   unification within a context; stage 04 separates the contexts.
3. **Units exist only where an active edge reaches them.**  A build with no build-context edges
   instantiates exactly the units it does today; a native build with the default library variant
   keeps today's paths.
4. **Different units never share products.**  Objects, archives, generated files, and module
   interfaces of units that differ in any identity component must not overwrite or satisfy each
   other: context, platform, and variant select distinct output directories, and everything else
   reaches the action's command line or a tracked input (the tool binary, a configuration stamp) so
   Ninja reruns it.  `BuildConfiguration::fingerprint` must cover every unit-configuration input
   (see [`crates/AGENTS.md`]).
5. **`cfg(...)` evaluates against the unit's platform.**  A predicate on a build-context unit sees
   the build machine; on a target unit it sees the target platform.
6. **`links` uniqueness applies within a link/runtime domain.**  Two independent domains (a build
   tool and the target program) may each contain the native identity once; one domain may not
   contain it twice.  Stage 04 implements this; until then the graph-wide check stays.

Worked examples the contract decides:

| Situation | Outcome |
| --- | --- |
| `app` uses `protoc` from package `protobuf` as a build tool and links `protobuf`'s runtime library. | One lockfile entry for `protobuf`.  Units: `protobuf:protoc` and the runtime library in the build context (protoc links it), the runtime library again in the target context.  With Cabin providing both contexts, the tool and the runtime are the same version by construction; mixing providers needs the checked agreement of section 3. |
| A target edge enables feature `x` of `zlib`; a build edge does not. | `zlib` in the target context has `x`; `zlib` in the build context does not.  Neither leaks into the other. |
| A native build with identical configuration in both contexts. | The two units may be deduplicated as an optimization.  Nothing may depend on that happening. |
| The same tree is built for `--target aarch64-unknown-linux-gnu`, then natively. | Target units land in different directories; no object is reused across the two. |
| A project without `cfg(...)` dependency tables is resolved on Linux and on macOS. | Identical `cabin.lock`: toolchains and provider probe results never appear in it.  Platform-conditional dependency tables still make the locked closure host-dependent today; stage 04 decides how lockfiles represent per-context closures. |

## 2. Usage requirements and dependency edges

A library target has four kinds of settings.  Each has one propagation rule, and every consumer of
the model - planner, driver lowering, `compile_commands.json`, system providers (stage 06), install
exports (stage 09), and C++ modules (stage 10) - uses the same rule instead of its own.

| Kind | Applies to | Examples today |
| --- | --- | --- |
| Producer-internal settings | The producer's own compiles and link steps only | `defines`, implementation `c-standard` / `cxx-standard`, `gnu-extensions`, package flag layers other than `link-libs` and `frameworks` (`ldflags` reaches the producer's own link steps) |
| Consumer interface | The producer and its consumers, along edges as described below; `interface-defines` reach consumers only | `include-dirs`, `interface-defines`, `interface-c-standard` / `interface-cxx-standard` |
| Private implementation dependency | Compiling the producer; not re-exported | a string `deps` entry |
| Final-link requirements | Every final link whose static closure contains the producer, regardless of edge visibility | static archives of the closure, `link-libs`, `frameworks` |

Edge rules:

- A **private** edge applies the dependency's consumer interface to compiling the producer only.
- A **public** edge applies it to the producer and re-exports it to the producer's consumers,
  transitively along chains of public edges.  The existing rule of thumb stays: an edge is public
  iff the producer's public headers include the dependency's headers.
- **Final-link closure ignores compile visibility.**  A static library's private dependency is
  still linked into every executable that links the library.  Link order follows the dependency
  graph (dependents before dependencies); repetition and grouping that the linker needs are kept,
  never removed by blanket sorting or deduplication.  A shared library (stage 08) ends the private
  part of the closure at its own link.
- **Header-only** targets never compile anything; they contribute a consumer interface and
  final-link requirements (Boost.Asio's `pthread` and Windows system libraries reach the final
  link today through `link-libs`).

Reused vocabulary and what changes:

- `public` keeps its spelling and its rule of thumb.  Today it only drives the post-resolution
  standard check and is otherwise declarative ([manifest](manifest.md#edge-visibility)); stage 02
  makes it govern include-directory and definition propagation too.
- `include-dirs` keeps meaning *public include directories*.  The producer-only form is the
  package flag layers' `include-dirs` (`[profile]` and its `cfg(...)` variants), which reach only
  the declaring package's own compiles; a per-target producer-only form waits for a package that
  needs one.
- `defines` keeps meaning *producer-only definitions*, and is rejected on `header-only` targets,
  which compile nothing.  Stage 02 adds `interface-defines`, which, like the interface standards,
  describe consumers' compiles and not the producer's own: a macro both need is listed in both.
- `interface-c-standard` / `interface-cxx-standard` keep their meaning and ranges.
- `links` stays a native-library identity claim, never a link-line entry or a metadata channel;
  `provides` / `conflicts` stay reserved.

**Migration (stage 02).**  Today every edge, private or public, carries a dependency's include
directories to all transitive consumers.  After stage 02 a private edge stops at the producer.
Consumers that compiled only because of that leak break, so stage 02 audits ports and examples,
marks the edges whose public headers need the dependency as public, and documents the change.
A published port whose correction changes only target `deps` visibility or other manifest fields
outside resolution metadata respins as an ordinary packaging revision; a correction that changes
dependencies, features, or standards waits for stage 03.

Further constraints:

- Typed requirements replace raw flags across trust boundaries.  Registry packages' raw `cflags` /
  `cxxflags` / `ldflags` stay dropped; a requirement that a registry package legitimately needs
  (an interface definition, a framework, a per-source ISA option) gets a typed field.
- Two different values for one macro reaching one compile through consumer interfaces are a
  diagnostic that names both paths, never last-wins; a package's own definitions keep today's
  behavior.  Cabin does not claim to classify macros as ABI-safe.
- Per-source options (an ISA flag on one source) stay on that source; they never spread to sibling
  sources or to consumers.
- C and C++ stay separate in every field that has a language: standards, flags, link-driver choice.
- System-include provenance stays: registry interfaces reach consumers as `-isystem` /
  `/external:I`.

## 3. Providers

**Cabin is the default provider**: a dependency is fetched (or read from its path) and built.  The
**final consumer** may replace it with a system-installed copy; nothing else may.

- Dependency declarations do not change.  `"cabin-ports/zlib" = "=1.3.1"` means "this library
  needs zlib", whoever provides it.  Ports never declare `system = true` (the
  [port publisher](foundation-ports.md#anatomy-of-a-foundation-port) already refuses it) and never
  grow mutually exclusive `system` / `vendored` features: features are additive.
- **The root owns the choice.**  Provider selection is root or workspace policy, like `[patch]`; a
  dependency's manifest cannot select or override it.  Stage 06 decides between extending `[patch]`
  and a separate table, which configuration layers may select a provider (`.cabin/config.toml` may
  add patches today), and whether the selection is recorded in the lockfile the way `[[patch]]`
  entries are.
- **One provider per package within a link/runtime domain.**  A system zlib and a Cabin-built zlib
  in one executable is an error.  The build context may use a different provider than the target
  context, so a cross build can use the build machine's installed `protoc` while building the
  target runtime from source.
- **A provider supplies the package's interface and artifacts**: version, available targets or
  components, per-target usage requirements in the section 2 model, and the prebuilt files behind
  them - libraries, runtime files, and executables a build-context consumer runs (a system
  `protoc`, Perl, `mpiexec`).  Package names are not pkg-config module names; the mapping is
  explicit.
- **Tools and runtimes from different providers must agree explicitly.**  When a build tool and the
  runtime its output needs come from different providers (a system `protoc` and a Cabin-built
  protobuf runtime), the version agreement is declared and checked (stage 07), never assumed.
- **Absence or incompatibility fails.**  An explicit system choice that cannot be satisfied is an
  actionable error, never a silent fallback to a source build.  Only facts that can be checked
  (version requirements, architecture, CRT choice) are checked; a version string does not prove ABI
  compatibility.
- **Selection is explicit; probe results are machine state.**  A provider is chosen by policy,
  never by discovery.  What discovery finds - paths, flags, and versions detected on this machine -
  never enters the lockfile, package archives, or index metadata (see [`crates/AGENTS.md`]).  Probe
  results reach build command lines or tracked inputs, so changes rebuild.
- **On MSVC, discovery is ABI-appropriate.**  GNU-style pkg-config tokens are not translated into
  MSVC link lines.

The existing `system = true` dependency names a pkg-config module for a library that has no Cabin
package.  It keeps working; stage 06 decides whether it becomes a special case of providers or
stays separate, and documents the migration either way.

## 4. Generated actions

An action that produces files declares, before it runs:

- its **tool**: an executable in the build context - a Cabin-built build unit, or an executable a
  system provider supplies (section 3);
- its **argv** as a list, never a shell string;
- its **inputs** (package files, other actions' outputs, the tool itself) and **outputs**, plus any
  byproducts, depfile, or discovered-input contract the tool needs;
- its **working directory** and the **environment variables** it depends on, which are tracked so
  a change reruns the action (whether the rest of the environment is scrubbed is stage 07's choice:
  Windows and MSVC tools need parts of it);
- its **ordering**: outputs exist before any compile or module scan that reads them.

Cabin enforces what it can check: outputs live under the unit's own output directory, each output
has exactly one writer, there are no cycles, nothing writes into package source or cache trees,
and a failed action leaves no output that a later build treats as up to date.

**A declared action is not a sandbox.**  The tool runs with the user's privileges and can read or
write anything the user can.  Validating argv and paths prevents mistakes; it does not contain
malicious code.  The trust model therefore states who decided to run what:

- Today `cabin build` runs only the user's chosen compiler, assembler, archiver, and linker over
  dependency sources, with registry-supplied raw flags dropped; `cabin run` and `cabin test` run the
  user's own programs, which contain dependency code because the user linked it.  Running a
  dependency's generator or upstream builder during the build is new: it executes code that the
  dependency's author controls, before the user asked to run anything.
- **The root decides.**  A dependency's actions run only under an explicit root execution policy;
  stage 07 defines the unit of trust (for example a package identity including its revision), how a
  changed revision affects trust, and the non-interactive behavior.  Without trust, the build fails
  closed and names the package.
- **Inspection never executes dependency code.**  `resolve`, `update`, `fetch`, `vendor`, `tree`,
  `package`, `publish`, and the registry verifier never run actions.  `metadata` and `explain`
  run only machine probes (toolchain `--version`, pkg-config), never dependency-supplied programs.
- **Target binaries never run on the build machine implicitly.**  Running one needs a native build
  or an explicitly configured runner (stage 04).

Generated outputs belong to their unit's context and variant directory, and generated public
headers join the unit's consumer interface (section 2) and its install export (stage 09).  When an
upstream project's own builder (Configure, CMake, Meson) is the authority for generation, Cabin
invokes that real tool behind an explicit artifact contract rather than porting its logic.

## 5. Packaging revisions

**Implemented today** ([package index](package-index.md#packaging-revisions),
[lockfile](lockfile.md#packaging-revisions)): the immutable registry unit is
`(name, version, revision)`; a revision id is the first 16 hex characters of the archive digest; the
lockfile's `checksum` pins the revision; plain resolution keeps a listed pin and `cabin update`
moves to the current revision.  Since stage 03, an index's version-level metadata is the current
revision's, and a revision whose `dependencies`, `features`, `standards`, or `links` differ carries
its own `resolution` in a schema-2 document, which resolution reads for every pin the lockfile mode
keeps.  The file registry and `cabin vendor` write it; the hosted registry still refuses a respin
that changes those fields ([foundation ports](foundation-ports.md#packaging-revisions)).

**Contract for stage 03**: a correction at the same upstream version may change resolution
metadata, and every existing pin keeps its meaning.

- A revision's archive bytes and its resolution metadata (dependencies, features, standards,
  `links`, and the future build-dependency and action declarations) belong together.  Resolution
  under a lock that pins revision r1 reads r1's metadata and fetches r1's bytes - never the current
  revision's metadata with r1's bytes.
- Fresh resolution and `cabin update` move to the current revision, metadata and bytes together.
- Older clients must never silently misread newer metadata: schema or capability negotiation, or
  explicit rejection.
- Legacy metadata keeps its meaning; nothing is retro-filled with guessed history, and no step
  depends on wiping the production registry.
- Machine-specific data (detected SDKs, absolute paths, environment selections) never enters
  published metadata.

Until the hosted registry serves `resolution`, its respin restrictions stay in force, and the
ledger records each port correction that waits for it.

## Compatibility expectations

- Existing manifests keep parsing.  A field's meaning changes only where a stage documents the
  migration (stage 02's private-edge include leak is the known one).  Unknown future syntax keeps
  failing through the generic unknown-field diagnostics.
- Existing lockfiles keep resolving to the same bytes.  A lockfile schema change carries a
  migration.
- Published archive bytes never change; old pins keep their bytes and their resolution meaning.
  Stage 02 is the one documented exception to build meaning: once private edges stop re-exporting
  include directories, an old pin of a package that relied on the leak can stop compiling its
  consumers, and the remedy is a packaging revision that marks the edge public, reached through
  `cabin update --package`.
- A plain `library` stays a static library unless the consumer or root selects otherwise (stage 08).
- Serialized JSON and TOML field names and diagnostic codes are user-facing API: stages add to
  them and document every change.
- Native builds keep their behavior when contexts and `--target` arrive (stage 04), except where a
  stage fixes a documented discrepancy.

[`crates/AGENTS.md`]: https://github.com/cabinpkg/cabin/blob/main/crates/AGENTS.md
