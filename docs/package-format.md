# Package Archive And Canonical Metadata

`cabin package` and `cabin publish --dry-run` run the same staging
pipeline:

1. Validate the package's `cabin.toml`.
2. Walk the source tree under a fixed include / exclude policy.
3. Build a deterministic `.tar.gz` archive whose root holds
   `cabin.toml`.
4. Compute the archive's SHA-256 digest.
5. Generate canonical per-version JSON metadata in the same shape
   a Cabin file registry serves to consumers.
6. Write both files into `--output-dir` (default `dist/`).

`cabin publish --dry-run` adds a "no registry was modified" report.
The dry-run path is **local-only**: nothing is uploaded, no registry
mutation happens, and `cabin publish` without `--dry-run` exits with
a clear error unless a local `--registry-dir` is provided.

## Source archive format

Archives are gzipped tar files conforming to the extractor contract:

- the archive root contains a file named `cabin.toml`;
- only regular files and directories are emitted; symlinks, hard
  links, char/block devices, fifos, and other tar entry types are
  rejected with a clear error;
- all entry paths are relative; a path containing `..`, an absolute
  path, or one that would escape the package root is rejected.

The default `<output-dir>/<name>-<version>.tar.gz` filename is
conventional; the in-archive path layout is what registries and
extractors care about.

## Determinism

The same logical input always produces byte-identical archives:

- the file enumeration is sorted lexicographically by relative
  path;
- each tar header has `mtime`, `uid`, `gid` zeroed and `username`
  and `groupname` cleared so the archive does not embed who built
  it;
- mode is `0o644` for regular files; directories are implied by
  the extractor and are not emitted as explicit entries;
- the gzip header carries `mtime = 0` and OS code `0xff` (unknown)
  so the archive bytes do not depend on when or where the build
  ran.

`cabin package` re-running with identical input succeeds silently
because the on-disk artifact already matches what the current run
would produce. If the on-disk archive or metadata file has
different bytes the run fails with `output file already exists with
different bytes`; remove the file and re-run.

## Include / exclude policy

By default the archive includes every regular file under the
package root and excludes a small fixed list of generated /
dependency / VCS artifacts:

| Excluded | Reason |
| --- | --- |
| `.git/`, `.hg/`, `.svn/` | VCS state. |
| `.cabin/` | Package-local Cabin caches plus the typed `.cabin/config.toml` file (config patches and `[source-replacement]` declarations live here). Local config is *user policy*, not package source spec, so it never enters published archives. See [`config.md`](config.md) and [`patch-overrides.md`](patch-overrides.md). |
| `build/` | Default `cabin build` output directory. |
| `dist/` | Default `cabin package` output directory. |
| `node_modules/` | Convention for JavaScript-style dependencies. |
| `.DS_Store` | macOS Finder metadata. |
| `compile_commands.json` | Generated tooling index. |
| `build.ninja` | Generated Ninja build file. |
| `cabin.lock` | Generated lockfile. |

Directory names are matched anywhere in the tree, not only at the
root, so nested submodules / build trees do not leak in.

A `.cabinignore` file format is **not** part of the current local
archive format and should not be implemented opportunistically.

## Package validation

Before archive bytes are written, `cabin-package` validates:

- the manifest parses (existing `cabin-manifest` rules);
- the manifest contains a `[package]` table — workspace-only roots
  are rejected with
  `cannot package workspace root without a [package] section; pass --manifest-path for a package`;
- the package name is path-safe for registry publishing: names
  containing `/`, `\`, `..`, leading dots, control characters, or
  Windows drive prefixes are rejected with
  `package name "<name>" is not path-safe for registry
  publishing`;
- no declared dependency uses `path = "..."`. Path dependencies are
  not publishable and produce
  `cannot package path dependency `foo`; path dependencies are not publishable`;
- no declared dependency uses `{ workspace = true }` without
  workspace context. The CLI passes a workspace-resolved
  `Package` from `cabin-workspace::load_workspace` when
  packaging a workspace member, so the inherited requirement is
  baked into the canonical metadata and the archived
  `cabin.toml` is normalized: `{ workspace = true }` dependency
  entries are rewritten to the literal requirement string from
  the matching `[workspace.<kind>-dependencies]` table, in the
  workspace root's original spelling. A marker-only entry
  collapses to the bare-string form (`fmt = ">=10 <11"`);
  sibling keys such as `features` are preserved. A standalone
  `cabin package --manifest-path <member>/cabin.toml` (no
  workspace context) errors with
  `dependency 'foo' uses workspace = true, but package metadata
  was generated without workspace resolution`;
- no `[package]`-level standard field uses `{ workspace = true }`
  without workspace context. With workspace context, the
  inherited value is baked into the canonical metadata and the
  marker-bearing standard fields in the archived `cabin.toml`
  are rewritten to their resolved literals (see
  [`language-standards.md`](language-standards.md)). A
  standalone `cabin package` against a marker-bearing manifest
  errors with
  `` `cxx-standard` uses workspace = true, but package metadata
  was generated without workspace resolution``;
- target source paths and include directories stay inside the
  package root (lexically — `..` walking is rejected).
- the manifest does not declare a `[patch]` table. Patches are
  local development policy; `cabin package` returns
  `package "<name>" declares a [patch] table; patches are local
  development policy and not publishable. Remove the [patch]
  table from this manifest before packaging, or move the
  patches to a .cabin/config.toml file.` See
  [`patch-overrides.md`](patch-overrides.md).

The two workspace-marker rewrites above (dependency entries and
standard fields) are the only case where an archived
`cabin.toml` differs from the on-disk bytes; packaging a
workspace-inheriting member produces an archive byte-identical
to a literal-declaring twin.

Symlink / hard-link / unsupported file-type errors are raised
later, during archive enumeration:

- `refusing to package symlink `<path>` because symlinks are not supported`
- `refusing to package `<path>` because only regular files and directories are supported`

If the archive enumeration yields no `cabin.toml` at the root the
run fails with
`package archive would not contain cabin.toml`.

## Canonical metadata

```json
{
  "schema": 1,
  "name": "fmt",
  "version": "10.2.1",
  "dependencies": {
    "zlib": ">=1.2.0, <2.0.0"
  },
  "dev-dependencies": {
    "gtest": "^1.14"
  },
  "system-dependencies": {
    "openssl": { "version": ">=3" }
  },
  "features": {
    "default": ["simd"],
    "features": { "simd": [], "ssl": [] }
  },
  "yanked": false,
  "checksum": "sha256:<archive-sha256>",
  "source": {
    "type": "archive",
    "path": "../artifacts/fmt/fmt-10.2.1.tar.gz",
    "format": "tar.gz"
  }
}
```

| Field | Description |
| --- | --- |
| `schema` | Always `1`. |
| `name`, `version` | From `[package].name` and `[package].version`. |
| `dependencies` | Normal-kind versioned registry dependencies (`[dependencies]`). Path / workspace dependencies cause validation to fail before metadata is generated. |
| `dev-dependencies` *(optional)* | `[dev-dependencies]` versioned dependencies. Omitted when empty. |
| `system-dependencies` *(optional)* | Entries declared with `system = true` in any of the dependency tables, grouped by package name: `{ name -> { "version", "dependency_kind" } }`. Omitted when empty. Every declared system dependency is required; the metadata document has no `required` field. Cabin never resolves or fetches these — they round-trip purely as metadata, and `cabin build` probes them via `pkg-config`. |
| `target` *(optional, per-entry)* | Canonical inner-expression form of a `cfg(...)` predicate copied from `[target.'cfg(...)'.<kind>]`. Present on Cabin package and system dep entries declared under a target table; absent otherwise. The wrapping `cfg(...)` is implicit. See [`target-dependencies.md`](target-dependencies.md). |
| `features` *(optional)* | The package's `[features]` declarations. Omitted from the JSON when no features are declared. |
| `toolchain` *(optional)* | The workspace root's `[toolchain]` plus any `[target.'cfg(...)'.toolchain]` overrides, exactly as written in the manifest. Environment- or CLI-derived selections are deliberately not written here. Omitted when no `[toolchain]` table was declared. See [`toolchains.md`](toolchains.md). |
| `build` *(optional)* | The package's `[profile]` plus any `[target.'cfg(...)'.profile]` overrides. Omitted when empty. |
| `compiler_wrapper` *(optional)* | The workspace root's `[profile.cache]` plus any `[target.'cfg(...)'.profile.cache]` overrides, written as the typed compiler-wrapper declaration model. Environment- or CLI-derived wrapper selections are deliberately not written here. Omitted when no cache table was declared. See [`compiler-cache.md`](compiler-cache.md). |
| `yanked` | Always `false` from `cabin package`. |
| `checksum` | `sha256:<hex>` digest of the archive bytes the run produced. |
| `source.type` | Always `"archive"`. |
| `source.format` | Always `"tar.gz"`. |
| `source.path` | File-registry relative reference: `../artifacts/<name>/<name>-<version>.tar.gz`. Dry-run staging does not publish that path, but the value matches the package-index `source` block shape used by file-registry publish. |

The metadata document is rendered with `serde_json::to_string_pretty`
in struct-declaration order, dependencies sorted by name, and a
trailing newline. Repeated runs over the same input produce the
same bytes.

## CLI surface

```sh
cabin package \
  [--manifest-path <path>] \
  [--output-dir <path>] \
  [--format human|json]
```

Default `--manifest-path` is `cabin.toml`, default `--output-dir` is
`dist`, default `--format` is `human`.

```sh
cabin publish --dry-run \
  [--manifest-path <path>] \
  [--output-dir <path>] \
  [--format human|json]
```

Same defaults. `cabin publish` without `--dry-run` and without
`--registry-dir` exits with
`actual publishing requires --registry-dir, or use --dry-run`.

## Output layout

```
dist/
  <name>-<version>.tar.gz
  <name>-<version>.json
```

Re-running with identical input is idempotent: if the on-disk file
already matches the current run's bytes, the file is left alone and
the run succeeds. If the bytes differ, the run fails with
`output file already exists with different bytes`; the user is
expected to remove the file and re-run.

## File-Registry Publish

File-registry publish runs on top of the same staging pipeline.
`cabin publish --registry-dir <path>` calls
`cabin-package`'s `stage()` to produce the same archive bytes +
canonical metadata, then hands them to `cabin-registry-file` to
write into a local file registry. The registry layout is described
in [`registry-design.md`](registry-design.md); the on-disk shape
of each per-package file is the same one this document defines.

Behavioral notes specific to registry publish:

- `source.path` in the registry's `packages/<name>.json` is the
  registry-relative
  `"../artifacts/<name>/<name>-<version>.tar.gz"`, regardless of
  what the dry-run `dist/<name>-<version>.json` happened to
  carry. The registry crate normalizes this so static sparse-HTTP
  serving can read the same layout without rewriting.
- Duplicate `(name, version)` publishes fail with a clear error.
- Existing artifact bytes are never silently overwritten; if an
  artifact file is present without a matching index entry, the
  publish run refuses.

## Scope

`cabin-package` and `cabin-publish` are the local-only archive
and file-registry surface. The repository-wide scope policy in
[`docs/architecture.md`](architecture.md) tracks broader registry
direction.
