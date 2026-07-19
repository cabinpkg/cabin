# Remote Registry Protocol (Experimental)

> **Everything in this document is experimental and gated behind `-Z remote-registry`.**  There is
> no compatibility promise: any route, field, framing, or status code described here may change or
> disappear between releases without a migration path.

> **Names are scoped.** Registry packages are always `<scope>/<name>` (e.g. `fmtlib/fmt`): every
> package route carries the `<scope>/<name>` pair, the artifact filename embeds the scope
> (`<scope>-<name>-<version>.zip`, so a downloaded archive stays self-identifying outside the
> directory tree), and publish/yank additionally require the token's user to be a member of the
> target scope (see `registry/docs/architecture.md`, "Scopes").  Bare names exist only in
> local-only manifests and local file registries; `cabin publish` rejects them before any
> connection.

This is the authoritative contract for Cabin's remote registry protocol: exactly what the Cabin
client (this repository) and a conforming registry server implement.  The registry *service*
itself - accounts, token issuance, hosted storage - is not part of the Cabin crates; its hosted
implementation lives under `registry/` in this repository, outside the OSS core boundary
described in [`registry-design.md`](registry-design.md).

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
  "api": "https://cabinpkg.com"
}
```

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `auth-required` | bool | `false` | When `true`, **every** request to this registry - including `config.json` itself, package metadata, and artifact downloads - must carry `Authorization: Bearer <token>`. |
| `api` | string | absent | Absolute base URL of the registry's API origin - on the hosted registry the **website origin** `"https://cabinpkg.com"`, following crates.io's `"api": "https://crates.io"` discipline (see [One role per hostname](#one-role-per-hostname)).  Non-`http(s)` schemes and URLs with `userinfo` credentials are rejected, mirroring the index-URL hygiene of the sparse HTTP client.  The read routes never consult it.  When absent, `cabin publish` fails with an error naming the field: mutation requests are only ever sent to an explicitly declared API origin. |

Both index parsers (the local `--index-path` loader and the sparse HTTP client) parse the fields
unconditionally, but *presence* of either field without `-Z remote-registry` fails the index load
with an error naming the field and instructing `-Z remote-registry`.  Silently ignoring the field
is forbidden: a client that ignored `auth-required` would surface it later as a confusing `401`.

## One role per hostname

The hosted registry splits its hostnames by role, exactly crates.io's discipline
(`index.crates.io` serves the index, `static.crates.io` the downloads, and `crates.io` the web
UI plus the entire API, glued together by `config.json`'s `dl`/`api` fields):

| Hostname | Role |
| --- | --- |
| `registry.cabinpkg.com` | The machine read plane **only**: `config.json`, package metadata, artifact downloads, and `/healthz`.  Cabin keeps artifacts on the index host - the client's same-origin artifact rule makes a separate download host pointless - so this one hostname covers what crates.io splits across `index.crates.io` and `static.crates.io`. |
| `cabinpkg.com` | The website, plus everything else the registry serves: the browser sign-in flow, the session-cookie user API, the Bearer mutation routes, and the one unauthenticated JSON route - `GET /api/v1/stats`, aggregate download/package totals for the website's own pages (service-local; not part of the client protocol).  `config.json`'s [`api`](#registry-configuration) field names this origin. |

On the index host, every path outside the read plane - the mutation routes included - answers
the same uniform `401` as a missing token, whatever credential comes along: the mutation
surface is indistinguishable from any unknown path there.

## Authentication

Requests authenticate with a bearer token:

```text
Authorization: Bearer cabin_<base62>
```

- Tokens are issued on the registry's web UI after GitHub sign-in.  Its URL is **not** derived
  from the index origin by convention; it is discovered through the
  [login-URL challenge](#the-login-url-challenge) below.
- Token scopes: `publish`, `yank`, and `verify` (the
  [verification lifecycle](#verification-lifecycle)'s verifier scope).  Any valid token grants
  read access regardless of scope.

### The login-URL challenge

Every unauthenticated (`401`) response from the Bearer plane carries a `WWW-Authenticate`
challenge naming the token-creation page, mirroring Cargo's `Cargo login_url` challenge:

```text
WWW-Authenticate: Cabin login_url="https://cabinpkg.com/settings/tokens"
```

The grammar is the scheme token `Cabin` (ASCII case-insensitive, per RFC 7235) followed by a
quoted `login_url` parameter carrying an absolute `http(s)` URL.  The header is byte-identical
on every path and failure reason - a missing token, an invalid token, and an unknown path all
answer the same challenge - so unauthenticated responses stay indistinguishable and leak
nothing about package existence.

## Client-side token handling

Everything in this section requires `-Z remote-registry`; without the flag, `cabin login` and
`cabin logout` fail with the standard experimental-feature error and the sparse HTTP client never
reads a credential.

### `cabin login` and `cabin logout`

`cabin login` resolves the registry from `--index-url` (or the `[registry] index-url` setting in
[`config.md`](config.md#registry) - a local `index-path` is rejected, since tokens only apply to
HTTP registries), discovers the token-creation page, and reads the token from stdin - without
echo when stdin is a terminal, as a plain read otherwise so piping works:

```console
$ echo "$TOKEN" | cabin -Z remote-registry login --index-url https://registry.cabinpkg.com
visit https://cabinpkg.com/settings/tokens to create a token
       Login token for `https://registry.cabinpkg.com` saved
```

Discovery is one advisory, always-unauthenticated `GET` of the index's `config.json`: on a
`401` carrying the [login-URL challenge](#the-login-url-challenge), the challenge's URL is
printed verbatim.  Every other outcome - a registry that does not require auth, a missing or
malformed challenge, an implausible URL, or a failed probe (offline) - degrades to a generic
`create a token in the registry's web interface` hint.  The probe never blocks login: the
pasted token is read and stored either way.

The token must start with `cabin_`; the confirmation only ever names the origin.  `cabin logout`
removes the entry for the effective index origin and reports whether one existed.

### Credential storage

Tokens live in `credentials.toml` inside the user config home - the same directory resolution as
the user-level `config.toml` in [`config.md`](config.md#file-locations): `$CABIN_CONFIG_HOME`
verbatim when set, else the platform user config home with the `cabin` suffix (Linux and macOS:
`$XDG_CONFIG_HOME/cabin` / `$HOME/.config/cabin`; Windows: `%APPDATA%\cabin`).

```toml
[registries."https://registry.cabinpkg.com"]
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
artifact downloads - carries `Authorization: Bearer <token>`.  A stored token is sent to exactly
two destinations: (a) the index origin it is stored under, and (b) the
[`api`](#registry-configuration) origin declared by that index's authenticated `config.json`,
where the mutation routes live - and nowhere else.  This mirrors Cargo, whose tokens go to the
api host named in `config.json`.  Neither destination ever sees the token over plain `http`
except loopback hosts (`127.0.0.0/8`, `::1`, `localhost`), which keeps local testing possible.
Client-side error mapping: a `401` without a stored credential advises
`cabin login --index-url <origin>`; a `401` despite one reports the token as rejected (revoked
or expired); a `403` reports a missing scope.  The token never appears in logs, error messages,
or debug output.

## Read routes

The read routes are the same shapes as the sparse HTTP index in
[`package-index.md`](package-index.md), served from the index origin:

| Route | Purpose |
| --- | --- |
| `GET /config.json` | Registry configuration (this document's fields included). |
| `GET /packages/<scope>/<name>.json` | Per-package index document. |
| `GET /artifacts/<scope>/<name>/<scope>-<name>-<version>.zip` | Source archive download. |

On an `auth-required` registry, all three return `401` with the
[error envelope](#error-envelope) body
`{"errors":[{"detail":"authentication required"}]}` and the
[login-URL challenge](#the-login-url-challenge) when the request carries no valid token.
Unauthenticated requests **must not** be able to distinguish existing from non-existing packages:
the `401` status, body, and challenge are identical whether or not the requested package exists -
and identical again on every [non-read-plane path](#one-role-per-hostname) of the index host.

On a registry with the [verification lifecycle](#verification-lifecycle), the composed
`/packages/<scope>/<name>.json` document contains **verified** versions only, and the artifact route
serves verified versions to ordinary tokens; a package with no verified versions is
indistinguishable from an unknown one.

## Publish

```text
PUT /api/v1/packages/<scope>/<name>/<version>
```

Requires a token with the `publish` scope.  The route lives on the API origin - the
[`api`](#registry-configuration) base URL (the website origin, on the hosted registry) the
registry's `config.json` must declare for mutations.
The request body is a length-prefixed frame (crates.io-style):

```text
[u32 LE metadata_len][canonical per-version metadata JSON]
[u32 LE archive_len][zip bytes]
```

The metadata JSON is exactly the canonical document `cabin package` emits - the same shape as one
version entry in [`package-index.md`](package-index.md).

Server-side behavior is part of the contract:

- **Validation.**  The server validates the framing, parses the metadata under the index schema
  (`schema` values other than `1` are refused - a document the
  [verifier](#the-verifiers-checks) cannot judge must never enter the pending queue), requires
  the URL's `<scope>/<name>` / `<version>` segments to match the metadata (the metadata's `name` field carries the full `<scope>/<name>` string), requires every key of the
  metadata's `dependencies` and `dev-dependencies` maps to be a canonical `<scope>/<name>`
  name (`system-dependencies` is exempt - its keys name system packages, not registry
  packages), and verifies the archive
  bytes against the metadata's `sha256:<hex>` checksum.  Failures are `400`.
  Two name-level rules join the same `400` family (`registry/docs/architecture.md`, "Name
  fidelity"): a reserved package name (`package name is reserved` - the DOS device stems plus a
  short project vocabulary), and, for a publish that would create a new package, a name that
  collides with an existing same-scope package under `-`/`_` folding (`package name conflicts
  with an existing package in this scope (differs only in '-' vs '_')`).
- **Idempotency.**  Re-publishing a version with byte-identical metadata and archive succeeds with
  `200` and body `{"ok":true,"no_op":true,"verification":"<status>"}`, reporting the row's current
  [verification status](#verification-lifecycle).  Publishing the same version with *different*
  bytes is rejected with `409` - unless the existing row is **rejected**, in which case any bytes
  are an accepted replacement: the row is updated in place (new checksum, metadata, size,
  publisher, and timestamp) and returns to `pending` with a fresh `201`.
- A first-time publish succeeds with `201` and body
  `{"ok":true,"name":...,"version":...,"checksum":...,"verification":"pending"}`: the version is
  accepted but becomes resolvable only once verified.  Clients read the `verification` field
  tolerantly - a registry without the lifecycle simply omits it.

### Publishing from the client

`cabin publish` targets a remote registry when the effective index source is an HTTP URL
(`--index-url`, or the `[registry] index-url` setting in [`config.md`](config.md#registry)) and no
`--registry-dir` is given.  Without `-Z remote-registry`, the `--index-url` flag (even combined
with `--dry-run`) and publishing against a config-supplied HTTP index both fail with the standard
experimental-feature error.  The flow is log in once, publish, then resolve like any consumer:

```console
$ echo "$TOKEN" | cabin -Z remote-registry login --index-url https://registry.cabinpkg.com
visit https://cabinpkg.com/settings/tokens to create a token
       Login token for `https://registry.cabinpkg.com` saved
$ cabin -Z remote-registry publish --manifest-path fmt/cabin.toml \
    --index-url https://registry.cabinpkg.com
Published fmt 10.2.1 to https://registry.cabinpkg.com
  checksum: sha256:...
$ cabin -Z remote-registry resolve --manifest-path app/cabin.toml \
    --index-url https://registry.cabinpkg.com
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
- When the response's optional `"verification"` field says `"pending"`, the report adds that the
  version was accepted and becomes resolvable after verification (typically within a few
  minutes).  The field is read tolerantly: a registry that omits it changes nothing.
- `--dry-run` stays entirely local: it stages into `--output-dir` (default `dist/`) and never
  opens a connection.

## Verification lifecycle

Every published version carries a verification status:

```text
publish (201) --> pending --verdict: verified--> verified (resolvable, immutable)
                    ^  |
                    |  +--verdict: rejected--> rejected (blob reclaimed, quota refunded)
                    |                             |
                    +--republish, any bytes (201)-+
```

- **pending** - accepted and stored, but not part of the registry yet: excluded from composed
  `/packages/<scope>/<name>.json` documents and not downloadable with ordinary tokens.  An external
  verifier inspects pending versions and renders a verdict through the
  [admin API](#admin-api-scope-verify).
- **verified** - part of the registry: composed, resolvable, downloadable, and covered by the
  immutability guarantee, which applies to verified versions **only**.
- **rejected** - the version never became part of the registry: its archive blob is reclaimed
  (unless another live version stores the same bytes), the publisher's storage quota is
  refunded, and the same `(name, version)` may be republished with any bytes - the row is
  replaced and returns to `pending` for a fresh verdict.

**Fail-safe direction.**  If the verifier never runs, nothing new ever becomes resolvable.
Broken verification infrastructure can only keep content unexposed; it must never expose
unverified content.

The verifier may also **abstain** - render no verdict at all, because an advisory name check
wants an operator's eyes first (`registry/docs/architecture.md`, "Name fidelity").  Abstain is
not a wire state: the version simply stays `pending`, and clients see exactly what they would
see while awaiting any verdict.

### Admin API (scope `verify`)

The `verify` scope belongs to the verifier: it may list pending versions and download their
artifacts (ordinary tokens cannot; rejected versions are downloadable by no one), and it gates
the two admin routes, which authenticate with the same `Authorization: Bearer` mechanism on the
same API origin.

```text
GET /api/v1/admin/versions?status=pending
```

Lists versions by status (`pending`, `verified`, or `rejected`; anything else is `400`) as a
single JSON object, `{"versions":[...]}`.  Each entry carries `name`, `version`, `checksum`
(lowercase SHA-256 hex), the publisher's registry-native user id as `published_by`,
`published_at`, and the stored canonical metadata document as `metadata`.  Deterministic:
ordered by name, then version.

```text
GET /api/v1/admin/packages
```

The package corpus for the verifier's [name advisories](#the-verifiers-checks):
`{"packages":[{"scope":...,"name":...,"vetted":<bool>}]}`, every package ordered by scope
then name, `vetted` reporting whether any of its versions is verified - the advisories skip a
name that was accepted once.  Deliberately not "has any verdict": a rejection never vets a
name, so rejecting an abstained squat cannot exempt that same name's next version.

```text
PATCH /api/v1/admin/versions/<scope>/<name>/<version>
{ "verdict": "verified" | "rejected", "reason": "...", "checksum": "...", "published_at": "..." }
```

Renders a verdict on a pending version; `reason` is required for rejections and recorded on the
version.  `checksum` and `published_at` echo what the listing reported and bind the verdict to
exactly that row generation - the checksum names the archive bytes, and `published_at` changes
on every replacement, so even a same-bytes republish with new metadata breaks the binding.
Both are **required** for `verified` verdicts (`400` without them - exposing content demands
naming what was inspected, because a rejected version can be republished at any moment and a
stale verdict must never land on content it never saw) and optional for rejections, the
conservative direction.  A binding that does not match the stored row conflicts (`409`), and
the same guard is enforced transactionally: a verdict racing a conflicting verdict or a
replacement answers `409` rather than applying.
An unbound rejection deliberately applies to whatever bytes are pending under the pair: refusing
uninspected bytes exposes nothing and serves operator takedowns, and republishing remains the
recovery path if a stale rejection catches a fresh replacement.
`verified` stamps `verified_at` and makes the version resolvable; `rejected` reclaims the blob
(when no live version references its bytes) and refunds the publisher's storage quota.  The
response reports the resulting state and whether the request changed it, mirroring yank:
`{"ok":true,"name":...,"version":...,"verification":"...","changed":<bool>}`.

Verdicts are idempotent for the same value: repeating the verdict a verified version already
carries is a `200` no-op.  Conflicts are `409`: a rejecting verdict on a verified version hits
the immutability wall, and **any** verdict on a rejected version is refused - republishing is
the recovery path, and a late duplicate verdict must never race the replacement.  An unknown
`(name, version)` is an authenticated `404`.

### The verifier's checks

The hosted registry's verifier is `cabin-registry-verify`, run every few minutes by a GitHub
Actions workflow (operations live in the service runbook).  It inspects each pending archive
against the canonical metadata the listing reported - parsing the zip container by hand,
decompressing each entry through a bounded reader, never extracting to disk, and assuming the
archive is hostile.  The archive must conform to the strict zip profile whose normative
definition is `registry/docs/archive-format.md`; this section is the user-facing summary.  The
checks, in order:

1. **Size discipline**: the sum of the entries' declared uncompressed sizes is a cheap up-front
   cap, and the running decompressed total is capped again as each entry is inflated, at
   `min(max(ratio x compressed size, floor), absolute cap)` - the floor covers the container
   framing the entry cap permits, since framing alone "expands" small archives beyond any sane
   ratio.  The entry count and per-entry path length are capped too, and crossing any cap aborts
   the inspection: a decompression bomb is rejected, never inflated.
2. **Structure**: the container must be a well-formed zip in the strict profile - the
   end-of-central-directory record at the fixed `len - 22` offset (single disk, zero comment, no
   zip64), the local records and the central directory tiling the file contiguously with no gaps,
   overlaps, or bytes outside the tiled regions, each entry stored or deflated with no data
   descriptors, extra fields, or comments and every general-purpose flag clear except the UTF-8
   bit on non-ASCII names, and every local header agreeing with its central header.  Each deflated
   entry must decompress to a clean stream end that consumes exactly its compressed span and
   yields exactly its declared uncompressed size, and its declared CRC-32 must match the bytes
   produced.  Entries are regular files only (directory entries or attributes are rejected;
   directories are implied), with safe relative paths - no absolute paths, no `..`, no duplicates,
   no `\`, no empty or `.` component, and none of the Windows-hostile shapes the shared path
   predicate forbids (`:`, a control character, `< > " | ? *`, a leading or trailing space, a
   trailing dot, or a reserved device name) - no name colliding with another under case-insensitive folding, no
   regular file used as another entry's parent directory, and `cabin.toml` at the archive root.
3. **Consistency**: the embedded manifest is parsed with Cabin's real manifest parser, must
   pass the same publishability rules `cabin package` enforces (no `[patch]` table, no path
   dependencies, no escaping source paths, no standard contradictions), must have every
   target source it declares present in the archive (a package missing a declared source would
   extract but fail to build), and must reproduce the
   entire stored canonical metadata document through the same derivation publish used - name,
   version, the three dependency tables, language-standard fields and the per-target standards
   table, features, profiles, toolchain, build settings, and the source block - and the archive
   bytes must hash to the recorded checksum (defense in depth; the server already checked at
   publish).

A rejection records machine-readable reason codes in the version's `verification_reason`:

| Code | Check |
| --- | --- |
| `decompressed_too_large` | decompression cap crossed |
| `too_many_entries` | entry-count cap crossed |
| `path_too_long` | path-length cap crossed |
| `unsupported_zip_feature` | a banned zip feature: a compression method other than store/deflate, a general-purpose flag outside the profile (any bit but the UTF-8 bit, which must be set exactly on non-ASCII names), a nonzero extra field, a comment, zip64, or a data descriptor |
| `header_mismatch` | a local header disagrees with its central header, or a declared uncompressed size or CRC-32 disagrees with the decompressed bytes |
| `forbidden_entry_type` | a non-regular entry: a symlink, a directory entry or attribute, or any other non-file type |
| `absolute_path` | absolute entry path (POSIX or Windows-drive form) |
| `path_traversal` | `..` path component |
| `invalid_path` | empty, non-UTF-8, `\`-bearing, or an empty/`.` component; a trailing-slash directory marker; or a Windows-hostile shape (`:`, a control character, `< > " | ? *`, a leading or trailing space, a trailing dot, or a reserved device name) |
| `duplicate_path` | the same path (byte for byte) twice |
| `case_conflict` | two paths that fold to the same string under Unicode default lowercasing, including a file used as a case-folded parent directory |
| `path_conflict` | a regular file used as another entry's parent directory |
| `missing_source` | the manifest declares a target source absent from the archive |
| `manifest_missing` | no `cabin.toml` at the archive root |
| `manifest_invalid` | the manifest does not parse as a publishable package |
| `name_mismatch` | manifest name disagrees with metadata or the listing |
| `version_mismatch` | manifest version disagrees with metadata or the listing |
| `dependency_mismatch` | manifest dependency tables disagree with metadata |
| `language_standard_mismatch` | manifest standard fields or the derived standards table disagree with metadata |
| `checksum_mismatch` | archive bytes do not hash to the recorded checksum |
| `metadata_mismatch` | any other canonical-metadata field disagrees with what the manifest derives |
| `archive_invalid` | not a well-formed zip container: a bad or misplaced EOCD, a non-contiguous layout, or bytes outside the tiled regions |

A recorded reason is the `code` above, optionally followed by one parenthesized detail that
narrows the cause - `unsupported_zip_feature (zip64)`, `header_mismatch (crc)`,
`invalid_path (trailing dot)`.  The machine-readable code is always the first token; the detail is
fixed text and never echoes archive bytes.

The cap mechanism is public contract; the cap values are configuration (`VERIFY_RATIO_CAP`,
`VERIFY_ABS_CAP_BYTES`, `VERIFY_MAX_ENTRIES`, `VERIFY_MAX_PATH_LEN`, defaulting to 10x,
256 MiB, 10000 entries, and 256 bytes).  Verifier failures leave versions pending - fail-safe:
broken verification infrastructure keeps content unexposed, never exposes it.

**Name advisories.**  Before downloading anything, the workflow checks each version that would
introduce a new package name against the [package corpus](#admin-api-scope-verify):
confusability under a skeleton fold (`-`/`_` fold away, `1`/`i` to `l`, `0` to `o`) against
every existing package and scope, edit distance 1 on the folded full name against other
scopes' packages, and a short unambiguous-profanity list matched as folded substrings.  A
finding never rejects - the workflow **abstains** (no verdict; the version stays `pending`)
and an operator reviews it, so a false positive costs a delay, never a rejection.  Once any
version of a package is **verified**, later versions skip the advisories; a rejection never
vets a name.  The rationale and
rules live in `registry/docs/architecture.md`, "Name fidelity".

### Server checks versus client extraction

These server-side checks agree in spirit with the client's
[extraction safety contract](package-format.md#extraction-safety-contract), and the two are
deliberately independent.  **The client's rules are the ones that must hold**: `cabin` extracts
archives from third-party registries and local file registries too, and must stay safe against a
hosted registry that has itself been compromised.  Nothing on the client trusts this verifier.

The verifier is at least as strict as the client on every axis they share.  It inspects only
archives `cabin package` produced, so it enforces the whole strict zip profile - rejecting the
zip64, extra-field, data-descriptor, and non-contiguous-layout constructions the client's extractor
merely tolerates.  It shares the client's lexical path-portability predicate through `cabin-fs`, so
the two reject the same Windows-hostile shapes (`\`, `:`, control characters, `< > " | ? *`,
leading or trailing spaces, trailing dots, reserved device names) by construction rather than by
parallel maintenance.
It additionally rejects case-folded name collisions the client deliberately tolerates (they would
refuse archives legitimate on case-sensitive Linux).  Its default caps (10x ratio, 256 MiB, 10000
entries, 256-byte paths) sit at or below the client's (32x ratio, 1 GiB, 10000 entries, 256-byte
paths).  A version that passes verification therefore clears the client's extraction rules.

That the verifier is stricter does not fold the two into one.  The caps above are *configurable*,
so a registry operator cannot widen the client's limits by widening their own - only the client's
constants do that.  "Verified" therefore means the archive is safe to extract by the client's own
rules; it is never a promise the client may delegate its safety to a registry.

## Yank

```text
PATCH /api/v1/packages/<scope>/<name>/<version>/yank
```

Requires a token with the `yank` scope, on the same API origin as publish.  The JSON body sets the
version's yanked state in the per-package index document:

```json
{ "yanked": true }
```

`{"yanked": false}` un-yanks.  The route is idempotent: setting the state a version already has
succeeds with `200` and body `{"ok":true}`.  Yank applies to
[**verified**](#verification-lifecycle) versions only - a pending or rejected version was never
part of the registry's resolvable surface, so there is nothing to retract and the pair answers
an authenticated `404`.

### Yanking from the client

`cabin yank` takes a strict `<scope>/<name>@<version>` spec - an exact scoped package name and an exact SemVer
version, no ranges - and resolves the registry exactly like remote publish: `--index-url`, else the
`[registry] index-url` setting in [`config.md`](config.md#registry); a local `index-path` is
rejected, since yanked state lives in the remote registry's index.  The registry's `config.json`
must declare the [`api`](#registry-configuration) origin the request is sent to.

```console
$ cabin -Z remote-registry yank fmtlib/fmt@10.2.1 --index-url https://registry.cabinpkg.com
fmtlib/fmt@10.2.1 is now yanked
$ cabin -Z remote-registry yank --undo fmtlib/fmt@10.2.1 --index-url https://registry.cabinpkg.com
fmtlib/fmt@10.2.1 is no longer yanked
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
| `401` | No token or an invalid token (never reveals whether the package exists).  Carries the [login-URL challenge](#the-login-url-challenge). |
| `402` | The registry's own budget breaker tripped (the hosted service blocks itself before provider limits or real spend are reached).  Writes are disabled service-wide; reads stay unaffected unless the registry's operator has configured a read budget and it is exhausted too, in which case authenticated reads (index and archive requests) answer `402` as well.  Carries `Retry-After` (seconds) and the envelope code `registry_over_budget`. |
| `403` | Valid token, but the scope the route requires is missing - or a per-user quota refusal, distinguished by the envelope's [`code`](#error-envelope) field. |
| `404` | Authenticated request for an unknown package or version - including versions that are not [verified](#verification-lifecycle), which are indistinguishable from unknown ones for ordinary tokens. |
| `409` | Publish of an existing (pending or verified) version with different bytes, or a conflicting [verdict](#admin-api-scope-verify). |
| `413` | The uploaded archive exceeds the per-archive size limit (envelope code `archive_too_large`). |
| `429` | Publish rate limit exceeded (token bucket).  Carries `Retry-After` (seconds) saying when the next publish will be accepted, and the envelope code `rate_limited`. |

## Error envelope

Every non-2xx response carries the same JSON envelope:

```json
{ "errors": [ { "detail": "authentication required" } ] }
```

Quota, rate-limit, and budget refusals additionally carry a machine-readable `code` field:

```json
{ "errors": [ { "detail": "total package quota exhausted", "code": "quota_packages_total" } ] }
```

Clients must ignore unknown fields in the envelope; errors without a `code` stay exactly as
before.  The defined codes:

| `code` | Status | Meaning |
| --- | --- | --- |
| `rate_limited` | `429` | The publish token bucket is empty; `Retry-After` says when it refills. |
| `archive_too_large` | `413` | The archive exceeds the per-archive size limit. |
| `quota_storage` | `403` | The publish would exceed the total stored-bytes quota. |
| `quota_packages_daily` | `403` | The daily new-package quota is exhausted. |
| `quota_packages_total` | `403` | The total package quota is exhausted. |
| `quota_versions_daily` | `403` | The daily per-package version quota is exhausted. |
| `registry_over_budget` | `402` | The service-wide budget breaker has writes - and, when a read budget is configured and exhausted, reads - paused; `Retry-After` covers the next re-evaluation. |

Cabin maps these refusals to actionable messages: a `402` on the mutation routes reports the
registry as temporarily not accepting publishes (over its free budget), a `402` on the read
routes reports package downloads and index reads as temporarily disabled to stay within the
registry's infrastructure budget, and the `429` reports the rate limit - each echoing
`Retry-After` as a "try again in N seconds" hint when the header is usable; the `413` reports the
archive as too large, appending the server's `detail`; and a `403` whose `code` starts with
`quota_` surfaces the server's `detail` verbatim - the detail itself embeds the registry's
usage-dashboard URL (`https://cabinpkg.com/dashboard` on the hosted registry), so the client
never derives a web URL from the index origin.  Unknown codes fall back to the plain detail
string.
