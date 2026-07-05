# Local JSON Package Index

Cabin resolves versioned dependencies against a tiny on-disk JSON index.  This is **not** a real
registry - it has no network protocol, no append-only structure, and no signing.  The format carries
resolver metadata, checksum data for `cabin.lock`, and a source block so `cabin fetch` and `cabin
build` can materialize registry packages.

For `--index-path` local file indexes, HTTP, OCI, Git, and remote source paths are **not**
supported.  The only source shape recognized there is `type = "archive"` / `format = "tar.gz"` with
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
      fmt-10.2.1.tar.gz
    spdlog/
      spdlog-1.13.0.tar.gz
```

When a `config.json` is present at the index root the loader uses the registry-root layout:
`config.packages` (default `"packages"`) points at the directory holding `<name>.json` files, and
source paths in those files resolve relative to that directory - i.e.
`"../artifacts/fmt/fmt-10.2.1.tar.gz"` lands at `registry/artifacts/fmt/fmt-10.2.1.tar.gz`.
`config.json` itself must satisfy `schema = 1`, `kind = "file-registry"`, and reject `..` or
absolute paths in the configured subdirectories.  See [`registry-design.md`](registry-design.md) for
the full layout contract.

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
      "checksum": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
      "source": {
        "type": "archive",
        "path": "../artifacts/fmt-10.2.1.tar.gz",
        "format": "tar.gz"
      }
    }
  }
}
```

| Field | Required | Description |
| --- | --- | --- |
| `schema` | yes | Schema version.  Only `1` is supported; other values produce a clear error. |
| `name` | yes | Package name.  Must equal the file's stem. |
| `versions` | yes | Map from SemVer version string to version metadata.  May be empty. |

Each version's metadata:

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `dependencies` | no | `{}` | Map from package name to version requirement string.  The same requirement subset as `cabin.toml` (see [`docs/manifest.md`](manifest.md)). |
| `yanked` | no | `false` | When `true`, the resolver excludes this version from candidate sets. |
| `checksum` | no | `null` | `sha256:<hex>` digest of the source archive's bytes.  Optional in the schema so resolver-only fixtures can omit it; required by `cabin fetch` and `cabin build` when the version must be materialized. |
| `source` | no | `null` | Source archive metadata.  Optional in the schema; required by `cabin fetch` and `cabin build`.  See [Source artifact](#source-artifact) below. |
| `features` | no | omitted | Declared `[features]`.  Older index entries that omit the field continue to load. |
| `standards` | no | omitted | Declared per-target language-standard table (interface requirements plus `header-only` / `gnu-extensions` flags).  Absence, at any granularity, means unconstrained, so older entries that omit the field continue to load.  See [Standard metadata](#standard-metadata). |

Unknown fields anywhere in the file are rejected.

## Source artifact

Each `source` block must take this exact shape:

```json
"source": {
  "type": "archive",
  "path": "../artifacts/fmt-10.2.1.tar.gz",
  "format": "tar.gz"
}
```

| Field | Allowed values | Description |
| --- | --- | --- |
| `type` | `"archive"` | Local source archives.  Other values (HTTP, OCI, Git, ...) produce a clear error. |
| `path` | non-empty string | Absolute or relative filesystem path to the `.tar.gz` archive.  Relative paths are resolved against the directory containing the `<package>.json` file at load time. |
| `format` | `"tar.gz"` | Gzipped tar archives. |

`cabin fetch` and `cabin build` copy each archive into the artifact cache, hashing as they go, and
reject any archive whose bytes do not match the entry's `checksum`.  The cache layout is documented
in [`artifacts.md`](artifacts.md).

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
  `{ "min": "<level>" }` table is a minimum standard the consuming code must meet.  A missing
  `standards` block, or a missing target, is unconstrained everywhere - so every pre-`standards`
  entry stays valid unchanged.
- `header-only` and `gnu-extensions` are per-target booleans, each omitted when `false`.

The stored value is each target's **own** declared requirement, not a transitively composed one;
the reserved `max` of a minimum cell is never written.  The full design, including how consumers
compose requirements across dependency edges, is in
[`design/standard-compatibility/registry-index.md`](design/standard-compatibility/registry-index.md).

## Package with dependencies

```json
{
  "schema": 1,
  "name": "spdlog",
  "versions": {
    "1.13.0": {
      "dependencies": { "fmt": ">=10.0.0 <11.0.0" },
      "yanked": false,
      "checksum": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    }
  }
}
```

## Yanked version

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
resolver returns "all matching versions of `fmt` are yanked".

## Validation

Loading rejects an index when:

- the path is not a directory
- a `*.json` file has unknown fields
- `schema` is not `1`
- the declared `name` doesn't equal the filename stem
- a version key is not a valid SemVer string
- a dependency requirement is not parseable
- a `source.type` is anything other than `"archive"`
- a `source.format` is anything other than `"tar.gz"`
- a `source.path` is empty
- a `standards` interface cell populates the reserved `max` field, or is a bare standard string
  (`"c++17"`) rather than `"none"` or a `{ "min": "<level>" }` table

## Not supported yet

The index format deliberately leaves the following out:

- OCI / GHCR or other remote-archive transports;
- Git sources;
- account or credential handling;
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
| 2 | `GET <url>/<config.packages>/<name>.json` | One request per package referenced by the manifest's versioned dependencies (and their transitive closure). |
| 3 | `GET <artifact-url>` | Source-archive download for each `(name, version)` `cabin fetch` / `cabin build` needs. |

Source-path resolution for each version:

- `source.path` is resolved against the package metadata URL using RFC 3986 rules.  The standard
  `"../artifacts/<name>/<name>-<version>.tar.gz"` therefore resolves to
  `<url>/artifacts/<name>/<name>-<version>.tar.gz` - the literal path components are joined per RFC
  3986; the `config.artifacts` field is not substituted into the URL.
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

`--locked --index-url` does work - the lockfile lives on the local filesystem, and the resolver can
validate fetched metadata against it.  Full offline / vendoring workflows are separate commands
documented in [`vendoring-offline.md`](vendoring-offline.md).

### End-to-end example

A registry written by `cabin publish --registry-dir` can be served as static HTTP and consumed by
every read command without conversion:

```sh
# 1. Publish a package into a local file registry.
cabin publish --manifest-path fmt/cabin.toml --registry-dir registry

# 2. Serve the registry as static HTTP files.
python3 -m http.server --directory registry 8000  # any static server works

# 3. Resolve / fetch / build using the HTTP URL.
cabin resolve --manifest-path app/cabin.toml --index-url http://localhost:8000
cabin fetch \
  --manifest-path app/cabin.toml --index-url http://localhost:8000 --cache-dir cache
cabin build \
  --manifest-path app/cabin.toml --index-url http://localhost:8000 --cache-dir cache \
  --build-dir build
```

## Relationship to `cabin package`

`cabin package` and `cabin publish --dry-run` produce a canonical per-version metadata document next
to the archive.  The generated document mirrors the shape of one entry inside this index file (same
`schema`, `dependencies`, `yanked`, `checksum`, `source`, and `standards` shape) so file-registry
publish can splice it into a `<package>.json` without re-deriving anything.  Packaging and dry-run publishing do
**not** modify any index - see [`package-format.md`](package-format.md).
