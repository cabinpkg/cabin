# Registry Service Runbook

All wrangler commands run with `registry/` as the working directory against
the single top-level configuration in `wrangler.jsonc` (no wrangler
environments, no `--env`). Authentication: `CLOUDFLARE_API_TOKEN` in the
environment (scopes: Workers Scripts Edit, D1 Edit, R2 Edit, and DNS Edit
on the cabinpkg.com zone).

## Data policy

The disposable/permanent boundary is **temporal** (pre-launch vs
post-launch), not spatial - there is one registry, running under its final
names from day one, and launch is a data/policy event rather than an
infrastructure cutover.

- **Pre-launch (`meta.launched` = `'false'`): disposable.** When the
  storage format changes incompatibly - `metadata_json` shape, R2 key
  layout, reshaped D1 columns - the registry is wiped and recreated
  (`scripts/wipe.sh`) rather than migrated. The schema ships as a
  single from-zero baseline (`migrations/0001_init.sql`) edited in
  place: while the data is disposable, no ALTER TABLE history
  accretes.
- **Post-launch (`meta.launched` = `'true'`): permanent.** Published
  archives and index state are never wiped, mutated, or deleted; format
  changes need real migrations. The flag is flipped exactly once, by
  hand, as a launch-checklist item (see "Launch checklist"), and every
  destructive maintenance path checks it first and refuses while it is
  `'true'` (the launch guard, `scripts/launch-guard.sh`).

## Zone security prerequisite

The registry hosts serve machine clients (cabin, curl, CI), so they must not
sit behind a Cloudflare visitor challenge. Zone-wide Bot Fight Mode on
cabinpkg.com answered every registry request with `403` /
`cf-mitigated: challenge` until the operator disabled it (2026-07-09,
dashboard: Security -> Bots). If zone-wide bot protection is ever wanted
again, it needs a plan that exempts the registry hosts first - e.g. a WAF
custom rule skipping the challenge products for
`http.host eq "registry.cabinpkg.com"` and the `/api/*` paths on
`cabinpkg.com` (note free-plan Bot Fight Mode ignores skip rules; it can
only be toggled zone-wide). Managing zone security needs API-token scopes
beyond the provisioning set (Zone WAF Edit, Zone Settings Edit) or the
dashboard.

## Zone rate limiting (WAF)

Zone-level defense for the Workers request budget: one rate limiting rule
on the cabinpkg.com zone, created 2026-07-09 via the dashboard (Security ->
WAF -> Rate limiting rules; the provisioning API token deliberately has no
WAF scopes). The Free plan allows exactly one rate limiting rule, with the
counting period and mitigation timeout fixed at 10 seconds and IP keying
only, so the conservative 300 requests/minute target is expressed as 50
requests per 10 seconds:

- Name: `registry-api-rate-limit`
- Expression:
  `(http.host eq "cabinpkg.com" and (starts_with(http.request.uri.path, "/api/") or http.request.uri.path eq "/login" or starts_with(http.request.uri.path, "/callback") or starts_with(http.request.uri.path, "/claim/")))`
- Same characteristics: IP. Rate: 50 requests per 10 seconds. Action:
  Block, mitigation timeout 10 seconds.
- The `/claim/` arm was added with the scope-claim flow; it is a
  dashboard-managed rule, so apply the updated expression by hand when
  deploying that step (`/callback/claim` was already covered by the
  `/callback` arm).

The rule keys on `cabinpkg.com` because the hostname-role split put the
whole write/auth surface on the website origin; it deliberately guards
only that surface, not `/healthz` or the read routes on
`registry.cabinpkg.com`. Covering reads with the same 50-per-10 s ceiling
would throttle legitimate `cabin` traffic - resolving and fetching a
dependency tree fans out many read requests from one IP in seconds -
while abuse of the omitted routes can at worst exhaust free-plan quotas
that fail closed without billing (Workers requests, D1 reads; artifact
downloads are R2 Class B). The one rule the Free plan grants goes where
the paid exposure (R2 Class A writes) and the heavy CPU live. The zone
security exemption above must likewise keep the machine `/api/*` traffic
on `cabinpkg.com` out of any visitor challenge.

Verified 2026-07-09 with a 70-request burst against an `/api/` path:
exactly 50 requests reached the Worker, the rest answered a Cloudflare
`429` with `retry-after: 10` (see `verification.md`). A WAF `429` carries
no error envelope; cabin's rate-limit mapping degrades to the same "try
again" hint off the header alone.

## Integrated topology and route management

Two hostnames, one zone:

- **`cabinpkg.com`** - the website Worker (`cabin-website`, deployed by
  Workers Builds from `website/` on every push to `main`) serves the
  marketing site, docs, and the account pages; the registry Worker
  (`cabin-registry`) takes exactly `/api/*`, `/login`, `/callback*`,
  and `/claim/*` via the zone routes below. This one origin is the
  registry's browser plane.
- **`registry.cabinpkg.com`** - the registry's machine read plane
  (custom domain of `cabin-registry`), nothing else.

Deploy skew: `cabin-website` deploys automatically on every push to
`main` (Workers Builds); `cabin-registry` deploys from CI (the
`deploy-registry` job in `.github/workflows/registry.yml`) on pushes
to `main` matching that workflow's paths filter, after its build and
conformance jobs pass. A merge that changes the session-plane JSON
contract therefore briefly has the account pages ahead of the live
registry Worker while the gate runs - accepted pre-launch (private
alpha), with no legacy-field fallbacks in the frontend. A red gate
leaves the previous Worker serving. A red deploy job may or may not
have activated the new version (`wrangler deploy` can fail after
activation), so check `npx --yes wrangler@4.112.0 deployments list` and run
the smoke checks below; `npx --yes wrangler@4.112.0 deploy` from this directory stays
the manual fallback. The CI deploy token is scoped to Workers
Scripts:Edit + Routes:Edit only, so D1 migrations and R2 provisioning
remain manual steps in this runbook. For exactly that reason the
auto-deploy stays skipped while `migrations/` content disagrees with
the `migrations-applied` stamp: after applying changed migrations to
the live database (or completing a wipe), refresh the stamp from this
directory and land it like any other change:
`cat migrations/*.sql | shasum -a 256 | cut -d' ' -f1 | tee migrations-applied`.

The Worker reaches the website origin through **zone routes** on
cabinpkg.com (`wrangler.jsonc`): `cabinpkg.com/api/*`,
`cabinpkg.com/login`, `cabinpkg.com/callback*` (which also covers the
claim flow's `/callback/claim`), and `cabinpkg.com/claim/*`. Route
facts that matter operationally:

- Path routes are more specific than the website Worker's own domain,
  so they take precedence on exactly these paths and nothing else
  (Cloudflare picks the most specific matching route). Verify after any
  route change: `/api/*` answers the registry Worker (uniform 401
  envelope), while `/` and `/login/denied` still render the website.
- A pattern without a trailing `*` never matches a URL carrying a query
  string - GitHub redirects to `/callback?code=...&state=...`, hence
  `cabinpkg.com/callback*`. `/login` deliberately stays exact so the
  website keeps serving `/login/denied`.
- A route pattern can point at only **one** Worker; all four patterns
  belong to `cabin-registry`.
- The website's `/dashboard`, `/settings/*`, and `/login/denied` pages
  are live on the origin ("Account pages" in `website/README.md`). For
  ops debugging, the session API also works directly: `curl -H "Cookie:
  cabin_session=..." -H "Content-Type: application/json" -H
  "X-CSRF-Protection: 1"` against `https://cabinpkg.com/api/v1/user/...`.

## First-time provisioning

Prerequisite besides the API token: the GitHub OAuth app (homepage
`https://cabinpkg.com`, authorization callback
`https://cabinpkg.com/callback` - the browser plane lives on the website
origin, see "Integrated topology and route management"; the claim
flow's `/callback/claim` needs no OAuth-app change, because GitHub
accepts a `redirect_uri` under the registered callback's path). Its client id is
public and lives in `wrangler.jsonc` (`vars.GITHUB_CLIENT_ID`), next to
`ALLOWED_GITHUB_IDS` (the numeric GitHub user ids allowed to sign in);
the wrangler secrets are the client secret, the session secret,
`ANALYTICS_API_TOKEN` for the budget cron, and `D1_EXPORT_API_TOKEN` for
the nightly dump (plus the optional `NOTIFY_WEBHOOK_URL`; see "Budget
breaker and service mode" and "Disaster recovery").

```sh
npx --yes wrangler@4.112.0 d1 create cabin-registry
# copy the printed database_id into d1_databases AND vars.D1_DATABASE_ID
# in wrangler.jsonc
npx --yes wrangler@4.112.0 r2 bucket create cabin-registry-blobs
npx --yes wrangler@4.112.0 r2 bucket create cabin-registry-backup
npx --yes wrangler@4.112.0 d1 migrations apply DB --remote
npx --yes wrangler@4.112.0 deploy
# deploy creates the registry.cabinpkg.com custom domain and its DNS
# record on the cabinpkg.com zone; deploy first so the secret puts below
# attach to a deployed Worker instead of prompting to create a draft.
printf '%s' "$GITHUB_CLIENT_SECRET" | npx --yes wrangler@4.112.0 secret put GITHUB_CLIENT_SECRET
openssl rand -base64 32 | npx --yes wrangler@4.112.0 secret put SESSION_SECRET
printf '%s' "$ANALYTICS_API_TOKEN" | npx --yes wrangler@4.112.0 secret put ANALYTICS_API_TOKEN
printf '%s' "$D1_EXPORT_API_TOKEN" | npx --yes wrangler@4.112.0 secret put D1_EXPORT_API_TOKEN
```

Idempotence: `d1 create` / `r2 bucket create` fail cleanly if the resource
exists (`d1 list` / `r2 bucket list` to check); `migrations apply` and
`deploy` are safe to re-run; a re-run `secret put` overwrites the value.

Smoke checks after any deploy:

```sh
curl -sS -o /dev/null -w '%{http_code}\n' https://registry.cabinpkg.com/healthz   # 200
curl -sS -D - https://registry.cabinpkg.com/config.json   # uniform 401 envelope,
# with WWW-Authenticate: Cabin login_url="https://cabinpkg.com/settings/tokens"
curl -sS -o /dev/null -w '%{http_code}\n' https://cabinpkg.com/api/v1/user   # 401 (session plane)
```

`/healthz` is a liveness check: it stays `200` in every service mode,
including the blocked ones, so availability monitoring that must notice a
tripped breaker has to probe an authenticated data-plane route instead
and treat a `503` carrying `registry_over_budget` as the signal.

Propagation caveat: for up to ~a minute after `deploy`, requests can still
reach the previous Worker version. Right after a wipe that skew can even
surface as a `500` `internal error` (old version, deleted database). Retry
before diagnosing.

## Wipe procedure (pre-launch only)

`scripts/wipe.sh` scripts the whole procedure and is the guarded
destructive path: it refuses to run unless the live `meta.launched` row
is exactly `'false'` (missing row or unreadable flag also refuse -
fail-safe; `scripts/launch-guard.sh`, host-target-tested in
`tests/launch_guard.rs` and exercised end to end by the smoke test).
What it does, in order:

1. Asks for interactive confirmation (`CABIN_WIPE_YES=1` skips it), then
   runs the guard immediately before anything destructive and reads the
   current `meta.registry_generation` (the input for step 5). The guard
   first proves the config's `DB` binding and the account's database
   named `cabin-registry` are the same database (a stale binding could
   otherwise have the flag read one database while `d1 delete` removes
   another), then reads `meta.launched` through the binding.
2. Deletes and recreates the database
   (`wrangler d1 delete cabin-registry -y` / `wrangler d1 create
   cabin-registry`) and bakes the new id into BOTH
   `d1_databases[0].database_id` and `vars.D1_DATABASE_ID` in
   `wrangler.jsonc`, verifying the file now carries it exactly twice
   (the nightly dump exports whatever database `D1_DATABASE_ID` names -
   a stale value backs up the wrong, deleted database). Commit that
   change.
3. Applies all migrations from zero:
   `wrangler d1 migrations apply DB --remote`.
4. Deletes every `blobs/`-prefixed object from `cabin-registry-blobs`
   through the R2 REST API (list by prefix, delete per key -
   `wrangler r2 object delete` removes exactly one object and has no
   prefix or bulk mode, which is why the script drives the API
   directly). The BACKUP bucket is never wiped.
5. Bumps `meta.registry_generation` to one more than the pre-wipe value
   read in step 1 (every authenticated response echoes it as
   `x-cabin-registry-generation`, so clients and smoke runs can tell the
   wipe happened).
6. Redeploys (`wrangler deploy`) so the new `database_id` is baked into
   the Worker's bindings.
7. Resets the governor ledger - a manual follow-up the script reminds
   you of: the ledger still accounts for the deleted primary blobs
   (conservative, so nothing overspends, but admission runs against
   ghosts). Sign in, mint a verify-scoped token, and
   `POST {"wipe":true}` to `/api/v1/admin/governor` with it; the
   endpoint refuses unless `meta.launched` reads exactly `'false'`,
   mirroring this script's guard. The wipe clears the primary storage
   rows and the daily fairness windows only - the `backup` and `dump`
   entries survive (the BACKUP bucket is never wiped and its objects
   keep billing), and the monthly operation windows survive too: they
   mirror R2 operations Cloudflare already metered this month and
   cannot be rebuilt, so zeroing them would re-mint a month of
   allowance for spend that already happened ("The cost governor").

`scripts/wipe.sh --local` is the same idea for the local `.wrangler/`
state (guard, drop the emulated D1/R2/Durable Object/cache state - the
emulated backup bucket included, since local state is test data rather
than a backup - reapply migrations, bump the generation); the smoke
test uses it to assert the refusal branch, and a re-run of the smoke
test wants it first so the governor ledger and the emulated edge cache
start empty.

If a remote wipe is interrupted between the delete and the end, the
guard cannot read the half-provisioned database and refuses the re-run;
finish by hand with the remaining steps ("First-time provisioning" has
the same commands).

Tokens live in the dropped database, so a wipe revokes everything; users
re-issue tokens through the website's token page and `cabin login` again.
A browser still holding a pre-wipe session cookie recovers by visiting
`/login`: GitHub auto-approves the already-authorized OAuth app, and
`/callback` recreates the user row (the session API answers 401 for a
session whose user row is gone until then). Re-provisioning also always
includes re-issuing the verifier's token (see "Verification pipeline").

## Launch checklist

Launch contains **no infrastructure work** - the Worker, domain, database,
buckets, crons, secrets, WAF rule, and verifier are already the production
ones. Launch is a data and policy event, in order:

1. Final wipe: `scripts/wipe.sh` (the guard still passes -
   `meta.launched` is `'false'`), so the registry starts empty of
   pre-launch test data.
2. Flip the launch flag - once, by hand:

   ```sh
   npx --yes wrangler@4.112.0 d1 execute DB --remote --command \
     "UPDATE meta SET value = 'true' WHERE key = 'launched'"
   ```

   From this moment the data policy is binding (no wipes, no deletes,
   real migrations only) and `scripts/wipe.sh` refuses to run. The flag
   lives in `meta`, so a disaster-recovery restore of a pre-launch dump
   would reset it - re-running this `UPDATE` is part of any such
   restore (see "Disaster recovery").
3. Remove the private-alpha labels from the website (the `private α`
   badges on the sign-in affordance and the account-page shell, and the
   private-alpha copy on `/login/denied` - see `website/`).
4. Decide and apply the access policy: expand `ALLOWED_GITHUB_IDS` or
   open sign-up, and keep `auth-required` reads or enable whatever
   public-read work package applies by then.
5. Re-issue any long-lived operational tokens (`REGISTRY_VERIFY_TOKEN`)
   against the post-wipe database and re-run the verification workflow
   once (see "Verification pipeline").

**Post-launch staging is intentionally not maintained.** There is no
standing second environment: with a single maintainer there is nothing to
coexist with, and a permanently-running staging copy would immediately go
stale. Risky changes (migrations, storage-format work, breaker changes)
are rehearsed against a temporary scratch deployment recreated from this
directory's `wrangler.jsonc` and `migrations/` - deploy under scratch
names (worker, database, buckets), run the rehearsal, tear it down. The
restore drill's scratch database (`scripts/restore-drill.sh`) is the
existing example of the pattern.

## Budget breaker and service mode

The scheduled handler (cron, every 15 minutes) evaluates usage against the
budgets and persists the result to `meta.service_mode`
(`normal` | `warn` | `writes_blocked` | `reads_blocked`) with a
human-readable `meta.service_mode_reason` (`docs/architecture.md`,
"Billing model: the governor and the breaker"; `reads_blocked` is unreachable
until a read budget is configured - see "Read budgets and paid-plan
activation" below). Inspect it:

```sh
npx --yes wrangler@4.112.0 d1 execute DB --remote --command \
  "SELECT key, value FROM meta WHERE key IN
   ('service_mode', 'service_mode_reason', 'total_stored_bytes')"
```

Override it (for example to force-block writes during an incident, or to
unblock after freeing storage):

```sh
npx --yes wrangler@4.112.0 d1 execute DB --remote --command \
  "UPDATE meta SET value = 'writes_blocked' WHERE key = 'service_mode'"
```

Two caveats. First, the next cron pass **overwrites** a manual override
with its own evaluation (within 15 minutes when analytics are healthy), so
an override is a stopgap, not a switch; to keep writes blocked durably,
lower the matching `BUDGET_*` var and redeploy. Second, the request path
caches the mode in isolate memory for ~60 s (`SERVICE_MODE_TTL_SECS`; the
smoke test pins it to 0 via `.dev.vars`), so an override can take up to a
minute to bite.

The breaker's numbers are approximate and cron-delayed; the hard,
request-time R2 enforcement lives in the governor ("The cost governor"
below), which the analytics here only audit - they can escalate the
mode, never grant governor allowance. The budget ceilings the cron
evaluates against, with their in-code
defaults (`src/breaker.rs`), each comfortably below the matching
Cloudflare free limit:

| Var | Default | Free limit |
| --- | --- | --- |
| `BUDGET_R2_STORAGE_BYTES` | 4 GiB | 10 GiB-month |
| `BUDGET_R2_CLASS_A_MONTH` | 800,000 | 1,000,000 / month |
| `BUDGET_WORKERS_REQ_DAY` | 80,000 | 100,000 / day |
| `BUDGET_D1_ROWS_READ_DAY` | 4,000,000 | 5,000,000 / day |
| `BUDGET_R2_CLASS_B_MONTH` | 8,000,000 (warn-only while unset) | 10,000,000 / month |

`BUDGET_R2_CLASS_B_MONTH` is deliberately different from the others:
while the var is **unset**, R2 Class B (read) operations are monitored
against the built-in default and can raise `warn` but never a block -
a write block cannot fix read-driven spend. **Setting** the var is the
act that arms the read-side breaker: the configured value becomes the
budget, and exhausting it moves the mode to `reads_blocked`, where
authenticated data-plane reads answer `503` (the session plane, the
public stats, the admin plane, and the verifier's config/artifact
fetches keep working - `docs/architecture.md`). Do not set it before
the activation procedure below.

The storage budget counts primary (BLOBS) bytes only, but every blob is
stored a second time in the backup bucket and the nightly dumps add
metadata copies there (see "Disaster recovery"), so its default stays
under half the account-wide free limit.

Overrides are ordinary `vars` entries in `wrangler.jsonc` and take effect
on the next deploy.

The analytics-sourced metrics need the `ANALYTICS_API_TOKEN` secret: an
API token whose **only** permission is Account | Account Analytics | Read,
scoped to the single account (dash.cloudflare.com -> My Profile -> API
Tokens -> Create Token -> Custom token). It reads aggregate usage numbers
and nothing else.

```sh
printf '%s' "$ANALYTICS_API_TOKEN" | npx --yes wrangler@4.112.0 secret put ANALYTICS_API_TOKEN
```

To rotate it, create the replacement token first, run the `secret put`
with the new value, and only then delete the old token in the dashboard -
revoking first would have the cron running degraded in between. Without a
working token the cron logs the skip, evaluates on the exact
self-accounted storage alone, and never de-escalates the persisted mode on
the missing data. Optionally set a `NOTIFY_WEBHOOK_URL` secret to receive
a JSON summary POST on every mode change.

## The cost governor

The governor is the hard, request-time authority for every billable R2
resource the Worker can initiate (`docs/architecture.md`, "The cost
governor"): one named SQLite-backed Durable Object (binding `GOVERNOR`,
class `Governor`, singleton `"governor"`) whose ledger admits or
refuses each R2 call before it is made. The breaker above stays the
approximate, analytics-driven service mode; the governor is exact and
per-request, and an unreachable governor fails every fresh R2 path
closed (`503 registry_over_budget`; edge-cache hits keep serving).

The hard limits, env-overridable in `wrangler.jsonc` `vars` with
in-code defaults (`src/governor.rs`):

| Var | Default | Pool |
| --- | --- | --- |
| `GOVERNOR_STORAGE_PRIMARY_BYTES` | 4 GiB | BLOBS archive bytes |
| `GOVERNOR_STORAGE_BACKUP_BYTES` | 4 GiB | BACKUP verified copies |
| `GOVERNOR_STORAGE_DUMP_BYTES` | 512 MiB | BACKUP `d1/` dumps + sidecars |
| `GOVERNOR_R2_CLASS_A_PUBLISH_MONTH` | 200,000 | publish-path puts |
| `GOVERNOR_R2_CLASS_A_INFRA_MONTH` | 200,000 | replication/dump puts, lists |
| `GOVERNOR_R2_CLASS_B_ORDINARY_MONTH` | 4,000,000 | artifact cache misses |
| `GOVERNOR_R2_CLASS_B_SOURCE_MONTH` | 500,000 | source-viewer ranged reads |
| `GOVERNOR_R2_CLASS_B_VERIFIER_MONTH` | 500,000 | pending-artifact fetches |
| `GOVERNOR_R2_CLASS_B_PUBLISH_MONTH` | 200,000 | publish-path heads |
| `GOVERNOR_R2_CLASS_B_INFRA_MONTH` | 200,000 | replication/dump reads |

Two knob rules that differ from the breaker's, on purpose:

- **A set-but-unparsable limit fails closed to zero** and blocks its
  pool loudly (the breaker's budgets fall back to defaults instead).
  These are hard spending caps; a typo must never silently widen one.
- **Operation windows are explicit UTC calendar months**, not
  Cloudflare's billing window, which cannot be inferred reliably; the
  defaults carry the headroom for that skew. Windows only roll
  forward - a regressed clock cannot mint allowance.

Storage sizing note: primary + backup + dump defaults sum to 8.5 GiB
against the 10 GB-month account-wide free limit, and the breaker's
`BUDGET_R2_STORAGE_BYTES` keeps watching the primary bytes
independently.

**Inspecting the ledger.** Every breaker cron pass logs one summary
line (`governor usage: storage ... ops ...`) next to the analytics
evaluation - `wrangler tail` or the dashboard shows both - and the
reconciliation report right after it: objects added conservatively,
plus `governor ledger divergence: N unreferenced entr(ies), M byte
mismatch(es)` when the ledger carries entries D1 does not prove live.
On demand, `GET /api/v1/admin/governor` (Bearer, `verify` scope)
returns the same snapshot as JSON:

```sh
curl -sS -H "Authorization: Bearer $REGISTRY_VERIFY_TOKEN" \
  https://cabinpkg.com/api/v1/admin/governor
```

**Reconciliation and operator releases.** Reconciliation is
increase-only: it records every live blob as committed usage and only
*reports* the rest. An `unreferenced` entry is a candidate orphan (a
crashed publish's blob, a lost race, or a blob you deleted by hand);
it keeps consuming allowance until an operator releases it with
evidence. To release one: confirm the object really is gone (or
delete it - dashboard or `wrangler r2 object delete`), then release
the ledger entry:

```sh
curl -sS -X POST \
  -H "Authorization: Bearer $REGISTRY_VERIFY_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"release":{"pool":"primary","key":"blobs/sha256/<hex>"}}' \
  https://cabinpkg.com/api/v1/admin/governor
```

Never release an entry for an object that still exists: the ledger
would understate reality, which is the one direction the design
forbids. The endpoint's other action, `{"wipe":true}`, clears the
primary storage rows and the daily fairness windows (backup, dump, and
the monthly operation windows all survive - the operation counters
mirror already-metered R2 spend and cannot be rebuilt) and only works
pre-launch (see "Wipe procedure").

**Analytics divergence.** The breaker cron's R2 numbers audit the
governor: if observed Class A/B operations or storage run well ahead
of the governor's consumed totals, something is spending outside the
governed paths (a leaked credential, dashboard activity, a bug).
Investigate; the emergency lever is the service mode override above,
which only ever makes the service more closed.

**The governor's own platform limits.** SQLite-backed Durable Objects
are the one kind the free plan offers (hence `new_sqlite_classes` in
`wrangler.jsonc`), with their own free limits (100k requests/day, 100k
rows written/day, 5 GB storage). Both daily quotas sit *below* the R2
budgets the object enforces, and whichever is reached first fails the
governed paths closed. The rows-written quota usually binds sooner: an
admitted ordinary read writes both its fairness row and its op-window
row, so ~50k charged reads exhaust the 100k rows/day before the
request count matters, and the `b_ordinary` budget alone
(4,000,000/month ≈ 133k/day) exceeds both quotas. Cache hits never
touch the object, so steady-state traffic stays far below either
wall; when one is hit the platform refuses the object's requests and
every governed path fails closed (an availability backstop, never
overspend). Moving to Workers Paid lifts these ceilings and is the
prerequisite for opening the registry beyond the allowlist; until
then, do not raise `GOVERNOR_R2_CLASS_B_ORDINARY_MONTH` toward its
cap without accepting that the object's daily quotas become the
guaranteed failure point.

**Reclaim and the ledger.** Rejected-content reclaim deletes the
primary object (deletes are free) but never releases its ledger entry:
the entry shows up as `unreferenced` in reconciliation reports, and
releasing it is the operator's action above. Expect the primary ledger
to accumulate a small tail of rejected/orphaned entries between
sweeps; each entry only ever holds allowance, never spends.

**Dump-retry conflicts.** A same-date dump retry whose export size
changed is refused (`KeyConflict`) instead of admitted past the cap;
the job fails, the freshness alert fires, and the next day's fresh key
(or an operator release of the stale `d1/<date>.sql` entry after
confirming the object) recovers.

**Known ceilings.** Two residual windows are documented rather than
engineered around: a
spurious FORWARD platform-clock jump across a month boundary would
roll an operation window early (a later regress cannot roll it back -
windows only move forward - so the net effect is one early reset,
with the analytics operation counts as the independent tell), and
a catastrophic loss of the Durable Object's storage would restart the
operation windows at zero. Storage rows are reconstructable but the
**operation windows are not** (R2 exposes no per-pool op counter), so
after a storage loss on a *launched* registry the month's operation
budget re-opens on top of ops already metered - Cloudflare's 30-day
Durable Object point-in-time recovery is the first-line fix, restoring
the object to just before the loss. Reconciliation only repopulates
**storage**, and only on the breaker cron (≤15 min after the object
comes back): the **primary** rows rebuild automatically from D1;
`scripts/backup-backfill.sh` re-ledgers the **backup** pool (its
queue rows walk every verified blob through the drain); and the
**dump** entries regrow only as nightly dumps land - audit the
`d1/` prefix against the usage snapshot by hand if the gap matters
before retention cycles it out. On a launched registry, hold publishes
(or run a reconcile by hand) until the first pass lands, so the empty
ledger cannot admit a write past the true R2 cap in the interim.

**Deploy notes.** The Durable Object class ships with the same
`wrangler deploy` as the Worker (the `migrations` block in
`wrangler.jsonc` tags `Governor` as a SQLite-backed class); the CI
deploy token's Workers Scripts:Edit scope covers Durable Object
migrations, but watch the first deploy that introduces the class and
fall back to a manual `npx --yes wrangler@4.112.0 deploy` if the API
refuses. A wiped or reinitialized object recreates its schema
idempotently; reconciliation then rebuilds the **primary** rows
conservatively from D1, the backfill script re-ledgers the **backup**
pool, and dump entries regrow with the nightly job ("Known ceilings"
above). The schema is applied with `CREATE TABLE IF NOT EXISTS` only,
so any future column change to a governor table needs an explicit
migration (an `ALTER TABLE`, not a silent no-op), and the `Governor`
class must only ever be migrated by `renamed_classes`/
`transferred_classes` - never dropped and recreated, and never a
`deleted_classes` migration on a launched registry - because a class
delete destroys the SQLite storage and the monthly operation windows
cannot be rebuilt.

## Read budgets and paid-plan activation

The read-side breaker (`reads_blocked`; `docs/architecture.md`, "Billing
model and the budget breaker") ships as dormant infrastructure: fully
implemented and tested, unreachable until `BUDGET_R2_CLASS_B_MONTH` is
set. Arming it is a policy decision tied to leaving the free plan, in
this order:

1. **Plan acceptance.** The registry is accepted onto a sponsored/paid
   Cloudflare plan (Project Alexandria) or funded paid usage.
2. **Confirm the actually granted limits.** Read them off the account,
   do not assume the application's numbers; budgets are raised only
   after the grants are confirmed.
3. **Derive read budgets conservatively** from the granted limits (or
   from sustainable funding, whichever is smaller), per the sizing
   rules below.
4. **Set the `BUDGET_*` vars** in `wrangler.jsonc` and deploy. Setting
   `BUDGET_R2_CLASS_B_MONTH` is what arms `reads_blocked`.
5. **Monthly review.** Compare the cron's webhook/usage numbers against
   the grants and adjust upward as growth justifies. Lowering an
   established read budget is a community-visible event (CI installs
   start hitting `503`) - avoid it; size conservatively at activation
   instead.

Sizing rules - the degrade-before-pay policy:

- **Headroom covers detection latency.** A budget sits far enough below
  the funded ceiling that the worst-case spend rate cannot cross the
  remaining gap within the detection window (the 15-minute analytics
  cron plus the Analytics API's own data lag). The analytics numbers
  are a conservative usage signal, not billing measurements, which
  argues for more headroom, not less.
- **Storage fits the fallback tier.** Storage is a stock, not a flow:
  the breaker can stop new bytes but cannot un-spend stored ones, so
  the storage budget must always fit within the capacity of the tier
  the registry would fall back to if the grant ended.
- **No stored payment method on free/granted plans.** The breaker
  closes the variable spend channels; the absent payment method closes
  everything else. Adding one is part of the same deliberate activation
  decision, never a convenience.

## Verification pipeline

The external verifier is the `registry-verify` GitHub Actions workflow
(`.github/workflows/registry-verify.yml`): every 5 minutes (plus
`workflow_dispatch`) it builds `cabin-registry-verify` from the root
workspace, lists pending versions through the admin API, inspects each
archive, and PATCHes the verdict back. The checks and reason codes are
documented in `docs/remote-registry.md` ("The verifier's checks").
The verifier addresses scoped names throughout: the artifact download
nests the directory (`artifacts/<scope>/<name>/`) and flattens the
filename (`<scope>-<name>-<version>.zip`), and the verdict PATCH
carries the `<scope>/<name>/<version>` triple.

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

**Abstained versions.** Before downloading anything, the run checks
each would-be-new name against the package corpus
(`docs/architecture.md`, "Name fidelity") and **abstains** on a
finding: no verdict is rendered, the version stays pending, and the
log shows
`<name>@<version>: abstain (<rules>); leaving it pending for operator review`
with the rules (`confusable_package (fmtlib/fmt)`,
`confusable_scope (...)`, `near_name (...)`, `profanity`). Abstain
does not fail the run, and every later cron pass re-logs it - that is
by design; the stuck-pending alert ("N version(s) have been pending
verification for over an hour") is the summons. To resolve one:

- Name is fine: the archive must still pass the real checks before
  anything is exposed - never PATCH `verified` from the name alone.
  Fetch the pending entry and the archive with the verify token
  exactly as the workflow does, run
  `cabin-registry-verify <archive.zip> <entry.json>` locally, and
  PATCH the verdict it prints **with the listing's `checksum` and
  `published_at`** (the admin API refuses an unbound `verified`).
  With a verified version on record the name counts as accepted, and
  every later version of the package skips the advisories.
- Name is not fine: PATCH `{"verdict":"rejected","reason":
  "name_advisory: <rule>"}`. Rejection frees the bytes and the
  publisher can republish under a better name. A rejection never
  vets the name: republishing the same name abstains again and
  re-summons you - by design, not a loop to "fix".

**Name fidelity knobs.** The reserved-name list is an in-code,
operator-maintained const (`registry/src/names.rs`); extend it when
the project starts speaking a new name, never shrink it. The claim
flow's skeleton confusability refusal has one override:
`CLAIM_SKELETON_EXEMPT_SCOPES` (`wrangler.jsonc` `vars`,
comma-separated **exact** scope names) admits a listed scope past the
confusability check only - reserved names and claim permanence always
hold. Set it just before the legitimate claimant walks the claim
flow, and empty it after.

`REGISTRY_VERIFY_TOKEN` is a registry token created on the website's
token page with **only** the `verify` scope (no publish, no yank - the
verifier never needs them), stored as a GitHub repository secret:

```sh
gh secret set REGISTRY_VERIFY_TOKEN
```

Rotate it like `ANALYTICS_API_TOKEN`: create the replacement token
first, `gh secret set REGISTRY_VERIFY_TOKEN` with the new value, and
only then revoke the old token - revoking first would have the cron
failing (versions pending) in between. A wipe drops the tokens table,
so re-provisioning always includes re-issuing this token and updating
the secret.

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
`https://registry.cabinpkg.com`. `REGISTRY_VERIFY_API_ORIGIN` selects the
expected API origin, defaulting to `https://cabinpkg.com`. The workflow
requires the index's `config.json` to declare that exact API origin before
it sends the verify token there; artifact downloads stay on the index
origin. Set both variables together when targeting a different deployment.

## Disaster recovery

Backups run entirely inside Cloudflare - R2/D1 bindings and one
D1-scoped API token, no second vendor holding credentials. Post-launch
registry data is a permanent commitment, so this machinery exists and
was rehearsed pre-launch (see [`verification.md`](verification.md)).

**What is backed up, and how.**

- **Verified archive blobs (durable queue; RPO ~0 from the verified
  transition).** The verdict batch that marks a version `verified`
  enqueues its blob into the `backup_pending` table in the same
  transaction; the verdict's `waitUntil` and every breaker cron pass
  drain the queue into the backup bucket
  (`cabin-registry-backup`, Worker binding `BACKUP`) under the same
  `blobs/sha256/<hex>` key. Only verified content is replicated -
  pending uploads are not yet part of the registry, and their
  recovery path is republishing - and the queue row is the durable
  record, so a crash anywhere between verdict and copy is retried
  rather than lost. Nothing in the service ever deletes from the
  backup bucket - the primary's reclaim paths do not propagate - so
  it is append-only, and a malicious or accidental deletion in the
  primary cannot reach it. Queue rows older than an hour raise the
  breaker's backup-health alert;
  `scripts/backup-backfill.sh` is the manual recovery path (it copies
  every verified blob the backup lacks and deliberately leaves queue
  rows for the Worker's drain to settle, governor ledger included).
  Run the backfill once when seeding backups over existing data.
- **D1 metadata (RPO <= 24 h).** A second cron schedule (`0 3 * * *`)
  runs a nightly logical dump from the Worker itself: it drives the D1
  REST export endpoint (the same API `wrangler d1 export --remote`
  uses) with the `D1_EXPORT_API_TOKEN` secret, follows the returned
  signed URL, and streams the official `.sql` dump into the backup
  bucket at `d1/<YYYY-MM-DD>.sql` with a `.sha256` sidecar (hash
  computed while streaming). A download without a valid `Content-Length`
  fails the job rather than falling back to an unbounded Worker buffer.
  Success requires validation: non-empty,
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
  `meta.last_backup_at` is older than 36 h (or missing) or any
  `backup_pending` queue row is older than an hour; the webhook payload
  always carries a `backup` block with `last_backup_at`, `freshness`,
  and the overdue-backup count. Note the alert also fires between
  provisioning and the
  first nightly pass - that is the "no dump recorded yet" state working
  as intended.

**The `D1_EXPORT_API_TOKEN` secret** is a custom API token whose only
permission is Account | D1 | Edit, scoped to this single account: it can
export (and at worst rewrite) D1 databases, nothing else - no Workers,
R2, or zone access, so the Worker holding it cannot escalate. Rotate it
like `ANALYTICS_API_TOKEN`: create the replacement token first, `wrangler
secret put D1_EXPORT_API_TOKEN` with the new value, then delete the old
token in the dashboard. While the token is broken or absent the nightly
job fails, which the freshness alert surfaces within 36 h.

**The three loss scenarios, and the recovery order.** Work down the
list; each later option covers a case the earlier one cannot.

1. **Bad deploy or migration** (data mangled in place, storage intact):
   use **D1 Time Travel** first. It is always on for production-version
   D1 databases with point-in-time restore at one-minute granularity -
   retention 7 days on the Workers free plan, 30 days on paid (verified
   against the Cloudflare docs 2026-07-10; re-check retention when
   planning an incident response). Restore is destructive and in-place:

   ```sh
   npx --yes wrangler@4.112.0 d1 time-travel info cabin-registry
   npx --yes wrangler@4.112.0 d1 time-travel restore cabin-registry --timestamp=<unix-ts>
   ```

   Blobs need nothing - R2 is untouched by a bad deploy, and archives
   are immutable and content-addressed.
2. **Accidental database loss** (database deleted, or overwritten beyond
   Time Travel's window): create a fresh database, import the newest
   dump, re-point the config, redeploy - exactly what
   `scripts/restore-drill.sh` rehearses against a scratch database:
   download `meta.last_backup_key`... except after a real loss that
   meta row is gone too; list the backup bucket's `d1/` prefix and take
   the newest date **whose `.sha256` sidecar exists and verifies**. The
   sidecar is written strictly after validation and the job deletes an
   invalid dump object again, so a failed export attempt cannot
   masquerade as a good dump. Then `wrangler d1 execute <db> --remote
   --file <dump>.sql`, update `database_id` + `D1_DATABASE_ID` in
   `wrangler.jsonc`, `wrangler deploy`. Loss bounded by the nightly
   cadence: at most 24 h of metadata. Blobs are still in both buckets.
   The restored `meta` rows are whatever the dump carried - after
   restoring any dump (or Time Travel point) that predates launch,
   immediately re-run the launch-checklist `UPDATE` that sets
   `meta.launched = 'true'`, or the launch guard would treat the
   restored registry as pre-launch and let a wipe through.
3. **Primary-bucket data loss** (bucket deleted or objects destroyed):
   the backup bucket is the artifact store of last resort. Recreate the
   primary bucket, then copy `blobs/sha256/*` back (the inverse of
   `scripts/backup-backfill.sh` - same loop with source and destination
   swapped, driven by the checksums in D1), and restore D1 from Time
   Travel or the newest dump as above. Because blobs are
   content-addressed and never mutated, the copied-back objects are
   byte-identical to what clients pinned in lockfiles.

**RPO / recovery time.** Blobs: RPO ~0 from the verified transition
(the durable queue drains on the verdict itself; the stale-row alert
plus backfill close the gaps; a pending upload's recovery path is
republishing). Metadata: RPO <= 24 h from
the nightly dump, and effectively minutes when Time Travel applies.
Recovery time is dominated
by operator response, not data volume, at today's scale: a Time Travel
restore is minutes; a dump import plus redeploy is well under an hour
(the drill's import of the dump takes seconds); copying blobs back
is bounded by object count - budget roughly an hour per few thousand
blobs with the wrangler loop, less with an S3-compatible bulk tool.

**Rehearsal.** `scripts/restore-drill.sh` downloads the latest
dump, verifies the sidecar checksum, imports it into a scratch D1
database (`cabin-registry-drill`), compares per-table row counts against
the live database, spot-checks one version's `metadata_json`
byte-for-byte, and deletes the scratch database. Run it after enabling
backups, after changing the dump machinery, and periodically before
launch milestones; record runs in
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
follow-up before the registry carries data that cannot be re-published.

## Orphaned R2 blobs

Publish writes the R2 blob before the D1 rows, so a crash between the two
writes can leave a blob no `versions` row references. It is unreachable
through the API (artifact lookups go through D1), and a retried publish
reuses it instead of re-uploading (and counts it into
`meta.total_stored_bytes` at that point). Under the governor an orphan
is never invisible spend: the crashed publish's reservation keeps
representing the bytes, reconciliation reports the entry as
`unreferenced` on every pass, and the cleanup is the operator pairing -
delete the object (dashboard or `wrangler r2 object delete`), then
release the ledger entry ("The cost governor"). There is deliberately
no automatic garbage collection, and the reclaim path never releases
primary ledger entries on its own - a successful delete of a
content-addressed key is no proof the key stays gone, because a
concurrent same-checksum publish can recreate it at any moment.

Orphaned bytes are also invisible to the storage self-accounting - the
counter tracks referenced blobs only, by design ("never analytics"), and
the storage budget's headroom below the free limit absorbs the bounded
drift. If the dashboard's bucket size ever diverges noticeably from
`meta.total_stored_bytes`, delete the orphans and recompute the counter
from D1 alone - every version row carries `archive_size`, and the
counter is one size per distinct live checksum:

```sh
wrangler d1 execute DB --remote --command "
  INSERT INTO meta (key, value) SELECT 'total_stored_bytes',
    CAST(COALESCE(SUM(size), 0) AS TEXT) FROM (
      SELECT MAX(archive_size) AS size FROM versions
      WHERE verification != 'rejected' GROUP BY checksum)
    WHERE true
  ON CONFLICT (key) DO UPDATE SET value = excluded.value;"
```

(The `WHERE true` is load-bearing: without it `SQLite` parses the `ON`
after a `SELECT ... FROM` as a join constraint and rejects the upsert.)

## Logs

`wrangler tail` (or the dashboard). One line per request:
`req=<id> method=<m> path=<p> status=<s> token=<token-row-id|->`. Tokens and
token hashes are never logged.
