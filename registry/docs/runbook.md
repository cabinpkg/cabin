# Registry Service Runbook

All wrangler commands run with `registry/` as the working directory and an
explicit `--env`. Authentication: `CLOUDFLARE_API_TOKEN` in the environment
(scopes: Workers Scripts Edit, D1 Edit, R2 Edit, and DNS Edit on the
cabinpkg.com zone).

## Data policy

- **dev (`dev-registry.cabinpkg.com`): disposable.** When the storage format
  changes - D1 schema, `metadata_json` shape, R2 key layout - the dev
  environment is wiped and recreated. There is deliberately no migration code
  for dev-format changes; `migrations/` only ever describes the current
  schema from scratch.
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

## First-time provisioning (dev)

Verified end to end on 2026-07-09 (see
[`verification.md`](verification.md)). Prerequisite besides the API token: a
GitHub OAuth app for dev (homepage `https://dev-registry.cabinpkg.com`,
authorization callback `https://dev-registry.cabinpkg.com/callback`). Its
client id is public and lives in `wrangler.jsonc` (`env.dev.vars`,
`GITHUB_CLIENT_ID`), next to `ALLOWED_GITHUB_IDS` (the numeric GitHub user
ids allowed to sign in at `/me`); only the client secret and the session
secret are wrangler secrets.

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
   npx wrangler r2 object delete cabin-registry-dev-blobs/blobs/sha256/<hex>
   ```

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

## Orphaned R2 blobs

Publish writes the R2 blob before the D1 rows, so a crash between the two
writes can leave a blob no `versions` row references. That is harmless,
content-addressed garbage: it is unreachable through the API (artifact
lookups go through D1), a retried publish reuses it instead of re-uploading,
and there is deliberately no garbage collection. Ignore such blobs, or
delete them manually from the dashboard if the storage ever bothers you.

## Logs

`wrangler tail --env dev` (or the dashboard). One line per request:
`req=<id> method=<m> path=<p> status=<s> token=<token-row-id|->`. Tokens and
token hashes are never logged.
