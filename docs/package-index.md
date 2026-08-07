# Local JSON Package Index

Cabin resolves versioned dependencies against a tiny on-disk JSON index.  This is **not** a real
registry - it has no network protocol, no append-only structure, and no signing.  The format carries
resolver metadata plus, per version, one archive record per [packaging
revision](#packaging-revisions), so `cabin.lock` can pin exact bytes and `cabin fetch` / `cabin
build` can materialize registry packages.

For `--index-path` local file indexes, HTTP, OCI, Git, and remote source paths are **not**
supported.  The only source shape recognized there is `type = "archive"` / `format = "zip"` with
a local filesystem `path`.  Sparse HTTP indexes are documented below and use the same archive source
records after URL resolution.

## Index sources

`cabin resolve / fetch / build / update` reaches the package index through one of two flags.  They
are mutually exclusive - passing both fails with `use either --index-path or --index-url, not both`.

| Flag | Backend | Section |
| --- | --- | --- |
| `--index-path <path>` | local filesystem directory | [Directory layouts](#directory-layouts) |
| `--index-url <url>` | sparse HTTP | [Sparse HTTP index](#sparse-http-index) |

Local-only projects (no versioned dependencies) require neither flag.

## Directory layouts

`--index-path <path>` accepts two on-disk shapes:

### Flat layout

```
index/
  fmt.json
  spdlog.json
  ...
```

Every file whose name ends in `.json` is treated as a package metadata file; other files
(`README.md`, `.gitignore`, ...) are ignored.  Source paths in package metadata resolve relative to
this directory.

### Registry-root layout

```
registry/
  config.json
  packages/
    fmt.json
    spdlog.json
  artifacts/
    fmt/
      fmt-10.2.1-0123456789abcdef.zip
      fmt-10.2.1-9a93b2b7dfdac77c.zip
    spdlog/
      spdlog-1.13.0-9a93b2b7dfdac77c.zip
```

When a `config.json` is present at the index root the loader uses the registry-root layout:
`config.packages` (default `"packages"`) points at the directory holding `<name>.json` files, and
source paths in those files resolve relative to that directory - i.e.
`"../artifacts/fmt/fmt-10.2.1-0123456789abcdef.zip"` lands at
`registry/artifacts/fmt/fmt-10.2.1-0123456789abcdef.zip`.  The trailing segment of every artifact
filename is the [packaging revision](#packaging-revisions), so each revision of a version keeps its
own file.

In both layouts a scoped package `<scope>/<name>` nests exactly one level deeper as
`<scope>/<name>.json` (e.g. `packages/fmtlib/fmt.json`); its declared `name` must be the full
`<scope>/<name>`, and its source paths resolve relative to the scope directory, matching the
published `"../../artifacts/<scope>/<name>/<scope>-<name>-<version>-<revision>.zip"` form.  Anything
nested deeper than one scope directory is ignored.
`config.json` itself must satisfy `schema = 1`, `kind = "file-registry"`, and reject `..` or
absolute paths in the configured subdirectories.  See [`registry-design.md`](registry-design.md) for
the full layout contract.

`config.json` may also carry two optional fields belonging to the remote-registry protocol:
`auth-required` (bool; every request to the registry must carry
`Authorization: Bearer <token>`) and `api` (string; absolute `http(s)` base URL of the registry
web/API origin, rejecting non-`http(s)` schemes and `userinfo` credentials).  Both index loaders -
this local loader and the sparse HTTP client - recognize the fields unconditionally, so a
vendored or mirrored copy of a hosted registry loads like any other file registry; the read
routes never consult `api` (only the experimental mutation commands do).  See
[`remote-registry.md`](remote-registry.md) for the full protocol.

In both layouts the filename stem (`fmt` for `fmt.json`) must equal the package's declared `name`
field.  Mismatches produce a clear error.

## Package file shape

```json
{
  "schema": 1,
  "name": "fmt",
  "versions": {
    "10.2.1": {
      "dependencies": {},
      "yanked": false,
      "revision": "9a93b2b7dfdac77c",
      "revisions": {
        "0123456789abcdef": {
          "checksum": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
          "published-at": "2026-01-14T09:12:33Z",
          "source": {
            "type": "archive",
            "path": "../artifacts/fmt/fmt-10.2.1-0123456789abcdef.zip",
            "format": "zip"
          }
        },
        "9a93b2b7dfdac77c": {
          "checksum": "sha256:9a93b2b7dfdac77c9a93b2b7dfdac77c9a93b2b7dfdac77c9a93b2b7dfdac77c",
          "published-at": "2026-02-02T18:40:07Z",
          "source": {
            "type": "archive",
            "path": "../artifacts/fmt/fmt-10.2.1-9a93b2b7dfdac77c.zip",
            "format": "zip"
          }
        }
      }
    }
  }
}
```

| Field | Required | Description |
| --- | --- | --- |
| `schema` | yes | Schema version.  Only `1` is supported; other values produce a clear error. |
| `name` | yes | Package name.  Must equal the file's stem. |
| `versions` | yes | Map from SemVer version string to version metadata.  May be empty.  Keys are plain upstream versions; a key carrying SemVer build metadata (`10.2.1+anything`) is rejected. |

Each version's metadata:

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `dependencies` | no | `{}` | Map from package name to version requirement string.  The same requirement subset as `cabin.toml` (see [`docs/manifest.md`](manifest.md)). |
| `yanked` | no | `false` | When `true`, the resolver excludes this version from candidate sets. |
| `revision` | no | `null` | The version's **current** packaging revision: the one a fetch uses when no lockfile pins another.  Must name a key of `revisions`. |
| `revisions` | no | `{}` | Every fetchable packaging revision of this version, keyed by revision id.  See [Packaging revisions](#packaging-revisions). |
| `features` | no | omitted | Declared `[features]`.  Older index entries that omit the field continue to load. |
| `standards` | no | omitted | Declared per-target language-standard table (interface requirements plus `header-only` / `gnu-extensions` flags).  Absence, at any granularity, means unconstrained, so older entries that omit the field continue to load.  See [Standard metadata](#standard-metadata). |
| `links` | no | omitted | Declared per-target [`links`](manifest.md#links) claims, keyed by target name.  Absence means the version claims nothing, so older entries that omit the field continue to load.  See [Links metadata](#links-metadata). |
| `upstream` | no | omitted | Declared `[package.upstream]` provenance: `url`, the algorithm-prefixed upstream `checksum` (`sha256:<hex>`, distinct from a revision's archive `checksum`), `format`, optional `strip-prefix`, optional `patches` list, optional `copy` steps.  Loaded into the typed provenance model, whose validation rules match the manifest's ([`manifest.md`](manifest.md#packageupstream)).  Inert for resolution and fetching; older entries that omit the field continue to load. |

Unknown fields anywhere in the file are rejected.

## Packaging revisions

The immutable unit of a registry is the triple **`(name, version, revision)`**.  A version is a
plain upstream version and never changes meaning; a *packaging revision* is one set of archive bytes
published under it, so a correction to how a version is packaged lands as a new revision rather than
as a new version or as replaced bytes.

A revision id is the **first 16 lowercase hex characters of the archive's SHA-256 digest**, which
has two consequences the whole system leans on: republishing the identical archive maps onto the
revision that already exists, and a revision id can always be re-derived from a `sha256:<hex>`
checksum by taking its prefix.  Resolution never looks at revisions - it picks a version - and the
revision is selected at *fetch* time, from the lockfile's `checksum` where there is one.

Each entry of a version's `revisions` map takes this exact shape:

```json
"9a93b2b7dfdac77c": {
  "checksum": "sha256:9a93b2b7dfdac77c9a93b2b7dfdac77c9a93b2b7dfdac77c9a93b2b7dfdac77c",
  "published-at": "2026-02-02T18:40:07Z",
  "source": {
    "type": "archive",
    "path": "../artifacts/fmt/fmt-10.2.1-9a93b2b7dfdac77c.zip",
    "format": "zip"
  }
}
```

| Field | Required | Description |
| --- | --- | --- |
| `checksum` | yes | `sha256:<hex>` digest of this revision's archive bytes.  The revision id must be this digest's leading hex prefix. |
| `published-at` | yes | Non-empty timestamp recorded by the registry that accepted this revision.  Carried verbatim; Cabin never orders revisions by it (the `revision` pointer names the current one). |
| `source` | yes | Where this revision's archive lives.  See [Source artifact](#source-artifact). |

Superseded revisions stay listed, and their archives stay in place, so a lockfile that pins one
keeps building.  `revision` and `revisions` may be **absent together** - a resolver-only fixture that
carries no archives at all.  Such a version resolves, but any command that has to materialize it
fails.

## Source artifact

Each revision's `source` block must take this exact shape:

```json
"source": {
  "type": "archive",
  "path": "../artifacts/fmt/fmt-10.2.1-9a93b2b7dfdac77c.zip",
  "format": "zip"
}
```

| Field | Allowed values | Description |
| --- | --- | --- |
| `type` | `"archive"` | Local source archives.  Other values (HTTP, OCI, Git, ...) produce a clear error. |
| `path` | non-empty string | Absolute or relative filesystem path to the `.zip` archive.  Relative paths are resolved against the directory containing the `<package>.json` file at load time. |
| `format` | `"zip"` | Zip archives. |

Published artifact filenames are `<scope>-<name>-<version>-<revision>.zip` for a scoped package and
`<name>-<version>-<revision>.zip` for a bare one, inside a per-package directory.  Embedding the
revision keeps a downloaded archive self-identifying and lets superseded revisions coexist.

`cabin fetch` and `cabin build` copy each archive into the artifact cache, hashing as they go, and
reject any archive whose bytes do not match the revision's `checksum`.  The cache layout is
documented in [`artifacts.md`](artifacts.md).

## Standard metadata

The optional `standards` block records each library-like target's **declared** language-standard
interface requirement, so index consumers can read a version's per-target requirements without
downloading the source archive:

```json
"standards": {
  "targets": {
    "fmt": { "interface": { "c": "none", "c++": { "min": "c++17" } } },
    "fmt-header-only": {
      "header-only": true,
      "interface": { "c": "none", "c++": { "min": "c++20" } }
    }
  }
}
```

- `targets` is keyed by the version's **library-like** target names (`library` and `header-only`
  kinds); executables, tests, and examples never constrain consumers and are omitted.
- `interface` maps a language key (`"c"`, `"c++"`) to a requirement cell.  A **missing** key is
  unconstrained; `"none"` marks the target's headers as not consumable from that language; a
  `{ "min": "<level>" }` table is a minimum-only requirement, and
  `{ "min": "<level>", "max": "<level>" }` a bounded inclusive range the consuming code must sit
  inside (`min <= max`, validated on read).  A missing `standards` block, or a missing target, is
  unconstrained everywhere - so every pre-`standards` entry stays valid unchanged.
- `header-only` and `gnu-extensions` are per-target booleans, each omitted when `false`.

The stored value is each target's **own** declared requirement, not a transitively composed one;
`max` is written exactly when the requirement is bounded.  The full design, including how consumers
compose requirements across dependency edges, is in
[`design/standard-compatibility/registry-index.md`](design/standard-compatibility/registry-index.md).

## Links metadata

The optional `links` block records each target's declared native-library identity claim
([`manifest.md`](manifest.md#links)), so the post-resolution uniqueness check can read a version's
claims without downloading the source archive:

```json
"links": { "z": "z" }
```

- Keys are target names (only `library` targets may claim); values are the claimed identities.
  Keys satisfy the target-name grammar, values the links identity grammar, and each identity
  appears at most once - the manifest and publish rules, re-validated on load.
- Each claimed target must also have a [standard metadata](#standard-metadata) row that is not
  `header-only`.  Publishing writes a row for every library-like target, so a missing row (or a
  header-only one) marks a claim the manifest's library-only rule would have rejected; the loader
  refuses such entries.
- A missing `links` block means the version claims nothing, so every pre-`links` entry stays valid
  unchanged.  An empty block is never written - absence is the empty encoding.

## Package with dependencies

```json
{
  "schema": 1,
  "name": "spdlog",
  "versions": {
    "1.13.0": {
      "dependencies": { "fmt": ">=10.0.0 <11.0.0" },
      "yanked": false,
      "revision": "9a93b2b7dfdac77c",
      "revisions": {
        "9a93b2b7dfdac77c": {
          "checksum": "sha256:9a93b2b7dfdac77c9a93b2b7dfdac77c9a93b2b7dfdac77c9a93b2b7dfdac77c",
          "published-at": "2026-02-02T18:40:07Z",
          "source": {
            "type": "archive",
            "path": "../artifacts/spdlog/spdlog-1.13.0-9a93b2b7dfdac77c.zip",
            "format": "zip"
          }
        }
      }
    }
  }
}
```

## Yanked version

Yanking is version-level: it covers every revision of the version at once.

```json
{
  "schema": 1,
  "name": "fmt",
  "versions": {
    "10.2.1": { "dependencies": {}, "yanked": true },
    "10.1.0": { "dependencies": {}, "yanked": false }
  }
}
```

`cabin resolve` will pick `10.1.0` from this index.  If every matching version is yanked, the
resolver returns "all matching versions of `fmt` are yanked".  Neither entry here carries
`revision` / `revisions`, so both are resolver-only: this fixture exercises candidate selection and
cannot be fetched.

## Validation

Loading rejects an index when:

- the path is not a directory
- a `*.json` file has unknown fields
- `schema` is not `1`
- the declared `name` doesn't equal the filename stem
- a version key is not a valid SemVer string, or carries SemVer build metadata
- a dependency requirement is not parseable
- a revision id is not exactly 16 lowercase hex characters
- a revision id is not the leading hex prefix of its own `checksum`
- a revision's `published-at` is empty
- `revisions` is present without a `revision` pointer, or the pointer does not name a listed revision
- a `source.type` is anything other than `"archive"`
- a `source.format` is anything other than `"zip"`
- a `source.path` is empty
- a `standards` interface cell carries an empty range (`max` older than `min`), or is a bare
  standard string (`"c++17"`) rather than `"none"` or a `{ "min": "<level>", "max": "<level>" }`
  table
- a `links` entry has a key outside the target-name grammar, a value outside the links identity
  grammar (non-empty ASCII letters, digits, `.`, `_`, `+`, and `-`), or an identity claimed by more
  than one target of the version
- an `upstream` block violates the provenance rules: a non-HTTPS or credential-bearing `url`, a
  `checksum` that is not `sha256:` followed by 64 lowercase hex characters, a `format` other than
  `"tar.gz"` / `"zip"`, a
  multi-component `strip-prefix`, an unsafe copy path, or a `patches` entry that is unsafe or
  conflicts with a copy path, another patch, or the root manifest

## Not supported yet

The index format deliberately leaves the following out:

- OCI / GHCR or other remote-archive transports;
- Git sources;
- account or credential handling beyond the bearer-token protocol of
  [`remote-registry.md`](remote-registry.md);
- append-only / immutable indexes;
- artifact signing or trust configuration;
- platform-specific dependency data beyond the current serialized dependency records;
- mirror configuration;
- a cabin-specific JSON schema document; the format is documented here and validated by code, but no
  formal `$schema` URL is published.

These are deferred.

## Sparse HTTP index

`--index-url <url>` consumes the same registry-root layout served as static HTTP files.  The base
URL may include or omit a trailing slash; the loader normalizes it.

Request shape:

| Step | URL | Purpose |
| --- | --- | --- |
| 1 | `GET <url>/config.json` | Validates `schema = 1`, `kind = "file-registry"`, and the configured `packages` / `artifacts` subdirectories. |
| 2 | `GET <url>/<config.packages>/<name>.json` (bare name) or `GET <url>/<config.packages>/<scope>/<name>.json` (scoped name) | One request per package referenced by the manifest's versioned dependencies (and their transitive closure). |
| 3 | `GET <artifact-url>` | Source-archive download for each `(name, version, revision)` triple `cabin fetch` / `cabin build` needs. |

The `config.json` fetched in step 1 recognizes the same optional `auth-required` / `api` fields
as the local loader (see [Registry-root layout](#registry-root-layout)); whenever a credential
is stored for the index origin, every request in the table carries it - and the hosted
registry's reads work with no credential at all
([`remote-registry.md`](remote-registry.md#client-side-token-handling)).

Source-path resolution for each revision:

- `source.path` is resolved against the package metadata URL using RFC 3986 rules.  The standard
  `"../artifacts/<name>/<name>-<version>-<revision>.zip"` therefore resolves to
  `<url>/artifacts/<name>/<name>-<version>-<revision>.zip` - the literal path components are joined
  per RFC 3986; the `config.artifacts` field is not substituted into the URL.  A scoped package's
  document sits one directory deeper, so its canonical
  `"../../artifacts/<scope>/<name>/<scope>-<name>-<version>-<revision>.zip"` climbs one extra level
  and lands on the same registry root.
- Absolute or scheme-relative `http://` / `https://` values are accepted only when the final
  artifact URL has the same origin (scheme, host, and effective port) as the package metadata URL.
  Cross-origin artifact URLs and URLs containing `userinfo` credentials are rejected before any
  download is attempted.

Error mapping:

- `404` on a package metadata URL -> ``package `<name>` was not found in HTTP index``.
- `5xx` -> ``HTTP index request failed for `<name>`: server returned <code>``.
- Malformed JSON -> ``invalid package metadata from HTTP index for `<name>`: ...``.
- Mismatched checksum on a downloaded archive -> the same artifact error (`checksum mismatch for
  ...`).

### Frozen / offline limits

There is no persistent HTTP metadata cache.  Combining `--frozen` with an effective HTTP index URL,
whether from `--index-url`, `[registry] index-url`, or source replacement, therefore fails with a
clear message:

```
cannot use --index-url with --frozen: there is no persistent HTTP index metadata cache,
so a frozen run would have to perform network fetches it is not allowed to perform
```

With no index source configured at all, the [default registry](remote-registry.md#the-default-registry)
would apply.  Source replacement applies to it first, exactly like a config URL; a default that
still resolves to a URL is refused with its own wording (`cannot resolve versioned dependencies
with --frozen: no index source is configured, ...`), pointing at `--index-path`.

`--locked --index-url` does work - the lockfile lives on the local filesystem, and the resolver can
validate fetched metadata against it.  Full offline / vendoring workflows are separate commands
documented in [`vendoring-offline.md`](vendoring-offline.md).

### End-to-end example

A registry written by `cabin publish --registry-dir` can be consumed locally with `--index-path`
or served as static HTTP and consumed with `--index-url` - the reads are the same either way.
(A *bare-name* registry assembled by hand stays readable over both, too.)

```sh
# 1. Publish a package into a local file registry.
cabin publish --manifest-path fmt/cabin.toml --registry-dir registry

# 2. Consume it locally...
cabin resolve --manifest-path app/cabin.toml --index-path registry
cabin fetch \
  --manifest-path app/cabin.toml --index-path registry --cache-dir cache
cabin build \
  --manifest-path app/cabin.toml --index-path registry --cache-dir cache \
  --build-dir build

# ...or serve the same directory over static HTTP:
python3 -m http.server --directory registry 8000  # any static server works
cabin resolve --manifest-path app/cabin.toml --index-url http://localhost:8000
```

## Relationship to `cabin package`

`cabin package` and `cabin publish --dry-run` produce a canonical metadata document next to the
archive.  That document describes exactly **one packaging revision**: it carries the archive's
`checksum` and a `source.path` whose filename already embeds the revision derived from that
checksum, alongside the same `schema`, `dependencies`, `yanked`, and `standards` shape as an index
entry.  File-registry publish splices it into a `<package>.json` - adding it to the version's
`revisions` map and stamping `published-at` - without re-deriving anything.  Packaging and dry-run
publishing do **not** modify any index - see [`package-format.md`](package-format.md).
