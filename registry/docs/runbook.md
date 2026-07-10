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
  `(http.host eq "dev-registry.cabinpkg.com" and (starts_with(http.request.uri.path, "/api/") or http.request.uri.path eq "/login" or http.request.uri.path eq "/callback"))`
- Same characteristics: IP. Rate: 50 requests per 10 seconds. Action:
  Block, mitigation timeout 10 seconds.

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

## First-time provisioning (dev)

Verified end to end on 2026-07-09 (see
[`verification.md`](verification.md)). Prerequisite besides the API token: a
GitHub OAuth app for dev (homepage `https://dev-registry.cabinpkg.com`,
authorization callback `https://dev-registry.cabinpkg.com/callback`). Its
client id is public and lives in `wrangler.jsonc` (`env.dev.vars`,
`GITHUB_CLIENT_ID`), next to `ALLOWED_GITHUB_IDS` (the numeric GitHub user
ids allowed to sign in at `/me`); the wrangler secrets are the client
secret, the session secret, and `ANALYTICS_API_TOKEN` for the budget cron
(plus the optional `NOTIFY_WEBHOOK_URL`; see "Budget breaker and service
mode" for both).

```sh
npx wrangler d1 create cabin-registry-dev
# copy the printed database_id into env.dev.d1_databases in wrangler.jsonc
npx wrangler r2 bucket create cabin-registry-dev-blobs
npx wrangler d1 migrations apply DB --env dev --remote
npx wrangler deploy --env dev
# deploy creates the dev-registry.cabinpkg.com custom domain and its DNS
# record on the cabinpkg.com zone; deploy first so the secret puts below
# attach to a deployed Worker instead of prompting to create a draft.
printf '%s' "$GITHUB_CLIENT_SECRET" | npx wrangler secret put GITHUB_CLIENT_SECRET --env dev
openssl rand -base64 32 | npx wrangler secret put SESSION_SECRET --env dev
printf '%s' "$ANALYTICS_API_TOKEN" | npx wrangler secret put ANALYTICS_API_TOKEN --env dev
```

Idempotence: `d1 create` / `r2 bucket create` fail cleanly if the resource
exists (`d1 list` / `r2 bucket list` to check); `migrations apply` and
`deploy` are safe to re-run; a re-run `secret put` overwrites the value.

Smoke checks after any deploy:

```sh
curl -sS -o /dev/null -w '%{http_code}\n' https://dev-registry.cabinpkg.com/healthz   # 200
curl -sS https://dev-registry.cabinpkg.com/config.json   # uniform 401 envelope
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
   # update the dev database_id in wrangler.jsonc with the new id
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
re-issue tokens at `/me` and `cabin login` again. A browser still holding a
pre-wipe session cookie recovers transparently: `/me` redirects through
`/login`, GitHub auto-approves the already-authorized OAuth app, and
`/callback` recreates the user row.

## Production checklist

Production has deliberately **not** been provisioned. When the time comes,
in order - and starting from an **empty** database: dev data (packages,
blobs, users, tokens) is never migrated or promoted to production.

1. Create a **separate** GitHub OAuth app for production: homepage
   `https://registry.cabinpkg.com`, authorization callback
   `https://registry.cabinpkg.com/callback`. Put its client id in
   `wrangler.jsonc` under `env.production.vars.GITHUB_CLIENT_ID`; confirm
   `ALLOWED_GITHUB_IDS` there lists exactly the intended operators.
2. Make sure the zone security exemption covers `registry.cabinpkg.com`
   (see "Zone security prerequisite").
3. Create the resources and deploy:

   ```sh
   npx wrangler d1 create cabin-registry-prod
   # copy the printed database_id into env.production.d1_databases
   npx wrangler r2 bucket create cabin-registry-prod-blobs
   npx wrangler d1 migrations apply DB --env production --remote
   npx wrangler deploy --env production
   printf '%s' "$PROD_GITHUB_CLIENT_SECRET" | npx wrangler secret put GITHUB_CLIENT_SECRET --env production
   openssl rand -base64 32 | npx wrangler secret put SESSION_SECRET --env production
   ```

   The `SESSION_SECRET` must be freshly generated for production - never
   reuse the dev value.
4. Verify: `/healthz` 200; the three data routes answer the uniform 401
   without a token; sign-in at `https://registry.cabinpkg.com/me` works and
   issues a token; an authenticated `config.json` read echoes
   `x-cabin-registry-generation: 1` (fresh seed).
5. From production's first real publish onward, the data policy above is
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
| `BUDGET_R2_STORAGE_BYTES` | 9 GiB | 10 GiB-month |
| `BUDGET_R2_CLASS_A_MONTH` | 800,000 | 1,000,000 / month |
| `BUDGET_WORKERS_REQ_DAY` | 80,000 | 100,000 / day |
| `BUDGET_D1_ROWS_READ_DAY` | 4,000,000 | 5,000,000 / day |

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
