# Registry Service Runbook

All wrangler commands run with `registry/` as the working directory and an
explicit `--env`. Authentication: `CLOUDFLARE_API_TOKEN` in the environment
(scopes: Workers Scripts Edit, D1 Edit, R2 Edit, and DNS Edit on the
cabinpkg.com zone).

## Data policy

- **dev (`dev-registry.cabinpkg.com`): disposable.** When the storage format
  changes incompatibly - `metadata_json` shape, R2 key layout, reshaped D1
  columns - the dev environment is wiped and recreated rather than migrated.
  Additive schema changes ship as ordinary migrations (e.g.
  `0002_quotas.sql`); where an additive migration leaves columns to
  backfill, a one-shot script is provided (see "Backfilling migration
  0002") so wiping stays optional.
- **production (`registry.cabinpkg.com`): permanent.** Published archives and
  index state are never wiped, mutated, or deleted. Once production carries
  real data, format changes need real migrations - which is exactly why the
  dev environment gets to burn its data instead while the format is still
  moving.

## Zone security prerequisite

The registry hosts serve machine clients (cabin, curl, CI), so they must not
sit behind a Cloudflare visitor challenge. Zone-wide Bot Fight Mode on
cabinpkg.com answered every registry request with `403` /
`cf-mitigated: challenge` until the operator disabled it (2026-07-09,
dashboard: Security -> Bots). If zone-wide bot protection is ever wanted
again, it needs a plan that exempts the registry hosts first - e.g. a WAF
custom rule skipping the challenge products for
`http.host in {"dev-registry.cabinpkg.com" "registry.cabinpkg.com"}` (note
free-plan Bot Fight Mode ignores skip rules; it can only be toggled
zone-wide). Managing zone security needs API-token scopes beyond the
provisioning set (Zone WAF Edit, Zone Settings Edit) or the dashboard.

## Zone rate limiting (WAF)

Zone-level defense for the Workers request budget: one rate limiting rule
on the cabinpkg.com zone, created 2026-07-09 via the dashboard (Security ->
WAF -> Rate limiting rules; the provisioning API token deliberately has no
WAF scopes). The Free plan allows exactly one rate limiting rule, with the
counting period and mitigation timeout fixed at 10 seconds and IP keying
only, so the conservative 300 requests/minute target is expressed as 50
requests per 10 seconds:

- Name: `dev-registry-api-rate-limit`
- Expression:
  `(http.host eq "cabinpkg.com" and (starts_with(http.request.uri.path, "/api/") or http.request.uri.path eq "/login" or starts_with(http.request.uri.path, "/callback")))`
- Same characteristics: IP. Rate: 50 requests per 10 seconds. Action:
  Block, mitigation timeout 10 seconds.

The hostname-role split moved the whole write/auth surface to the
website origin (see "Integrated topology and route management"), so
the rule keys on
`cabinpkg.com` - a rule still watching `dev-registry.cabinpkg.com`
would protect only that host's cheap uniform-401 surface. Update the
dashboard rule alongside deploying the split. The zone security
exemption above must likewise keep the machine `/api/*` traffic on
`cabinpkg.com` out of any visitor challenge.

The rule deliberately guards only the write/auth surface, not `/healthz`
or the read routes. Covering reads with the same 50-per-10 s ceiling
would throttle legitimate `cabin` traffic - resolving and fetching a
dependency tree fans out many read requests from one IP in seconds -
while abuse of the omitted routes can at worst exhaust free-plan quotas
that fail closed without billing (Workers requests, D1 reads; artifact
downloads are R2 Class B). The one rule the Free plan grants goes where
the paid exposure (R2 Class A writes) and the heavy CPU live.

Verified 2026-07-09 with a 70-request burst against an `/api/` path:
exactly 50 requests reached the Worker, the rest answered a Cloudflare
`429` with `retry-after: 10` (see `verification.md`). A WAF `429` carries
no error envelope; cabin's rate-limit mapping degrades to the same "try
again" hint off the header alone.

When production is provisioned, the single Free-plan slot must cover both
hosts: widen the host test to
`http.host in {"dev-registry.cabinpkg.com" "registry.cabinpkg.com"}`
instead of adding a second rule.

## Integrated topology and route management

Three hostnames, one zone, verified live on 2026-07-11 (see
[`verification.md`](verification.md), "Hostname-role split and
integrated-system verification"):

- **`cabinpkg.com`** - the website Worker (`cabin-website`, deployed by
  Workers Builds from `website/` on every push to `main`) serves the
  marketing site, docs, and the account pages; the registry Worker
  takes exactly `/api/*`, `/login`, and `/callback*` via the zone
  routes below. This one origin is the browser plane for whichever
  registry environment holds the routes - env **dev** today.
- **`dev-registry.cabinpkg.com`** - the dev registry's machine read
  plane (custom domain of `cabin-registry-dev`), nothing else.
- **`registry.cabinpkg.com`** - the production registry's machine read
  plane, once production is provisioned.

The production cutover is a pure route move: the three `cabinpkg.com`
patterns transfer from `env.dev` to `env.production` in
`wrangler.jsonc` and both envs are redeployed. Nothing else about the
topology changes - the website keeps its origin, each registry keeps
its custom domain, and `config.json`'s `api` field keeps pointing at
`https://cabinpkg.com` from both environments. What changes is which
registry's browser plane answers there, so the dev registry loses
sign-in, token issuance, and its session API at that moment (recovery
options below).

The Worker reaches the website origin through **zone routes** on
cabinpkg.com (`wrangler.jsonc`): `cabinpkg.com/api/*`,
`cabinpkg.com/login`, and `cabinpkg.com/callback*`. Route facts that
matter operationally:

- Path routes are more specific than the website Worker's own domain,
  so they take precedence on exactly these paths and nothing else
  (Cloudflare picks the most specific matching route). Verify after any
  route change: `/api/*` answers the registry Worker (uniform 401
  envelope), while `/` and `/login/denied` still render the website.
- A pattern without a trailing `*` never matches a URL carrying a query
  string - GitHub redirects to `/callback?code=...&state=...`, hence
  `cabinpkg.com/callback*`. `/login` deliberately stays exact so the
  website keeps serving `/login/denied`.
- A route pattern can point at only **one** Worker, so `env.dev` holds
  the three patterns for now and `env.production` declares none. The
  production cutover consists of moving these exact patterns from the
  dev Worker to the production Worker (edit `wrangler.jsonc`, deploy
  both envs). At that moment the dev registry loses its browser plane:
  recovery options are standing up a `dev.cabinpkg.com` website origin
  for dev then (new routes + a dev `WEB_ORIGIN`), or falling back to
  manual token insertion via `wrangler d1 execute` (mirror the INSERT
  in `scripts/smoke.sh`, seeding `users` and a `tokens` row whose
  `token_hash` is the SHA-256 hex of a locally generated
  `cabin_<base62>` value).
- The website's `/dashboard`, `/settings/*`, and `/login/denied` pages
  are live on the origin ("Account pages" in `website/README.md`). For
  ops debugging, the session API also works directly: `curl -H "Cookie:
  cabin_session=..." -H "Content-Type: application/json" -H
  "X-CSRF-Protection: 1"` against `https://cabinpkg.com/api/v1/user/...`.

## First-time provisioning (dev)

Verified end to end on 2026-07-09 (see
[`verification.md`](verification.md)). Prerequisite besides the API
token: the GitHub OAuth app (homepage `https://cabinpkg.com`,
authorization callback `https://cabinpkg.com/callback` - the browser
plane lives on the website origin, see "Integrated topology and route
management"; the same app carries through to production). Its
client id is public and lives in `wrangler.jsonc` (`env.dev.vars`,
`GITHUB_CLIENT_ID`), next to `ALLOWED_GITHUB_IDS` (the numeric GitHub user
ids allowed to sign in); the wrangler secrets are the client
secret, the session secret, `ANALYTICS_API_TOKEN` for the budget cron,
and `D1_EXPORT_API_TOKEN` for the nightly dump (plus the optional
`NOTIFY_WEBHOOK_URL`; see "Budget breaker and service mode" and
"Disaster recovery").

```sh
npx wrangler d1 create cabin-registry-dev
# copy the printed database_id into env.dev.d1_databases AND
# env.dev.vars.D1_DATABASE_ID in wrangler.jsonc
npx wrangler r2 bucket create cabin-registry-dev-blobs
npx wrangler r2 bucket create cabin-registry-dev-backup
npx wrangler d1 migrations apply DB --env dev --remote
npx wrangler deploy --env dev
# deploy creates the dev-registry.cabinpkg.com custom domain and its DNS
# record on the cabinpkg.com zone; deploy first so the secret puts below
# attach to a deployed Worker instead of prompting to create a draft.
printf '%s' "$GITHUB_CLIENT_SECRET" | npx wrangler secret put GITHUB_CLIENT_SECRET --env dev
openssl rand -base64 32 | npx wrangler secret put SESSION_SECRET --env dev
printf '%s' "$ANALYTICS_API_TOKEN" | npx wrangler secret put ANALYTICS_API_TOKEN --env dev
printf '%s' "$D1_EXPORT_API_TOKEN" | npx wrangler secret put D1_EXPORT_API_TOKEN --env dev
```

Idempotence: `d1 create` / `r2 bucket create` fail cleanly if the resource
exists (`d1 list` / `r2 bucket list` to check); `migrations apply` and
`deploy` are safe to re-run; a re-run `secret put` overwrites the value.

Smoke checks after any deploy:

```sh
curl -sS -o /dev/null -w '%{http_code}\n' https://dev-registry.cabinpkg.com/healthz   # 200
curl -sS -D - https://dev-registry.cabinpkg.com/config.json   # uniform 401 envelope,
# with WWW-Authenticate: Cabin login_url="https://cabinpkg.com/settings/tokens"
curl -sS -o /dev/null -w '%{http_code}\n' https://cabinpkg.com/api/v1/user   # 401 (session plane)
```

Propagation caveat: for up to ~a minute after `deploy`, requests can still
reach the previous Worker version. Right after a wipe that skew can even
surface as a `500` `internal error` (old version, deleted database). Retry
before diagnosing.

## Dev wipe procedure

Verified against the real dev database on 2026-07-09, generation bump
included.

1. Drop and recreate the dev database, then reapply migrations:

   ```sh
   npx wrangler d1 delete cabin-registry-dev -y
   npx wrangler d1 create cabin-registry-dev
   # update BOTH the dev database_id and vars.D1_DATABASE_ID in
   # wrangler.jsonc with the new id (the nightly dump exports whatever
   # database D1_DATABASE_ID names - a stale value backs up the wrong,
   # deleted database)
   npx wrangler d1 migrations apply DB --env dev --remote
   ```

2. Delete the archive blobs (keys are `blobs/sha256/<hex>`). Known keys can
   go one at a time:

   ```sh
   npx wrangler r2 object delete cabin-registry-dev-blobs/blobs/sha256/<hex> --remote
   ```

   (`r2 object` commands default to local `.wrangler/` state; `--remote`
   targets the deployed bucket.)

   For a full wipe use the Cloudflare dashboard (R2 ->
   `cabin-registry-dev-blobs` -> delete the `blobs/` folder): `wrangler r2
   object delete` removes exactly one object and has no prefix or bulk mode,
   so the dashboard - or any S3-compatible bulk tool - is the practical way
   to wipe the prefix.

3. Bump the registry generation so clients and smoke runs can tell the wipe
   happened (every authenticated response echoes it as
   `x-cabin-registry-generation`):

   ```sh
   npx wrangler d1 execute DB --env dev --remote --command \
     "UPDATE meta SET value = CAST(value AS INTEGER) + 1 WHERE key = 'registry_generation'"
   ```

   The `0001_init.sql` seed starts a fresh database at `'1'`, so the `+1`
   yields `2` - correct when the previous database was at `1`. In general,
   set the value to one more than the *previous* database's generation.

4. Redeploy (the new `database_id` must be baked into the Worker's
   bindings):

   ```sh
   npx wrangler deploy --env dev
   ```

Tokens live in the dropped database, so a wipe revokes everything; users
re-issue tokens through the website's token page and `cabin login` again.
A browser still holding a pre-wipe session cookie recovers by visiting
`/login`: GitHub auto-approves the already-authorized OAuth app, and
`/callback` recreates the user row (the session API answers 401 for a
session whose user row is gone until then).

## Production checklist

Production has deliberately **not** been provisioned. When the time comes,
in order - and starting from an **empty** database: dev data (packages,
blobs, users, tokens) is never migrated or promoted to production.

1. The GitHub OAuth app carries through from dev: its callback is
   already `https://cabinpkg.com/callback`, and which environment it
   signs users into follows the Worker holding the cabinpkg.com routes
   (see "Integrated topology and route management") - no new app and no
   callback change. Put the same client id in `wrangler.jsonc` under
   `env.production.vars.GITHUB_CLIENT_ID` (`WEB_ORIGIN` is already
   declared there); confirm `ALLOWED_GITHUB_IDS` lists exactly the
   intended operators.
2. Make sure the zone security exemption covers `registry.cabinpkg.com`
   (see "Zone security prerequisite").
3. Create the resources and deploy:

   ```sh
   npx wrangler d1 create cabin-registry-prod
   # copy the printed database_id into env.production.d1_databases AND
   # env.production.vars.D1_DATABASE_ID
   npx wrangler r2 bucket create cabin-registry-prod-blobs
   npx wrangler r2 bucket create cabin-registry-prod-backup
   npx wrangler d1 migrations apply DB --env production --remote
   npx wrangler deploy --env production
   printf '%s' "$GITHUB_CLIENT_SECRET" | npx wrangler secret put GITHUB_CLIENT_SECRET --env production
   openssl rand -base64 32 | npx wrangler secret put SESSION_SECRET --env production
   printf '%s' "$PROD_ANALYTICS_API_TOKEN" | npx wrangler secret put ANALYTICS_API_TOKEN --env production
   printf '%s' "$PROD_D1_EXPORT_API_TOKEN" | npx wrangler secret put D1_EXPORT_API_TOKEN --env production
   ```

   `GITHUB_CLIENT_SECRET` is the carried-through OAuth app's secret -
   the same value dev holds. The `SESSION_SECRET` must be freshly
   generated for production - never reuse the dev value. After the
   first nightly dump lands, run `scripts/restore-drill.sh production`
   once (see "Disaster recovery").
4. Move the cabinpkg.com route patterns from `env.dev` to
   `env.production` in `wrangler.jsonc` and deploy both envs. **The dev
   registry loses its browser plane at this moment** - no sign-in, no
   token page, no session API for dev. The two recovery options if dev
   still needs them: stand up a `dev.cabinpkg.com` website origin then
   (new zone routes plus a dev `WEB_ORIGIN`), or insert tokens manually
   with `wrangler d1 execute` (mirror the INSERT in `scripts/smoke.sh`,
   seeding `users` and a `tokens` row whose `token_hash` is the SHA-256
   hex of a locally generated `cabin_<base62>` value).
5. Re-run the hostname-role verification from `verification.md`
   ("Hostname-role split and integrated-system verification"), now
   against `registry.cabinpkg.com`: `/healthz` 200; every non-read
   path on the registry domain (including `/api/*`) answers the
   byte-identical uniform 401 with the
   `WWW-Authenticate: Cabin login_url="https://cabinpkg.com/settings/tokens"`
   challenge; the read plane does not exist on the website origin
   (`/config.json` and `/packages/*` are the website's 404); sign-in
   via `https://cabinpkg.com/login` works and the token page issues a
   token; an authenticated `config.json` read reports
   `"api": "https://cabinpkg.com"` and echoes
   `x-cabin-registry-generation: 1` (fresh seed).
6. From production's first real publish onward, the data policy above is
   binding: no wipes, no deletes, format changes need real migrations.

## Budget breaker and service mode

The scheduled handler (cron, every 15 minutes) evaluates usage against the
free-plan budgets and persists the result to `meta.service_mode`
(`normal` | `warn` | `writes_blocked`) with a human-readable
`meta.service_mode_reason` (`docs/architecture.md`, "Billing model and the
budget breaker"). Inspect it:

```sh
npx wrangler d1 execute DB --env dev --remote --command \
  "SELECT key, value FROM meta WHERE key IN
   ('service_mode', 'service_mode_reason', 'total_stored_bytes')"
```

Override it (for example to force-block writes during an incident, or to
unblock after freeing storage):

```sh
npx wrangler d1 execute DB --env dev --remote --command \
  "UPDATE meta SET value = 'writes_blocked' WHERE key = 'service_mode'"
```

Two caveats. First, the next cron pass **overwrites** a manual override
with its own evaluation (within 15 minutes when analytics are healthy), so
an override is a stopgap, not a switch; to keep writes blocked durably,
lower the matching `BUDGET_*` var and redeploy. Second, the request path
caches the mode in isolate memory for ~60 s (`SERVICE_MODE_TTL_SECS`; dev
pins it to 0), so an override can take up to a minute to bite in
production.

The budget ceilings the cron evaluates against, with their in-code
defaults (`src/breaker.rs`), each comfortably below the matching
Cloudflare free limit:

| Var | Default | Free limit |
| --- | --- | --- |
| `BUDGET_R2_STORAGE_BYTES` | 4 GiB | 10 GiB-month |
| `BUDGET_R2_CLASS_A_MONTH` | 800,000 | 1,000,000 / month |
| `BUDGET_WORKERS_REQ_DAY` | 80,000 | 100,000 / day |
| `BUDGET_D1_ROWS_READ_DAY` | 4,000,000 | 5,000,000 / day |

The storage budget counts primary (BLOBS) bytes only, but every blob is
stored a second time in the backup bucket and the nightly dumps add
metadata copies there (see "Disaster recovery"), so its default stays
under half the account-wide free limit.

Overrides are ordinary per-environment `vars` entries in `wrangler.jsonc`
and take effect on the next deploy.

The analytics-sourced metrics need the `ANALYTICS_API_TOKEN` secret: an
API token whose **only** permission is Account | Account Analytics | Read,
scoped to the single account (dash.cloudflare.com -> My Profile -> API
Tokens -> Create Token -> Custom token). It reads aggregate usage numbers
and nothing else. Set on dev 2026-07-09.

```sh
printf '%s' "$ANALYTICS_API_TOKEN" | npx wrangler secret put ANALYTICS_API_TOKEN --env dev
```

To rotate it, create the replacement token first, run the `secret put`
with the new value, and only then delete the old token in the dashboard -
revoking first would have the cron running degraded in between. Without a
working token the cron logs the skip, evaluates on the exact
self-accounted storage alone, and never de-escalates the persisted mode on
the missing data. Optionally set a `NOTIFY_WEBHOOK_URL` secret to receive
a JSON summary POST on every mode change.

## Verification pipeline

The external verifier is the `registry-verify` GitHub Actions workflow
(`.github/workflows/registry-verify.yml`): every 5 minutes (plus
`workflow_dispatch`) it builds `cabin-registry-verify` from the root
workspace, lists pending versions through the admin API, inspects each
archive, and PATCHes the verdict back. The checks and reason codes are
documented in `docs/remote-registry.md` ("The verifier's checks").

Fail-safe: a failed or skipped run leaves versions pending, which only
keeps content unexposed. GitHub cron schedules are **best-effort** and
can be delayed or dropped under load, so do not treat "the workflow ran
recently" as the health signal - the breaker cron's stuck-pending
webhook alert ("N version(s) have been pending verification for over an
hour") is the detection mechanism. On that alert, check the workflow's
recent runs first, then re-run by hand:

```sh
gh workflow run registry-verify.yml
gh run list --workflow registry-verify.yml --limit 5
```

Per-version operational failures (a download error, a verifier crash,
a `409` from the verdict PATCH because the version was republished
between listing and verdict) leave that version pending and move on to
the rest of the list; the run fails at the end so the failure is
visible, and the next run retries whatever is still pending. Rejected
verdicts do not fail the run - a rejection is the verifier working as
designed, visible in the run log as
`<name>@<version>: rejected (<reason codes>)`.

`REGISTRY_VERIFY_TOKEN` is a registry token created on the website's
token page with **only** the `verify` scope (no publish, no yank - the
verifier never needs them), stored as a GitHub repository secret:

```sh
gh secret set REGISTRY_VERIFY_TOKEN
```

Rotate it like `ANALYTICS_API_TOKEN`: create the replacement token
first, `gh secret set REGISTRY_VERIFY_TOKEN` with the new value, and
only then revoke the old token - revoking first would have the cron
failing (versions pending) in between. A dev wipe drops the
tokens table, so re-provisioning dev always includes re-issuing this
token and updating the secret.

The verifier's caps are GitHub **repository variables** (`gh variable
set <NAME>`), passed through to the binary; unset or empty means the
in-code default. The mechanism is public contract, the values are
tuning:

| Var | Default |
| --- | --- |
| `VERIFY_RATIO_CAP` | 10 |
| `VERIFY_ABS_CAP_BYTES` | 268435456 (256 MiB) |
| `VERIFY_MAX_ENTRIES` | 10000 |
| `VERIFY_MAX_PATH_LEN` | 256 |

`REGISTRY_VERIFY_ORIGIN` (also a repository variable) selects the
registry to verify - the **index** origin, defaulting to
`https://dev-registry.cabinpkg.com`; point it at production when
production exists. The workflow reads that index's `config.json` and
sends the admin listing and verdicts to the `api` origin it declares
(the website origin), while artifact downloads stay on the index
origin.

## Disaster recovery

Backups run entirely inside Cloudflare - R2/D1 bindings and one
D1-scoped API token, no second vendor holding credentials. Production
registry data is a permanent commitment, so this machinery exists and is
rehearsed against dev before production launch (see
[`verification.md`](verification.md)).

**What is backed up, and how.**

- **Archive blobs (RPO ~0).** After the primary R2 put succeeds, publish
  replicates the blob to the per-environment backup bucket
  (`cabin-registry-dev-backup` / `cabin-registry-prod-backup`, Worker
  binding `BACKUP`) under the same `blobs/sha256/<hex>` key, off the
  response path via `waitUntil`. Nothing in the service ever deletes
  from the backup bucket - the primary's reclaim paths do not propagate -
  so it is append-only, and a malicious or accidental deletion in the
  primary cannot reach it. Replication is best-effort: a failed copy is
  logged with its key in the `backup_replication_failures` table, the
  breaker cron alerts while any row exists, and
  `scripts/backup-backfill.sh <env>` re-copies everything missing and
  clears each verified key from the log (only handled keys, so a
  publish racing the backfill cannot have its fresh failure erased).
  Run the backfill once when enabling backups on an environment with
  existing data.
- **D1 metadata (RPO <= 24 h).** A second cron schedule (`0 3 * * *`)
  runs a nightly logical dump from the Worker itself: it drives the D1
  REST export endpoint (the same API `wrangler d1 export --remote`
  uses) with the `D1_EXPORT_API_TOKEN` secret, follows the returned
  signed URL, and streams the official `.sql` dump into the backup
  bucket at `d1/<YYYY-MM-DD>.sql` with a `.sha256` sidecar (hash
  computed while streaming). Success requires validation: non-empty,
  every expected `CREATE TABLE` present, and the re-read object matching
  the checksum; only then are `meta.last_backup_at` and
  `meta.last_backup_key` updated. Retention, pruned in the same job: the
  30 most recent daily dumps plus the first dump of each month for 12
  months. The cron handler routes on the expression - the breaker's
  `*/15 * * * *` exactly; anything else runs the dump job - so a
  temporary extra schedule in `wrangler.jsonc` is all it takes to force
  a dump for a rehearsal. One validated dump per date: a re-run on a
  date whose dump is already recorded skips instead of re-exporting, so
  a failed re-export can never overwrite the day's verified copy (a
  failed attempt never records itself, and is overwritten by the next
  try).
- **Freshness alerting.** A backup system's classic failure is stopping
  silently, so every breaker pass evaluates backup health: it logs an
  error and POSTs to `NOTIFY_WEBHOOK_URL` (when configured) while
  `meta.last_backup_at` is older than 36 h (or missing) or the
  replication failure log is non-empty; the webhook payload always
  carries a `backup` block with `last_backup_at`, `freshness`, and the
  failure count. Note the alert also fires between provisioning and the
  first nightly pass - that is the "no dump recorded yet" state working
  as intended.

**The `D1_EXPORT_API_TOKEN` secret** is a custom API token whose only
permission is Account | D1 | Edit, scoped to this single account: it can
export (and at worst rewrite) D1 databases, nothing else - no Workers,
R2, or zone access, so the Worker holding it cannot escalate. Rotate it
like `ANALYTICS_API_TOKEN`: create the replacement token first, `wrangler
secret put D1_EXPORT_API_TOKEN --env <env>` with the new value, then
delete the old token in the dashboard. While the token is broken or
absent the nightly job fails, which the freshness alert surfaces within
36 h.

**The three loss scenarios, and the recovery order.** Work down the
list; each later option covers a case the earlier one cannot.

1. **Bad deploy or migration** (data mangled in place, storage intact):
   use **D1 Time Travel** first. It is always on for production-version
   D1 databases with point-in-time restore at one-minute granularity -
   retention 7 days on the Workers free plan, 30 days on paid (verified
   against the Cloudflare docs 2026-07-10; re-check retention when
   planning an incident response). Restore is destructive and in-place:

   ```sh
   npx wrangler d1 time-travel info cabin-registry-dev
   npx wrangler d1 time-travel restore cabin-registry-dev --timestamp=<unix-ts>
   ```

   Blobs need nothing - R2 is untouched by a bad deploy, and archives
   are immutable and content-addressed.
2. **Accidental wipe of the wrong environment** (database deleted, or
   overwritten beyond Time Travel's window): create a fresh database,
   import the newest dump, re-point the config, redeploy - exactly what
   `scripts/restore-drill.sh` rehearses against a scratch database:
   download `meta.last_backup_key`... except after a real wipe that
   meta row is gone too; list the backup bucket's `d1/` prefix and take
   the newest date **whose `.sha256` sidecar exists and verifies**. The
   sidecar is written strictly after validation and the job deletes an
   invalid dump object again, so a failed export attempt cannot
   masquerade as a good dump. Then `wrangler d1 execute <db> --remote
   --file <dump>.sql`, update `database_id` + `D1_DATABASE_ID` in
   `wrangler.jsonc`, `wrangler deploy`. Loss bounded by the nightly
   cadence: at most 24 h of metadata. Blobs are still in both buckets.
3. **Primary-bucket data loss** (bucket deleted or objects destroyed):
   the backup bucket is the artifact store of last resort. Recreate the
   primary bucket, then copy `blobs/sha256/*` back (the inverse of
   `scripts/backup-backfill.sh` - same loop with source and destination
   swapped, driven by the checksums in D1), and restore D1 from Time
   Travel or the newest dump as above. Because blobs are
   content-addressed and never mutated, the copied-back objects are
   byte-identical to what clients pinned in lockfiles.

**RPO / recovery time.** Blobs: RPO ~0 (replicated at publish; the
failure log plus backfill close the gaps). Metadata: RPO <= 24 h from
the nightly dump, and effectively minutes when Time Travel applies.
Recovery time is dominated
by operator response, not data volume, at today's scale: a Time Travel
restore is minutes; a dump import plus redeploy is well under an hour
(the drill's import of the dev dump takes seconds); copying blobs back
is bounded by object count - budget roughly an hour per few thousand
blobs with the wrangler loop, less with an S3-compatible bulk tool.

**Rehearsal.** `scripts/restore-drill.sh <env>` downloads the latest
dump, verifies the sidecar checksum, imports it into a scratch D1
database (`cabin-registry-drill`), compares per-table row counts against
the live database, spot-checks one version's `metadata_json`
byte-for-byte, and deletes the scratch database. Run it after enabling
backups, after changing the dump machinery, and periodically before
production milestones; record runs in
[`verification.md`](verification.md). Row counts legitimately drift on
an active database (the dump is nightly) - investigate only differences
the timeline cannot explain.

**Known limitation - account-level compromise.** Everything above lives
in one Cloudflare account, and no in-account (or in-account-targeting)
pipeline can defend against losing the account itself: a compromised
operator account or a hostile account closure takes primary and backup
alike. The future hedge is an off-Cloudflare copy of the backup bucket -
e.g. an `rclone` sync to Backblaze B2's free tier or to local disk,
pulling with read-only credentials from outside - accepted as a
follow-up before production carries data that cannot be re-published.

## Backfilling migration 0002

Migration `0002_quotas.sql` adds `versions.archive_size`,
`versions.published_by`, and `packages.created_by` with `0` defaults, and
seeds `meta.total_stored_bytes` at `'0'`. Pre-existing rows therefore carry no
usage attribution until either the environment is wiped (dev policy above)
or the one-shot backfill runs:

```sh
scripts/backfill-0002.sh dev        # or production
```

It sizes each distinct archive blob via `wrangler r2 object get`, writes
`archive_size` per checksum, attributes every unattributed version and
package (`published_by`, `created_by`) to the sole existing user (it
refuses to guess when several users exist), and sets
`meta.total_stored_bytes` to the sum of the distinct blob sizes. Re-running
it is safe; quotas under-count until it has run.

## Orphaned R2 blobs

Publish writes the R2 blob before the D1 rows, so a crash between the two
writes can leave a blob no `versions` row references. That is harmless,
content-addressed garbage: it is unreachable through the API (artifact
lookups go through D1), a retried publish reuses it instead of re-uploading
(and counts it into `meta.total_stored_bytes` at that point), and there is
deliberately no garbage collection. Ignore such blobs, or delete them
manually from the dashboard if the storage ever bothers you.

Orphaned bytes are also invisible to the storage self-accounting - the
counter tracks referenced blobs only, by design ("never analytics"), and
the storage budget's headroom below the free limit absorbs the bounded
drift. If the dashboard's bucket size ever diverges noticeably from
`meta.total_stored_bytes`, delete the orphans and re-run
`scripts/backfill-0002.sh`, which recomputes the counter from the
referenced blobs.

## Logs

`wrangler tail --env dev` (or the dashboard). One line per request:
`req=<id> method=<m> path=<p> status=<s> token=<token-row-id|->`. Tokens and
token hashes are never logged.
