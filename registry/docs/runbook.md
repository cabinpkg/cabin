# Registry Service Runbook

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

## Dev wipe procedure

1. Drop and recreate the dev database, then reapply migrations:

   ```sh
   npx wrangler d1 delete cabin-registry-dev
   npx wrangler d1 create cabin-registry-dev
   # update the dev database_id in wrangler.jsonc with the new id
   npx wrangler d1 migrations apply DB --env dev --remote
   ```

2. Delete the archive blobs: in the Cloudflare dashboard, open R2 ->
   `cabin-registry-dev-blobs` and delete the `blobs/` folder. (`wrangler r2
   object delete` removes exactly one object and has no prefix or bulk mode,
   so the dashboard - or any S3-compatible bulk tool - is the practical way
   to wipe the prefix.)

3. Bump the registry generation so clients and smoke runs can tell the wipe
   happened (every authenticated response echoes it as
   `x-cabin-registry-generation`):

   ```sh
   npx wrangler d1 execute DB --env dev --remote --command \
     "UPDATE meta SET value = CAST(value AS INTEGER) + 1 WHERE key = 'registry_generation'"
   ```

   The `0001_init.sql` seed starts a fresh database at `'1'`; after a wipe,
   set it to one more than the previous database's value.

4. Redeploy:

   ```sh
   npx wrangler deploy --env dev
   ```

Tokens live in the dropped database, so a wipe revokes everything; users
re-issue tokens on the (future) web UI and `cabin login` again.

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
