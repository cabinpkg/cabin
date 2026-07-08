# Remote Registry Protocol (Experimental)

> **Everything in this document is experimental and gated behind `-Z remote-registry`.**  There is
> no compatibility promise: any route, field, framing, or status code described here may change or
> disappear between releases without a migration path.

This is the authoritative contract for Cabin's remote registry protocol: exactly what the Cabin
client (this repository) and a conforming registry server implement.  The registry *service*
itself - accounts, token issuance, hosted storage - is not part of this repository; see
[`registry-design.md`](registry-design.md).

Client status today: the [`config.json` fields](#registry-configuration) below are recognized
behind the flag (presence without `-Z remote-registry` fails the index load).  Client-side token
handling and the publish / yank commands land incrementally on the same gate.

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
| `api` | string | absent | Absolute base URL of the registry web/API origin, e.g. `"https://dev-registry.cabinpkg.com"`.  Non-`http(s)` schemes and URLs with `userinfo` credentials are rejected, mirroring the index-URL hygiene of the sparse HTTP client.  When absent, the API origin defaults to the index origin. |

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

Requires a token with the `publish` scope.  The route lives on the API origin (`api` when set,
otherwise the index origin).  The request body is a length-prefixed frame (crates.io-style):

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
- A first-time publish succeeds with `200` and body `{"ok":true}`.

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

## Status codes

| Code | Meaning |
| --- | --- |
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
