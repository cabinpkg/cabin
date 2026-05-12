# Dependency kinds

Cabin classifies every dependency into one of three **kinds**,
each declared under its own manifest section:

| Section                  | Kind     | What it means                                                  |
|--------------------------|----------|-----------------------------------------------------------------|
| `[dependencies]`         | `normal` | Linked into ordinary builds.                                    |
| `[build-dependencies]`   | `build`  | Resolved and fetched for build preparation metadata. They are not put on `PATH` yet — see Limitations. |
| `[dev-dependencies]`     | `dev`    | For tests / examples / local development tasks.                 |

An individual entry inside any of these tables can additionally
opt out of the Cabin registry / resolver / fetcher path and be
sourced from the system instead by setting `system = true`:

```toml
[dependencies]
zlib = { version = ">=1.2", system = true }
```

This is *where the dependency comes from*; the *when* (always,
build-time, test-time) is still driven by which table the entry
lives in. For selected primary packages, Cabin probes active
`system = true` entries through `pkg-config` at build time to
obtain compile and link flags. Declarations in local path
dependencies and registry packages are preserved as metadata but
are not probed for the downstream build — see
[`system-dependencies.md`](system-dependencies.md) for the full
probe behaviour.

Every kind also accepts the platform-conditional form
`[target.'cfg(...)'.<kind>]`. The condition is evaluated against
the host platform; non-matching declarations are filtered out
before they reach resolution / fetch / build. The full grammar
and evaluation rules live in
[`target-dependencies.md`](target-dependencies.md).

## Manifest syntax

```toml
[package]
name = "demo"
version = "0.1.0"

[dependencies]
fmt = ">=10 <11"
zlib = { version = ">=1.2", system = true }

[build-dependencies]
codegen = "^1.2"

[dev-dependencies]
gtest = "^1.14"
```

`[dependencies]`, `[build-dependencies]`, and `[dev-dependencies]`
accept the following value forms:

- bare version-requirement string (`name = ">=10"`),
- `{ version = "..." }`,
- `{ path = "../local" }`,
- `{ workspace = true }` (looks up the matching
  `[workspace.<kind>-dependencies]` table — see below),
- `{ version = "...", system = true }` —
  externally-provided system dep, probed via `pkg-config` at
  build time.

`system = true` is mutually exclusive with `path`, `workspace`,
`git`, `registry`, `source`, `features`, `default-features`, and
`optional`; mixing the flag with any of those surfaces a clear
parser error. Every declared `system = true` dependency is
required — the manifest has no `required` field.

## Resolver behaviour

The Cabin resolver runs over the union of dependency kinds that
participate in *ordinary* commands:

- **Normal** dependencies — included.
- **Build** dependencies — included for resolution, lockfile,
  fetch, and metadata reproducibility. They are not put on
  `PATH` yet.
- **Dev** dependencies — **excluded by default**. Declaration
  only for ordinary commands. They round-trip through metadata
  but are not resolved, fetched, or built. `cabin test`
  activates them as real graph edges for the *selected* primary
  packages so test executables can link against test-only
  packages — see [`docs/testing.md`](testing.md). The activation
  never propagates: a transitive dep's own dev-deps stay
  declaration-only.
- **System** dependencies — **never resolved**. They never reach
  the resolver.

The same package name may appear under multiple kinds (e.g. a
package used both as a normal dep and as a build dep). When the
resolver runs over a request that includes the same package
across multiple resolved-by-default kinds, all the requirements
are joined and resolved to a single compatible version. If the
constraints across kinds are mutually incompatible, the resolver
reports a clear conflict.

## Lockfile behaviour

Old `cabin.lock` files remain valid. The lockfile records
resolved package versions; dependency-kind metadata is not
duplicated there because the resolver re-runs from the manifest
on every command and re-decides which kinds to include. New
manifests with build deps simply produce a lockfile that covers
more packages.

## Workspace dependency inheritance

`[workspace]` roots may declare shared requirements per kind:

```toml
[workspace]
members = ["packages/*"]

[workspace.dependencies]
fmt = ">=10"

[workspace.build-dependencies]
codegen = "^1"

[workspace.dev-dependencies]
gtest = "^1.14"
```

A member then opts into the workspace requirement with
`{ workspace = true }`:

```toml
[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = { workspace = true }       # looks up [workspace.dependencies]

[build-dependencies]
codegen = { workspace = true }   # looks up [workspace.build-dependencies]

[dev-dependencies]
gtest = { workspace = true }     # looks up [workspace.dev-dependencies]
```

The lookup is **strictly kind-specific**: a `{ workspace = true }`
under `[build-dependencies]` does not fall back to
`[workspace.dependencies]`. If the matching workspace table does
not declare the dependency, Cabin reports an explicit error
naming the section pair. `system = true` entries cannot use
`workspace = true`; the two flags are mutually exclusive (the
parser rejects the combination).

## Command behaviour

| Command                              | Behaviour                                                                                                |
|--------------------------------------|----------------------------------------------------------------------------------------------------------|
| `cabin metadata`                     | Reports each Cabin package dep with its `dependency_kind`, plus a separate `system_dependencies` array.   |
| `cabin resolve` / `update` / `fetch` | Walks normal + build deps; excludes dev deps; never sees system deps.                                    |
| `cabin build`                        | Same resolution as above. Only **normal**-kind edges link into ordinary C++ targets — build / dev deps cannot resolve through `target.<X>.deps`. |
| `cabin test`                         | Walks normal + build deps **plus** `[dev-dependencies]` of the selected primary packages, so `cpp_test` targets can depend on test-only packages. Dev-dep activation never propagates to transitive deps. |
| `cabin package`                      | Includes per-kind dependency tables and `system-dependencies` in the canonical metadata document.         |
| `cabin publish --dry-run`            | Validates the same metadata; never touches the registry.                                                  |
| `cabin publish --registry-dir`       | Publishes the per-kind metadata into the local file registry.                                             |

Output ordering is deterministic in every command: dependency
kinds iterate in canonical order (`normal`, `build`, `dev`),
names sort ascending within each kind, and system deps sort by
name.

## Package and index metadata

`cabin package` emits a canonical per-version metadata document
that round-trips dependency kinds. The on-disk shape is:

```json
{
  "schema": 1,
  "name": "demo",
  "version": "0.1.0",
  "dependencies":      { "fmt":     ">=10 <11" },
  "build-dependencies":{ "codegen": "^1.2" },
  "dev-dependencies":  { "gtest":   "^1.14" },
  "system-dependencies": {
    "zlib": { "version": ">=1.2" }
  },
  "yanked":   false,
  "checksum": "sha256:...",
  "source":   { "type": "archive", "path": "...", "format": "tar.gz" }
}
```

Empty kind tables are omitted, so manifests that only use
`[dependencies]` produce the exact byte-for-byte metadata they
always did.

The local file index and the sparse HTTP index use the same
shape. **Old index entries that omit the new fields stay
readable**: an entry with only `dependencies` is interpreted as
having no build / dev deps for that version.

## Optional dependencies

Cabin package dependencies in `[dependencies]` and
`[build-dependencies]` may declare `optional = true`:

```toml
[dependencies]
openssl = { version = "^3", optional = true }
```

Optional dependencies only enter ordinary resolution / fetch /
build when a feature enables them via `dep:<name>` or
`<name>/<feature>` from `[features]`. Until then they appear in
package / index metadata but never in the resolver input or the
lockfile. Per-edge `features = [...]` and `default-features =
false` are also supported on these kinds and are applied
additively across all dependency edges that include the same
package — see
[`features-options-variants.md`](features-options-variants.md)
for the full feature-resolution behaviour.

`optional = true` is **not** supported on `[dev-dependencies]`
or on `system = true` entries. The manifest layer reports the
violation as `OptionalNotSupportedForKind` (dev) or
`SystemConflictsWith` (system).

## Limitations

- **Registry optional-dep activation is conservative.** Optional
  dependencies declared by registry packages are preserved in
  metadata, but the resolver does not yet activate them from
  transitive registry-package feature state.
- **System deps are primary-only and pkg-config-only.** Cabin
  invokes pkg-config for active `system = true` entries declared
  by selected primary packages. It does not probe system
  declarations from local path or registry dependencies, does not
  call vcpkg / conan / apt / brew, and never attempts a network
  probe or install.

## Examples

### Library with a code generator

```toml
[package]
name = "fast-codec"
version = "0.4.1"

[dependencies]
fmt = ">=10"

[build-dependencies]
codegen = "^1"
protoc = "^25"
```

`fmt` is linked into the `cpp_library`. `codegen` and `protoc`
are resolved and fetched, but Cabin does not yet put them on
`PATH` — invoking fetched executables is future work.

### Application that depends on a system library

```toml
[package]
name = "app"
version = "0.1.0"

[dependencies]
mylib = ">=2"
zlib = { version = ">=1.2", system = true }
```

`zlib` is the responsibility of whoever builds the package — it
is not fetched through Cabin. At build time, Cabin probes it
via `pkg-config` and merges the resulting cflags / ldflags into
the compile commands; `cabin metadata` reports the declaration
in a separate `system-dependencies` block alongside the regular
`dependencies` array.
