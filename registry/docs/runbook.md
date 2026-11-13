# Registry Service Runbook

All wrangler commands run with `registry/` as the working directory against
the single top-level configuration in `wrangler.jsonc` (no wrangler
environments, no `--env`), as `node_modules/.bin/wrangler`: the version
`package.json` pins, installed by `npm ci --ignore-scripts` there (again
after `package-lock.json` changes). Every `wrangler deploy` without
`CABIN_WORKER_PREBUILT`, including the one `cargo registry-wipe` runs after
recreating the database, builds the Worker with `worker-build` from `PATH`: install the version CI pins first
(`registry/README.md`, "Development"). Authentication: `CLOUDFLARE_API_TOKEN` in the
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
  (`cargo registry-wipe`) rather than migrated - the database and
  every object in the four buckets. The schema ships as a
  from-zero baseline (`migrations/0001_init.sql`) edited in
  place: while the data is disposable, no ALTER TABLE history
  accretes. A later migration file holds a change the live database
  took without a wipe (a data fix, an added index).
- **Post-launch (`meta.launched` = `'true'`): permanent.** Published
  archives and index state are never wiped, mutated, or deleted; format
  changes need real migrations. The flag is flipped exactly once, by
  hand, as a launch-checklist item (see "Launch checklist"), and every
  destructive maintenance path checks it first and refuses while it is
  `'true'` (the launch guard, `cargo registry-launch-guard`, run from
  the repository root). The wipe is then forbidden.

## Zone security prerequisite

The registry hosts serve machine clients (cabin, curl, CI), so they must not
sit behind a Cloudflare visitor challenge. Zone-wide Bot Fight Mode
stays off on cabinpkg.com (dashboard: Security -> Bots; disabled
2026-07-09): with it on, every registry request answers `403` /
`cf-mitigated: challenge`. If zone-wide bot protection is ever wanted,
it needs a plan that exempts the registry hosts first - e.g. a WAF
custom rule skipping the challenge products for
`http.host eq "registry.cabinpkg.com"` and the `/api/*` paths on
`cabinpkg.com` (note free-plan Bot Fight Mode ignores skip rules; it can
only be toggled zone-wide). Managing zone security needs API-token scopes
beyond the provisioning set (Zone WAF Edit, Zone Settings Edit) or the
dashboard.

A dashboard-managed WAF custom rule (Security -> WAF -> Custom rules)
challenges requests to `cabinpkg.com` that do not come from a verified
bot, except the registry's zone routes (`/api/*`, `/login`,
`/callback*`, `/claim/*`), `/catalog-provenance.json`, which the
registry's cron reads ("Website catalog refresh" below), and `/demo.gif`,
which the repository README embeds. The rule matches
`http.host eq "cabinpkg.com"`, so `registry.cabinpkg.com` is outside it.
A new zone route in `wrangler.jsonc`, or another website path a machine
client fetches, needs a hand edit of the rule's exclusions; until then
that path answers `403` / `cf-mitigated: challenge`.

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
- The rule is dashboard-managed: a change to the website origin's
  registry routes needs a hand edit of the expression. `/callback/claim`
  is covered by the `/callback` arm.

The rule keys on `cabinpkg.com` because the hostname-role split put the
whole write/auth surface on the website origin; it deliberately guards
only that surface, not `/healthz` or the read routes on
`registry.cabinpkg.com`. Covering reads with the same 50-per-10 s ceiling
would throttle legitimate `cabin` traffic - resolving and fetching a
dependency tree fans out many read requests from one IP in seconds -
while abuse of the omitted routes can at worst exhaust free-plan quotas
that fail closed without billing (Workers requests; D1 reads, though a
read-route request reads a few point rows plus rows of only the package
it names, and a package document the edge cache still holds none -
except an uncached `/catalog.json`, which scans the verified corpus and
is safe only while the read plane admits nobody anonymous;
artifact downloads are R2 Class B). The one rule the Free plan grants goes where
the paid exposure (R2 Class A writes) and the heavy CPU live. The zone
security exemption above must likewise keep the machine `/api/*` traffic
on `cabinpkg.com` out of any visitor challenge.

A WAF `429` carries `retry-after: 10` but no error envelope; cabin's
rate-limit mapping degrades to the same "try again" hint off the header
alone.

This dashboard-managed rule is the OUTER layer only. The two public
OIDC endpoints (the trusted-publishing exchange and the verdict PATCH)
and the login-session mint additionally sit behind the Worker's own
admission control: the `OIDC_LIMITER` ratelimit binding in
`wrangler.jsonc` admits all three per client IP, and `JWKS_LIMITER`
bounds the two OIDC endpoints' JWKS refetches; both are required by
`cargo check-deploy` and fail closed when missing. Losing
or misconfiguring the WAF rule therefore degrades defense in depth but
never re-opens the unmetered-JWKS-refetch exposure; the worker's `429`
carries the registry's own `rate_limited` envelope with a
`retry-after: 60`, which is how it is told apart from the WAF's.

## Integrated topology and route management

Two hostnames, one zone:

- **`cabinpkg.com`** - the website Worker (`website`, deployed by
  Workers Builds from `website/` on every push to `main`) serves the
  marketing site, docs, and the account pages; the registry Worker
  (`registry`) takes exactly `/api/*`, `/login`, `/callback*`,
  and `/claim/*` via the zone routes below. This one origin is the
  registry's browser plane.
- **`registry.cabinpkg.com`** - the registry's machine read plane
  (custom domain of `registry`), nothing else.

Deploy skew: the `website` Worker deploys automatically on every push to
`main` (Workers Builds); `registry` deploys from CI (the
`deploy-registry` job in `.github/workflows/registry.yml`) on pushes
to `main` with registry-relevant changes (the `registry` filter in
`.github/path-filters.yml`); the conformance job, and the build job
for any change to what the Worker builds from, validate the change
pre-merge, on its pull-request and merge-queue runs, and a
red merge-queue run never merges, so the previous Worker keeps
serving. A merge that changes the session-plane JSON
contract therefore briefly has the account pages ahead of the live
registry Worker while the deploy runs - accepted pre-launch (private
alpha), with no legacy-field fallbacks in the frontend. A red deploy job may or may not
have activated the new version (`wrangler deploy` can fail after
activation), so check `node_modules/.bin/wrangler deployments list` and run
the smoke checks below; `node_modules/.bin/wrangler deploy` from this directory stays
the manual fallback. The CI deploy token is scoped to Workers
Scripts:Edit + Routes:Edit only, so D1 migrations and R2 provisioning
remain manual steps in this runbook. For exactly that reason a
`deploy-registry` run ends one of three ways. It **deploys**: its
Deploy step ran and succeeded, and the next step then read
Cloudflare's latest deployment routing all traffic to the
one version Deploy tagged with the run attempt (`wrangler deploy
--tag <run id>.<attempt>`). It is **superseded**: main already
carries a newer registry-relevant commit, so it skips Deploy and stays
green, and that commit's run owns the deploy. Or it is **blocked**:
`migrations/` content disagrees with the `migrations-applied` stamp
(or the gate cannot compare them), so the `build-registry-worker`
job fails and `deploy-registry` fails red at its first step, deploying
nothing, with a `Registry deploy blocked` error annotation naming the
recovery; nothing in CI applies a migration. A blocked run never
deploys. The same gate runs before the merge: the `merge-readiness`
job, behind the required `registry-build` check, runs it over every
pull request's and merge group's whole tree, so a change that edits
`migrations/` cannot merge without its stamp, and while main itself is
out of step no pull request merges but the one that repairs the stamp. `cargo registry-migrate
--remote`, run from the repository root of the change's branch before
it merges, is the one honest way to move both: it applies pending
migration files, verifies D1 reports nothing left pending, and only
then refreshes the stamp, which you commit to that same pull request.
Wait for in-flight `deploy-registry` runs to finish before the apply,
and merge the pull request right after it: until it merges, any main
deploy ships a Worker built for the schema the database just left. A
pull request abandoned after its apply leaves production ahead of
main; pre-launch, wipe again from main's tree with the branch's
`wrangler.jsonc` (the wipe refuses a binding that names a database it
no longer finds) and land the resulting id and stamp through a pull
request, and post-launch land the
migration file on main, since `registry-migrate` refuses an applied
migration whose file is gone. `registry-migrate` refuses the
state where
nothing is pending yet the files differ from the stamp - that means an
already-applied file was edited in place, which D1 (tracking applied
migrations by filename) would never replay; pre-launch that edit ships
through `cargo registry-wipe`, run from the change's branch in the
same window (it reapplies from zero, refreshes the stamp and the
database id, and redeploys that branch's Worker; commit both files to
the pull request), and post-launch it must become a new migration file.
Hand-refreshing the stamp bypasses those checks; don't.

A Deploy step that exits 0 without its tagged version then serving
all traffic fails the job with a `Registry deploy unconfirmed` error
annotation instead of counting as a deploy. Re-run all jobs (a rerun
of `deploy-registry` alone fails, since it would reuse the build's
freshness answer): it rebuilds and redeploys under a fresh tag, or
stays green if a newer commit superseded it meanwhile. The
confirmation reads the control plane, not
an edge response (no request tells one version from another:
`/healthz` reads the same from all of them), so the smoke checks below
remain the edge-side check.

**Deploy credential boundary.** The deploy token
(`CF_REGISTRY_DEPLOY_TOKEN`, a `production` environment secret whose
deployment branch policy admits only `main`) never shares a runner
with a build. `build-registry-worker` holds no secret: it runs the
freshness and migration gates, `worker-build --release`, and the
deploy-config guard, bundles the result with `wrangler deploy
--dry-run --outdir dist`, and uploads `dist/` as the run's
`registry-worker-<attempt>` artifact with a SHA-256 digest of its
files. `deploy-registry` starts on a fresh runner, installs wrangler
from `package-lock.json` with `--ignore-scripts`, downloads that run's
artifact, refuses it unless it holds only regular files matching the
digest, and deploys it with `wrangler deploy --no-bundle` and
`CABIN_WORKER_PREBUILT=1`, which turns `wrangler.jsonc`'s
`build.command` into a no-op: no Cargo command, build script,
proc-macro, or bundler runs next to the token. Right before deploying
it rechecks freshness with plain git, against the
`.github/path-filters.yml` pathspecs the build job resolved, because
the build's answer is minutes old and the deploy queue does not order
runs. The recheck and the activation are not atomic: a commit landing
between them does not stop this deploy, but that commit's own deploy
waits in the queue until this job ends, so an older run never replaces
a newer run's Worker. Limits: this keeps build-time code from reading
the token, but such code can still change the Worker it builds (or the pathspecs it
hands over), and that Worker runs with its own bindings and secrets;
the digest catches a missing, damaged, or substituted artifact, not
one a compromised build job produced; wrangler and its npm
dependencies run with the token, as they always have; wrangler still
parses the artifact's entry with esbuild (without bundling) and reads
the source map its `sourceMappingURL` comment names, which a
compromised build could point at another JSON file on the deploy
runner (the token itself is only in the environment); and when
Cloudflare rejects a deploy for startup time, wrangler loads the
bundle in a local `workerd` to profile it, so the Worker's own code
runs on that runner, inside the V8 sandbox.

Because the stamp covers migration *content*, an applied file's
comments are frozen with its SQL: a rename elsewhere in the repository
cannot be swept into one without either a pre-launch wipe or a new
migration. `migrations/0001_init.sql` still names the guard and the wipe
by their pre-migration script paths for that reason - read
`cargo registry-launch-guard` and `cargo registry-wipe` as current, and
an applied migration's prose as a record of what was true when it was
applied.

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
  belong to `registry`.
- The website's `/dashboard`, `/settings/*`, and `/login/denied` pages
  are live on the origin ("Account pages" in `website/README.md`). For
  ops debugging, the session API also works directly: `curl -H "Cookie:
  cabin_session=..." -H "Content-Type: application/json" -H
  "X-CSRF-Protection: 1"` against `https://cabinpkg.com/api/v1/user/...`.
- Each browser sign-in is a `browser_sessions` row that its cookie
  needs; signing out deletes only that browser's row. To end every
  browser session of an account (a stolen cookie), delete its rows:
  `DELETE FROM browser_sessions WHERE provider_account_id = '<github id>'`.

**Website catalog refresh.** The `*/15` cron keeps the website's
package pages on the registry's catalog (`docs/remote-registry.md`,
"The catalog"). It fetches `https://cabinpkg.com/catalog-provenance.json`
(a same-zone `fetch` reaches the website Worker because it serves its
own domain, not a route) and, when that record differs from what
`/catalog.json` answers anonymously, POSTs the `WEBSITE_DEPLOY_HOOK`
secret: a Workers Builds deploy hook of `website` for `main`
(the Worker's Settings, Builds, Deploy Hooks). The `website_refresh`
meta row counts the triggers per catalog, at most three; `wrangler
tail` shows the `website refresh:` lines. Without the secret the step
does nothing. A website rollback whose recorded catalog no longer
matches, or that predates the record (a 404), is rebuilt over by the
next pass; delete the secret first to keep one.

## First-time provisioning

Prerequisite besides the API token: the GitHub OAuth app (homepage
`https://cabinpkg.com`, authorization callback
`https://cabinpkg.com/callback` - the browser plane lives on the website
origin, see "Integrated topology and route management"; the claim
flow's `/callback/claim` needs no OAuth-app change, because GitHub
accepts a `redirect_uri` under the registered callback's path). Its client id is
public and lives in `wrangler.jsonc` (`vars.GITHUB_CLIENT_ID`), next to
`ALLOWED_GITHUB_IDS` (the numeric GitHub user ids allowed to sign in and,
during the private alpha, to use the registry at all - every Bearer token
is re-checked against it on use);
the wrangler secrets are the client secret, the session secret, and
`ANALYTICS_API_TOKEN` for the budget cron (plus the optional
`NOTIFY_WEBHOOK_URL` and `WEBSITE_DEPLOY_HOOK`; see "Budget breaker and
service mode" and "Integrated topology and route management").
`CACHE_PURGE_TOKEN` joins them once the edge serves the buckets ("Upload
lifecycle", the publish pass).

```sh
node_modules/.bin/wrangler d1 create registry
# copy the printed database_id into d1_databases in wrangler.jsonc
for bucket in archives uploads download-logs index; do
  node_modules/.bin/wrangler r2 bucket create "registry-$bucket"
done
# UPLOADS and DOWNLOAD_LOGS expire their objects: the Worker never
# deletes one ("Upload lifecycle").
for bucket in uploads download-logs; do
  node_modules/.bin/wrangler r2 bucket lifecycle add "registry-$bucket" \
    expire-after-14-days --expire-days 14 -y
done
node_modules/.bin/wrangler d1 migrations apply DB --remote
node_modules/.bin/wrangler deploy
# deploy creates the registry.cabinpkg.com custom domain and its DNS
# record on the cabinpkg.com zone; deploy first so the secret puts below
# attach to a deployed Worker instead of prompting to create a draft.
printf '%s' "$GITHUB_CLIENT_SECRET" | node_modules/.bin/wrangler secret put GITHUB_CLIENT_SECRET
openssl rand -base64 32 | node_modules/.bin/wrangler secret put SESSION_SECRET
printf '%s' "$ANALYTICS_API_TOKEN" | node_modules/.bin/wrangler secret put ANALYTICS_API_TOKEN
```

Idempotence: `d1 create` / `r2 bucket create` fail cleanly if the resource
exists (`d1 list` / `r2 bucket list` to check); `r2 bucket lifecycle
list <bucket>` shows a bucket's rules; `migrations apply` and
`deploy` are safe to re-run; a re-run `secret put` overwrites the value.

Smoke checks after any deploy:

```sh
curl -sS -o /dev/null -w '%{http_code}\n' https://registry.cabinpkg.com/healthz   # 200
curl -sS -o /dev/null -w '%{http_code}\n' https://registry.cabinpkg.com/config.json   # 200 (503 under reads_blocked)
curl -sS -D - https://registry.cabinpkg.com/unknown/path   # uniform 401 envelope,
# with WWW-Authenticate: Cabin login_url="https://cabinpkg.com/docs/remote-registry"
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

`cargo registry-wipe` (run from the repository root) is the whole
procedure and the guarded destructive path: it refuses to run unless the
live `meta.launched` row is exactly `'false'` (missing row or unreadable
flag also refuse - fail-safe; the guard is `cargo registry-launch-guard`,
called directly rather than spawned, and exercised end to end by the
smoke test). What it does, in order:

1. Asks for interactive confirmation (`CABIN_WIPE_YES=1` skips it), then
   runs the guard immediately before anything destructive. The guard
   first proves the config's `DB` binding and the account's database
   named `registry` are the same database (`d1 delete` resolves the
   name through the binding, so a stale binding would otherwise delete a
   database other than the account's `registry`), then reads
   `meta.launched` through the binding.
2. Empties the four buckets through the R2 REST API, before the
   database goes: every object in `registry-archives`,
   `registry-uploads`, `registry-download-logs`, and
   `registry-index`, in that order (list, delete
   per key, and list again until a listing comes back empty;
   `wrangler r2 object delete` removes exactly one object and has no
   prefix or bulk mode, which is why the wipe drives the API directly).
   A failed or malformed listing, a refused delete (the API's rate
   limit, about 1,200 requests per five minutes, included - a bucket of
   more than a thousand-odd objects can take a second run), or a key
   still listed after its delete answered success stops the run - and since
   the database and its flag still exist, re-running the command
   resumes the sweep. The cron puts `config.json` back into
   `registry-index` when it finds none ("Upload lifecycle", the
   publish pass), and a sweep that lists it again after deleting it
   stops that way. The buckets themselves, their lifecycle rules, their
   bindings, and every other Cloudflare resource stay.
3. Deletes and recreates the database
   (`wrangler d1 delete registry -y` / `wrangler d1 create
   registry`) and bakes the new id into the `DB` binding's
   `database_id` in `wrangler.jsonc` (refusing if more than one D1
   binding exists - the rewrite targets the only one), verifying the
   file now carries it exactly once. Commit that change.
4. Applies all migrations from zero
   (`wrangler d1 migrations apply DB --remote`) and refreshes the
   `migrations-applied` deploy-gate stamp - the recreated database now
   runs exactly the files' content. Commit the stamp with the
   `wrangler.jsonc` change.
5. Redeploys (`wrangler deploy`) so the new `database_id` is baked into
   the Worker's bindings. A colo's edge cache can keep serving a
   pre-wipe package document for up to its 60 s TTL.

The wipe leaves the governor Durable Object alone: its monthly operation
windows mirror R2 operations Cloudflare already metered this month and
cannot be rebuilt ("The cost governor").

`cargo registry-wipe --local` is the same idea for the local `.wrangler/`
state (guard, drop the emulated D1/R2/Durable Object/cache state,
reapply migrations); the smoke
test uses it to assert the refusal branch, and a re-run of the smoke
test wants it first so the governor's windows and the emulated edge
cache start empty.

A remote wipe that stops during the sweeps (step 2) has not touched the
database: fix the cause and re-run it. If one is interrupted between the
database delete and the end, the guard cannot read the half-provisioned
database and refuses the re-run; finish by hand with the remaining
steps ("First-time provisioning" has the same commands).

Tokens and browser sessions live in the dropped database, so a wipe
revokes everything; users re-authenticate afterward
(`docs/remote-registry.md`). A browser still holding a pre-wipe session
cookie - the operator's included - gets 401 from the session API until
it visits `/login`: GitHub auto-approves the already-authorized OAuth
app, and `/callback` recreates the user row and a fresh session. The
verifier needs no re-provisioning, only re-enabling in step 3 (see
"Verification pipeline").

### Post-wipe re-provisioning

Do the whole sequence in one sitting, in this order. Before the wipe
itself, disable the ports publish workflow (`gh workflow disable
publish.yml -R cabin-ports/ports`) and cancel any in-flight run of it:
publish runs come from a manual dispatch or its schedule, and one left
going would span the database drop, or publish into the wiped registry
as soon as its deploy wait finds a deploy containing its pinned Cabin
commit, usually at once, ahead of the receipts reset in step 3.
Re-enable it in step 3.
Disable the verifier too (`gh workflow disable registry-verify.yml`,
which stops its schedule but not the runs it already queued) and cancel
its queued or in-flight runs, so none spans the database drop.
An archive a writer slips in anyway - a publish landing between the
UPLOADS sweep and the database drop, while the old Worker still
serves - is left with no row naming it and expires with the bucket's
lifecycle rule. One the old Worker's placement copies into ARCHIVES
after that bucket's sweep stays, since ARCHIVES has no such rule; it
is harmless, because its key names exactly those bytes and a later
publication of them converges on it. A document the old Worker puts
into INDEX after that bucket's sweep is not harmless: it records a
generation of the dropped database, and the publish pass leaves it
alone for every change the new database numbers at or below that ("The
publish pass"). Only the cron and a verdict's request put documents: a
cron invocation ends within 15 minutes, and the verifier's runs were
cancelled before the wipe. Step 3 looks at the bucket after both. The
cron also puts `config.json` back there; that records no generation,
and every pass replaces one that differs.

1. Commit the `wrangler.jsonc` database-id change and the refreshed
   `migrations-applied` stamp to the pull request that changes the
   migrations (or to one of their own when main already carries them)
   and merge it; its main push deploys the Worker bound to the new
   database ("Deploy skew").
2. Sign in and re-claim scopes (`/claim/<scope>`). A sign-in that lands
   on `/login/denied` with no reason in the URL has its refusing check
   in the Worker's log (`sign-in refused: ...`; `node_modules/.bin/wrangler
   tail` while retrying, since the stored logs are sampled). Type or
   paste the claim URL rather than following a link from another site: initiation refuses a
   cross-site navigation, and does it with the same uniform
   `?claim=denied` redirect as a genuinely refused claim. A GitHub org's
   OAuth app grant survives the wipe, so an org-backed re-claim grants
   immediately - no third-party-access dance the second time. The
   `cabin-ports` claim is what re-arms ports publishing: the
   trusted-publishing exchange mints for the scope's oldest owner and
   refuses while the scope is unclaimed.
3. Start the ports receipts log over while the publish workflow is
   still disabled (its `docs/publishing.md`, "Registry wipe"), then re-enable
   it (`gh workflow enable publish.yml -R cabin-ports/ports`), prove
   the trusted-publishing binding with an `exchange-check`
   dispatch (exchange + immediate revocation, publishes nothing), then
   dispatch it plainly from `main` to republish the set. There is
   no publish token to mint and no `CABIN_PORTS_TOKEN` secret: the run's
   OIDC exchange is the credential. Before re-enabling the verifier,
   and no sooner than 15 minutes after the wipe, look at
   `registry-index` in the dashboard and delete any object it
   lists other than `config.json` (`wrangler r2 object delete
   registry-index/<key> --remote`): nothing puts a document
   until a verdict arrives. Then
   re-enable the verifier (`gh workflow enable registry-verify.yml`);
   its schedule verifies the republished
   set. The verifier workflow needs no secret of its own: each run
   mints its token through the trusted-publishing exchange, and the
   baseline migration seeds the backing identity its verifier arm
   resolves (`VERIFIER_BACKING_ACCOUNT_ID`).
4. Rerun whatever main CI went red against the old registry
   (`gh run rerun <id> --failed`). A ports publish run whose upload
   failed does not resume by a rerun: it keeps the receipts log's lock
   until the log is reconciled, and only then is the workflow
   dispatched again (its `docs/publishing.md`, "Reconciling the log").

## Launch checklist

Launch contains **no infrastructure work** - the Worker, domain, database,
buckets, crons, secrets, WAF rule, and verifier are already the production
ones. Launch is a data and policy event, in order:

1. Final wipe: `cargo registry-wipe` (the guard still passes -
   `meta.launched` is `'false'`), so the registry starts empty of
   pre-launch test data.
2. Require license metadata on read - a code change, after the wipe
   and before the flag flips. `cabin publish` and publish validation
   already refuse a package without a license, so after the wipe no
   entry lacks one and strict readers never meet a legacy entry. Index
   readers still accept a version entry without `license`
   (`crates/cabin-index` `RawVersion`, marked `Private-alpha
   leniency`); make the field mandatory there and remove the
   private-alpha allowance from `docs/manifest.md`,
   `docs/package-index.md`, and `docs/remote-registry.md`
   ("Idempotency"). `cabin vendor` must then write a superseded
   revision's own license instead of dropping the field, and a file
   registry written by an older Cabin is republished, not read.
3. Flip the launch flag - once, by hand:

   ```sh
   node_modules/.bin/wrangler d1 execute DB --remote --command \
     "UPDATE meta SET value = 'true' WHERE key = 'launched'"
   ```

   From this moment the data policy is binding (no wipes, no deletes,
   real migrations only) and `cargo registry-wipe` refuses to run. The flag
   lives in `meta`, so restoring the database to a point before launch
   would reset it - re-running this `UPDATE` is part of any such
   restore (see "Disaster recovery").
4. Remove the private-alpha labels from the website (the `private α`
   badges on the sign-in affordance and the account-page shell, and the
   private-alpha copy on `/login/denied` - see `website/`).
5. Decide and apply the access policy: expand `ALLOWED_GITHUB_IDS` or
   open sign-up. During the private alpha the allowlist also gates every
   read except `config.json`, plus the stats, checked on each Bearer use
   (`src/glue/mod.rs` `authenticate`), and `config.json` declares
   `auth-required: true` (`src/documents.rs`): reopening public reads is
   a code change, not a data flip (`docs/remote-registry.md`, "Read
   routes").
6. Re-enable the `registry-verify` workflow if the final wipe disabled
   it - after the INDEX check in step 3 of "Post-wipe re-provisioning" -
   and dispatch it once (see "Verification pipeline"); it carries no
   long-lived token to re-issue - each run mints its own through the
   trusted-publishing exchange.

**Post-launch staging is intentionally not maintained.** There is no
standing second environment: with a single maintainer there is nothing to
coexist with, and a permanently-running staging copy would immediately go
stale. Risky changes (migrations, storage-format work, breaker changes)
are rehearsed against a temporary scratch deployment recreated from this
directory's `wrangler.jsonc` and `migrations/` - deploy under scratch
names (worker, database, buckets), run the rehearsal, tear it down.

## Budget breaker and service mode

The scheduled handler (cron, every 15 minutes) evaluates usage against the
budgets and persists the result to `meta.service_mode`
(`normal` | `warn` | `writes_blocked` | `reads_blocked`) with a
human-readable `meta.service_mode_reason` (`reads_blocked` is unreachable
until a read budget is configured - see "Read budgets and paid-plan
activation" below). Inspect it:

```sh
node_modules/.bin/wrangler d1 execute DB --remote --command \
  "SELECT key, value FROM meta
   WHERE key IN ('service_mode', 'service_mode_reason')
   UNION ALL SELECT 'stored_bytes', COALESCE(SUM(stored_bytes), 0) FROM users"
```

Override it (for example to force-block writes during an incident, or to
unblock after freeing storage):

```sh
node_modules/.bin/wrangler d1 execute DB --remote --command \
  "UPDATE meta SET value = 'writes_blocked' WHERE key = 'service_mode'"
```

Two caveats. First, the next cron pass **overwrites** a manual override
with its own evaluation (within 15 minutes when analytics are healthy), so
an override is a stopgap, not a switch; to keep writes blocked durably,
lower the matching `BUDGET_*` var and redeploy. Second, the request path
caches the mode in isolate memory for ~60 s (`SERVICE_MODE_TTL_SECS`; the
smoke test pins it to 0 via `.dev.vars`), so an override can take up to a
minute to bite.

The breaker's numbers are approximate and cron-delayed. The hard,
request-time limits live elsewhere: R2 operations in the governor ("The
cost governor" below), which the analytics here only audit - they can
escalate the mode, never grant governor allowance - and stored and
uploaded bytes in the publish admission ("Upload lifecycle"). The
storage figure is the sum of what publishers are charged
(`users.stored_bytes`), not what the buckets hold. The budget ceilings
the cron evaluates against, with their in-code
defaults (`src/breaker.rs`), each comfortably below the matching
Cloudflare free limit:

| Var | Default | Free limit |
| --- | --- | --- |
| `BUDGET_R2_STORAGE_BYTES` | 4 GiB | 10 GB-month |
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
stats, the admin plane, and the verifier's config/artifact
fetches keep working). Do not set it before
the activation procedure below.

Overrides are ordinary `vars` entries in `wrangler.jsonc` and take effect
on the next deploy.

The analytics-sourced metrics need the `ANALYTICS_API_TOKEN` secret: an
API token whose **only** permission is Account | Account Analytics | Read,
scoped to the single account (dash.cloudflare.com -> My Profile -> API
Tokens -> Create Token -> Custom token). It reads aggregate usage numbers
and nothing else.

```sh
printf '%s' "$ANALYTICS_API_TOKEN" | node_modules/.bin/wrangler secret put ANALYTICS_API_TOKEN
```

To rotate it, create the replacement token first, run the `secret put`
with the new value, and only then delete the old token in the dashboard -
revoking first would have the cron running degraded in between. Without a
working token the cron logs the skip, evaluates on the exact
self-accounted storage alone, and never de-escalates the persisted mode on
the missing data. Optionally set a `NOTIFY_WEBHOOK_URL` secret to receive
a JSON summary POST on every mode change and on every pass that finds
the verifier stuck or the publication stalled.

## The cost governor

The governor is the hard, request-time authority for every billable R2
operation the Worker can initiate (`src/governor.rs`): one named
SQLite-backed Durable Object (binding `GOVERNOR`,
class `Governor`, singleton `"governor"`) that admits or
refuses each R2 call before it is made. The breaker above stays the
approximate, analytics-driven service mode; the governor is exact and
per-request, and an unreachable governor fails every fresh R2 path
closed (`503 registry_over_budget`; edge-cache hits keep serving).
Stored bytes are not the governor's to bound: the publish admission
does that in D1 ("Upload lifecycle").

The hard limits, env-overridable in `wrangler.jsonc` `vars` with
in-code defaults (`src/governor.rs`):

| Var | Default | Pool (as snapshots name it) |
| --- | --- | --- |
| `GOVERNOR_R2_CLASS_A_PUBLISH_MONTH` | 200,000 | `a_publish` - publish-path puts |
| `GOVERNOR_R2_CLASS_A_INFRA_MONTH` | 500,000 | `a_infra` - archive placement puts; document puts |
| `GOVERNOR_R2_CLASS_B_ORDINARY_MONTH` | 4,000,000 | `b_ordinary` - artifact cache misses |
| `GOVERNOR_R2_CLASS_B_SOURCE_MONTH` | 500,000 | `b_source` - source-viewer ranged reads |
| `GOVERNOR_R2_CLASS_B_VERIFIER_MONTH` | 500,000 | `b_verifier` - pending-artifact fetches |
| `GOVERNOR_R2_CLASS_B_PUBLISH_MONTH` | 200,000 | `b_publish` - publish-path heads |
| `GOVERNOR_R2_CLASS_B_INFRA_MONTH` | 700,000 | `b_infra` - the cron's object checks of expired reservations; archive placement's check and read; document checks |

The two infrastructure defaults cover a month of the cron (2,976
invocations) with every pass at its ceiling: 142 class A operations an
invocation (20 placements, their 20 documents, the publish pass's 100
documents, the catalog and the configuration) and 212 class B (those, a
second for each placement, and 50 reservation checks). They are sized
that way because yank has no quota: a member's yanks can hold the
publish pass at its ceiling, and a limit that did not cover it would
let them stop archive placement.

An invocation at those ceilings also makes about 200 D1 queries (a
batch counts as one). D1 allows an invocation 1,000 on Workers Paid and
50 on the free plan, where a publish pass over more than a few dozen
packages would fail before it completes a row and start over at the
next invocation: changing that many packages at once, as a ports
republish does, needs Workers Paid.

A limit var only takes effect through `wrangler.jsonc` `vars` and a
deploy; `cargo check-deploy` (also a CI step) refuses a var whose
name it does not know or whose value does not parse as a
u64, because a misspelled name is silently ignored and an unparsable
value fails closed to a zero limit in production. It still accepts the
three `GOVERNOR_STORAGE_*_BYTES` names the engine parses, which bound
nothing: no path reserves storage in the governor.

Two knob rules that differ from the breaker's, on purpose:

- **A set-but-unparsable limit fails closed to zero** and blocks its
  pool loudly (the breaker's budgets fall back to defaults instead).
  These are hard spending caps; a typo must never silently widen one.
- **Operation windows are explicit UTC calendar months**, not
  Cloudflare's billing window, which cannot be inferred reliably; the
  defaults carry the headroom for that skew. Windows only roll
  forward - a regressed clock cannot mint allowance.

**Inspecting the windows.** Every breaker cron pass logs one summary
line (`governor usage: ops ...`) next to the analytics
evaluation - `wrangler tail` shows both, the dashboard only what its
sample kept (see "Logs"). On demand
(with a login-session token in `CABIN_REGISTRY_TOKEN`):

```sh
(cd .. && cargo registry-governor usage)
```

It renders `GET /api/v1/admin/governor` (Bearer, `verify` scope), is
read-only, and is safe on a live registry. The named object is created
on first use and applies its schema idempotently before answering, so
`usage` also initializes a fresh object.

**Analytics divergence.** The breaker cron's R2 numbers audit the
governor: if observed Class A/B operations run well ahead
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
overspend). Moving to Workers Paid lifts these ceilings - and the free
plan's documented 10 ms CPU limit, which near-cap publishes and
downloads exceed (hashing alone costs ~4 ms/MiB, so no useful
`max_archive_bytes` fits under it) - and is the prerequisite for opening
the registry beyond the allowlist; until
then, do not raise `GOVERNOR_R2_CLASS_B_ORDINARY_MONTH` toward its
cap without accepting that the object's daily quotas become the
guaranteed failure point.

**Known ceilings.** Two residual windows are documented rather than
engineered around: a
spurious FORWARD platform-clock jump across a month boundary would
roll an operation window early (a later regress cannot roll it back -
windows only move forward - so the net effect is one early reset,
with the analytics operation counts as the independent tell), and
a catastrophic loss of the Durable Object's storage would restart the
operation windows at zero. They cannot be rebuilt (R2 exposes no
per-pool op counter), so
after a storage loss on a *launched* registry the month's operation
budget re-opens on top of ops already metered - Cloudflare's 30-day
Durable Object point-in-time recovery is the first-line fix, restoring
the object to just before the loss.

**Deploy notes.** The Durable Object class ships with the same
`wrangler deploy` as the Worker (the `migrations` block in
`wrangler.jsonc` tags `Governor` as a SQLite-backed class); the CI
deploy token's Workers Scripts:Edit scope covers Durable Object
migrations; if the API refuses a deploy that carries a new migration,
fall back to a manual `node_modules/.bin/wrangler deploy`. A wiped or
reinitialized object recreates its schema idempotently and starts its
windows at zero ("Known ceilings" above). The schema is applied with
`CREATE TABLE IF NOT EXISTS` only,
so any future column change to a governor table needs an explicit
migration (an `ALTER TABLE`, not a silent no-op), and the `Governor`
class must only ever be migrated by `renamed_classes`/
`transferred_classes` - never dropped and recreated, and never a
`deleted_classes` migration on a launched registry - because a class
delete destroys the SQLite storage and the monthly operation windows
cannot be rebuilt. `cargo check-deploy` (a CI step after the
Worker build, runnable locally any time) enforces the static half of
all of this before a deploy can: bindings present, the `v1` migration
verbatim, no `deleted_classes` for a bound class, no mixing of the
`migrations` array with wrangler's newer `exports` lifecycle, the
bundle really exporting `Governor`, and every `GOVERNOR_*`/`BUDGET_*`
var name and value valid.

**Validating a governor change.** Before merging, run the registry
checks (`registry/AGENTS.md`, "Checks") and the full local smoke run
with local state wiped first, so the windows and edge cache start empty.
After the deploy lands ("Integrated topology and route management"),
run the smoke checks from "First-time provisioning" and read the
windows:

```sh
(cd .. && CABIN_WIPE_YES=1 cargo registry-wipe --local)
(cd .. && CABIN_REGISTRY_SMOKE_TOKEN=cabin_smoke cargo registry-smoke)
(cd .. && cargo check-deploy --require-bundle)   # the smoke run built the bundle
# after the deploy:
(cd .. && CABIN_REGISTRY_TOKEN=... cargo registry-governor usage)
```

What the local smoke run proves, and what it cannot: `wrangler dev`
runs the production runtime (workerd) with a real local Cache API,
Durable Object, R2, and D1 under `.wrangler/`, so governor admission,
cache-hit-versus-charged-read, every refusal path, and the
bounded-concurrency invariant execute for real locally. What local
runs cannot prove is the production **edge** cache's behavior -
per-colo entries, eviction under memory pressure, TTL interaction with
the immutable headers. The exact remote verification: with a real
token, download one verified artifact twice and compare
`cargo registry-governor usage` (from the repository root) before and
after - the first read may
charge `b_ordinary` once (a miss), the second must not move it (a
hit); on a cold colo or after eviction both charge, which is the
governed-and-budgeted path working, not a bug.

**Rollback.** Worker code and governor state are separate by design:
the windows live in the Durable Object and no rollback, redeploy, or
`wrangler rollback` touches them (Cloudflare rollbacks change no
resources). Two rules bound what a rollback may target:

- **Never below the governor.** The platform refuses a rollback across
  a Durable Object lifecycle change (the `v1` migration), and that
  refusal is load-bearing: a pre-governor Worker would spend R2 with
  no admission control at all. Do not work around it with a manual
  re-deploy of old code.
- **Adjacent governor-era versions only.** The DO's stored schema and
  the worker's protocol evolve together; roll back only to a version
  whose governor protocol matches what is deployed today (compare
  `registry/src/governor.rs` and `registry/src/governor_do.rs` between
  the two versions), and prefer a `git revert`
  through CI - it keeps the freshness and migrations gates and leaves
  an auditable trail. `node_modules/.bin/wrangler rollback` is for the
  emergency where CI itself is the problem.

```sh
node_modules/.bin/wrangler deployments list   # find the target version
node_modules/.bin/wrangler rollback           # interactive; or pass the version id
# then validate exactly like a deploy:
curl -sS -o /dev/null -w '%{http_code}\n' https://registry.cabinpkg.com/healthz
(cd .. && CABIN_REGISTRY_TOKEN=... cargo registry-governor usage)
```

A rollback leaves the windows as they are: what the month already
consumed stays consumed. If the rollback was prompted by suspected
accounting corruption,
stop before rolling back and capture `cargo registry-diagnose` output (from
the repository root) plus
`cargo registry-governor usage`, from the repository root, first -
Cloudflare's 30-day Durable Object
point-in-time recovery ("Known ceilings" above) needs the incident
time, and a rollback does not restore DO state.

## Read budgets and paid-plan activation

The read-side breaker (`reads_blocked`; see "Budget breaker and service
mode" above) ships as dormant infrastructure: fully
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
  the registry would fall back to if the grant ended. The budget counts
  the bytes publishers are charged. What UPLOADS keeps until its
  lifecycle rule removes it - the archives of rejected, released, and
  timed-out uploads, and the upload of every served archive beside
  its copy in ARCHIVES - is outside it, bounded only by the daily
  allowances ("What stays in UPLOADS"), and at today's allowances that
  bound is above the fallback tier's 10 GB.
- **No stored payment method on free/granted plans.** The breaker
  closes the variable spend channels; the absent payment method closes
  everything else. Adding one is part of the same deliberate activation
  decision, never a convenience.

## Upload lifecycle

Every publish names its operation with a `Cabin-Upload-Id` header
(`docs/remote-registry.md`, "Publish"), and an admitted operation
stores its archive as one object of its own, `uploads/<upload id>.zip`
in UPLOADS. A revision row is in one of five states
(`revisions.verification`, `stored_at`, and `archived_at`):

| State | Row | Leaves it by |
| --- | --- | --- |
| reservation | `pending`, `stored_at` NULL | its request storing and confirming the archive, else the reservation sweep |
| pending | `pending`, `stored_at` set | a verdict, else the pending timeout |
| verified | `verified`, `archived_at` NULL | archive placement |
| served | `verified`, `archived_at` set | never |
| rejected | `rejected` | a republish of the same bytes |

- **Admission is one D1 transaction** (`src/sql/admission.rs`). It
  takes everything the upload needs - the publisher's storage, the
  package quotas, the daily upload allowances, a pending slot - or
  nothing, before any R2 write. Only an admitted upload is stored, and
  only a stored one is listed to the verifier.
- **The reservation sweep** runs on the `*/15` cron. A reservation
  older than 15 minutes is settled from what UPLOADS holds: confirmed
  when its object is there, otherwise released - `rejected` with reason
  `upload_not_stored`, its storage refunded. A pass settles at most 50,
  oldest first, and charges one `b_infra` operation per object check; a
  governor refusal ends the pass and the next one continues.
- **The pending timeout.** A stored upload still pending 14 days after
  `stored_at` is rejected with reason `verification_timeout` and
  refunded by the same cron: its object is about to expire and could
  no longer be verified.
- **Archive placement** (`src/glue/publication.rs`). A verified
  revision is served only once its archive is in ARCHIVES under
  `archives/<scope>/<name>/<scope>-<name>-<version>-<revision>.zip` and
  the row's `archived_at` is set. The verdict's own request runs the
  placement pass and the `*/15` cron runs it again for whatever is
  left: at most 20 revisions a pass, oldest verdict first, each charged
  two `b_infra` operations and one `a_infra`; a governor refusal ends
  the pass. R2 checks the copy against the row's SHA-256, and no pass
  replaces an object: a key that already holds the archive counts as
  placed, and one that holds anything else is logged on every pass
  (`archive placement: ...`) and left for the operator. When UPLOADS
  no longer holds the verified bytes, the revision is rejected with
  reason `publication_source_lost` and refunded - it was never served,
  and the same bytes may be published again.
- **The publish pass** (`src/glue/publication.rs`) keeps INDEX holding
  the documents the read plane composes: each package's under
  `packages/<scope>/<name>.json`, and `catalog.json`. A revision
  becoming served, a yank, and an un-yank each take the next value of
  `meta.catalog_generation`, give it to their package
  (`packages.generation`), and add a row to `publication_jobs`, in the
  transaction that makes the change. A verdict's request puts its
  package's document once the archive is placed; a yank's request makes
  no R2 call. The `*/15` cron puts the
  documents of the packages with open rows - at most 100
  packages a pass, oldest change first - then the catalog, and
  completes the rows (`completed_at`) recorded at least 15 minutes
  before the pass started. Each object records the generation it was
  rendered at, and a pass replaces only an object it found at an
  earlier one. R2 compares
  that replacement by the object's bytes, so when a document returns
  to earlier bytes (a yank, then the un-yank), a request or pass that
  rendered between the two can still put its document over the current
  one; the 15 minutes are there so that
  such a writer has finished before the row is completed, and the pass
  that completes the row puts the current document first. Until then INDEX
  may show the earlier document, up to the next pass that reaches the
  package. Every
  attempt at a document is charged one `b_infra` operation and one
  `a_infra`, whether or not it has to write. A row is open
  for 15 to 30 minutes, longer while more than 100 packages have open
  rows. One open for 45 minutes with fewer than that means a document
  is not getting into INDEX or its purge is failing; the pass logs why
  (`document publication: ...`). The cron reports a row open that
  long, and a verified archive unplaced that long, on every pass until
  it clears (`publication health: ...`, and `publication.alert` in the
  webhook's summary). The report does not tell a backlog from a
  failure. Both passes take the oldest first, so a backlog shows as
  `publication.stalled_archive` or `publication.stalled_job` moving on
  from pass to pass (`cargo registry-diagnose` prints `open_jobs`);
  a time that stays the same is an archive or a row the pass cannot
  finish. A completed row is deleted 30 days after its
  `completed_at`; an open row is never deleted for its age.

  The same pass keeps `config.json` in INDEX: it reads the object every
  time (one `b_infra` operation and one `a_infra`, as for a document)
  and puts the configuration this deployment serves when the bytes
  differ. Around a deployment that changes it, a pass of the previous
  version can put the old one back until the next pass.

  **Purging the edge** is off until the buckets' own domains serve
  INDEX and ARCHIVES. It is switched on by `INDEX_ORIGIN` in
  `wrangler.jsonc` `vars`, and then needs `ARCHIVES_ORIGIN` (both
  without a trailing slash), `CF_ZONE_ID`, and the `CACHE_PURGE_TOKEN`
  secret - an API token with the zone's Cache Purge permission and
  nothing else. From then on a publish pass that found documents and
  then the catalog in INDEX purges them all, and completes no row when
  that purge failed: without the secret every row stays open, and once
  more than 100 packages have open rows the pass no longer reaches the
  rest. A placement also purges its archive's URL and its package's
  document, and a put of `config.json` purges that; when one of these
  fails (`purge: ...`, `configuration publication: ...`) the edge keeps
  its copy until it expires - an hour for `config.json`, which is worth
  purging by hand after a deployment that changed it.
- **Returning to a released reservation.** The same upload id and
  publisher may return within 14 days of the operation's first charge
  (`charged_at`); returning never extends that window. It takes a
  pending slot and the storage again and is not charged the daily
  allowances a second time. An upload id whose operation was rejected
  after it was stored, or whose window has passed, is refused with a
  `409`; any other republish of rejected bytes is a new operation,
  charged in full. A request that finds its own reservation released
  under it is also a `409` (`the upload's reservation expired before
  the archive was stored; retry the upload`): only a later request
  returns.
- **Upload ids are forgotten** 30 days after a row turned terminal:
  the cron clears `upload_id`, `charged_at`, and `charged_class`.
- **Daily allowances** are the rows of `user_daily_upload_usage` and
  `shared_daily_upload_usage`, keyed by UTC date. A new day starts
  from absent rows, so the allowances reopen at 00:00 UTC whether or
  not a cron pass ran; the cron only deletes the rows from before
  yesterday.

What each class allows (`src/quota.rs`; `users.quota_class`, any
unknown name reading as `default`):

| Limit | `default` | `operator` |
| --- | --- | --- |
| Archive size | 16 MiB | 16 MiB |
| Stored bytes per user | 128 MiB | 4 GiB |
| Packages per user | 20 | 10,000 |
| New packages per user per UTC day | 5 | 1,000 |
| Revisions per package per UTC day | 30 | 1,000 |
| Uploaded bytes per user per UTC day | 256 MiB | 4 GiB |
| Uploads per user per UTC day | 30 | 1,000 |
| Uploads pending at once per user, reservations included | 3 | 1,000 |
| Uploaded bytes per UTC day, the whole class | 5 GiB | 4 GiB |
| Uploads per UTC day, the whole class | 500 | 1,000 |

The `operator` class is charged to shared allowances of its own, so
bulk port publishing never spends what `default` accounts share.

**What a rejection gives back.** A verifier's rejection, a released
reservation, a pending timeout, and an archive that could not be placed
each refund the archive's bytes to
`users.stored_bytes` in the transaction that rejects the row, and the
row leaving `pending` frees its slot. The daily allowances are never
refunded: an admitted upload stays charged whatever becomes of it.

**What stays in UPLOADS.** The Worker never deletes an UPLOADS object;
the bucket's 14-day lifecycle rule ("First-time provisioning") does, so the
archive of a rejected, released, or timed-out upload stays until the
rule removes it while nobody is charged for it, and a served archive
is held twice, charged once, until the rule removes its upload. The
breaker's storage figure, `SUM(users.stored_bytes)`, is therefore what
publishers are charged, not what the buckets hold; what UPLOADS holds
is bounded by the classes' shared daily allowances over 28 days - a
resumed upload stores its archive up to 14 days after it was charged,
and the object then lives 14 more - not by `BUDGET_R2_STORAGE_BYTES`.
Do not delete UPLOADS objects by hand: a row may still name one.

A served revision is read from ARCHIVES (`src/glue/read.rs`), which
has no lifecycle rule: the Worker never deletes or replaces an object
there. An archive stays there uncharged, for good, when its revision
is rejected as `publication_source_lost` after one placement pass had
copied it and before that pass recorded it - a second pass found the
upload expired in between; a later publication of the same bytes is
served from that object. INDEX holds the publish pass's documents,
which nothing reads yet: the read plane composes its answers from D1.
Nothing writes DOWNLOAD_LOGS.

## Verification pipeline

The external verifier is the `registry-verify` GitHub Actions workflow
(`.github/workflows/registry-verify.yml`): it builds
`cabin-registry-verify` from the root workspace and runs
`cargo registry-verify`, which lists pending versions through the
admin API, inspects each archive with that binary, and PATCHes the
verdict back. It runs a normal pass every 5 minutes, the private alpha
included - a verdict makes a revision resolvable only to the accounts
the allowlist admits - and when an operator dispatches it (a plain
dispatch is a normal pass too). The
checks and reason codes are
documented in `docs/remote-registry.md` ("The verifier's checks").
The verifier addresses scoped names throughout: the artifact download
nests the directory (`archives/<scope>/<name>/`) and flattens the
filename (`<scope>-<name>-<version>-<revision>.zip`), and the verdict
PATCH carries the `<scope>/<name>/<version>` triple plus the listing's
`checksum`, which is what names the revision.

Two credentials, two planes: the listings and pending downloads ride
a `verify`-scoped registry token the run mints for itself at start -
the trusted-publishing exchange's verifier arm answers the workflow's
own OIDC JWT (audience `cabinpkg.com`) with a 30-minute token, revoked
best-effort at run end - while the verdict PATCH authenticates
**only** a fresh JWT for the verdict audience
(`cabinpkg.com/verifier`), pinned to this repository and workflow by
the `VERIFIER_*` vars in `wrangler.jsonc` (`docs/remote-registry.md`,
"Admin API (scope `verify`)"). No registry token - the exchanged
verify token included - can deliver a verdict, and each JWT
authenticates exactly one request (its `jti` is consumed on use).

Fail-safe: a failed or skipped run leaves versions pending, which only
keeps content unexposed. The breaker cron's stuck-pending webhook
alert ("N revision(s) have been pending verification for over an hour")
is the detection mechanism, never "the workflow ran recently" - GitHub
cron schedules are **best-effort** and can be delayed or dropped under
load. On the alert, dispatch a run:

```sh
gh workflow run registry-verify.yml
gh run list --workflow registry-verify.yml --limit 5
```

The alert's summary carries `verification.verifier_seen_at`
(`meta.verifier_seen_at`), the last time the verifier exchanged its
identity for a token: it tells a verifier that has obtained no token
since (it did not run, or its exchange was refused) from one that
runs and leaves the versions pending. It is not itself an alert.

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
(`docs/remote-registry.md`, "The verifier's checks") and **abstains** on a
finding: no verdict is rendered, the version stays pending, and the
log shows
`<name>@<version>: abstain (<rules>); leaving it pending for operator review`
with the rules (`confusable_package (fmtlib/fmt)`,
`confusable_scope (...)`, `near_name (...)`, `profanity`). Abstain
does not fail the run, and every later pass re-logs it - that is
by design; the stuck-pending alert ("N revision(s) have been pending
verification for over an hour") is the summons. Verdicts require the
verify workflow's own OIDC identity (see above) - a manual `curl`
with the verify token does not authenticate - so resolution is a
`workflow_dispatch` of `registry-verify.yml` naming the version:

- Name is fine - waive the advisories for that one version:

  ```sh
  gh workflow run registry-verify.yml -f resolve='<scope>/<name>@<version>'
  ```

  The dispatched run skips only the name advisories and still runs
  every real check against the archive; the verdict delivered is
  whatever the verifier renders - never `verified` from the name
  alone. With a verified version on record the name counts as
  accepted, and every later version of the package skips the
  advisories.

- Name is not fine - deliver the advisory rejection:

  ```sh
  gh workflow run registry-verify.yml \
    -f resolve='<scope>/<name>@<version>' \
    -f resolution=reject -f reason='<rule>'
  ```

  The run PATCHes `{"verdict":"rejected","reason":"name_advisory:
  <rule>", ...}` bound to the listing's `checksum` and `published_at`
  (the checksum selects the revision, the publish stamp pins the
  generation; a republish in between is a `409` and a re-dispatch).
  Rejection frees the bytes and the publisher can republish under a
  better name. A rejection never vets the name: republishing the same
  name abstains again and re-summons you - by design, not a loop to
  "fix".

A resolution dispatch touches exactly the named version - every other
pending version waits for a normal pass - and a version the listing
no longer reports fails the run without a verdict. A dispatch still
queued behind a running pass is superseded when the next scheduled pass
queues (`cancelled` in `gh run list`); dispatch it again. If two revisions of
the same version are pending at once (a `new-revision` republish while
the first still pends), the run refuses rather than pick one; re-dispatch
with the revision selector, `-f resolve='<scope>/<name>@<version>#<revision>'`,
copying the revision from the refusal message or the pending listing.

**Name fidelity knobs.** The reserved-name list is an in-code,
operator-maintained const (`registry/src/names.rs`); extend it when
the project starts speaking a new name, never shrink it. The claim
flow's skeleton confusability refusal has one override:
`CLAIM_SKELETON_EXEMPT_SCOPES` (`wrangler.jsonc` `vars`,
comma-separated **exact** scope names) admits a listed scope past the
confusability check only - reserved names and claim permanence always
hold. Set it just before the legitimate claimant walks the claim
flow, and empty it after.

The workflow carries no registry-token secret. Each run's listings
credential is minted through the trusted-publishing exchange (the
verifier arm, `docs/remote-registry.md`, "Exchanging an Actions OIDC
token") and revoked at
run end, so there is nothing to rotate, nothing a wipe invalidates,
and nothing to re-provision: the baseline migration seeds the backing
identity (`VERIFIER_BACKING_ACCOUNT_ID` in `wrangler.jsonc`).
Dispatching the workflow with `check_oidc: true` proves the whole
credential path - the `id-token` grant, the exchange, and the
revocation - without verifying anything. The local operator flows are
the exception: `cargo registry-governor` and `cargo registry-diagnose`
still read a manually minted token from
`CABIN_REGISTRY_TOKEN` - the operator's own login-session token
(`docs/remote-registry.md`), the one session shape that carries the
`verify` scope these flows need.

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

The registry keeps no backup of its own, of the archives or of D1.

**D1 metadata.** The database's recovery path is D1 Time Travel, the
platform's point-in-time restore. Its retention and commands are
Cloudflare's to document: read the current D1 documentation when a
restore is needed. A deleted database, or damage older than that
retention, has no recovery path; that loss is an accepted risk. A
restore moves D1 alone, so afterwards check the restored rows against
UPLOADS and ARCHIVES, and after restoring to a point that predates
launch, immediately re-run the launch-checklist `UPDATE` that
sets `meta.launched = 'true'`, or the launch guard would treat the
restored registry as pre-launch and let a wipe through. INDEX is not
restored either: its documents then record generations the restored
database has not reached, and the publish pass leaves a document alone
for a change the database numbers at or below the generation that
document records - the change's row completes all the same. Nothing
reads INDEX yet.

**Bucket data loss** (a bucket deleted or its objects destroyed): there
is no second copy, and a byte-identical re-publish does not bring an
archive back - for a revision already recorded as stored it is the
`200` no-op and writes nothing. This is an accepted risk.

**Known limitation - account-level compromise.** Everything above lives
in one Cloudflare account: a compromised operator account or a hostile
account closure takes all of it. This is an accepted risk; no
off-account copy is kept.

## Quota tallies

`users.package_count` and `users.stored_bytes` are what the publish
preflight and the admission gates read for
the per-user package and storage quotas. They are exact by
construction, not caches: each is the count of the user's
`created_by` packages or the `archive_size` sum of the user's
`published_by` revisions that are not rejected - reservations
included - and every batch that
writes such a row adjusts the tally in the same transaction: an
admission adds, and a verifier's rejection, a released reservation, and
a pending timeout each refund the row's
publisher, while a write its gate refuses counts nothing. A
fresh schema starts both at zero over empty tables, so a wipe needs no
backfill. The daily package and revision quotas have no tally: each
count is an index range
over one UTC day's rows (`packages_created_by`,
`revisions_published_at`), which the daily quota itself bounds, so a
publish's reads never grow
with the account's or the package's history. The daily upload
allowances are the two usage tables of "Upload lifecycle". Nothing
should ever make
the tallies drift; this check, which reads
every package and revision once, must return no rows:

```sh
node_modules/.bin/wrangler d1 execute DB --remote --command "
  SELECT id, package_count, stored_bytes FROM users u
  WHERE package_count != (SELECT COUNT(*) FROM packages WHERE created_by = u.id)
     OR stored_bytes != (SELECT COALESCE(SUM(archive_size), 0) FROM revisions
                         WHERE published_by = u.id AND verification != 'rejected');"
```

A row it returns is a bug to investigate first; the repair recomputes
the tallies from the rows:

```sh
node_modules/.bin/wrangler d1 execute DB --remote --command "
  UPDATE users SET
    package_count = (SELECT COUNT(*) FROM packages WHERE created_by = users.id),
    stored_bytes = (SELECT COALESCE(SUM(archive_size), 0) FROM revisions
                    WHERE published_by = users.id AND verification != 'rejected');"
```

## Logs

`wrangler tail` (or the dashboard). One line per request:
`req=<id> method=<m> path=<p> status=<s> token=<token-row-id|->`. Tokens and
token hashes are never logged. The dashboard's stored logs are a sample:
`wrangler.jsonc` sets `head_sampling_rate` to 0.1, so Workers Logs keeps
about one request in ten, and every line of a request it drops - an
internal error or a refusal reason included - is absent.

For an incident report or a bug thread, `cargo registry-diagnose`, run from
the repository root, gathers
the shareable aggregate state in one pass - config and stamp status,
service mode, corpus counts and the pipeline's oldest waits, the
governor snapshot (with
`CABIN_REGISTRY_TOKEN`), and the deployment list - and deliberately
prints no tokens, object keys, content checksums, package names, or
user data.
