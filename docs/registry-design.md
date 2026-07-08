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
other server-side control planes are outside this repository's boundary.
The *client* side of a remote registry protocol is an experimental,
`-Z remote-registry`-gated track; see
[Experimental remote registry client](#experimental-remote-registry-client).

## Local File Registry

`cabin publish --registry-dir <dir>` writes a deterministic file registry
that the resolver can read back through `--index-path <dir>`.

```text
<registry>/
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

`config.json` identifies the registry layout and the relative `packages`
and `artifacts` directories.  Each `packages/<name>.json` file contains
the deterministic per-package version list.  Each version points at its
source archive by a registry-relative path such as
`../artifacts/fmt/fmt-10.2.1.tar.gz`.

The file-registry writer:

- stages package archives and canonical metadata through the same
  package code used by `cabin package`;
- validates package names before they become path components;
- rejects duplicate versions;
- writes version entries in deterministic semver order;
- replaces the artifact and the per-package index atomically: each
  file is staged in a sibling temporary file and only renamed onto
  its destination after a successful write, so an interrupted
  publish leaves the previous artifact and index in place;
- uses a simple registry lock file to avoid concurrent mutation;
- keeps archive checksums in the index so the artifact pipeline can
  verify bytes before extraction.

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
- `GET <url>/packages/<name>.json`
- `GET <url>/artifacts/<name>/<name>-<version>.tar.gz`

The client is read-only.  It does not publish packages, mutate registry
state, persist HTTP metadata for offline use, or infer a default remote
source.  Commands that need an index source require `--index-path`,
`--index-url`, or a local config default.

HTTP archive bytes still flow through the artifact cache.  Cabin hashes
the bytes, checks them against the index's `sha256:<hex>` value, stores
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
- `cabin-artifact` verifies, caches, and extracts source archives
  without knowing whether bytes came from a file or HTTP.

The separation is intentional: adding a new local or static read
transport should not require changing package metadata, the lockfile,
or the build planner.

## Experimental Remote Registry Client

Gated behind `-Z remote-registry`, with no compatibility promise, Cabin
is growing a client for authenticated remote registries.  The protocol
both sides implement - bearer-token reads, network-backed package
publishing, and a yank command - is specified in
[`remote-registry.md`](remote-registry.md).  Under this experimental
track:

- the registry `config.json` fields `auth-required` and `api` are
  recognized; without `-Z remote-registry` their presence fails the
  index load with an error naming the field (never a silent ignore);
- client-side credential handling uses `Authorization: Bearer` tokens
  issued on the registry web UI at `<origin>/me`;
- publishing uses `PUT /api/v1/packages/<name>/<version>` with a
  length-prefixed metadata + archive frame, and yanking uses
  `PATCH /api/v1/packages/<name>/<version>/yank`.

The registry *service* itself - accounts, token issuance, storage -
remains outside this repository.

## Out Of Scope

The local OSS core deliberately excludes:

- registry account or ownership services (the server side of the
  experimental protocol above);
- artifact signing policy;
- Git repository indexes;
- persistent HTTP metadata caching;
- binary artifact caches or remote build caches.

See [`package-index.md`](package-index.md),
[`package-format.md`](package-format.md), and
[`artifacts.md`](artifacts.md) for the concrete on-disk documents and
archive rules.
