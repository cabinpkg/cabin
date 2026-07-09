# Dev Environment Verification (2026-07-09)

End-to-end verification of the dev registry (`dev-registry.cabinpkg.com`)
against a from-source build of the client (`cabin 0.17.0`,
`cargo build --release -p cabinpkg`, `-Z remote-registry`). Executed by the
operator (ken-matsui, GitHub id 26405363) with Claude driving. Tokens are
redacted throughout; both walkthrough tokens were revoked or destroyed by the
wipe-procedure verification at the end of this run.

The exact provisioning and wipe commands live in
[`runbook.md`](runbook.md); this document records what was run, what was
observed, and the friction found, so client-side follow-ups can be filed
from it.

## Provisioning (summary)

Resources created with wrangler from `registry/` exactly as recorded in the
runbook: D1 `cabin-registry-dev`, R2 `cabin-registry-dev-blobs`, migrations
applied remotely, `GITHUB_CLIENT_ID` + `ALLOWED_GITHUB_IDS` as plain vars in
`wrangler.jsonc`, `GITHUB_CLIENT_SECRET` + fresh `SESSION_SECRET` (32 random
bytes, base64) as secrets, deploy with `--env dev` (the custom domain and
its DNS record were created by the deploy). No production resource was
touched.

**Zone-level blocker found:** the cabinpkg.com zone had Cloudflare Bot Fight
Mode challenging every request (`403`, `cf-mitigated: challenge`) on all
hosts, for curl and cabin alike - a hosted registry cannot serve machine
clients behind a visitor challenge. The operator disabled Bot Fight Mode
zone-wide; see the runbook's "Zone security prerequisite" section for the
constraint and options.

## Service verification

```console
$ curl -sS -o /dev/null -w '%{http_code}' https://dev-registry.cabinpkg.com/healthz
200        # empty body

$ curl -sS https://dev-registry.cabinpkg.com/config.json
{"errors":[{"detail":"authentication required"}]}    # 401

$ curl -sS https://dev-registry.cabinpkg.com/packages/zz-no-such-pkg.json
{"errors":[{"detail":"authentication required"}]}    # 401

$ curl -sS https://dev-registry.cabinpkg.com/artifacts/zz-no-such-pkg/zz-no-such-pkg-9.9.9.tar.gz
{"errors":[{"detail":"authentication required"}]}    # 401
```

The three unauthenticated 401 bodies were compared with `cmp`:
byte-identical, so existing and non-existing packages are
indistinguishable without a token. `x-cabin-registry-generation` was absent
on every unauthenticated response (including `/healthz`) and present on
every authenticated response:

```console
$ curl -sS -D - -H "Authorization: Bearer cabin_<redacted>" \
    https://dev-registry.cabinpkg.com/config.json
HTTP/2 200
x-cabin-registry-generation: 1
{"schema":1,"kind":"file-registry","packages":"packages","artifacts":"artifacts","auth-required":true,"api":"https://dev-registry.cabinpkg.com"}
```

Unauthenticated `/me` answered `302` with `location: /login`.

## Bug found and fixed: canonical envelope leaked into version entries

First publish succeeded, but the unchanged republish - and any
resolve/fetch/build against the package - failed client-side:

```text
invalid package metadata from HTTP index for `hello_registry`: unknown field
`schema`, expected one of `dependencies`, `dev-dependencies`, ...
```

`packages/<name>.json` embedded each stored canonical per-version document
verbatim, so version entries carried the document-level
`schema`/`name`/`version` envelope that `docs/package-index.md` forbids
("unknown fields anywhere in the file are rejected"). The server's unit
tests hand-wrote envelope-free entries and never caught it; the local file
registry (`cabin-registry-file::version_value_from_metadata`) already emits
entries without the envelope.

Fixed in `src/documents.rs` (`package_json` now strips
`schema`/`name`/`version` at compose time - `shift_remove`, because plain
`remove` is a swap-remove under serde_json's `preserve_order` and would
scramble entry key order), with a regression test storing a realistic
enveloped entry. Because the strip happens at read time, rows already
stored verbatim were healed by the redeploy without a wipe. Follow-up worth
filing: a conformance check that the *served* document parses under the
client's index schema (the `#[ignore]`d fixture test only covers publish
validation).

## Operator UX walkthrough

Sign-in at `https://dev-registry.cabinpkg.com/me` via GitHub (OAuth app
"Cabin (dev)", public-data-only scope) worked first try; the allowlist
admitted the operator and the token page rendered. A token
`dev-verification` with `publish` + `yank` scopes was created; plaintext
shown exactly once.

```console
$ cabin -Z remote-registry login --index-url https://dev-registry.cabinpkg.com
visit https://dev-registry.cabinpkg.com/me to create a token
       Login token for `https://dev-registry.cabinpkg.com` saved
```

Sample package: `cabin new --lib hello_registry` (scaffold untouched:
c++17, one `add(int, int)` function), published as-is:

```console
$ cabin -Z remote-registry publish --index-url https://dev-registry.cabinpkg.com
Published hello_registry 0.1.0 to https://dev-registry.cabinpkg.com
  checksum: sha256:7f1ded07a18e471c9fb2121bc35ae7982c901b833b277b58b4fd926a9eb4a137

$ cabin -Z remote-registry publish --index-url https://dev-registry.cabinpkg.com
hello_registry 0.1.0 is already published to https://dev-registry.cabinpkg.com with identical bytes; nothing to do
  checksum: sha256:7f1ded07a18e471c9fb2121bc35ae7982c901b833b277b58b4fd926a9eb4a137
```

Consumer (`cabin new consumer`, `hello_registry = "^0.1"` under
`[dependencies]`, `deps = ["hello_registry"]` on the target, `main.cc`
calling `hello_registry::add`):

```console
$ cabin -Z remote-registry resolve --index-url https://dev-registry.cabinpkg.com
Resolved dependencies for consumer 0.1.0:
  hello_registry 0.1.0
# cabin.lock pins checksum = "sha256:7f1ded07a18e471c9fb2121bc35ae7982c901b833b277b58b4fd926a9eb4a137"

$ cabin -Z remote-registry fetch --index-url https://dev-registry.cabinpkg.com
Fetched artifacts:
  hello_registry 0.1.0 -> ~/.cache/cabin/sources/sha256/7f1ded07...
# content-addressed by the lockfile checksum; a mismatched archive cannot land

$ cabin -Z remote-registry build --index-url https://dev-registry.cabinpkg.com
   Compiling hello_registry v0.1.0
   Compiling consumer v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.10s

$ ./build/dev/packages/consumer/consumer
2 + 3 = 5
```

Yank cycle:

```console
$ cabin -Z remote-registry yank hello_registry@0.1.0 --index-url https://dev-registry.cabinpkg.com
hello_registry@0.1.0 is now yanked

$ cabin -Z remote-registry update --index-url https://dev-registry.cabinpkg.com   # in consumer/
error: all matching versions of "hello_registry" are yanked
  help: loosen the version requirement so a non-yanked release is in range,
        or contact the package maintainer to republish

$ cabin -Z remote-registry yank --undo hello_registry@0.1.0 --index-url https://dev-registry.cabinpkg.com
hello_registry@0.1.0 is no longer yanked

$ cabin -Z remote-registry update --index-url https://dev-registry.cabinpkg.com
Resolved dependencies for consumer 0.1.0:
  hello_registry 0.1.0
```

Logout and the guidance on the next read:

```console
$ cabin -Z remote-registry logout --index-url https://dev-registry.cabinpkg.com
      Logout token for `https://dev-registry.cabinpkg.com` removed

$ cabin -Z remote-registry resolve --index-url https://dev-registry.cabinpkg.com
error: authentication required by registry `https://dev-registry.cabinpkg.com`;
run `cabin login --index-url https://dev-registry.cabinpkg.com` with
`-Z remote-registry` to store a token
```

## Wipe/recreate verification

The runbook's wipe procedure was executed against this real dev database
after the walkthrough (drop + recreate D1, re-point `database_id`,
re-apply migrations, delete the R2 blob, bump the generation, redeploy).
Verified afterwards: `/healthz` 200, uniform 401 unchanged, authenticated
reads carry `x-cabin-registry-generation: 2`, `packages/hello_registry.json`
is an authenticated 404, and a browser holding a pre-wipe session cookie
recovers transparently (`/me` -> `/login` -> GitHub auto-approves the
already-authorized app -> `/callback` recreates the user row). Pre-wipe
tokens are dead, as documented.

## UX friction observed

1. **(client, worth filing)** A versioned dependency declared in
   `[dependencies]` but wired into no target's `deps` is silently inert:
   `resolve` and `fetch` succeed, then the build fails with a bare
   `'hello_registry/hello_registry.hpp' file not found` compile error and no
   mention that the fetched package was never attached to a target. A
   warning for resolved-but-unconsumed versioned deps (or a hint appended to
   the compile failure when the missing header matches a fetched package's
   include tree) would have saved the longest debugging detour of this
   walkthrough.
2. **(client, minor)** `cabin fetch -v` prints the cache path but never says
   "checksum verified"; the guarantee is real (content-addressed layout)
   but invisible. One verbose line would make the property observable.
3. **(service/ops)** For a few seconds after `wrangler deploy --env dev`,
   requests can still hit the previous worker version - observed once as a
   stale package document and once as a `500` `internal error` right after
   the wipe's redeploy (old version bound to the deleted D1). Retry after
   ~a minute before diagnosing.
4. **(ops)** Zone-wide bot protection and a machine-facing registry host on
   the same zone conflict; this must be handled deliberately (see the
   runbook's zone security prerequisite).

Everything else - sign-in, token issuance, login, publish wording, no-op
wording, lockfile checksums, yank cycle wording, logout guidance - behaved
exactly as documented and needed no explanation beyond the CLI's own
output.
