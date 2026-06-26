# Metadata, tree, and explain

Cabin gives you three complementary observability commands for the loaded workspace state:

- `cabin metadata` - full deterministic JSON document covering workspace, packages, lockfile,
  patches, source replacements, profile, toolchain, config, and the resolved `BuildConfiguration`
  per selected package.
- `cabin tree` - the loaded workspace / local-path dependency graph, rooted at the selected primary
  packages, as either a Unicode-drawing tree (`--format human`, the default) or a structured JSON
  document.
- `cabin explain` - typed answers to "why is X selected?" / "where does X come from?" / "what does
  the build configuration for X look like?", available as five subcommands: `package`, `target`,
  `source`, `feature`, `build-config`.

All three commands operate on the same workspace + lockfile + patch set + source-replacement state,
so two commands run back-to-back never see different answers about the same project inputs.  They
load the current manifests on each invocation, but do not resolve registry versions or touch the
network: you can interleave metadata / tree / explain queries against a project without altering the
lockfile, the artifact cache, or the working copy.  None of these commands materialize
registry-versioned dependencies.  The graph-shaped views (`cabin tree` and `cabin explain
package/source`) reflect the workspace closure (workspace members + local-path edges) plus any
active `[patch]` overrides.  Registry packages appear in `cabin metadata` through the lockfile
section only; they are not turned into tree nodes and their dependency edges are not walked.

## When to use which

| Want to... | Use |
|---|---|
| Pipe the full state into `jq` or another tool | `cabin metadata --format json` |
| See the dep graph at a glance | `cabin tree` |
| Filter the tree to one dependency kind | `cabin tree --kind normal` |
| See why a package is in the closure | `cabin explain package <name>` |
| See where a package's source bytes come from | `cabin explain source <name>` |
| See a target's kind, language, and deps | `cabin explain target <name>` |
| See whether a feature is enabled and what it implies | `cabin explain feature <package>/<feature>` |
| See the resolved build configuration for a package | `cabin explain build-config <name>` |

`cabin metadata` is the contract for tooling.  `cabin tree` and `cabin explain` are layered on top:
their JSON output shares schema fragments with metadata where appropriate (for example, `cabin
explain build-config <pkg>` emits exactly the same `BuildConfiguration` shape that appears under
each package's `configuration` key in metadata).

## `cabin tree`

```
$ cabin tree
app v0.1.0 (workspace)
|-- lib v0.1.0 [normal] (workspace)
`-- codegen v0.1.0 [normal] (workspace)
```

Rules:

- Roots are the selected primary packages.  `cabin tree --package <name>` narrows the roots,
  `--workspace` widens them.  Without a selection flag, a workspace root uses
  `[workspace.default-members]` when present and otherwise walks every primary workspace package.
- Children sort by `(dependency_kind, name, version)`.  The canonical kind order is `normal -> dev`.
- Repeated `(name, version)` nodes are pruned with a `(*)` marker on the first re-occurrence, so
  cyclic graphs render finitely.
- Provenance labels are surfaced in parentheses: `workspace` for workspace members, `local path` for
  `path = "..."` dependencies outside the workspace, and `patched via <layer>` for active `[patch]`
  overrides.  Registry-versioned dependencies are not materialized into `cabin tree` nodes; inspect
  `cabin metadata` lockfile entries for their checksums.
- Edges show their dependency kind in `[brackets]`.  Roots carry no edge label.

`--format json` emits the same forest as a structured document.  Each node carries `name`,
`version`, `edge_kind` (omitted for roots), `source` (a tagged union matching the human label),
`repeated`, and `children`.  Output is byte-stable across runs for the same workspace + lockfile +
config inputs.

`--kind {all|normal}` restricts the walk to one edge kind.  `all` (the default) walks every kind.
Dev edges are declaration-only and never appear in the tree.  `cabin test` activates
dev-dependencies for the selected primary packages; `cabin run` does not.  The filter therefore
exposes no `dev` value.

Feature flags (`--features`, `--all-features`, and `--no-default-features`) are parsed and validated
through the same feature resolver used by `cabin metadata`, so unknown features and invalid `dep:`
entries surface here too.

## `cabin explain`

`cabin explain` runs the same workspace / config / patch / lockfile preamble `cabin metadata` runs,
then dispatches to the selected subcommand.  Every subcommand accepts workspace selection
(`--package`, `--workspace`, `--exclude`), feature selection (`--features`, `--all-features`,
`--no-default-features`), and `--no-patches`.  `--format human` (the default) prints a concise
summary; `--format json` prints a tagged document for tooling.

Five subcommands:

### `cabin explain package <name>`

Reports the resolved package's source provenance plus every minimal path from a selected root that
reaches it.  Paths are sorted by `(length, joined name sequence)` so the answer is deterministic.
The `is_selected_root` flag distinguishes "this package is itself a root" from "this package was
pulled in by a root".

The JSON shape is `{ "kind": "package", "name", "version", "source", "paths": [ [ { "name",
"version", "edge_kind" }, ... ] ], "is_selected_root" }`.

### `cabin explain target <name>`

Reports a target's owning package, kind (the same string the manifest uses: `library`, `executable`,
`test`, ...), source-language summary (any subset of `c`, `cxx`, `rust`), declared deps (in
declaration order), and three classification flags (`is_buildable`, `is_test`, `is_dev_only`).

The JSON shape uses `"target_kind"` for the target's kind so it does not collide with the outer
`Explanation` discriminator field `"kind"`.

If the name is ambiguous within the selected packages, every declaring package is listed in the
diagnostic.

### `cabin explain source <name>`

Reports where a package's source bytes come from.  The provenance shape is the same
`SourceProvenance` enum `cabin tree` uses.  Active source-replacement entries from the merged
effective config are listed alongside, since one chain may rewrite many packages.

With `--no-patches`, `explain source` does not apply source-replacement policy to dependency / index
inputs, but it still lists the configured source-replacement declarations as observability data.
Use `cabin metadata --no-patches` when you need a metadata document whose `source_replacements`
array is empty for that invocation.

### `cabin explain feature <package>/<feature>`

Reports a feature's enablement on a specific package: whether the resolver enabled it for the
current selection, what other features it implies, and whether it is in the package's default group.
The query string must contain a single `/`; querying `default` is supported.

### `cabin explain build-config <name>`

Reports the resolved `BuildConfiguration` for the package: profile, toolchain (with the same
per-tool source labels metadata uses), build flags (with profile / manifest / condition overlays),
enabled features, and the SHA-256 fingerprint of the configuration.  A future cache layer would key
on this value.

The JSON shape wraps the existing `BuildConfiguration::as_json()` document under a top-level
`{"kind": "build-config", "package", "configuration": ...}` envelope so the inner shape matches what
`cabin metadata` already emits per package.

## Interactions with the rest of the toolchain

| Subject | Behavior |
|---|---|
| Workspace selection | `--package` / `--workspace` / `--exclude` apply uniformly across `metadata`, `tree`, and `explain` and constrain the closure both commands inspect |
| Feature selection | Feature flags run the cross-package feature resolver, so unknown features / `dep:` errors surface from `cabin metadata`, `cabin tree`, `cabin explain`, and `cabin build`. |
| Dependency kinds | Tree's `--kind` filter walks all edges (default) or `normal` edges only; explain's `package` view reports edge kind on each step.  Dev edges stay declaration-only - `cabin metadata` lists them, but tree and explain do not walk them |
| Patches | `[patch]` entries (manifest + config) light up `patched via <layer>` provenance in tree and explain. `--no-patches` disables patch application and source-replacement resolution for package inputs |
| Source replacements | Surfaced in `cabin explain source` and `cabin metadata`; `explain source --no-patches` still lists configured declarations as observability data |
| Vendoring / offline | `cabin tree` and `cabin explain` never reach the network and accept no `--index-path` / `--offline` flag of their own; they operate on the workspace closure plus active patches |
| C/C++ | The language summary in `cabin explain target` reports `c`, `cxx`, or both, classified through the same `classify_source` helper Cabin uses elsewhere |

## Determinism guarantees

Given the same workspace tree, the same lockfile, and the same merged effective config:

- `cabin metadata --format json`,
- `cabin tree --format json`,
- every `cabin explain ... --format json` subcommand,

produce byte-identical output across runs and machines (modulo non-machine-specific paths that
originate in the user's input).  The human formats are also stable: tree children sort by
`(dependency_kind, name, version)`, explanation paths sort by `(length, joined name sequence)`, and
JSON object keys are emitted in struct-declaration order through `serde`.

If you observe non-determinism, please file a bug.  Every consumer of these commands assumes
byte-stability.

## Architecture

The typed model lives in the dedicated `cabin-explain` crate.  `cabin` only orchestrates the
workspace / config / patch / lockfile / feature preamble and hands typed values to that crate.  This
mirrors the existing split between thin CLI glue and dedicated domain crates.
