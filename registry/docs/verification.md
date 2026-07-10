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

---

# Quota, breaker, and client-mapping verification (2026-07-09)

Follow-up run after the per-user quotas and budget breaker landed on the
server (PR #1495) and the client learned to map the new refusals
(`cabin-registry-api`, this change). Same operator and dev environment as
above; client built from this branch (`cargo build --release -p cabinpkg`,
`-Z remote-registry`).

## Provisioning delta

- `scripts/smoke.sh` (local `wrangler dev`, full token run): `smoke OK`,
  including the writes-blocked 402 leg and its restore.
- Migration `0002_quotas.sql` applied remotely. The dev database held one
  user and one token and zero packages/versions, so nothing needed
  backfilling and `meta.total_stored_bytes` seeded correctly at `'0'`.
- `ANALYTICS_API_TOKEN` (an API token whose only permission is Account
  Analytics Read) stored as a dev secret by the operator. No
  `NOTIFY_WEBHOOK_URL` configured.
- `wrangler deploy --env dev`; the deploy output listed the cron trigger
  (`schedule: */15 * * * *`).
- Cron verified end to end: the 01:45 UTC pass appeared in `wrangler tail`
  (`{"cron":"*/15 * * * *"}`, outcome `ok`, 4 ms CPU, no analytics-skip
  log) and overwrote the manually cleared `service_mode_reason` with its
  own evaluation, `all budgets under 80%`.

## Near-limit publish and CPU headroom

A throwaway package carrying 16.3 MB of incompressible (random) payload
published successfully: `versions.archive_size` recorded 16,303,328 bytes
against the 16,777,216-byte per-archive cap, ~474 KB of headroom.
Per-request CPU from `wrangler tail` (`cpuTime`, wall time in
parentheses):

| Request | Status | CPU |
| --- | --- | --- |
| `PUT` near-limit publish (16.3 MB body) | 201 | 61 ms (811 ms) |
| `PUT` oversize publish (18 MB body) | 413 | 53 ms (241 ms) |
| `GET` artifact download (16.3 MB) | 200 | 279 ms (1,137 ms) |
| `PUT` small publish (scaffold-sized) | 201 | 5-10 ms |
| Refused writes (402 / 429) | - | 2 ms |
| Small reads (`config.json`, package docs) | 200 | 1-3 ms |

**Decision note.** The Workers free plan documents a 10 ms CPU limit per
invocation. Hashing costs ~4 ms/MiB, so a near-limit publish sits at
~60 ms, and the 16.3 MB artifact download at 279 ms - both far past the
documented limit, yet every request completed with outcome `ok` (no
`exceededCpu` was observed): enforcement is evidently lenient at this
volume. Lowering `max_archive_bytes` cannot buy real headroom - reads
dominate, and 10 ms corresponds to a ~2.5 MiB archive, too small to be
useful. Plan of record: keep dev as-is under its trivial traffic, and move
to Workers Paid (30 s CPU per request) before production serves real
traffic. Plans were deliberately not switched in this step.

## Breaker end to end

`meta.service_mode` forced to `writes_blocked` via `d1 execute` (dev pins
`SERVICE_MODE_TTL_SECS=0`, so it bites immediately):

```console
$ cabin -Z remote-registry publish --index-url https://dev-registry.cabinpkg.com
error: the registry is temporarily not accepting publishes (over its free budget); try again in 900 seconds
```

The 402's `Retry-After: 900` (the cron cadence) reached the message. While
writes were blocked, `cabin resolve` and `cabin fetch` (the 16.3 MB
artifact) worked unchanged from a consumer package. After restoring
`service_mode = 'normal'`, publishes succeeded again.

## Quota and rate-limit UX

Observed client messages, in the order the walkthrough hit them:

```console
# oversize archive (18 MB > 16 MiB cap), HTTP 413
error: the package archive is too large for this registry: archive exceeds the plan's per-archive size limit

# bucket below one token after a charged idempotent republish, HTTP 429;
# the 1 s Retry-After reflects the fractional refill (1 token/min) - a
# fully drained bucket reports ~60 s
error: the registry rate limited this request; try again in 1 seconds

# sixth new package of the (UTC) day, HTTP 403 code quota_packages_daily
error: the plan's daily new-package quota is exhausted; see https://dev-registry.cabinpkg.com/me for current usage
```

All three are actionable as-is; the `429`'s "1 seconds" plural was the one
wart, fixed in the client in this same change ("try again in 1 second").
Usage numbers moved as expected: packages created by the operator 0 -> 5,
`meta.total_stored_bytes` 0 -> 16,304,759 (the near-limit archive's
16,303,328 bytes plus four scaffold-sized archives of 357-359 bytes,
exactly `SUM(archive_size)` over the published versions), and `/me`
showed the matching usage (operator-confirmed in the browser). Note the daily quotas run on
UTC days: the five throwaway packages exhaust the operator's new-package
quota until the next UTC midnight (dev data is disposable; the rows can be
deleted per the runbook if that ever blocks real work).

## WAF rate limiting rule

The operator created the dashboard rule recorded in `runbook.md` ("Zone
rate limiting (WAF)"): 50 requests per 10 s per IP over
`dev-registry.cabinpkg.com` paths `/api/*`, `/login`, `/callback`, action
Block for 10 s - the Free plan's single rule slot, with period, timeout,
and IP keying all fixed by the plan. Verified with a 70-request burst
against an `/api/` path: exactly 50 reached the Worker (uniform 401), the
remaining 20 answered a Cloudflare `429` with `retry-after: 10` and no
error envelope.

## Friction observed

1. **(client, fixed here)** "try again in 1 seconds" - the retry hint now
   pluralizes.
2. **(ops, minor)** The Workers observability API was transiently
   unavailable ("Upstream Cloudflare API unavailable") during the CPU
   checks; `wrangler tail --format json` (which carries `cpuTime` and
   `wallTime` per event) was sufficient on its own.

## Backups and restore drill (2026-07-10, UTC)

The backup pipeline (runbook, "Backups and disaster recovery") was
provisioned and rehearsed end to end against dev. Executed with Claude
driving; the two rehearsals below used the operator's provisioning token
(S3 credentials derived as token id + SHA-256 of the token value) - the
workflow itself runs on the narrowly scoped tokens in the runbook table.

### Bucket provisioning

`npx wrangler r2 bucket create cabin-registry-dev-backup` created the
bucket; a second run failed cleanly with "The bucket you tried to create
already exists, and you own it. [code: 10004]", confirming the create is
safely re-runnable. `cabin-registry-prod-backup` was deliberately not
created (production checklist item 6).

### Local pipeline rehearsal

Every workflow step run by hand, in order, against the real dev
resources: `wrangler d1 export cabin-registry-dev --remote` produced a
7,374-byte dump (31 statements; `d1_migrations` plus the five canonical
tables - no `_cf_KV`, which `d1 export` excludes);
`backup-verify-dump.sh` accepted it; gzip + sidecar uploaded to
`d1/2026-07-10.sql.gz`; the re-downloaded object passed `sha256 -c` and
`cmp` byte-identical (also confirming `gzip -n` determinism); the blob
copy landed all 6 primary blobs under `blobs/`; the retention prune
dry-run correctly reported "nothing to prune" for a one-dump bucket.

### Restore drill

```console
$ scripts/restore-drill.sh dev
==> locating the newest dump in cabin-registry-dev-backup/d1/
    d1/2026-07-10.sql.gz
==> downloading and checking the dump
2026-07-10.sql.gz: OK
dump OK: .../2026-07-10.sql
==> importing into the scratch database cabin-registry-drill
==> comparing per-table row counts against cabin-registry-dev
    d1_migrations  live=2 restored=2
    meta           live=4 restored=4
    packages       live=5 restored=5
    tokens         live=2 restored=2
    users          live=1 restored=1
    versions       live=5 restored=5
==> spot-checking one version's metadata JSON
    qv-a@0.1.0: metadata JSON parses, matches live byte for byte
restore drill OK: d1/2026-07-10.sql.gz restores cleanly
==> tearing down the scratch database
```

`wrangler d1 list` afterwards shows only `cabin-registry-dev`: the
scratch database was torn down. The import wrote 81 rows across 31
statements and left the scratch database at 0.08 MB.

### Time Travel

Checked against the current D1 docs (2026-07-10): Time Travel is
available on every D1 database with no setup, 7-day retention on Workers
Free (current plan), 30-day on Workers Paid; restore is in-place and
destructive but reversible via the pre-restore bookmark. Recorded in the
runbook as the first-line recovery option.

### Workflow validation (GitHub Actions)

The five repository secrets were provisioned from freshly minted tokens
per the runbook table and sanity-tested before use: the D1 token is an
account-owned token that verifies as active and can `d1 list`; the
primary-read S3 pair lists the 6 blobs but is **denied writes** to the
primary bucket (probed and confirmed); the backup-write pair reads and
writes the backup bucket only.

GitHub does not register a `workflow_dispatch` trigger for a workflow
that only exists on a branch (the dispatch API answers 404 until the
file lands on the default branch), so the pre-merge validation ran the
workflow via a temporary `push` trigger on the PR branch, removed again
before merge; the standing triggers are the nightly cron and
`workflow_dispatch`.

Run: <https://github.com/cabinpkg/cabin/actions/runs/29067663994> - all
steps green on the first attempt: export (`dump OK: d1.sql`), compress +
checksum, blob copy, publish (same-day pair cleared, sidecar first),
re-download (`2026-07-10.sql.gz: OK`, byte-identical), prune ("nothing
to prune"). The restore drill was then re-run against the
workflow-written dump (not the rehearsal one) with identical results,
including teardown.
