# Package Archive And Canonical Metadata

`cabin package` and `cabin publish --dry-run` run the same staging pipeline:

1. Validate the package's `cabin.toml`.
2. Walk the source tree under a fixed include / exclude policy.
3. Build a deterministic `.zip` archive whose root holds `cabin.toml`.
4. Compute the archive's SHA-256 digest.
5. Generate canonical per-version JSON metadata in the same shape a Cabin file registry serves to
   consumers.
6. Write both files into `--output-dir` (default `dist/`).

`cabin publish --dry-run` adds a "no registry was modified" report.  The dry-run path is
**local-only**: nothing is uploaded, no registry mutation happens, and `cabin publish` without
`--dry-run` exits with a clear error unless a local `--registry-dir` is provided or, behind
`-Z remote-registry`, an HTTP index source is in effect
([`remote-registry.md`](remote-registry.md)).

## Source archive format

Archives are zip files in the strict profile `registry/docs/archive-format.md` defines, conforming
to the extractor contract:

- the archive root contains a file named `cabin.toml`;
- only regular files are emitted (directories are implied, never stored); symlinks, hard links, and
  other non-file entries are rejected with a clear error;
- all entry paths are relative; a path containing `..`, an absolute path, one that would escape the
  package root, or a non-portable component (`\`, `:`, a control character, `< > " | ? *`, a
  trailing dot or space, or a reserved Windows device name) is rejected.

The default `<output-dir>/<stem>-<version>.zip` filename is conventional - the stem flattens a
scoped name (`fmtlib/fmt` -> `fmtlib-fmt`) so the file stays self-identifying; the in-archive path
layout is what registries and extractors care about.

## Extraction safety contract

Cabin downloads registry archives, verifies their SHA-256 against the lockfile or index, and
extracts them.  Extraction assumes the archive is **hostile**.  The trust model is bidirectional:
the hosted registry inspects what it serves (see
[`remote-registry.md`](remote-registry.md#the-verifiers-checks)), but a client also talks to
third-party registries, local file registries, and — if a registry were ever compromised — to an
attacker.  The rules below are the ones that *must* hold, because the client cannot assume a
well-behaved registry.  They are not gated behind any `-Z` flag.

An archive is refused, with a typed error naming the offending entry, when any entry:

| Rejected | Why |
| --- | --- |
| has an absolute path, a `..` component, or a joined destination outside the target | directory traversal |
| is not a regular file or a directory | symlinks, hard links, devices, fifos, and sparse entries can redirect writes outside the target or materialize special files.  Foundation ports opt into *skipping* symlink entries (never materializing them); see [`foundation-ports.md`](foundation-ports.md) |
| resolves to a destination another entry already claimed | a duplicate would silently "last-win", so the bytes a reviewer read are not the bytes the build gets |
| is a regular file that another entry uses as a parent directory (`foo` plus `foo/bar`) | no consistent extraction exists |
| declares a header `size` that disagrees with the bytes the archive holds | a short read would silently truncate a source file |
| contains a component that a Windows filesystem aliases to a different destination — a trailing `.`, a leading or trailing space, a `:` (drive or NTFS alternate-data-stream separator), or a reserved DOS device name (`NUL`, `CON`, `COM1`, the superscript `COM¹`/`LPT²` forms, …) | two such entries collide, or the write routes to a device instead of a file.  Rejected on every platform so a Linux-built archive cannot smuggle them to a Windows client.  Case-only collisions (`a.c` vs `A.C`) are the one alias *not* rejected — they would refuse archives legitimate on case-sensitive Linux, and both entries still land inside the target (content confusion, not escape) |

Every extraction is bounded by these named limits:

| Limit | Value | Bounds |
| --- | --- | --- |
| entry path length | 256 bytes | per-entry allocation, and nesting depth transitively |
| entry count | 10 000 | inodes materialized from cheap-to-ship headers |
| per-entry decompressed bytes | 256 MiB | one bomb entry |
| aggregate decompressed bytes | 1 GiB | total disk written |
| whole decompressed stream | `min(max(32 x compressed size, 64 MiB), 1 GiB + framing)` | amplification: a small download cannot inflate toward the absolute cap.  The 64 MiB floor exists because tar framing is mostly zeros and compresses far better than any content ratio, so small legitimate archives "expand" 15-30x on framing alone |
| tar framing and metadata records | 4 KiB x the entry count (40 MiB) | **memory**.  The tar reader buffers a GNU long-name or PAX record in full before the entry it decorates, and therefore before any type or path check can reject it |

One duplicate shape is deliberately *not* caught: two entries whose paths differ only by case
(`foo.c` and `FOO.c`).  They are distinct files on a case-sensitive filesystem, and rejecting them
would refuse archives that are legitimate on Linux; on a case-insensitive filesystem (the default
on macOS and Windows) the second overwrites the first.  Both still land inside the extraction
target, so this is a content-confusion hazard, not a containment one.

The caps are enforced against the bytes actually decompressed, never the sizes an archive's headers
claim.  They are a *consumer* contract: `cabin package` does not itself enforce them, so a
pathological hand-built source tree (a 300-byte path, or more than 10 000 files) can produce an
archive that this extractor — and the registry verifier — rejects.  What is guaranteed is that
every archive produced from a *conventional* source tree passes with wide margin: a real C/C++
package sits orders of magnitude below every limit, and a test packages every example under
`examples/`, asserts the margin against each cap directly, and round-trips it through the extractor
on every platform CI covers.

Extraction is atomic within a run.  The tree is built in a sibling scratch directory unique to the
process and renamed into place only after it extracts, validates, and — for a cold-cache foundation
port — finishes its copies, overlay, and identity cross-check.  A rejected archive therefore leaves
no partial source tree, no completion marker, and no scratch directory behind.  This is the
guarantee that matters for the hostile-archive threat model, and it holds regardless of concurrency.

Cross-process cache coordination is *not* lock-protected, and this change does not add it.  When two
processes materialize the same entry at once, each builds its own scratch tree and one wins the
rename; the loser's rename fails, it discards its scratch, and it surfaces the error so its caller
retries (finding the winner's now-complete entry on the retry).  A torn interleaving can still
transiently leave a completion marker beside a directory another process is rebuilding; the next run
detects the mismatch and re-extracts.  These are liveness edges of a lockless content-addressed
cache, not safety holes — no hostile archive escapes through them, and they predate this change.

Zip archives - the registry package format, and some foundation-port upstreams - obey the same
rules, including the header-size and duplicate checks.  They have no separate whole-stream ratio
cap: zip decompresses per entry, so the per-entry
caps already bound every decompressed byte.  Zip metadata is read from bytes the archive really
contains rather than from a compressed stream that can amplify, so its memory cost is bounded by the
archive file size — which is itself capped at the aggregate limit, since the central directory
cannot exceed the file.

The "whole decompressed stream" cap and the framing/metadata budget apply to the bytes the tar
reader actually pulls.  A valid tar ends at its terminator, and the reader stops there, so content
appended *after* the terminator is neither decompressed nor materialized — the client does not
inspect it (the hosted verifier does, since it is checking archives it will serve). This is why the
cap bounds real memory and disk regardless of any trailing bytes.

## Determinism

The same logical input always produces byte-identical archives.  The archive is a zip in the strict
profile `registry/docs/archive-format.md` defines; the producer pins the bytes that make it
reproducible:

- the file enumeration is sorted lexicographically by relative path;
- every entry's last-modified time is pinned to `1980-01-01 00:00:00` (the zip DOS-time epoch), so
  the archive does not embed when it was built;
- version-made-by is pinned to the Unix system code, so a build on Windows is byte-identical to one
  on Unix;
- files are deflated at a fixed compression level; only regular-file entries are emitted, with a
  fixed `0o644` permission mode; directories are implied, never stored, and the extractor ignores
  the stored mode;
- no zip64, no data descriptors, no extra fields, and no comments, so the container embeds no
  incidental bytes that depend on where the build ran.

`cabin package` re-running with identical input succeeds silently because the on-disk artifact
already matches what the current run would produce.  If the on-disk archive or metadata file has
different bytes the run fails with `output file already exists with different bytes`; remove the
file and re-run.

## Include / exclude policy

By default the archive includes every regular file under the package root and excludes a small fixed
list of generated / dependency / VCS artifacts:

| Excluded | Reason |
| --- | --- |
| `.git/`, `.hg/`, `.svn/` | VCS state. |
| `.cabin/` | Package-local Cabin caches plus the typed `.cabin/config.toml` file (config patches and `[source-replacement]` declarations live here).  Local config is *user policy*, not package source spec, so it never enters published archives.  See [`config.md`](config.md) and [`patch-overrides.md`](patch-overrides.md). |
| `build/` | Default `cabin build` output directory. |
| `dist/` | Default `cabin package` output directory. |
| `node_modules/` | Convention for JavaScript-style dependencies. |
| `.DS_Store` | macOS Finder metadata. |
| `compile_commands.json` | Generated tooling index. |
| `build.ninja` | Generated Ninja build file. |
| `cabin.lock` | Generated lockfile. |
| `credentials.toml` | Cabin registry bearer credentials. This filename is matched case-insensitively and excluded anywhere in the tree, including when `CABIN_CONFIG_HOME` points inside a package root. |

Cabin matches directory names anywhere in the tree.  Nested submodules and build trees stay out of
the archive.

A `.cabinignore` file format is **not** part of the current local archive format and should not be
implemented opportunistically.

## Package validation

Before archive bytes are written, `cabin-package` validates:

- the manifest parses (existing `cabin-manifest` rules);
- the manifest contains a `[package]` table - workspace-only roots are rejected with `cannot package
  workspace root without a [package] section; pass --manifest-path for a package`;
- the package name parses under the `PackageName` grammar (`cabin-manifest` enforces this at load
  time): a bare `name` or a scoped `<scope>/<name>` with exactly one `/`.  Bare names stage locally
  but cannot be published - `cabin publish` (including `--dry-run`) rejects them with a diagnostic
  showing the `name = "..."` line to change;
- no declared dependency uses `path = "..."`.  Path dependencies are not publishable and produce
  `cannot package path dependency `foo`; path dependencies are not publishable`;
- no declared dependency uses `{ workspace = true }` without workspace context.  The CLI passes a
  workspace-resolved `Package` from `cabin-workspace::load_workspace` when packaging a workspace
  member, so the inherited requirement is baked into the canonical metadata and the archived
  `cabin.toml` is normalized: `{ workspace = true }` dependency entries are rewritten to the literal
  requirement string from the matching `[workspace.<kind>-dependencies]` table, in the workspace
  root's original spelling.  A marker-only entry collapses to the bare-string form (`fmt = ">=10
  <11"`); sibling keys such as `features` are preserved.  A standalone `cabin package
  --manifest-path <member>/cabin.toml` (no workspace context) errors with `dependency 'foo' uses
  workspace = true, but package metadata was generated without workspace resolution`;
- no `[package]`-level standard field uses `{ workspace = true }` without workspace context.  With
  workspace context, the inherited value is baked into the canonical metadata and the marker-bearing
  standard fields in the archived `cabin.toml` are rewritten to their resolved literals (see
  [`language-standards.md`](language-standards.md)).  A standalone `cabin package` against a
  marker-bearing manifest errors with `` `cxx-standard` uses workspace = true, but package metadata
  was generated without workspace resolution``;
- target source paths and include directories stay inside the package root (lexically - `..` walking
  is rejected).
- the manifest does not declare a `[patch]` table.  Patches are local development policy; `cabin
  package` returns `package "<name>" declares a [patch] table; patches are local development policy
  and not publishable.  Remove the [patch] table from this manifest before packaging, or move the
  patches to a .cabin/config.toml file.` See [`patch-overrides.md`](patch-overrides.md).

The two workspace-marker rewrites above (dependency entries and standard fields) are the only case
where an archived `cabin.toml` differs from the on-disk bytes; packaging a workspace-inheriting
member produces an archive byte-identical to a literal-declaring twin.

Symlink / hard-link / unsupported file-type errors are raised later, during archive enumeration:

- `refusing to package symlink `<path>` because symlinks are not supported`
- `refusing to package `<path>` because only regular files and directories are supported`

If the archive enumeration yields no `cabin.toml` at the root the run fails with `package archive
would not contain cabin.toml`.

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
    "path": "../artifacts/fmt/fmt-10.2.1.zip",
    "format": "zip"
  }
}
```

| Field | Description |
| --- | --- |
| `schema` | Always `1`. |
| `name`, `version` | From `[package].name` and `[package].version`. |
| `dependencies` | Normal-kind versioned registry dependencies (`[dependencies]`).  Path / workspace dependencies cause validation to fail before metadata is generated. |
| `dev-dependencies` *(optional)* | `[dev-dependencies]` versioned dependencies.  Omitted when empty. |
| `system-dependencies` *(optional)* | Entries declared with `system = true` in any of the dependency tables, grouped by package name: `{ name -> { "version", "dependency_kind" } }`.  Omitted when empty.  Every declared system dependency is required; the metadata document has no `required` field.  Cabin never resolves or fetches these - they round-trip purely as metadata, and `cabin build` probes them via `pkg-config`. |
| `target` *(optional, per-entry)* | Canonical inner-expression form of a `cfg(...)` predicate copied from `[target.'cfg(...)'.<kind>]`.  Present on Cabin package and system dep entries declared under a target table; absent otherwise.  The wrapping `cfg(...)` is implicit.  See [`target-dependencies.md`](target-dependencies.md). |
| `features` *(optional)* | The package's `[features]` declarations.  Omitted from the JSON when no features are declared. |
| `toolchain` *(optional)* | The workspace root's `[toolchain]` plus any `[target.'cfg(...)'.toolchain]` overrides, exactly as written in the manifest.  Environment- or CLI-derived selections are deliberately not written here.  Omitted when no `[toolchain]` table was declared.  See [`toolchains.md`](toolchains.md). |
| `build` *(optional)* | The package's `[profile]` plus any general `[target.'cfg(...)'.profile]` and named `[target.'cfg(...)'.profile.<name>]` overrides.  Named entries carry an optional profile-name discriminator.  Omitted when empty. |
| `compiler_wrapper` *(optional)* | The workspace root's `[build] compiler-wrapper` declaration, written as the typed compiler-wrapper request. Environment- or CLI-derived wrapper selections are deliberately not written here. Omitted when no wrapper was declared. See [`compiler-cache.md`](compiler-cache.md). |
| `yanked` | Always `false` from `cabin package`. |
| `checksum` | `sha256:<hex>` digest of the archive bytes the run produced. |
| `source.type` | Always `"archive"`. |
| `source.format` | Always `"zip"`. |
| `source.path` | File-registry relative reference: `../artifacts/<name>/<name>-<version>.zip` for a bare name, `../../artifacts/<scope>/<name>/<scope>-<name>-<version>.zip` for a scoped one (the index document nests one scope directory deeper and the filename embeds the scope).  Dry-run staging records this value for parity with the package-index `source` block.  It does not publish that path. |

The metadata document is rendered with `serde_json::to_string_pretty` in struct-declaration order,
dependencies sorted by name, and a trailing newline.  Repeated runs over the same input produce the
same bytes.

## CLI surface

```sh
cabin package \
  [--manifest-path <path>] \
  [--output-dir <path>] \
  [--format human|json]
```

Default `--manifest-path` is `cabin.toml`, default `--output-dir` is `dist`, default `--format` is
`human`.

```sh
cabin publish --dry-run \
  [--manifest-path <path>] \
  [--output-dir <path>] \
  [--format human|json]
```

Same defaults.  `cabin publish` without `--dry-run`, without `--registry-dir`, and without an
effective HTTP index source exits with a clear error naming both options.  Behind
`-Z remote-registry`, an HTTP index source (`--index-url` or the `[registry] index-url` config
setting) publishes the same staged bytes remotely; see
[`remote-registry.md`](remote-registry.md).

## Output layout

```
dist/
  <stem>-<version>.zip
  <stem>-<version>.json
```

`<stem>` is the bare name, or `<scope>-<name>` for a scoped package - the two files always sit flat
in the output directory.

Re-running with identical input is idempotent: if the on-disk file already matches the current run's
bytes, the file is left alone and the run succeeds.  If the bytes differ, the run fails with `output
file already exists with different bytes`; the user is expected to remove the file and re-run.

## File-Registry Publish

File-registry publish runs on top of the same staging pipeline.  `cabin publish --registry-dir
<path>` calls `cabin-package`'s `stage()` to produce the same archive bytes + canonical metadata,
then hands them to `cabin-registry-file` to write into a local file registry.  The registry layout
is described in [`registry-design.md`](registry-design.md); the on-disk shape of each per-package
file is the same one this document defines.

Behavioral notes specific to registry publish:

- Registry packages are always scoped: the publish is rejected (before any lint or write) when the
  `[package]` name has no `<scope>/` prefix, with a diagnostic showing the `name = "..."` line to
  change.  Local-only builds and path dependencies keep bare names.
- `source.path` in the registry's package index file is the registry-relative reference described
  above, regardless of what the dry-run metadata happened to carry.  The registry crate normalizes
  this so static sparse-HTTP serving can read the same layout without rewriting.
- Duplicate `(name, version)` publishes fail with a clear error.
- Existing artifact bytes are never silently overwritten; if an artifact file is present without a
  matching index entry, the publish run refuses.

## Scope

`cabin-package` and `cabin-publish` are the local-only archive and file-registry surface.  The
repository-wide scope policy in [`docs/architecture.md`](architecture.md) tracks broader registry
direction.
