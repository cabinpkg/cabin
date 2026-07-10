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

---

# Backup and restore-drill verification (2026-07-10)

Rehearsal of the backup machinery (blob replication, nightly D1 dump,
freshness alerting, restore drill - see `runbook.md`, "Disaster
recovery") against the dev environment, before production exists. Same
operator; Claude driving.

## Provisioning delta

- `cabin-registry-dev-backup` R2 bucket present (pre-created by the
  operator; `wrangler r2 bucket create` fails cleanly on re-run, as the
  runbook documents).
- Migration `0003_backup.sql` (the `backup_replication_failures` table)
  applied remotely.
- `wrangler deploy --env dev`: bindings list `env.BACKUP`
  (cabin-registry-dev-backup) and `env.D1_DATABASE_ID`; the deploy
  registered both schedules, `*/15 * * * *` and `0 3 * * *`.
- `D1_EXPORT_API_TOKEN` (custom API token, sole permission
  Account | D1 | Edit, this account only) created by the operator and
  stored as a dev secret.
- `scripts/backup-backfill.sh dev`: all 5 referenced blobs were already
  present in the backup bucket (`copied 0, already present 5`), and the
  replication failure log was (harmlessly) cleared - the reconciliation
  loop and the presence checks work against real remote buckets.

## Local smoke (mocked export API)

`scripts/smoke.sh` full-token run: `smoke OK`. The new legs verified,
against local `wrangler dev --test-scheduled`: the published blob
appears in the BACKUP bucket via the `waitUntil` replication and is
byte-identical to the uploaded archive; `/__scheduled?cron=0+3+*+*+*`
routes to the dump job, which polled the mocked export endpoint,
streamed the dump into `d1/<today>.sql`, wrote a `.sha256` sidecar that
`shasum -c` accepts, and recorded `meta.last_backup_at` /
`meta.last_backup_key` (the script prints `last_backup_at` at the end).
The mock serves a real `wrangler d1 export --local` dump, so the
validation patterns run against the genuine dump format.

## Real dump against the deployed dev worker

The scheduled handler routes any non-breaker cron expression to the dump
job, so the rehearsal used the documented path: a temporary third
schedule (`*/5 * * * *`) was added to `env.dev.triggers` and deployed.
The 05:10 UTC fire ran the job end to end against the real D1 export
API with the operator's `D1_EXPORT_API_TOKEN`:

```console
$ npx wrangler d1 execute DB --env dev --remote --json --command \
    "SELECT key, value FROM meta WHERE key LIKE 'last_backup%'"
last_backup_at   2026-07-10T05:10:01.588Z
last_backup_key  d1/2026-07-10.sql
```

`d1/2026-07-10.sql` and its sidecar were downloaded from the backup
bucket with `wrangler r2 object get --remote`: 7,564 bytes,
`shasum -a 256 -c` accepts the sidecar, and the dump carries the
`CREATE TABLE` statements for all five canonical tables (plus
`d1_migrations` and `backup_replication_failures`). The temporary
schedule was then removed and the final two-schedule config redeployed.

## Restore drill

`scripts/restore-drill.sh dev`, run twice on purpose:

1. The first run **failed the meta row-count comparison** (live 6,
   restored 4) - the drill catching a real timeline artifact: the dump
   is exported before the job records its own success, so the first
   dump can never contain `last_backup_at` / `last_backup_key`. The
   comparison now excludes exactly those two keys (they are the record
   of the dump succeeding, not registry data).
2. The second run passed everything:

```console
$ scripts/restore-drill.sh dev
==> resolving the latest dump from meta.last_backup_key
==> downloading d1/2026-07-10.sql and its checksum sidecar from cabin-registry-dev-backup
2026-07-10.sql: OK
==> creating the scratch database cabin-registry-drill
==> importing the dump into cabin-registry-drill
==> comparing per-table row counts against the live database
    backup_replication_failures  live      0  restored      0
    d1_migrations                live      3  restored      3
    meta                         live      4  restored      4
    packages                     live      5  restored      5
    tokens                       live      2  restored      2
    users                        live      1  restored      1
    versions                     live      5  restored      5
==> spot-checking one version's metadata JSON
    qv-a@0.1.0: metadata_json matches and parses (478 bytes)
==> tearing down cabin-registry-drill
restore drill OK (d1/2026-07-10.sql)
```

The import processed 33 queries (53 rows read, 87 written) into the
scratch database; teardown deleted it (`wrangler d1 list` shows only
`cabin-registry-dev` afterwards).

## Notes

1. The backup bucket and the 5 referenced blobs were already in place
   before this run (operator pre-provisioning); the backfill script
   verified convergence rather than performing first copies. Live
   publish-time replication was exercised via the local smoke's real
   `waitUntil` path; a remote publish could not be exercised this run
   because the operator's daily new-package quota was still consumed by
   the previous verification's `qv-*` packages (UTC window).
2. The first drill run's failure is recorded deliberately: a restore
   drill that can fail - and explain why - is the point of rehearsing.

---

# Verifier and verification lifecycle (2026-07-10)

End-to-end rehearsal of the external verifier (`cabin-registry-verify` +
the `registry-verify` GitHub Actions workflow - see `runbook.md`,
"Verification pipeline", and `docs/remote-registry.md`, "The verifier's
checks") against the dev environment. The full pending -> verified ->
resolvable path, the reject path (a malicious archive that passes the
server's synchronous checks), the quota refund, and the reject ->
republish recovery were all exercised. Same operator; Claude driving.
The verifier loop was run locally with the same steps the workflow
scripts (list, download, run the binary, PATCH the verdict), using a
registry token created on `/me` with **only** the `verify` scope.

## Provisioning delta

- Migration `0004_verification.sql` applied remotely (`wrangler d1
  migrations apply DB --env dev --remote`): the `versions.verification`
  / `verification_reason` / `verified_at` columns plus the backfill of
  the 5 pre-pipeline rows to `verified`.
- `wrangler deploy --env dev` with the verification code (publish sets
  `pending`, reads gate on `verified`, the admin list/verdict API, the
  publish-time `schema != 1` refusal).
- A `verify`-only token (`github-actions-verifier`) created at `/me`;
  its scope column is exactly `verify`. This is the credential the
  workflow carries as the `REGISTRY_VERIFY_TOKEN` secret.

## Benign lifecycle: pending -> verified -> resolvable

Published a new **version** of an existing package (`qv-a@0.2.0`); the
operator's daily *new-package* quota was still consumed by the previous
verifications' `qv-*` packages, and a new version is gated only by the
per-package cap, so this exercised the same publish path.

```console
$ cabin -Z remote-registry publish --manifest-path .../qv-a/cabin.toml \
    --index-url https://dev-registry.cabinpkg.com
Published qv-a 0.2.0 to https://dev-registry.cabinpkg.com
  checksum: sha256:ee8d454f...
  verification: pending (the version was accepted and becomes resolvable
    after verification, typically within a few minutes)

# read gate: the pending version is absent from the composed document
$ GET /packages/qv-a.json           # -> versions: ["0.1.0"]

# admin listing (verify scope) reports it with its canonical metadata
$ GET /api/v1/admin/versions?status=pending
  -> {"versions":[{"name":"qv-a","version":"0.2.0","checksum":"ee8d454f...",
       "published_by":26405363,"published_at":"...","metadata":{...}}]}

# the verify scope may download the pending artifact (ordinary tokens 404)
$ GET /artifacts/qv-a/qv-a-0.2.0.tar.gz   # 200, 241 bytes

$ cabin-registry-verify benign.tar.gz entry.json
  {"verdict":"verified"}

$ PATCH /api/v1/admin/versions/qv-a/0.2.0
    {"verdict":"verified","checksum":"ee8d454f...","published_at":"..."}
  -> {"ok":true,"name":"qv-a","version":"0.2.0","verification":"verified",
      "changed":true}

# now composed and resolvable
$ GET /packages/qv-a.json           # -> versions: ["0.1.0","0.2.0"]
$ cabin -Z remote-registry resolve  # consumer depends on qv-a = "=0.2.0"
  Resolved dependencies for consumer 0.1.0:
    qv-a 0.2.0
```

## Reject path: a hostile archive the server accepts

A `qv-a@0.3.0` archive was hand-crafted to pass every *synchronous*
server check - canonical metadata, matching name/version/source path,
and a `checksum` that is the real SHA-256 of the archive bytes - while
carrying a path-traversal entry (`../escape.h`) next to a valid
`cabin.toml`. The server has no reason to refuse it; only the verifier
stands between it and a resolvable version.

```console
$ PUT /api/v1/packages/qv-a/0.3.0     # framed metadata + archive
  -> 201 {"ok":true,...,"verification":"pending"}   # server accepts it

# storage self-accounting rose by the archive's 192 bytes
  total_stored_bytes: 16305000 -> 16305192

$ cabin-registry-verify hostile.tar.gz entry.json
  {"verdict":"rejected","reasons":["path_traversal"]}

$ PATCH /api/v1/admin/versions/qv-a/0.3.0
    {"verdict":"rejected","reason":"path_traversal","checksum":"417ac796...",
     "published_at":"..."}
  -> {"ok":true,...,"verification":"rejected","changed":true}
```

Consequences, all confirmed against D1 and the composed document:

- `GET /packages/qv-a.json` still lists only `["0.1.0","0.2.0"]` - the
  rejected version never surfaced.
- The row: `0.3.0 rejected path_traversal`.
- `total_stored_bytes` back to `16305000` - the 192 bytes were
  **refunded** when the row flipped to rejected (it was the blob's sole
  live reference).

## Recovery: reject -> republish -> verified

The same `(name, version)` accepts a replacement with any bytes and
returns to `pending`:

```console
$ cabin -Z remote-registry publish   # a clean qv-a 0.3.0
  Published qv-a 0.3.0 ...   verification: pending
  checksum: sha256:245b9452...

$ cabin-registry-verify fixed.tar.gz entry.json   # {"verdict":"verified"}
$ PATCH .../qv-a/0.3.0 {"verdict":"verified",...}  # changed:true
$ GET /packages/qv-a.json            # -> ["0.1.0","0.2.0","0.3.0"]
$ cabin -Z remote-registry resolve   # consumer qv-a = "=0.3.0"
  Resolved dependencies for consumer 0.1.0:
    qv-a 0.3.0
```

## Notes

1. The verifier loop was driven locally with the exact steps the
   `registry-verify` workflow scripts. The workflow itself is not
   scheduled until the `REGISTRY_VERIFY_TOKEN` secret is set on the
   repository; GitHub cron is best-effort, and the stuck-pending webhook
   alert (breaker cron) is the detection mechanism, not the schedule.
2. `python-urllib`'s default user agent is 403'd by the zone's Bot Fight
   Mode; the helper sets an explicit `user-agent`, mirroring the trap
   already noted for `qv-*` provisioning.
3. The malicious fixture is a legitimate archive shape with one hostile
   entry, so it exercises the boundary the task targets: the server's
   synchronous checks (framing, checksum, metadata) all pass, and the
   verifier is the only thing that catches it.
