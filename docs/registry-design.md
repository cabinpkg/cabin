# Registry Interface

This repository contains the local registry interface used by Cabin's
public OSS core:

- a deterministic local JSON package index;
- a local file-registry write path for `cabin publish --registry-dir`;
- a read-only sparse HTTP client that consumes the same static layout;
- package archive metadata that preserves the manifest-declared fields
  the resolver and build pipeline need.

It does **not** contain the registry service itself: account systems,
ownership workflows, token issuance, hosted storage, signing policy, and
other server-side control planes are outside the OSS core's boundary
(the hosted service is developed under the repository's separate
`registry/` workspace, not in the Cabin crates).
The *client* side of the remote registry protocol has stable reads and
experimental (`-Z remote-registry`-gated) mutations; see
[Remote registry client](#remote-registry-client).

## Local File Registry

`cabin publish --registry-dir <dir>` writes a deterministic file registry
that the resolver can read back through `--index-path <dir>`.

```text
<registry>/
  config.json
  packages/
    fmtlib/
      fmt.json
    gabime/
      spdlog.json
  artifacts/
    fmtlib/
      fmt/
        fmtlib-fmt-10.2.1-0123456789abcdef.zip
    gabime/
      spdlog/
        gabime-spdlog-1.13.0-9a93b2b7dfdac77c.zip
```

`config.json` identifies the registry layout and the relative `packages`
and `artifacts` directories.  Registry packages are always scoped
(`<scope>/<name>`), so each package's index file nests one scope
directory deep and its artifact filename embeds the scope and the
[packaging revision](package-index.md#packaging-revisions)
(`<scope>-<name>-<version>-<revision>.zip`, self-identifying outside the
tree, and one file per revision).
Each `packages/<scope>/<name>.json` file contains the deterministic
per-package version list.  Each revision points at its source archive by
a registry-relative path such as
`../../artifacts/fmtlib/fmt/fmtlib-fmt-10.2.1-0123456789abcdef.zip`.  Legacy
bare-name registries (`packages/<name>.json`,
`artifacts/<name>/<name>-<version>-<revision>.zip`) remain readable and
vendorable; only new publication requires a scope.

The file-registry writer:

- stages package archives and canonical metadata through the same
  package code used by `cabin package`;
- requires a scoped name and validates each name component before it
  becomes a path component;
- treats `(name, version, revision)` as the immutable unit: republishing
  byte-identical bytes is a no-op onto the recorded revision; different
  bytes for a version that already has one require the `--new-revision`
  opt-in and must leave the resolver-consumed metadata
  (`dependencies`, `features`, `standards`) unchanged - `links` may be
  added where the version had none, never changed or removed; and two
  different archives whose digests share a revision id fail loudly;
- makes the newly written revision the version's current one while
  keeping superseded revisions listed and their artifacts in place;
- writes version entries in deterministic semver order;
- replaces the artifact and the per-package index atomically: each
  file is staged in a sibling temporary file and only renamed onto
  its destination after a successful write, so an interrupted
  publish leaves the previous artifact and index in place;
- uses a simple registry lock file to avoid concurrent mutation;
- keeps each revision's archive checksum in the index so the artifact
  pipeline can verify bytes before extraction.

The existing read path (`cabin resolve`, `cabin fetch`,
`cabin build --index-path`) accepts either a registry root with
`config.json` or the legacy flat-fixture layout described in
[`package-index.md`](package-index.md), so a registry written by
`cabin publish --registry-dir` can be consumed end to end without
any conversion step:

```sh
cabin publish --manifest-path fmt/cabin.toml --registry-dir registry
cabin resolve --manifest-path app/cabin.toml --index-path registry
cabin fetch   --manifest-path app/cabin.toml --index-path registry --cache-dir cache
cabin build \
  --manifest-path app/cabin.toml --index-path registry --cache-dir cache \
  --build-dir build
```

## Sparse HTTP Read Path

The sparse HTTP client reads the same file-registry shape over plain
`GET` requests:

- `GET <url>/config.json`
- `GET <url>/packages/<scope>/<name>.json`
- `GET <url>/artifacts/<scope>/<name>/<scope>-<name>-<version>-<revision>.zip`

A bare name (legal only in locally-produced file registries) reads
from the flat `packages/<name>.json` /
`artifacts/<name>/<name>-<version>-<revision>.zip` shape; the hosted
registry serves scoped routes only.  Which revision a read requests
comes from the lockfile's checksum pin, not from resolution.

The client is read-only.  It does not publish packages, mutate registry
state, or persist HTTP metadata for offline use.  Commands that need an
index source use `--index-path`, `--index-url`, a local config default,
or - when none of those apply - Cabin's default hosted index origin
([`remote-registry.md`](remote-registry.md#the-default-registry)).

HTTP archive bytes still flow through the artifact cache.  Cabin hashes
the bytes, checks them against the revision's `sha256:<hex>` value, stores
the archive under the content-addressed cache path, and extracts it with
the same path-traversal protections used for local archives.

## Transport And Format Boundaries

The package index document shape is shared by the local filesystem and
sparse HTTP read paths.  Transport-specific code lives outside the
domain model:

- `cabin-index` parses package index documents from local files;
- `cabin-registry-file` owns the local mutable file-registry layout;
- `cabin-index-http` performs read-only HTTP fetches and hands the
  retrieved JSON to the same typed index model;
- `cabin-registry-api` owns the authenticated mutation routes
  (publish / yank, behind `-Z remote-registry`);
- `cabin-artifact` verifies, caches, and extracts source archives
  without knowing whether bytes came from a file or HTTP.

The separation is intentional: adding a new local or static read
transport should not require changing package metadata, the lockfile,
or the build planner.

## Remote Registry Client

Cabin's client for authenticated remote registries has stable reads;
its mutations (network-backed package publishing and a yank command)
stay behind `-Z remote-registry` with no compatibility promise.  The
protocol both sides implement is specified in
[`remote-registry.md`](remote-registry.md).  Under this track:

- the registry `config.json` fields `auth-required` and `api` are
  recognized unconditionally; the read routes never consult `api`
  (only the gated mutation commands do);
- client-side credential handling uses `Authorization: Bearer` tokens
  minted by the registry's login-session and trusted-publishing flows;
  where to get one the client discovers from
  the `WWW-Authenticate` `Cabin login_url` challenge the registry's
  authenticated surfaces answer unauthenticated requests with (the
  hosted registry's verified reads themselves are public);
- publishing uses `PUT /api/v1/packages/<scope>/<name>/<version>`
  with a length-prefixed metadata + archive frame, and yanking uses
  `PATCH /api/v1/packages/<scope>/<name>/<version>/yank`;
- remote `cabin publish` (an HTTP index source without
  `--registry-dir`) reuses the local staging pipeline byte-for-byte:
  the same validation, the same publish lints, and the same
  deterministic archive plus canonical per-version metadata document
  as `cabin publish --registry-dir` - only the final write differs,
  going through the typed `cabin-registry-api` client to the API
  origin the registry's `config.json` declares.

The registry *service* itself - accounts, token issuance, storage -
remains outside the OSS core, in the repository's separate `registry/`
workspace.

## Out Of Scope

The local OSS core deliberately excludes:

- registry account or ownership services (the server side of the
  remote-registry protocol above);
- artifact signing policy;
- Git repository indexes;
- persistent HTTP metadata caching;
- binary artifact caches or remote build caches.

See [`package-index.md`](package-index.md),
[`package-format.md`](package-format.md), and
[`artifacts.md`](artifacts.md) for the concrete on-disk documents and
archive rules.
