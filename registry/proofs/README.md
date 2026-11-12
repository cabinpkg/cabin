# Registry lifecycle model

`RegistryLifecycle.tla` is a TLA+ specification of the hosted registry's
publish / verification lifecycle for one package, checked with the TLC
model checker. It covers what the Kani harnesses (single functions) and
the SQL validation tests (single statements) cannot: how the admission
batch, the UPLOADS writes, the confirmation, and the cron's sweeps
interact under concurrent publishes, retries of one upload, respins,
verdicts, rejections, revivals, released and resumed reservations,
timeouts, lost races, and requests that die between two steps.
The external verifier is a participant: its listing, artifact download,
digest check, archive inspection (an oracle), operator resolutions, and
verdict `PATCH` progress on their own schedule against the server's
rules.

CI runs the configurations marked below whenever the model, its
configurations, or the witness-check xtask change
(`.github/workflows/registry.yml`, jobs `tlc-respins`, `tlc-shared`
and `tlc-scan`, one configuration each, gated by the `tlc` list in
`.github/path-filters.yml` and feeding the required `registry-build`
aggregate). A missing tool, a digest mismatch, a violation, or a job
timeout fails the check. `tlc-scan` then runs each witness trap alone,
where the reverse holds: a trap TLC does not report violated fails it
("Non-vacuity").

## Running it

```sh
cd registry/proofs
curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
  --output tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar
sha256sum --check tla2tools.jar.sha256
java -XX:+UseParallelGC -jar tla2tools.jar -workers auto -cleanup -config RegistryLifecycle.cfg RegistryLifecycle.tla
java -XX:+UseParallelGC -jar tla2tools.jar -workers auto -cleanup -config RegistryLifecycleShared.cfg RegistryLifecycle.tla
java -XX:+UseParallelGC -jar tla2tools.jar -workers auto -cleanup -config RegistryLifecycleScan.cfg RegistryLifecycle.tla
```

The pinned tool is the TLA+ `v1.7.4` release (TLC 2.19, 2024-08-08); its
`tla2tools.jar` digest is `tla2tools.jar.sha256`, and the release page's
SHA-1 for the same file is `bee4a54f3ee3d4afc347c3240ec2d9e93b075104`. Any
Java 11 or later runs it. TLC exits 0 only when the check completed with
no violation.

## What is modeled

The model follows the registry sources as of the commit this file ships
in (`registry/src/glue/bearer/package.rs`, `registry/src/glue/bearer/verifier.rs`,
`registry/src/glue/cron.rs`, `registry/src/upload.rs`,
`registry/src/verify.rs`, `registry/src/sql/admission.rs`,
`registry/src/sql/packages.rs`, `registry/src/sql/quota.rs`,
`registry/src/sql/downloads.rs`,
`registry/migrations/0001_init.sql`) and the verifier
(`crates/xtask-registry-admin/src/verify.rs`, `crates/cabin-registry-verify/src/lib.rs`,
`.github/workflows/registry-verify.yml`).

One package; `Versions` version slots; `Checksums` distinct archive
contents; `Users` publishing accounts; `Uploads` upload ids. A revision
row is keyed by `(version, checksum)`, because the packaging-revision id
is the checksum's leading hex and names the same row (the astronomically
unlikely prefix collision, answered `409` in `upload::dispose`, is
excluded). Each content's `archive_size` is the number that names it
(`Size(c) == c`), so the stored-bytes tallies are discriminating.

**What a publish request decides to attempt is left free.** In
production `upload::dispose` reads the version's rows and picks one
outcome - answer, converge, or one of the three admissions - and by the
time the request writes, that read may be stale. The model does not
reproduce the choice: a request may run any admission gate, as any
user, for any bytes, under any upload id, and may converge on any
reservation it read. Every property below therefore holds whatever the
handler decides; the D1 gates and a request's store-then-confirm order
carry them. Which outcome `dispose` picks for which rows is
`registry/src/upload.rs`'s own unit tests' claim, and a retry of an
upload is simply another request.

State:

| Variable | Production state |
| --- | --- |
| `status[v][c]` | `revisions.verification`, or `absent` when no row exists |
| `pubAt[v][c]` | `revisions.published_at`: the clock sample of the admission that wrote the row's current generation - a new revision, a revival, or a resumed reservation |
| `clock` | the greatest stamp sampled so far |
| `spare` | the stamps requests sampled and have not committed: a request samples once, before it reads any revision row, and may write much later or never |
| `stored[v][c]` | `revisions.stored_at IS NOT NULL`: a `pending` row without it is a reservation, a `rejected` row without it a released one |
| `opOf[v][c]` | `revisions.upload_id`: the upload operation the row is bound to |
| `owner[v][c]` | `revisions.published_by` |
| `storedBytes[u]` | `users.stored_bytes` |
| `dailyUploads[u]` | the `uploads` count of the user's `user_daily_upload_usage` row, for the one UTC day modeled |
| `object[o]` | the UPLOADS object `uploads/<o>.zip`: absent, or the content it holds |
| `pub[p]`, `ver[w]`, `cron` | what a publish handler holds between its admission (or the read it converges from) and its confirmation, a verifier run between its listing and its `PATCH`, and the cron's reservation pass between its object check and its write |
| `gen[v][c]`, `decided[v][c]` | ghost history for the properties - the row's publish-event count, and the generation its terminal state was decided against; no guard reads them |
| `basis[v][c]` | ghost: what turned the row terminal - the inspector (`inspected`), an operator's reject resolution (`manual`), the cron releasing a reservation (`released`), or the pending timeout (`timeout`); `none` while pending |

### Actions and the code they abstract

| Action | Rust | D1 / R2 |
| --- | --- | --- |
| `Sample` | `Preflight.now` as the admission sees it: the sample `publish` takes once it holds the body | none |
| `PubAdmit` | `package.rs` `admit_upload`, when its gate changes a row (a gate that changes none leaves no trace: the request decides again or answers) | `ADMIT_NEW_UPLOAD` (the gate, which is the user's daily charge), `CHARGE_SHARED_DAILY_UPLOAD`, `COUNT_ADMITTED_UPLOAD`, `ENSURE_PACKAGE`, `ENSURE_VERSION`, `INSERT_ADMITTED_REVISION`; or `REVIVE_REJECTED_UPLOAD` (the gate; `published_at < ?1`: the stamp must advance), `CHARGE_USER_DAILY_UPLOAD`, `CHARGE_SHARED_DAILY_UPLOAD`, `COUNT_USER_STORED_BYTES`; or `RESUME_RELEASED_UPLOAD` (the gate, with the same stamp rule), `COUNT_USER_STORED_BYTES` - one transaction, the gate evaluated against the rows as they are then, every later statement applying only by `changes()` |
| `PubConverge` | `attempt`'s `Converge` arm: the read of a reservation with the request's bytes | `EXISTING_REVISIONS`: snapshots `(upload id, published_at)` |
| `PubStore` | `store`: the put under the reservation's key, after a head when the operation resumes or converges (an object already there is kept) | `bucket.head`, `bucket.put` on `uploads/<upload id>.zip` |
| `PubConfirm` | `confirm`, `changed_rows == 0` as the reservation's generation being gone | `CONFIRM_UPLOAD_STORED`, guarded on the upload id and the `published_at` the request stored for |
| `PubDrop` | the request ends after its admission: a failed put or batch, or the Worker dying | none |
| `CronList` | `cron.rs` `settle_reservations`: one listed reservation and its object check | `EXPIRED_RESERVATIONS`: snapshots `(upload id, published_at)`; `bucket.head` |
| `CronSettle` | the confirmation or the release | `CONFIRM_UPLOAD_STORED`, or `RELEASE_EXPIRED_RESERVATION` then `REFUND_USER_STORED_BYTES` - one transaction, guarded on the listed generation |
| `CronTimeout` | `cron.rs` `sweep_uploads` | `REFUND_TIMED_OUT_UPLOADS` then `TIME_OUT_PENDING_UPLOADS` - one transaction over the stored `pending` rows |
| `CronForget` | the same batch | `PRUNE_SETTLED_UPLOAD_IDS`: a terminal row's `upload_id` is cleared, which frees the id |
| `VerList` | one verifier run over one listed entry: `admin_versions_response`, then the driver's `Pass::version` or `Pass::resolve` ("The verifier" below) | `REVISIONS_BY_VERIFICATION_STATUS('pending')`, which leaves reservations out: snapshots `(checksum, published_at)`; the artifact download reads the row's object |
| `VerApply` | the `PATCH`: `verdict_authn` (the run's OIDC JWT, single-use `jti`; not modeled), `verdict_response`, `apply_verdict`, `changed_rows == 0` as the lost race; a failed batch | `MARK_REVISION_VERIFIED`, or `MARK_REVISION_REJECTED` then `REFUND_USER_STORED_BYTES` - one transaction, `checksum = ?` and `published_at = ?` bound to the body's values (the listing's binding) |

The verdict handler's own read (`VERDICT_TARGET`) and
`verdict_binding_matches` can only refuse a `PATCH`, so the model lets
every `PATCH` a run renders reach the guarded update, which binds the
same values. The guarded updates do not test `stored_at`: `VerList`'s
`stored` conjunct stands for the listing's condition and for
`VERDICT_TARGET`'s, to which a reservation is a missing row. No action
deletes an object or a row.

The read plane is modeled as the two predicates the invariants use:
`Current(v)` is the `current_revisions` view (the verified revision with
the greatest `(published_at, revision)`), and `ArtifactReadable` is
`verify::artifact_readable` over the stored rows
(`ARTIFACT_BY_REVISION`'s `stored_at IS NOT NULL`).

### The verifier

`cargo registry-verify` (the `registry-verify.yml` workflow's verify
step) is the only writer of verdicts in production: the verdict route
accepts the OIDC identity of that workflow alone. One `VerList` step is
one run's work on one listed entry; what the driver does between the
listing and the `PATCH` maps as follows.

| Driver step (`xtask-registry-admin/src/verify.rs`) | Model |
| --- | --- |
| `exchange`: mint an OIDC JWT for the exchange audience, trade it for a verify-scoped registry token | not modeled (see "Assumptions") |
| `GET /api/v1/admin/versions?status=pending` with that token | the listing snapshot `(checksum, published_at)` of a stored pending row |
| `name_advisories` (`cabin-registry-verify --name-advisories`): `abstain` leaves the row pending for an operator | the run ends without a `PATCH` (no step) |
| `download` of the artifact by revision with the same token; a failure leaves it pending | the row's object must hold content, else the run ends without a `PATCH`; `bytes` is what the object holds |
| `undeclared_digest`: the bytes must hash to the listed checksum, else pending | `bytes = c` |
| the optional upstream fetch; a stored cleartext URL leaves it pending, a failed download continues without the file (the inspector can still reject, and a version whose metadata pins an upstream then fails operationally: pending) | folded into the inspector's result |
| `cabin-registry-verify <archive> <entry>`: `verified` or `rejected(reasons)`; an operational failure or an unparsable answer leaves it pending | a verdict from `Inspections(bytes)`: only `rejected` for `bytes \in Malicious`, either verdict otherwise; failures end the run without a `PATCH` |
| `Minter::mint` a fresh OIDC JWT for the verifier audience, then `PATCH` the verdict with the listing's `checksum` and `published_at` | the `decided` request, served by `VerApply` |
| `Pass::resolve`, `verify`: the same pass with the advisory gate waived | the inspected arm (waiving the gate changes no later step) |
| `Pass::resolve`, `reject`: `PATCH` `rejected` with `name_advisory: <rule>` (the rule from the dispatch input, the `checksum` and `published_at` binding from the listing entry), inspecting nothing | the `manual` arm |

The inspector is an oracle. `Malicious` names the contents its archive
scan rejects whatever else is true (traversal, bombs, forbidden zip
features: `scan.rs`); for every other content the verdict is left free,
because it also depends on the generation's stored metadata - a revival
of the same bytes may carry corrected metadata (`REVIVE_REJECTED_UPLOAD`
replaces `metadata_json`), so identical bytes can be rejected and later
verified - on the bytes the pinned upstream host serves that run, on the
listing row's own `name` and `version` (the archive's manifest must
agree, so one content verifies under at most one version: the
same-bytes configuration, where the model lets both rows verify, admits
more than production, which is safe for the properties checked), and on
the caps the workflow's repository variables set. Whether
`cabin-registry-verify` rejects what it should is its own test suite's
claim (`crates/cabin-registry-verify/tests/verify.rs`, and `advise.rs`
for the advisory gate, which is a stutter here), not this model's. The
digest check appears twice in production - the driver's
`undeclared_digest` before the inspector runs, and the inspector's own
hash of the archive against the listing (`checksum_mismatch_is_rejected`)
- and `bytes = c` stands for both.

The listing and the pass are one step although the driver performs them
minutes apart. A `PATCH` lands only while the row is still pending at the
listed stamp, and a generation keeps its upload id, so at the download
instant of any `PATCH` that lands the merged step taken then snapshots
the same stamp, reads the same object, and renders the same request. A
`PATCH` the server refuses changes nothing in either shape. A run whose
download finds no object ends without a `PATCH`, like every other
failure. The driver's parsing of the listing and of the child answers,
its digest comparison, the advisory gate's scope, and the resolution
lookup are unit tests in `crates/xtask-registry-admin/src/verify.rs`
(`only_the_canonical_checksum_of_the_downloaded_bytes_matches`,
`the_advisory_gate_runs_only_on_a_normal_walk`,
`a_resolution_finds_exactly_the_named_version`, the `answer` and
`read_stream` tests); `tests/verify_guards.rs` covers the environment and
dispatch guards that fail a run before it lists anything. The model takes
their outcomes as given.

### Assumptions

- **The clock is trusted only where a repeated stamp is harmless.**
  `published_at` is `now_iso8601()`, sampled once per request with the
  body in hand, before it reads any revision row, and reused by its
  second pass, so a slow request commits with an older stamp than a
  faster one that sampled later; the model reproduces
  that with `spare` (across rows, commit order need not follow stamp
  order, and `current_revisions` serves by stamp, not commit order).
  Within one row the stamp is the generation identity, and the model
  does not assume the clock makes it unique: a request returning to a
  rejected row may hold that row's current stamp again (two publish
  events of one row in one millisecond, or Worker clocks that disagree
  by at least the gap between them), and the `published_at < ?1` of
  `REVIVE_REJECTED_UPLOAD` and `RESUME_RELEASED_UPLOAD` refuses it;
  the `ReviveGate` stamp row under "Sensitivity" shows the revival's
  to be load-bearing. The guards otherwise compare the stamp as the SQL
  compares the column; no generation counter replaces the production
  identity. Equal stamps across different rows would only reach the
  `(published_at, revision)` tie-break of `current_revisions`, which
  the model does not exercise.
- **Time.** No duration is modeled. The reservation sweep may take any
  reservation and the timeout any stored `pending` row at any moment,
  and `CronForget` any terminal row, which admits every schedule the
  fifteen-minute, fourteen-day, and thirty-day thresholds allow and
  more. `RESUME_RELEASED_UPLOAD`'s retry window only ever refuses a
  resume, so it is left out. UPLOADS objects never expire in the model:
  the bucket's fourteen-day rule counts from the put, `stored_at` is
  never later than the object's own upload time (`store`, and the
  cron's confirmation, which records the object's), and the timeout
  rejects a stored upload fourteen days after `stored_at` - so a row is
  rejected no later than a cron period after its object can first
  expire, and a verifier run that finds the object gone in that window
  ends without a `PATCH`, which is the stutter.
- **An upload id reused for other bytes** is the one way an object can
  disagree with the row that names it, and the model keeps it reachable
  rather than assuming it away (`NoStaleObject` and
  `NoVerifiedObjectReplaced` under "Non-vacuity"). An id leaves its row
  when the row is revived under another id or thirty days after the row
  turned terminal, and a client that then sends the same id with
  different bytes meets one of two forms. In the first, a head-first
  path - the cron's confirmation, a resume, a convergence - finds the
  earlier object and confirms the new row stored. A new operation's own
  put replaces the object when it lands; if it never does, the row's
  upload holds the wrong bytes, the digest check keeps it from being
  verified (`VerifiedByInspection`), and it times out, which costs only
  that client's own upload. In the second, a put of the earlier
  operation that is still outstanding lands after a run downloaded the
  new row's object, and replaces it: the row is verified on the bytes
  the run inspected while its object holds others. No property here
  covers a verified row's object, and the Worker serves that object for
  as long as verified archives stay in UPLOADS. It takes the reused id
  and a put outstanding across a release and a whole verification,
  which no client controls. Cabin draws a fresh random id for each
  upload, so both forms take a client that breaks that rule.
- One package: `ENSURE_PACKAGE`, `ENSURE_VERSION`, and the `-`/`_` twin
  guard always succeed; the twin `400` is unreachable.
- One UTC day and one quota class. Of an admission's conditions the
  model keeps the opt-in, the unused revision key and upload id, the
  publisher's stored bytes (`StorageLimit`), the user's daily upload
  count (`DailyLimit`), and the user's pending count (`PendingLimit`).
  The daily byte allowance, the shared rows, the package quotas, and
  the package's daily revisions are further conjuncts of the same gate
  (`tests/sql_validation/admission.rs` checks each, and that a gate
  changing no row leaves every later statement of its batch changing
  none).
- Everything that ends a publish before its admission - the write gate,
  scope membership, rate limiting, the size check, the quota preflight,
  a governor refusal, a lost gate, every `200` no-op and `409` - writes
  nothing to the modeled state and is the stutter. The governor is
  asked before the admission, so no admitted operation is refused its
  store. A governor refusal of the cron's object check ends that pass,
  which is the stutter too.
- Collapsed steps: the head and the put of one `store` call are one
  step, as are the cron's listing and its object check, and a timeout
  or a prune takes one row per step where production takes every due
  row in one statement. None of the checked properties looks between
  those steps.
- The verifier's runs that end without a `PATCH` (abstain, download or
  digest or upstream or inspector or mint failure) change nothing on the
  server, so they are the stutter; a run that does `PATCH` is one
  `decided` request. The model lets two runs overlap and a run's `PATCH`
  arrive arbitrarily late, which over-approximates the workflow's
  `concurrency` group (one run at a time): the server's guards are what
  is checked, not the group. One cron pass runs at a time.
- Credentials are not modeled. The listing and the download carry the
  exchanged verify-scoped token (`has_verify_scope`); every `PATCH`
  carries a freshly minted OIDC JWT whose `jti` `verdict_authn` consumes
  in its own transaction before the business logic, so a replayed `PATCH`
  is refused before any read (the exchange consumes its own JWT's `jti`
  too, coupled to the token mint). That split is the structure of the workflow and the
  handlers, not a property TLC can establish here; `registry/src/trustpub.rs`
  and the `check_oidc` diagnostic test it.
- `MaxStamps` bounds the stamps one execution samples. A sample no
  admission commits stays in `spare`, so the bound counts samples, not
  rows; with the clock spent and no sample left an execution may have
  nothing more to do, which the specification allows as its own end
  rather than a deadlock.

## Properties

Checked as `INVARIANTS` and action `PROPERTIES` on every state and step
of every configuration:

| Property | Statement |
| --- | --- |
| `TypeOK`, `RowsStamped` | domains; a row exists iff stamped, and stamps are unique (a sample is committed at most once and the stamp guards refuse the repeated one - a sanity check on the model, not a production property) |
| `PublicReadsAreVerified` | without the `verify` scope, an artifact is readable only when `verified`; the served pointer names a verified row (both follow from the two read predicates' definitions: this documents the gate rather than tests the write path) |
| `TerminalDecidedOnCurrentGeneration` | a `verified` or `rejected` row was decided - by a verdict, a release, or the timeout - against its current generation (the ghost `gen`, not the stamp): no delayed verdict or release lands on a revival or a resume of the same bytes |
| `PendingUndecided` | a pending row carries no decision and no basis (every admission clears both) |
| `VerifiedByInspection` | a `verified` row was rendered by the inspector's answer on the listed bytes, never by the `reject` resolution and never on bytes the scan rejects (the abstain and failure outcomes are the stutter, so this constrains the two arms that `PATCH`) |
| `VerdictOnlyOnStored` | a verdict and the timeout land only on stored uploads, and only a reservation is released: a verified row is stored, a row whose basis is `inspected`, `manual`, or `timeout` is stored, and one whose basis is `released` is not |
| `UploadIdNamesOneRow` | a pending row names its operation, and no two rows are bound to one upload id (in D1 the unique index on `upload_id` holds the second whatever the gates do; the model checks that the gates never rely on it) |
| `StoredHasObject` | a stored upload awaiting its verdict has an object under its upload id |
| `StoredBytesExact` | each user's `stored_bytes` equals the archive sizes of the live (pending or verified, reservations included) rows they published - the same bytes on two versions twice - in every D1-visible state |
| `WithinLimits` | no user's pending rows (reservations included) exceed `PendingLimit`, no user's daily count exceeds `DailyLimit`, and no user's stored bytes exceed `StorageLimit`, however admissions interleave |
| `VerifiedImmutable` | a verified row never changes status or stamp |
| `RejectedOnlyRevives` | a rejected row only ever returns to pending (never to verified) |
| `LiveGenerationStable` | a live row is never restamped, moved to another publisher, or un-stored, and a pending row keeps its upload id: no publish, retry, convergence, or revival rewrites it |
| `ServedChangesOnlyByVerification` | what a version serves changes only when a row becomes verified - a pending respin, a rejection, a release, or a revival never disturbs it |
| `RespinNeedsOptIn` | a different-bytes revision joins a live sibling only through a request that carried `new-revision=true` |
| `ChargedOncePerOperation` | a user's daily count moves by exactly one for each row bound to an upload id it was not bound to: a resumed operation is not charged again, and nothing refunds a charge |
| `SpentIdStaysSpent` | an upload rejected after it was stored - by a verdict or the timeout - returns to pending only under an upload id other than the one it holds, which `ChargedOncePerOperation` charges; only a reservation that never stored resumes for free |

`PendingUndecided` and `VerifiedByInspection` follow from how the
actions are written; they pin the model's shape against later edits
rather than test a rule of the server. `VerifiedByInspection` in
particular is a fact about the driver as modeled, not a server rule:
the verdict route cannot tell an inspected verdict from any other
`PATCH` by the pinned OIDC identity, so whoever holds that identity can
verify anything, and no change to the transition table or the guarded
updates can make this invariant fire. The remaining properties are
stated over the rows and the ghost history, not over the actions' own
guard expressions, so weakening a guard breaks them (see
"Sensitivity"). Of those, `RespinNeedsOptIn`, `WithinLimits`,
`SpentIdStaysSpent`, `ServedChangesOnlyByVerification`, and the timeout
and release halves of `VerdictOnlyOnStored` each follow step by step
from one atomic gate: they are tripwires for a weakened guard and need
no interleaving to hold, where the rest depend on how the handlers'
steps interleave.

## Configurations and results

All runs are exhaustive breadth-first checks under the pinned TLC with
symmetry over `Versions`, `Users`, `Uploads`, `Publishers`, and
`Verifiers`; "distinct" is the number of states checked after symmetry
reduction.

| Configuration | Constants | Distinct / generated states | Depth | Result | Local time (10 cores) |
| --- | --- | --- | --- | --- | --- |
| `RegistryLifecycle.cfg` (CI) | `Versions = {v1}`, `Checksums = {1, 2}`, `Users = {u1}`, `Uploads = {i1, i2}`, `Publishers = {p1, p2}`, `Verifiers = {w1}`, `MaxStamps = 3`, `Malicious = {}`, `PendingLimit = 1`, `DailyLimit = 2`, `StorageLimit = 3` | 862,832 / 5,316,433 | 26 | complete, no violation | 2 min 27 s |
| `RegistryLifecycleShared.cfg` (CI) | `Versions = {v1, v2}`, `Checksums = {1}`, `Users = {u1, u2}`, `Uploads = {i1, i2}`, `Publishers = {p1, p2}`, `Verifiers = {w1}`, `MaxStamps = 3`, `Malicious = {}`, `PendingLimit = 1`, `DailyLimit = 2`, `StorageLimit = 2` | 2,714,787 / 17,960,678 | 26 | complete, no violation | 5 min 6 s |
| `RegistryLifecycleScan.cfg` (CI) | `Versions = {v1}`, `Checksums = {1, 2}`, `Users = {u1}`, `Uploads = {i1, i2}`, `Publishers = {p1, p2}`, `Verifiers = {w1, w2}`, `MaxStamps = 2`, `Malicious = {2}`, `PendingLimit = 2`, `DailyLimit = 2`, `StorageLimit = 2` | 297,450 / 2,041,850 | 23 | complete, no violation | 1 min 20 s |

The first configuration exercises respins (two contents of one version,
both of which the inspector may verify, so the served revision switches),
revivals beside a live sibling, the opt-in guard, and one upload's
retries, releases, and resumes, with one verifier run at a time. There
the pending limit and the daily count bind (one pending upload at a
time, two charged uploads of three samples) and the storage cap holds
both contents exactly. The second has the same bytes under two versions
and two publishing accounts: two uploads with an object and a charge
each, where a rejection or a release must refund its own row's
publisher and no other. The third has the content the archive scan
rejects (`Malicious`) beside one it may verify, with two concurrent
runs - the one configuration where two verdicts race for a row - and a
storage cap the two contents do not fit under together. In the first two the
inspector's rejections are of bytes it may also verify, and operator
resolutions reject as well. All three are exhaustive finite-state
checks of the model, not simulations and not proofs of the unbounded
system. The local times were taken on a machine busy with other work,
so an idle run is faster. An idle run of an earlier model's first
configuration took 2 min 3 s locally and 6 min 30 s in the merge-group
run of #1932; each `tlc-*` job's 30-minute timeout is sized against
that ratio.

## Non-vacuity

`RegistryLifecycle_witness.cfg` lists traps: each is the negation of a
behavior the model must be able to reach, so TLC must report it
**violated**, and the printed trace is the witness. Its constants are
`RegistryLifecycleScan.cfg`'s with `StorageLimit = 3`, so both contents
can be live at once. `RegistryLifecycleShared_witness.cfg` holds
`NoSameBytesTwice`, which needs two versions carrying one checksum. TLC
stops at the first violation, so the `tlc-scan` job runs each trap
alone, and fails if TLC does not report one violated. From the
repository root:

```sh
cargo check-tlc-witnesses --jar registry/proofs/tla2tools.jar \
  registry/proofs/RegistryLifecycle.tla \
  registry/proofs/RegistryLifecycle_witness.cfg \
  registry/proofs/RegistryLifecycleShared_witness.cfg
```

To see one witness's trace, keep only its line in a copy of the
configuration. The lengths below are the states of the trace the
multi-worker search printed (not necessarily the shortest):

| Trap | Witnessed behavior | States |
| --- | --- | --- |
| `NoVerified` | a revision is admitted, stored, confirmed, verified, and served | 7 |
| `NoRevivedVerified` | a rejected revision returns - a revival or a resume - and is then verified | 11 |
| `NoStaleVerdictRefused` | a verdict is refused by its guarded update: the row was decided, or returned under a new stamp, since the listing | 9 |
| `NoLostConfirm` | a request stores for a reservation that is released, or taken again, before it confirms | 8 |
| `NoRespin` | two revisions of one version are live at once | 5 |
| `NoSameBytesTwice` | the same bytes are live under two versions, as two uploads | 5 |
| `NoConvergence` | a request completes a reservation another request left unstored, and is charged nothing | 6 |
| `NoCronConfirm` | the cron confirms a reservation whose request died after its put | 6 |
| `NoRelease` | the cron releases a reservation whose object never arrived, and refunds its bytes | 7 |
| `NoResume` | the released operation returns under its own id, is not charged again, and is verified | 11 |
| `NoLateObject` | a put lands under a reservation the cron already released (a resume then finds the object) | 6 |
| `NoStaleObject` | the upload-id reuse corner ("Assumptions"): a stored upload whose object holds other bytes | 12 |
| `NoVerifiedObjectReplaced` | the corner's second form: a late put under the reused id replaces a row's object after a run downloaded it, and the verified row's object holds other bytes | 13 |
| `NoTimeout` | a stored pending upload times out | 6 |
| `NoInspectedRejection` | the inspector's rejection lands | 7 |
| `NoManualRejection` | an operator's reject resolution lands on bytes the scan accepts, nothing inspected | 7 |

## Sensitivity

Each mutation below was applied alone, the configurations re-run until
one reported a violation, and the model restored; the unmodified
properties must detect the weakened guard. The last two columns are
what TLC reported and in which configuration (the first violation a
breadth-first search reaches, so another property may also fail).

| Mutation | Weakened guard | Reported violation | Configuration |
| --- | --- | --- | --- |
| `VerApply`'s guard drops `pubAt[v][c] = ver[w].stamp` | `MARK_REVISION_*` without `published_at = ?` | `TerminalDecidedOnCurrentGeneration` | `RegistryLifecycleScan.cfg` |
| `VerApply`'s guard drops `status = "pending"` | `MARK_REVISION_*` without `verification = 'pending'` | `StoredBytesExact` | `RegistryLifecycleScan.cfg` |
| `ReviveGate` drops `pubAt[v][c] < stamp` | `REVIVE_REJECTED_UPLOAD` without `published_at < ?1`: a revival holding the rejected generation's stamp lands, and a verdict listed against that generation lands on it | `TerminalDecidedOnCurrentGeneration` | `RegistryLifecycleScan.cfg` |
| `ReviveGate` drops the rejected test | `REVIVE_REJECTED_UPLOAD` without `verification = 'rejected'` | `StoredBytesExact` | `RegistryLifecycleScan.cfg` |
| `GateCommon` drops the opt-in conjunct | the gates without the `?9 OR NOT EXISTS` guard | `RespinNeedsOptIn` | `RegistryLifecycle.cfg` |
| `GateCommon` drops the pending-count conjunct | the gates without `COUNT(*) ... < ?20` | `WithinLimits` | `RegistryLifecycle.cfg` |
| `GateCommon` drops the storage conjunct | the gates without `stored_bytes + ?3 <= ?12` | `WithinLimits` | `RegistryLifecycleScan.cfg` |
| `NewGate` drops the daily-count conjunct | `ADMIT_NEW_UPLOAD` without `uploads < ?17` | `WithinLimits` | `RegistryLifecycle.cfg` |
| `NewGate` drops `~Bound(i)` | `ADMIT_NEW_UPLOAD` without `NOT EXISTS (... upload_id = ?10)` | `UploadIdNamesOneRow` | `RegistryLifecycleScan.cfg` |
| `ReviveGate` drops `~Bound(i)` | `REVIVE_REJECTED_UPLOAD` without `NOT EXISTS (... upload_id = ?10)`: a rejected row is revived under its own id | `ChargedOncePerOperation` | `RegistryLifecycleScan.cfg` |
| a revival keeps `stored` | `REVIVE_REJECTED_UPLOAD` without `stored_at = NULL` | `StoredHasObject` | `RegistryLifecycleScan.cfg` |
| `ResumeGate` drops `~stored[v][c]` | `RESUME_RELEASED_UPLOAD` without `stored_at IS NULL`: an upload a verdict rejected returns uncharged | `SpentIdStaysSpent` | `RegistryLifecycleScan.cfg` |
| `ResumeGate` drops `owner[v][c] = u` | `RESUME_RELEASED_UPLOAD` without `published_by = ?2` | `StoredBytesExact` | `RegistryLifecycleShared.cfg` |
| `ResumeGate` drops `opOf[v][c] = i` | `RESUME_RELEASED_UPLOAD` without `upload_id = ?10`: a released reservation whose id was cleared returns to pending with none (in D1 the retry window refuses it too, since the prune clears `charged_at`) | `UploadIdNamesOneRow` | `RegistryLifecycleScan.cfg` |
| `ResumeGate` drops `pubAt[v][c] < stamp` | `RESUME_RELEASED_UPLOAD` without `published_at < ?1`: a resume holding the released generation's stamp lands | `TypeOK` | `RegistryLifecycleScan.cfg` |
| a resume charges the daily count | `RESUME_RELEASED_UPLOAD` followed by `CHARGE_USER_DAILY_UPLOAD` | `ChargedOncePerOperation` | `RegistryLifecycleScan.cfg` |
| `Confirmable` keeps only the pending test | `CONFIRM_UPLOAD_STORED` without `upload_id = ?6 AND published_at = ?7` | `StoredHasObject` | `RegistryLifecycleScan.cfg` |
| the release drops `pubAt[v][c] = cron.stamp` | `RELEASE_EXPIRED_RESERVATION` without `published_at = ?6`: a release listed before a resume lands on the resumed reservation | `TerminalDecidedOnCurrentGeneration` | `RegistryLifecycleScan.cfg` |
| the release drops `~stored[v][c]` | `RELEASE_EXPIRED_RESERVATION` without `stored_at IS NULL` | `VerdictOnlyOnStored` | `RegistryLifecycleScan.cfg` |
| the release refunds nothing | `settle_reservations` without `REFUND_USER_STORED_BYTES` | `StoredBytesExact` | `RegistryLifecycleScan.cfg` |
| `CronTimeout` takes reservations | `TIME_OUT_PENDING_UPLOADS` on rows without `stored_at` | `VerdictOnlyOnStored` | `RegistryLifecycleScan.cfg` |
| the timeout refunds nothing | `sweep_uploads` without `REFUND_TIMED_OUT_UPLOADS` | `StoredBytesExact` | `RegistryLifecycleScan.cfg` |
| `CronForget` takes pending rows | `PRUNE_SETTLED_UPLOAD_IDS` on a row with no terminal time | `UploadIdNamesOneRow` | `RegistryLifecycleScan.cfg` |
| the rejection refunds nothing | `apply_verdict` without `REFUND_USER_STORED_BYTES` | `StoredBytesExact` | `RegistryLifecycleScan.cfg` |
| a refused rejection refunds | `REFUND_USER_STORED_BYTES` applied when the guarded update before it changed no row | `TypeOK` | `RegistryLifecycleScan.cfg` |
| `VerList` lists reservations | both `REVISIONS_BY_VERIFICATION_STATUS` and `VERDICT_TARGET` without the `stored_at` conjunct (either alone still refuses): an operator's reject resolution lands on a reservation | `VerdictOnlyOnStored` | `RegistryLifecycleScan.cfg` |
| `Inspections` allows `verified` for every content | the archive scan bypassed | `VerifiedByInspection` | `RegistryLifecycleScan.cfg` |
| the run inspects bytes other than the listed content | both checksum checks removed, the driver's `undeclared_digest` and the inspector's own hash; either alone still refuses | `VerifiedByInspection` | `RegistryLifecycleScan.cfg` |
| the `reject` resolution renders `verified` | an operator verifying from the name alone | `VerifiedByInspection` | `RegistryLifecycleScan.cfg` |

The `ReviveGate` stamp row is the stamp guard's role made visible: the
verdict guards compare `published_at` exactly, so a revival that
repeated the rejected generation's stamp would let a delayed verdict of
either direction land on it; the revival gate refuses such a sample,
and without it the ghost generation detects what the stamp alone
cannot. The `ResumeGate` stamp row reports `TypeOK` for the same
reason: a resume that repeats its row's stamp adds a generation without
spending a sample, and the ghost count outruns the bound `MaxStamps`
gives it. The last three rows test the driver model's arms, not a server
statement: the first makes `Inspections` inconsistent with `Malicious`,
the second lets a run verify a row from an object that holds other
bytes (reachable through the upload-id reuse corner), and only the
third has a code anchor - `Pass::resolve` routes `verify` through the
full pass rather than a direct `PATCH`.

## What this does and does not establish

TLC exhaustively checks the stated properties over the finite
configurations above, for the model. It does not prove the Rust, the
SQL, D1, R2, or the Durable Object. The correspondence table is the
abstraction boundary: a statement whose guards drift from the model's
(`NewGate`, `ReviveGate`, `ResumeGate`, the confirmation's and the
release's generation guards, `VerApply`'s guard, and the `changes()`
chain of the admission and refund batches) silently invalidates the
result, which is why `registry/AGENTS.md` asks a change to those
statements to keep the model faithful. The SQL validation tests
(`tests/sql_validation/`) check the individual statements against the
schema; the Kani harnesses check the quota functions; `upload.rs`'s
unit tests check which outcome a publish picks; this model checks that
the gates compose under concurrency whatever a publish picks, with the
verifier's pass reduced to the outcomes its own tests establish ("The
verifier" above). Nothing here verifies the inspection of archive bytes
itself, the credentials, the yank column, the byte and shared
allowances, the package quotas, the governor, the responses a request
is answered with, or anything that depends on how long a step takes.

## Interpreting a counterexample

TLC prints the violated property and a trace: the initial state, then
each state with the action that produced it (`<PubAdmit line N, col M>`,
and so on). Read the request slots to see which handler was between its
read and its write when another one committed. Decide whether the trace
is a modeling mistake (an action that does not match its row of the
table above), a stated assumption being false, or a real interleaving
the code permits; only the last is an implementation finding, and it
belongs in its own fix PR with a regression test, never in a weakened
property.
