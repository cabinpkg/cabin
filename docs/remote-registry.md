# Remote Registry Protocol (Experimental)

> **Everything in this document is experimental and gated behind `-Z remote-registry`.**  There is
> no compatibility promise: any route, field, framing, or status code described here may change or
> disappear between releases without a migration path.

This is the authoritative contract for Cabin's remote registry protocol: exactly what the Cabin
client (this repository) and a conforming registry server implement.  The registry *service*
itself - accounts, token issuance, hosted storage - is not part of this repository; see
[`registry-design.md`](registry-design.md).

Client status today: the [`config.json` fields](#registry-configuration) below are recognized
behind the flag (presence without `-Z remote-registry` fails the index load), client-side token
handling is implemented - `cabin login` / `cabin logout` plus
[authenticated reads](#client-side-token-handling) - and so are
[publishing](#publishing-from-the-client) (`cabin publish` against an HTTP index source) and
[yanking](#yanking-from-the-client) (`cabin yank`).

## Registry configuration

A remote registry serves the same registry-root layout as the sparse HTTP index documented in
[`package-index.md`](package-index.md).  Its `config.json` may carry two additional fields:

```json
{
  "schema": 1,
  "kind": "file-registry",
  "packages": "packages",
  "artifacts": "artifacts",
  "auth-required": true,
  "api": "https://dev-registry.cabinpkg.com"
}
```

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `auth-required` | bool | `false` | When `true`, **every** request to this registry - including `config.json` itself, package metadata, and artifact downloads - must carry `Authorization: Bearer <token>`. |
| `api` | string | absent | Absolute base URL of the registry web/API origin, e.g. `"https://dev-registry.cabinpkg.com"`.  Non-`http(s)` schemes and URLs with `userinfo` credentials are rejected, mirroring the index-URL hygiene of the sparse HTTP client.  The read routes never consult it.  When absent, `cabin publish` fails with an error naming the field: mutation requests are only ever sent to an explicitly declared API origin. |

Both index parsers (the local `--index-path` loader and the sparse HTTP client) parse the fields
unconditionally, but *presence* of either field without `-Z remote-registry` fails the index load
with an error naming the field and instructing `-Z remote-registry`.  Silently ignoring the field
is forbidden: a client that ignored `auth-required` would surface it later as a confusing `401`.

## Authentication

Requests authenticate with a bearer token:

```text
Authorization: Bearer cabin_<base62>
```

- Tokens are issued on the registry web UI at `<origin>/me` after GitHub sign-in.  The login page
  URL is derived from the **index origin** by convention - on an `auth-required` registry,
  `config.json` itself requires auth, so the client cannot discover a login URL from it.
- Token scopes: `publish` and `yank`.  Any valid token grants read access regardless of scope.

## Client-side token handling

Everything in this section requires `-Z remote-registry`; without the flag, `cabin login` and
`cabin logout` fail with the standard experimental-feature error and the sparse HTTP client never
reads a credential.

### `cabin login` and `cabin logout`

`cabin login` resolves the registry from `--index-url` (or the `[registry] index-url` setting in
[`config.md`](config.md#registry) - a local `index-path` is rejected, since tokens only apply to
HTTP registries), prints `visit <origin>/me to create a token`, and reads the token from stdin -
without echo when stdin is a terminal, as a plain read otherwise so piping works:

```console
$ echo "$TOKEN" | cabin -Z remote-registry login --index-url https://dev-registry.cabinpkg.com
visit https://dev-registry.cabinpkg.com/me to create a token
       Login token for `https://dev-registry.cabinpkg.com` saved
```

The token must start with `cabin_`; the confirmation only ever names the origin.  `cabin logout`
removes the entry for the effective index origin and reports whether one existed.

### Credential storage

Tokens live in `credentials.toml` inside the user config home - the same directory resolution as
the user-level `config.toml` in [`config.md`](config.md#file-locations): `$CABIN_CONFIG_HOME`
verbatim when set, else the platform user config home with the `cabin` suffix (Linux and macOS:
`$XDG_CONFIG_HOME/cabin` / `$HOME/.config/cabin`; Windows: `%APPDATA%\cabin`).

```toml
[registries."https://dev-registry.cabinpkg.com"]
token = "cabin_..."
```

Keys are normalized index origins - scheme + host + port, no path, no trailing slash.  Unknown
fields are rejected.  On Unix the file is created with mode `0600`, and Cabin warns once per
invocation when an existing file is group- or world-readable.  Writes are atomic (sibling temp
file + rename).  Credentials are deliberately **not** part of `config.toml`:
[`config.md`](config.md#what-config-does-not-do) rejects credential-shaped tables so a secret can
never ride along in a published archive.

### Environment override

When [`CABIN_REGISTRY_TOKEN`](environment-variables.md) is set and non-empty, its value wins over
`credentials.toml` for **every** registry the invocation touches.  Useful for CI, where writing a
credentials file is undesirable; the override also works when no user config home can be resolved
at all.  Cabin removes the variable from the environment of every child it spawns - `cabin run` /
`cabin test` executables, the Ninja build backend (and the compile / wrapper commands it runs),
the toolchain detection probes, `clang-format`, `run-clang-tidy`, and `pkg-config` - so spawned
code cannot read the credential.

### When the token is sent

With a credential available, every request to the registry - `config.json`, package metadata, and
artifact downloads - carries `Authorization: Bearer <token>`.  The token is only ever sent to the
exact origin it is stored under, and never over plain `http` except to loopback hosts
(`127.0.0.0/8`, `::1`, `localhost`), which keeps local testing possible.  Client-side error
mapping: a `401` without a stored credential advises `cabin login --index-url <origin>`; a `401`
despite one reports the token as rejected (revoked or expired); a `403` reports a missing scope.
The token never appears in logs, error messages, or debug output.

## Read routes

The read routes are the same shapes as the sparse HTTP index in
[`package-index.md`](package-index.md), served from the index origin:

| Route | Purpose |
| --- | --- |
| `GET /config.json` | Registry configuration (this document's fields included). |
| `GET /packages/<name>.json` | Per-package index document. |
| `GET /artifacts/<name>/<name>-<version>.tar.gz` | Source archive download. |

On an `auth-required` registry, all three return `401` with the
[error envelope](#error-envelope) body
`{"errors":[{"detail":"authentication required"}]}` when the request carries no valid token.
Unauthenticated requests **must not** be able to distinguish existing from non-existing packages:
the `401` status and body are identical whether or not the requested package exists.

## Publish

```text
PUT /api/v1/packages/<name>/<version>
```

Requires a token with the `publish` scope.  The route lives on the API origin - the
[`api`](#registry-configuration) base URL the registry's `config.json` must declare for mutations.
The request body is a length-prefixed frame (crates.io-style):

```text
[u32 LE metadata_len][canonical per-version metadata JSON]
[u32 LE archive_len][tar.gz bytes]
```

The metadata JSON is exactly the canonical document `cabin package` emits - the same shape as one
version entry in [`package-index.md`](package-index.md).

Server-side behavior is part of the contract:

- **Validation.**  The server validates the framing, parses the metadata under the index schema,
  requires the URL's `<name>` / `<version>` segments to match the metadata, and verifies the
  archive bytes against the metadata's `sha256:<hex>` checksum.  Failures are `400`.
- **Idempotency.**  Re-publishing a version with byte-identical metadata and archive succeeds with
  `200` and body `{"ok":true,"no_op":true}`.  Publishing the same version with *different* bytes is
  rejected with `409`.
- A first-time publish succeeds with `201` and body `{"ok":true}`.

### Publishing from the client

`cabin publish` targets a remote registry when the effective index source is an HTTP URL
(`--index-url`, or the `[registry] index-url` setting in [`config.md`](config.md#registry)) and no
`--registry-dir` is given.  Without `-Z remote-registry`, the `--index-url` flag (even combined
with `--dry-run`) and publishing against a config-supplied HTTP index both fail with the standard
experimental-feature error.  The flow is log in once, publish, then resolve like any consumer:

```console
$ echo "$TOKEN" | cabin -Z remote-registry login --index-url https://dev-registry.cabinpkg.com
visit https://dev-registry.cabinpkg.com/me to create a token
       Login token for `https://dev-registry.cabinpkg.com` saved
$ cabin -Z remote-registry publish --manifest-path fmt/cabin.toml \
    --index-url https://dev-registry.cabinpkg.com
Published fmt 10.2.1 to https://dev-registry.cabinpkg.com
  checksum: sha256:...
$ cabin -Z remote-registry resolve --manifest-path app/cabin.toml \
    --index-url https://dev-registry.cabinpkg.com
```

Client-side behavior:

- The staging pipeline is the *same* one `cabin package` and the local
  `cabin publish --registry-dir` run - same validation, same publish lints
  ([`package-format.md`](package-format.md)), same deterministic archive and canonical per-version
  metadata document.  The uploaded bytes are byte-identical to what `cabin package` writes into
  `dist/` for the same source tree.
- `config.json` (which supplies the [`api`](#registry-configuration) origin) and the lint baseline
  ride the authenticated read path; the upload carries the same bearer token to the API origin,
  under the same https-or-loopback cleartext rule.
- On `201` the client reports the published name, version, and checksum.  On `200` it reports that
  byte-identical bytes were already published and exits successfully - the same "re-running with
  identical input succeeds" semantics as the local flows.  On `409` it explains that the version
  exists with different bytes and that published versions are immutable.
- `--dry-run` stays entirely local: it stages into `--output-dir` (default `dist/`) and never
  opens a connection.

## Yank

```text
PATCH /api/v1/packages/<name>/<version>/yank
```

Requires a token with the `yank` scope, on the same API origin as publish.  The JSON body sets the
version's yanked state in the per-package index document:

```json
{ "yanked": true }
```

`{"yanked": false}` un-yanks.  The route is idempotent: setting the state a version already has
succeeds with `200` and body `{"ok":true}`.

### Yanking from the client

`cabin yank` takes a strict `<name>@<version>` spec - an exact package name and an exact SemVer
version, no ranges - and resolves the registry exactly like remote publish: `--index-url`, else the
`[registry] index-url` setting in [`config.md`](config.md#registry); a local `index-path` is
rejected, since yanked state lives in the remote registry's index.  The registry's `config.json`
must declare the [`api`](#registry-configuration) origin the request is sent to.

```console
$ cabin -Z remote-registry yank fmt@10.2.1 --index-url https://dev-registry.cabinpkg.com
fmt@10.2.1 is now yanked
$ cabin -Z remote-registry yank --undo fmt@10.2.1 --index-url https://dev-registry.cabinpkg.com
fmt@10.2.1 is no longer yanked
```

The report states the *resulting* state.  Because the route is idempotent, that wording also
covers the no-op: yanking an already-yanked version succeeds and prints the same line.  A `404`
reports that the version is not published on this registry; `401` / `403` follow the
[authenticated read path's conventions](#when-the-token-is-sent).

What yanking means - matching the resolver behavior in
[`package-index.md`](package-index.md#yanked-version):

- A yanked version is excluded from **new** resolution: `cabin resolve` skips it when picking
  candidates, and if every matching version is yanked, resolution fails.
- The artifact stays downloadable: existing lockfiles that already pin the yanked version keep
  building.  Yanking never mutates or deletes the archive - published bytes stay immutable.
- Unpublish / delete is deliberately not offered: removing bytes other projects may already
  depend on breaks reproducible builds, so the strongest retraction is the yank flag.

## Status codes

| Code | Meaning |
| --- | --- |
| `200` | Success without a state change: an idempotent no-op (byte-identical re-publish, or a yank set to the state the version already has). |
| `201` | Publish of a version that did not exist before. |
| `400` | Malformed request: bad framing, invalid metadata, or an invalid JSON body. |
| `401` | No token or an invalid token (never reveals whether the package exists). |
| `403` | Valid token, but the scope the route requires is missing. |
| `404` | Authenticated request for an unknown package or version. |
| `409` | Publish of an existing version with different bytes. |

## Error envelope

Every non-2xx response carries the same JSON envelope:

```json
{ "errors": [ { "detail": "authentication required" } ] }
```
