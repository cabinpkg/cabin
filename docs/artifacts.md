# Source Artifacts

Cabin source-artifact fetching takes a resolved package set plus an
index that points at `.tar.gz` archives. It verifies SHA-256
checksums, copies archives into a
checksum-addressed cache, safely extracts them, validates the
extracted manifests, and feeds the result into the build planner.

`cabin-artifact` itself is transport-neutral: local indexes provide a
filesystem path, and the sparse HTTP client downloads bytes before
handing them to the same verifier and extractor. OCI / Git transports
are deferred.

## Source archive format

Every registry package source archive must be a `.tar.gz` whose root
contains the package's `cabin.toml`. `cabin package` produces
archives in exactly this shape — see
[`package-format.md`](package-format.md) for the producer contract,
including the determinism rules and the include / exclude policy.
`cabin publish --registry-dir` writes those archives into a local file
registry under
`<registry>/artifacts/<name>/<name>-<version>.tar.gz`, matching
what the artifact fetcher reads back; see
[`registry-design.md`](registry-design.md).

```
fmt-10.2.1.tar.gz
  ├── cabin.toml
  ├── include/
  │   └── fmt.h
  └── src/
      └── fmt.cc
```

Rules:

- the archive root must contain a file named `cabin.toml`;
- the extracted manifest's `[package].name` must equal the resolved
  package name;
- the extracted manifest's `[package].version` must equal the
  resolved package version;
- archives whose root is a single top-level directory are **not**
  supported — `cabin.toml` must sit at the very top.

Any deviation produces a clear error such as
`source archive for `fmt 10.2.1` does not contain cabin.toml at its root`
or
`source archive for `fmt 10.2.1` contains package `fmt 10.1.0``.

## Index reference

In the local JSON index, each version that should be materialisable
carries a `source` block alongside its `checksum`:

```json
"source": {
  "type": "archive",
  "path": "../artifacts/fmt-10.2.1.tar.gz",
  "format": "tar.gz"
}
```

| Field | Allowed values | Description |
| --- | --- | --- |
| `type` | `"archive"` | Local source archive. |
| `format` | `"tar.gz"` | Gzipped tar. |
| `path` | non-empty string | Absolute or relative filesystem path. Relative paths resolve against the directory containing the `<package>.json` index file. |

`checksum` is the `sha256:<hex>` digest of the archive's bytes. It
is required for any version `cabin fetch` or `cabin build` is asked
to materialise; it is optional for resolver-only fixtures.

## Cache layout

Cabin uses a user-global checksum-addressed cache by default
(`~/.cache/cabin/` on a Unix-like system; see the precedence
chain below):

```
<cache>/
  archives/
    sha256/
      <hex>.tar.gz
  sources/
    sha256/
      <hex>/
        cabin.toml
        include/
        src/
```

- The archive cache key is the lower-case hex of the archive's
  SHA-256.
- The source cache key is the same hex; the extracted tree lives at
  `sources/sha256/<hex>/`.
- If a cached archive's bytes still hash to the expected value, it is
  reused as-is.
- If a cached source directory exists and its `cabin.toml` matches
  the resolved name and version, it is reused as-is.
- A partial or corrupt source extraction is removed and re-extracted
  on the next non-`--frozen` run.

The cache directory is selected by, in order:

1. `--cache-dir <path>` (CLI flag);
2. `CABIN_CACHE_DIR` environment variable;
3. `CABIN_CACHE_HOME` environment variable (a Cabin-specific
   override, used verbatim with no `cabin` application prefix
   appended);
4. The user XDG cache home computed by the
   [`xdg`](https://crates.io/crates/xdg) crate with the `cabin`
   application prefix — typically `$XDG_CACHE_HOME/cabin` or
   `$HOME/.cache/cabin` per the XDG Base Directory specification.

Cabin's cache home fallback is XDG-native: Cargo's `$CARGO_HOME`
model is not used. Standard XDG environment variables follow the
XDG spec — empty or relative `XDG_CACHE_HOME` is treated as unset
and the `$HOME/.cache` fallback applies.

The default on a Unix-like system with no overrides is
`~/.cache/cabin/`. The cache is shared across projects on the
same machine — content is checksum-addressed, so identical
downloads materialize at the same on-disk path regardless of
which project triggered them.

## `cabin fetch` workflow

```sh
# Fetch every resolved registry package: write cabin.lock, verify
# checksums, copy archives into the cache, and extract sources.
cabin fetch --manifest-path app/cabin.toml --index-path index

# Use a non-default cache directory.
cabin fetch --manifest-path app/cabin.toml --index-path index --cache-dir /tmp/cabin-cache

# CI mode: require existing cache + lockfile; refuse to populate.
cabin fetch --frozen --manifest-path app/cabin.toml --index-path index
```

`cabin build --index-path <path>` accepts the same `--cache-dir`,
`--locked`, and `--frozen` flags. With versioned dependencies the
build path runs the same fetch pipeline, then plans and runs a
unified build over local and registry packages. See
[`lockfile.md`](lockfile.md) for the `--locked` / `--frozen`
contract.

## HTTP artifact downloads

`cabin <fetch|build> --index-url <url>` reuses exactly the same
artifact cache. The HTTP path differs only in how archive bytes
arrive at `cabin-artifact`:

1. The HTTP index loader resolves each version's `source.path`
   into an absolute URL and rejects the result unless it stays on
   the same origin as the package metadata URL and contains no
   `userinfo` credentials.
2. `cabin-cli` calls `cabin_index_http::HttpClient::download` to
   fetch the archive bytes once per `(name, version)` that the
   resolved set requires.
3. The bytes are handed to `cabin-artifact` as a
   [`FetchSource::InMemoryArchive`]. From there, the existing
   artifact path verifies SHA-256, atomic-renames the bytes into
   `<cache>/archives/sha256/<hex>.tar.gz`, and safely extracts
   into `<cache>/sources/sha256/<hex>/`.

For local archive sources, cache-hit behaviour is unchanged: an
archive whose SHA-256 is already present in the artifact cache is
not read again from the source path. For HTTP sources, Cabin
currently downloads the archive bytes before `cabin-artifact` can
observe the cache hit; the existing cache entry is still verified
and reused, but the HTTP archive request still happens once per
resolved package in that run.

## Checksum verification

`cabin-artifact` always hashes the bytes it reads:

- on a cache hit, the cached archive is hashed and compared against
  the expected `sha256` digest before being reused;
- on a cache miss (and not `--frozen`), the source archive is copied
  into a `<hex>.tar.gz.partial` sibling while being hashed; if the
  bytes do not match the expected digest, the partial file is removed
  and the run fails with `checksum mismatch for ...: expected ...,
  got ...`.

There is no way to skip checksum verification.

## Safe extraction

Source archives are extracted with fail-closed rules:

- archive entries with absolute paths are rejected;
- archive entries containing `..` components are rejected;
- archive entries whose joined destination escapes the source
  directory are rejected;
- only `Regular` files and `Directory` entries are accepted; symlinks,
  hard links, char/block devices, fifos, sparse, GNU long-name, pax
  extension, and continuation entries are rejected with a clear
  error;
- symlinks are never followed; nothing is written outside the
  destination directory.

Errors look like
`refusing to extract unsafe archive entry `../escape.txt`` or
`refusing to extract unsupported archive entry `evil``.

## `cabin fetch`

```sh
cabin fetch \
  [--manifest-path <path>] \
  [--index-path <path>] \
  [--cache-dir <path>] \
  [--locked | --frozen] \
  [--format human|json]
```

Behaviour:

1. Load manifest / workspace.
2. Resolve versioned dependencies with the same lockfile-aware
   semantics as `cabin resolve`.
3. Write or update `cabin.lock` unless `--locked` or `--frozen`
   forbids it.
4. For each resolved registry package, build a fetch entry from the
   index's `source` and `checksum`.
5. Verify checksums and copy archives into the cache.
6. Safely extract sources into the cache and validate each
   extracted package's `cabin.toml`.
7. Print the fetched packages.

`cabin build --index-path <path>` runs the same pipeline before
planning the build.

### `--frozen`

`--frozen` does not write the lockfile and does not populate the
artifact cache. Already-cached, already-extracted artifacts may be
reused. If a required archive or source tree is not already cached,
the run fails with
`cannot fetch artifact for `fmt 10.2.1` because --frozen was specified
and the artifact is not cached`.

For HTTP indexes, `--frozen` always fails when the effective
index source is a URL, whether it came from `--index-url`,
`[registry] index-url`, or source replacement, because the HTTP
index loader has no persistent metadata cache. The error message
is:

```
cannot use --index-url with --frozen: there is no persistent HTTP index metadata cache, so a frozen run would have to perform network fetches it is not allowed to perform
```

Vendoring / offline workflows are separate and still require a
local `--index-path`; they do not make frozen HTTP index URLs
usable.

## Limitations

`cabin-artifact` deliberately does **not** implement any of the
following:

- HTTP downloads itself — the HTTP read path lives in
  `cabin-index-http` and hands archive bytes to this
  crate as `FetchSource::InMemoryArchive`;
- Git / OCI / GHCR transports;
- network publish or non-local registry write paths;
- package publishing (`cabin package`, `cabin publish`);
- a binary artifact cache or remote build cache;
- artifact signing or trust configuration;
- account, ownership, or hosted-service policy features.

See [`registry-design.md`](registry-design.md) for what is deferred
and what is out of scope.
