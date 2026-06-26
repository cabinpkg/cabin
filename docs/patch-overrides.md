# Patch, override, and source replacement

Cabin's typed local-policy layer lets a developer or workspace swap a registry-resolved package for
a local working copy (*patches*) and redirect one supported index source to another (*source
replacement*).  Both features are deliberately narrow: they cover developer / CI flows that already
worked with hand- edited paths, but they do **not** introduce new package semantics, new registry
protocols, credentials, vendoring, or publication of override state.

This document is the canonical specification.  The behavior described here is what
`cabin-core::patch`, `cabin-core::source_replacement`, `cabin-manifest`, `cabin-config`,
`cabin-workspace::patch`, the artifact pipeline in `cabin`, the lockfile, the metadata view, and the
package archiver all agree on.

## Patch syntax

Patches replace a *registry-resolved package candidate* with a local source.  Today only local-path
patches are supported.

### Workspace-root manifest

```toml
[patch]
fmt = { path = "../fmt" }
spdlog = { path = "../forks/spdlog" }
```

The `[patch]` table only applies on the *entry-point* manifest: either a single-package project's
`cabin.toml` or the workspace root's `cabin.toml`.  Member manifests that declare `[patch]` are
rejected with `patch declarations may only appear in the workspace root manifest`.

### `.cabin/config.toml`

```toml
[patch]
fmt = { path = "../forks/fmt" }
```

Config-supplied patches follow the same shape.  Relative paths resolve against the *config file's
directory* (not the manifest's).  Multiple config files may declare patches: higher- priority files
override lower files on overlap, mirroring the rest of the config layer's precedence ladder.

### Supported source kinds

| Kind   | Manifest / config syntax                  |
| ------ | ----------------------------------------- |
| `path` | `{ path = "../fmt" }`                     |

`path` is the only patch source kind today.  Any other key is rejected as an unknown field.  New
kinds would extend [`PatchSource`] explicitly.

## Patch precedence

For each patched package name, Cabin walks the following layers top-down and keeps the first that
declares an entry.  Higher layers fully replace lower layers on overlap.

1. `[patch]` in the file pointed at by `CABIN_CONFIG` (`explicit-config`).
2. `[patch]` in the project-local `<root>/.cabin/config.toml` (`project-config`).
3. `[patch]` in the workspace-level `<workspace-root>/.cabin/config.toml` (`workspace-config`).
4. `[patch]` in `$XDG_CONFIG_HOME/cabin/config.toml` (or its `$HOME` fallback) (`user-config`).
5. `[patch]` in the workspace-root `cabin.toml` (`manifest`).
6. No patch.

The resolved provenance label appears verbatim under `patches[].provenance` in `cabin metadata` so
the chosen layer is auditable.

## Patch validation

Before any consumer sees a resolved patch, Cabin validates each entry:

- The patch path must point at a directory containing a `cabin.toml`.  Missing files surface `patch
  for package <name> points to <path>, but that path does not contain a cabin.toml`.
- The patched package's `[package].name` must equal the patch table key.  Mismatches surface `patch
  for package <name> points to package <actual>; patch package name must match <name>`.
- For every active dependency edge that requests the patched name with a SemVer constraint, the
  patched package's `[package].version` must satisfy that constraint.  Mismatches surface `patch
  package <name> has version <ver>, which does not satisfy dependency requirement <req>`.

"Active" means the edge would contribute to the resolver input on this invocation.  Cabin skips:

 - dev / system kinds - declaration-only, never resolved by the default build;
 - `[target.<cfg>]` deps whose condition does not match the host platform - dormant on this run;
 - `optional = true` deps - feature resolution decides their membership; if a feature later enables
   one, the patched manifest is used directly and any version mismatch surfaces against the real
   resolver input.

This means a patch on `foo = ">= 99"` declared only as a dev-dep does not block validation, because
that requirement is not part of the default build closure.

- Within a single layer the same name cannot appear twice (TOML table-key uniqueness already handles
  this); across layers the higher layer wins (documented above).

## Resolver / fetch / build integration

Once patches are validated, Cabin treats them as *synthesized local-path packages*:

- The `cabin-workspace` loader stitches each patched manifest into the package graph as `kind =
  Local`.  Existing workspace-loader behaviors (cycle detection, name uniqueness, dependency edges)
  apply unchanged.
- The artifact pipeline filters patched names from versioned- dep closure detection and from the
  registry-fetch pass; the patched working copy never enters the artifact cache.
- Feature resolution, dependency-kind handling, and target-conditioned dependencies flow through the
  patched manifest exactly as they would for any path dependency.

## Source replacement syntax

Source replacement redirects one supported index source to another. **Config-only** - manifests
cannot declare source replacements.

```toml
# .cabin/config.toml
[source-replacement]
"https://example.com/index" = { index-path = "../mirror" }
"/abs/old-index" = { index-url = "https://new.example.com/index" }
```

Each row carries exactly one of `index-path` or `index-url`.  Other fields (including `git` and
`replace-with = "<name>"`) are rejected with stable error messages.

URLs containing `userinfo` (e.g., `https://user:pw@example.com/index`) are rejected at parse time so
credentials never leak into the lockfile, log output, or the metadata view.

### Replacement chain + cycle detection

When the orchestration layer opens the configured index source, it walks the replacement map once:
each hop replaces the current locator with the entry's `replacement` value, until a locator with no
replacement is reached (the *terminal* source).  Cycles surface `source replacement cycle detected:
<hop-1> -> <hop-2> -> ...` before any index is opened.

## Per-command precedence

For each command that consults a patch / source-replacement policy:

1. `--no-patches` short-circuits patch application and source- replacement resolution for the
   command's dependency / index inputs.  Manifest `[patch]` and config `[patch]` entries do not add
   replacement packages, and config `[source-replacement]` entries do not rewrite the selected index
   source.  Ordinary `path = "..."` dependency declarations and ordinary dependency edges remain
   active.
2. Otherwise the merged manifest + config policy applies as described above.

Observability commands may still render configured policy as configuration data.  In particular,
`cabin explain source --no-patches <name>` still lists the merged `[source-replacement]`
declarations under `source_replacements`; the flag means they were not applied to resolve package
inputs.

`--no-patches` is available on `cabin metadata`, `cabin build`, `cabin run`, `cabin test`, `cabin
resolve`, `cabin update`, `cabin fetch`, `cabin vendor`, `cabin tree`, and `cabin explain`.

## Lockfile behavior

The lockfile records active patch policy and active source- replacement policy as deterministic
top-level arrays:

```toml
[[patch]]
package = "fmt"
version = "10.2.1"
kind = "path"
provenance = "manifest"
path = "../fmt"

[[source-replacement]]
original = "https://example.com/index"
original-kind = "index-url"
replacement = "../mirror"
replacement-kind = "index-path"
provenance = "user-config"
```

Old lockfiles without these arrays remain valid (the parser treats the missing fields as empty).
Under `--locked`, if the recorded arrays differ from the active policy, the resolver errors with
`--locked cannot be used because active patch / source- replacement policy differs from <lockfile>;
re-run without --locked to refresh the lockfile`.

## Metadata view

`cabin metadata --format json` adds two top-level arrays:

```json
"patches": [
  {
    "package": "fmt",
    "version": "10.2.1",
    "kind": "path",
    "path": "../fmt",
    "provenance": "manifest"
  }
],
"source_replacements": [
  {
    "original": "https://example.com/index",
    "original_kind": "index-url",
    "replacement": "../mirror",
    "replacement_kind": "index-path",
    "provenance": "user-config"
  }
]
```

Both arrays are sorted (patches by package name, replacements by `original`) and contain only
entries that survived validation.  `--no-patches` empties both arrays.

## Package + publish behavior

Patches are local development policy.  They never enter:

- the canonical per-version package metadata (`cabin package` derives metadata from the typed
  `Package`, which strips patch tables before serialization);
- the source archive (`cabin package` *rejects* manifests with a non-empty `[patch]` table - see
  below);
- the file / sparse-HTTP registry index;
- the lockfile's `[[package]]` array (only the orthogonal `[[patch]]` array reflects patch state).

`cabin package` returns `package <name> declares a [patch] table; patches are local development
policy and not publishable.  Remove the [patch] table from this manifest before packaging, or move
the patches to a .cabin/config.toml file.`

Config-derived patches and source replacements live entirely inside `.cabin/`, which `cabin package`
already excludes from deterministic source archives via `EXCLUDED_DIR_NAMES`.

## Layer boundaries

These responsibilities live outside this layer and are intentionally not handled here:

- **Vendor materialization.** `cabin vendor` may consume patch / source-replacement state during
  resolution, but the on-disk write logic lives in `cabin-vendor`.
- **Offline-mode enforcement.** `--offline` / `CABIN_NET_OFFLINE` are enforced by the CLI / config
  network policy, not by the patch layer.
- **Source replacement** swaps between the existing local-path and sparse-HTTP index source kinds;
  it does not add new registry protocols, authentication, or credential handling.

## Examples

### Local fork during development

```toml
# <workspace-root>/cabin.toml
[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"

[patch]
fmt = { path = "../forks/fmt" }
```

The fork at `../forks/fmt` ships a `cabin.toml` with `name = "fmt"` and a version that satisfies
`^10`.  `cabin build` resolves `fmt` to the fork without contacting the registry; the lockfile
records the patch so `--locked` re-runs see the same state.

### Workspace-wide local index mirror

```toml
# <workspace-root>/.cabin/config.toml
[source-replacement]
"https://example.com/index" = { index-path = "../mirror" }
```

Every `cabin resolve / fetch / build / update` for this workspace uses
`<workspace-root>/.cabin/../mirror` as the effective index source.  Local config never leaks into
published metadata.

### Disabling patches for one invocation

```sh
cabin build --no-patches
```

The active manifest / config patch policy is ignored for this one run; ordinary dependency
declarations stay in effect.
