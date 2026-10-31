# Workspaces

Cabin treats a workspace as a package graph rooted at one `cabin.toml` that declares a `[workspace]`
table.  The root manifest may itself be a package (`[package]` is allowed alongside `[workspace]`)
or a pure workspace root (`[workspace]` only).

Cabin workspaces support:

- recursive **member discovery** through path globs;
- **`[workspace.exclude]`** to drop unwanted directories;
- **`[workspace.default-members]`** to pick a subset for the no-flag default;
- **`[workspace.dependencies]`** / **`[workspace.dev-dependencies]`** plus `dep = { workspace = true
  }` for shared, kind-specific dependency requirements;
- **workspace standard defaults** - shared language-standard values on `[workspace]` that members
  opt into per field with `<field> = { workspace = true }`;
- **root discovery from member directories** so commands invoked anywhere under the workspace find
  the workspace root;
- consistent **package selection** flags across the commands that operate on a workspace.

All workspace operations are local-only and deterministic.

## Manifest syntax

```toml
[workspace]
members = [
  "libs/*",
  "tools/driver",
]
exclude = [
  "libs/experimental",
  "third_party/*",
]
default-members = [
  "libs/core",
  "tools/driver",
]

[workspace.dependencies]
fmt    = ">=10 <11"
spdlog = "^1.12"
```

A member `cabin.toml` opts into a shared dependency with:

```toml
[dependencies]
fmt = { workspace = true }
```

### Rules

- `members` and `exclude` entries are paths or single-`*` trailing globs (e.g.  `libs/*`).
  Multi-level globs (`a/*/b`) are intentionally rejected with a clear error.
- Excluded paths are removed from the candidate set before any member is loaded.  An exclude pattern
  that does not drop at least one member is reported as `unused exclude pattern`.
- `default-members` entries must resolve to declared members.  Unknown entries produce ``workspace
  default member `libs/missing` is not listed in workspace.members``.
- Duplicate member paths are deduplicated deterministically; the resulting member order is sorted.
- Two workspace members may not share a `[package].name`.
- Nested workspaces are rejected.  The loader rejects the case where a member directory's
  `cabin.toml` declares its own `[workspace]` table; the upward discovery walk additionally errors
  when a `cabin.toml` with `[workspace]` sits above another `cabin.toml` with `[workspace]`
  regardless of whether the outer claims the inner as a member (see "Workspace root discovery"
  below).
- `dep = { workspace = true }` looks up the `[workspace.<kind>-dependencies]` table that matches the
  section it was declared in (`[dependencies]` -> `[workspace.dependencies]`, `[dev-dependencies]`
  -> `[workspace.dev-dependencies]`).  The lookup is strictly kind-specific - a `{ workspace = true
  }` under `[dev-dependencies]` does not fall back to `[workspace.dependencies]`.  If the matching
  workspace table does not declare the dependency, Cabin reports a clear error naming the
  dependency, the declaring section, and the expected workspace section.
- `workspace = true` cannot be combined with `path = "..."` or `version = "..."`; pick exactly one
  source.
- Published members are self-contained: `cabin package` rewrites `{ workspace = true }` entries in
  the archived `cabin.toml` to the workspace table's literal requirement strings, so a consumer
  never re-resolves them against its own workspace.  See [`package-format.md`](package-format.md).

### Workspace standard defaults

The `[workspace]` table also accepts the four language-standard fields as shared defaults:

```toml
[workspace]
members = ["packages/*"]
cxx-standard = "c++20"
```

```toml
# packages/core/cabin.toml
[package]
name = "core"
version = "0.1.0"
cxx-standard = { workspace = true }
```

- `c-standard`, `cxx-standard`, `interface-c-standard`, and `interface-cxx-standard` are accepted on
  `[workspace]` with literal values only.
- A member opts in per field with `<field> = { workspace = true }` on `[package]`.  Opting in
  counts as declaring; a member that does not opt in inherits nothing, and - since Cabin has no
  built-in standard defaults - must declare its own standard for every language it compiles.
- The lookup is field-specific.  If the workspace root does not declare the opted-into field, Cabin
  reports a clear error naming the package, the field, and the expected `[workspace]` location.
- The marker is only valid on `[package]`-level fields; a `{ workspace = true }` on a
  `[target.<name>]` standard field is rejected.
- The workspace root's own `[package]` may opt into the root's `[workspace]` values.
- See [`language-standards.md`](language-standards.md) for the full semantics: precedence, the
  escape-hatch conflict rule, interface enforcement, and publish-time archive normalization.

## Workspace root discovery

When the user runs `cabin <subcommand>` without an explicit `--manifest-path`, Cabin walks
**upward** from the current directory and looks for a `cabin.toml` whose root declares a
`[workspace]` table.

- **Zero** workspace roots above the cwd -> fall back to `./cabin.toml` exactly as before.
- **Exactly one** workspace root -> use it as the entry point.
- **Two or more** stacked workspace roots -> discovery errors out with `nested workspace detected:
  nearest workspace is <inner> but outer workspace is <outer>`.  This rule strict: previous releases
  either silently picked the outer or let the loader's member-list rejection produce a
  similar-looking error only when the outer happened to claim the inner as a member.  The strict
  rule means stacking workspaces is always surfaced to the user, regardless of how the outer's
  `[workspace]` table is configured.

A root found **above the current directory** is the trust boundary for every setting discovery applies
on the user's behalf (`[toolchain]`, `[patch]`, the root's `.cabin/config.toml`), so it is honored only
under these rules:

- On Unix, the root manifest (the file itself, not a symlink or hard-link target) and its directory
  must have the same owner as the nearest package manifest below it, or as the current directory when
  no package manifest lies between, and neither may be world-writable or a symlink; the root's
  `.cabin/config.toml` and `.cabin/` directory, when present, pass the same owner, mode and
  no-symlink checks.  Otherwise discovery errors with `owned by a different user,
  hard-linked, world-writable, or a symlink`.  This stops a world-writable ancestor
  such as `/tmp` or a shared drop directory from planting a root, and a world-writable manifest or
  config file from being edited into one, whose toolchain would run for every package below it.  Group-writable team checkouts pass: anyone who can write to the root
  directory is trusted as its owner is.  Windows has no owner or mode check.
- A package the root lists as a member (directly or through a glob) adopts the root, from the package
  directory or any subdirectory of it.
- A package the root `exclude`s builds standalone, as if no root were above it.
- A package the root neither lists nor excludes errors with `is not a member of the workspace`; add it
  to `members` (or, when a `members` glob already matches it, to `exclude`), or pass `--manifest-path`.
- A directory with no package manifest of its own (a workspace's `docs/`, say) adopts the nearest root
  above it, subject to the owner rule.

When discovery returns an error, the user is expected to disambiguate by passing `--manifest-path`
explicitly.  A user-supplied `--manifest-path /some/path/cabin.toml` always wins - root discovery
only triggers when the user did not pass `--manifest-path` at all.

Discovery never touches the network and never crosses unusual filesystem boundaries (it stops at the
filesystem root).

## Package-selection flags

The same flag bundle applies to `cabin build`, `cabin metadata`, `cabin resolve`, `cabin fetch`,
`cabin package`, and `cabin publish`:

```
--workspace                      operate on every workspace member
-p, --package <PACKAGE>          operate on the named member; repeatable
--default-members                operate on [workspace.default-members]
--exclude <PACKAGE>              drop a member from --workspace / default
```

### Default behavior with no flags

| Context | Selected packages |
|---|---|
| Single-package project | That package. |
| Workspace root with `[workspace.default-members]` | The declared default-members. |
| Workspace root without `[workspace.default-members]` | **All** workspace members. |
| Inside a member directory | Same as the workspace root above (root discovery picks it up). |

### Constraints

- `--workspace`, `-p / --package`, and `--default-members` are mutually exclusive.
- **Selection flags:** `--exclude` is only valid in combination with `--workspace` or
  `--default-members`, so a typo on a single-package project surfaces a clear error rather than
  silently doing the wrong thing.
- Unknown package names (whether selected or excluded) produce `package 'foo' is not a member of
  this workspace; available members: alpha, beta, gamma`.

### Per-command notes

- **`cabin metadata`** reports `workspace.members`, `workspace.default_members`,
  `workspace.excluded_members`, and `workspace.selected_packages`.  All four lists are sorted by
  package name (or path, for `excluded_members`) so the JSON shape is deterministic.
- **`cabin build`** plans only the C/C++ targets in the selected packages.  `cabin build` does not
  offer a single-target selector flag, so the build always enumerates every default-buildable target
  in the selected packages.  Unselected packages are not built, so the resulting `build.ninja` is
  the smallest graph that covers the request.
- **`cabin resolve`** walks the **selected package closure** - the resolved selection plus every
  local path-dependency reachable from it - and unions every reachable member's versioned
  dependencies into a single resolution, so a registry dep declared by a path-dep `lib` reaches
  the resolver when the user picks `app`.  The
  selection-aware closure extends all the way down into *registry materialization*: when the loader
  expands versioned dependencies into the package graph, it only requires registry entries for
  packages reachable from the selected closure.  Versioned deps of unrelated workspace members (or
  unrelated path-deps) are silently skipped, so `cabin resolve -p app` does not require the index
  to know about an unrelated member's dependency on `spdlog`.  The lockfile, by contrast, is still
  workspace-wide once produced - a member-scoped run rewrites the entries its closure resolves and
  keeps the rest (only a selection covering every member, root package included, prunes), so
  selection only affects what the loader has to materialize *for this command*.

Pure workspace roots (no `[package]`) work too: `cabin resolve --workspace` over a workspace root
that only has members with `[dependencies]` produces a lockfile rooted at a synthetic
`cabin/workspace` 0.0.0 identity.  The hosted registry reserves the `cabin` scope, and resolving
a pure workspace against any other index that carries or depends on `cabin/workspace` fails.  Member-level requirement conflicts (`fmt = "^10"` and `fmt
= "^11"` in two members) surface as a clear `incompatible workspace requirements for 'fmt'` error.
- **`cabin update`** takes `--package <name>` as the registry dependency to refresh, not as a
  workspace member.  To avoid colliding with that flag, `cabin update` exposes a *reduced*
  workspace-selection bundle - `--workspace`, `--default-members`, and `--exclude` - but **not**
  `-p` / `--package`.

Cabin makes the scope explicit: `cabin update --package
  <name>` only targets *direct* versioned dependencies of the
root package - those declared under `[dependencies]` (or the workspace-inherited equivalent) of the
manifest you are updating.  Transitive locked packages cannot be refreshed individually; to update a
transitive lockfile entry, drop the `--package` flag (`cabin update`) so resolution rolls forward
every relaxable constraint, or scope the refresh to a wider selection (`cabin update --workspace`,
etc.).  An unknown or transitive name produces "package 'foo' is not a direct versioned dependency
of `<root>`; cabin update --package only refreshes direct dependencies declared in [dependencies]".
- **`cabin fetch`** validates the workspace selection up-front (so `cabin fetch -p missing` errors
  even when the workspace has no versioned deps) and then unions selected members' versioned deps
  for the resolution.  The artifact cache itself remains workspace-flat - every required artifact is
  downloaded exactly once.
- **`cabin package`** in a workspace requires exactly one `--package <name>` selection.  The
  workspace root itself is not packageable.
- **`cabin publish`** in a workspace requires exactly one `--package <name>` selection for both
  `--dry-run` and `--registry-dir` flows.

`-p / --package <name>` always matches by package name (the `[package].name` declared by the
member).  Workspace member paths (`libs/core`) are never accepted by `--package`; they live only
inside the manifest's `[workspace] members = [...]` list.

## Worked examples

### LLVM-style monorepo

```toml
# cabin.toml at the repository root
[workspace]
members = [
  "llvm",
  "lld",
  "lldb",
  "clang",
  "clang-tools-extra",
  "compiler-rt/*",
]
exclude = [
  "third-party/*",
]
default-members = [
  "llvm",
  "clang",
]

[workspace.dependencies]
fmt = "^11"
```

```sh
# Build the default (llvm + clang).
cabin build

# Build the entire monorepo, minus the LLDB tests.
cabin build --workspace --exclude lldb

# Build only one component.
cabin build -p llvm -p clang

# Inspect what Cabin sees.
cabin metadata
```

### Per-team monorepo with shared dependencies

```toml
[workspace]
members = ["services/*", "libs/*"]

[workspace.dependencies]
fmt    = ">=10 <11"
spdlog = "^1.12"
```

```toml
# services/api/cabin.toml
[package]
name = "api"
version = "0.1.0"
cxx-standard = "c++17"

[dependencies]
fmt    = { workspace = true }
spdlog = { workspace = true }

[target.api]
type = "executable"
sources = ["src/main.cc"]
```

Bumping `fmt` from `>=10 <11` to `^12` then becomes a one-line change at the workspace root rather
than a dozen individual member edits.

## Boundaries

Workspace support covers the local package graph and the workspace-aware command surfaces documented
above: dependency kinds, feature unification, target-conditioned dependencies, profiles, toolchain
settings, compiler-cache settings, config discovery, patches, source replacement, vendoring /
offline mode, and dev / test / example target kinds all participate in workspace selection where
their owning feature requires it.

Registry access is not workspace-specific: [`remote-registry.md`](remote-registry.md) covers
reading from, publishing to, and logging in to a remote registry.  Cabin has no remote build cache.
