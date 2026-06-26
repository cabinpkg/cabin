# Features

Cabin features are public, additive, named-boolean capabilities the user (or a downstream consumer)
selects at build time.  Features may imply other features and may enable optional dependencies on
the same package.  They are declared in the `[features]` table of `cabin.toml`.

## Manifest syntax

```toml
[package]
name = "demo"
version = "0.1.0"

[features]
default = ["simd"]
simd = []
ssl = []
full = ["simd", "ssl"]
```

### Identifier grammar

Feature names are:

- non-empty;
- ASCII letters, digits, `_`, `-`;
- no whitespace, `/`, `.`, or `:`.

This keeps ordinary feature identifiers separate from the feature-entry syntax described below.

### Rules

- The reserved `default` key holds the list of features enabled when the user does not pass
  `--no-default-features`.  It may be omitted.
- A feature value is a list of *feature entries*, each of which is one of:
 - `"feature_name"` - enables another local feature on the same package (transitive feature
   implication).
 - `"dep:dependency_name"` - enables an optional Cabin package dependency declared by this package's
   `[dependencies]` table.
 - `"dependency_name/feature_name"` - requests a feature on a Cabin package dependency.  If the
   dependency is optional this form also enables it.
- The on-disk shape stays a list of strings; the typed
  [`FeatureEntry`](https://github.com/cabinpkg/cabin/blob/main/crates/cabin-core/src/config.rs) view
  is produced lazily by the feature resolver.
- Local feature references must point at another declared feature in the same package.  Unknown
  local references are rejected with a clear error.
- Cycles between local features are rejected with a clear `feature definitions contain a cycle: a ->
  b -> a` error.
- Declaring a normal feature called `default` is rejected (the key is reserved for the default
  group).
- Feature entries may only use ASCII letters, digits, `_`, `-`, `.`, plus the leading `dep:` prefix
  or a single `/` separator.  Anything else is rejected with a clear error.

## Optional dependencies and the feature resolver

Features can also turn Cabin package dependencies on and off, and can request features on dependency
packages that are present.  This layers cleanly on top of the dependency-kind model:

```toml
[dependencies]
fmt = { version = "^10", features = ["compile"], default-features = false }
openssl = { version = "^3", optional = true }

[features]
default = []
ssl = ["dep:openssl"]
full = ["ssl", "openssl/vendored"]
```

- `optional = true` declares an optional Cabin package dependency (supported in `[dependencies]`;
  rejected in `[dev-dependencies]` and ``system = true` deps`).
- `features = ["..."]` requests features on the dependency package; entries must be feature names
  declared by that dependency.
- `default-features = false` drops *this edge's* request for the dependency's `default` feature.  It
  does **not** globally disable the dependency's defaults - if any other edge requests defaults for
  the same dependency, the unified result still includes them.

The cross-package feature resolver lives in the `cabin-feature` crate.  Given a typed
`PackageGraph`, the selected root indices, and a `RootFeatureRequest` built from the CLI flags, it
computes the *additive* closure of:

- enabled features per package;
- enabled optional dependencies per package;
- per-edge feature requests applied to the depended-on package.

Resolution is deterministic (sorted iteration, fixed-point worklist) and never touches the network.
Errors are explicit and testable: unknown root features, `dep:` on non-optional dependencies, and
requests for features the depended-on package does not declare all surface with stable wording.

Effects on commands:

- **`cabin resolve` / `update` / `fetch`** filter disabled optional dependencies declared on local
  (workspace / path) packages out of the resolver / fetch / lockfile inputs.  Optional dependencies
  declared on *registry* packages are skipped regardless of feature state.  A feature request on a
  registry package does not enable that registry package's own optional dependencies.  Dev
  dependencies remain excluded by default; system dependencies remain declaration-only.
- **`cabin build`** sees the same filtered dep set, so a disabled optional dependency never enters
  the build graph or links into ordinary C++ targets.
- **`cabin package`** preserves `optional`, `features`, and `default-features` per dependency in the
  canonical metadata document.  Bare entries without overrides serialize as plain
  version-requirement strings so older readers stay happy.

## CLI selection

`cabin build` / `cabin resolve` / `cabin metadata` accept the same selection flag bundle:

```
--features <names>          # repeatable; each value may be comma-separated
--all-features              # enable every declared feature
--no-default-features       # drop the [features].default set
```

`cabin tree` and the graph-only `cabin explain` subcommands (`package`, `target`, `source`, and
`feature`) also run the feature-selection part of this bundle so unknown features and invalid `dep:`
feature entries surface consistently.

Default behavior:

- without `--no-default-features`, the names listed under `[features].default` are enabled and
  expanded transitively;
- `--all-features` overrides everything else and enables every declared feature;
- CLI selections apply to the selected root / primary packages.  Dependency feature requests
  declared on edges are then resolved additively through the graph.

Errors are validated up-front and reported with a clear message:

```
$ cabin build --features missing
unknown feature "missing" for package "demo"
```

## `cabin metadata`

`cabin metadata --format json` includes a per-package `features` block (omitted when empty so older
consumers see the same JSON shape they always have) and a resolved `configuration` block whenever
the package declared features:

```json
{
  "configuration": {
    "features": ["simd"],
    "fingerprint": "<64 hex chars>"
  }
}
```

Selection flags passed to `cabin metadata` flow into the configuration block exactly the way they do
for `cabin build`.

## Package metadata and registry preservation

`cabin package` writes feature declarations into the `<name>-<version>.json` document next to the
archive.  `cabin publish --registry-dir <path>` carries them through the registry's per-package
index file.  `cabin-index` (local file index) and `cabin-index-http` (sparse HTTP index) parse the
optional `features` field on every version entry and preserve it on `VersionMetadata`.  Older
registry entries that omit the field continue to load.

The resolver consults package features and dependency feature requests when deciding whether
optional package dependencies declared on local (workspace / path) packages are active.  Optional
dependencies declared on registry packages are conservatively skipped: their per-edge `features` /
`default-features` requests round-trip through registry metadata, but transitive feature state for
registry packages does not gate them on.  The lockfile records resolved registry versions only;
per-`BuildConfiguration` state lives in the run-time fingerprint and is not persisted to
`cabin.lock`.
