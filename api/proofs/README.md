# Registry lifecycle model

`RegistryLifecycle.tla` is a TLA+ specification of the hosted registry's
publish / verification lifecycle for one package, checked with the TLC
model checker. It covers what the Kani harnesses (single functions) and
the SQL validation tests (single statements) cannot: how the admission
batch, the UPLOADS writes, the confirmation, the cron's sweeps, and the
archive placement interact under concurrent publishes, retries of one
upload, respins, verdicts, rejections, revivals, released and resumed
reservations, timeouts, concurrent placement passes, lost races, and
requests that die between two steps.
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
cd api/proofs
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
in (`api/src/glue/bearer/package.rs`, `api/src/glue/bearer/verifier.rs`,
`api/src/glue/cron.rs`, `api/src/glue/publication.rs`,
`api/src/glue/read.rs`, `api/src/upload.rs`,
`api/src/publication.rs`,
`api/src/verify.rs`, `api/src/sql/admission.rs`,
`api/src/sql/packages.rs`, `api/src/sql/publication.rs`,
`api/src/sql/quota.rs`, `api/src/sql/downloads.rs`,
`api/migrations/0001_init.sql`) and the verifier
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
`api/src/upload.rs`'s own unit tests' claim, and a retry of an
upload is simply another request.

State:

| Variable | Production state |
| --- | --- |
| `status[v][c]` | `revisions.verification`, or `absent` when no row exists |
| `pubAt[v][c]` | `revisions.published_at`: the clock sample of the admission that wrote the row's current generation - a new revision, a revival, or a resumed reservation |
| `clock` | the greatest stamp sampled so far |
| `spare` | the stamps requests sampled and have not committed: a request samples once, before it reads any revision row, and may write much later or never |
| `stored[v][c]` | `revisions.stored_at IS NOT NULL`: a `pending` row without it is a reservation, a `rejected` row without it a released one |
| `archived[v][c]` | `revisions.archived_at IS NOT NULL`: a `verified` row without it passed verification and is not served yet |
| `opOf[v][c]` | `revisions.upload_id`: the upload operation the row is bound to |
| `owner[v][c]` | `revisions.published_by` |
| `storedBytes[u]` | `users.stored_bytes` |
| `dailyUploads[u]` | the `uploads` count of the user's `user_daily_upload_usage` row, for the one UTC day modeled |
| `object[o]` | the UPLOADS object `uploads/<o>.zip`: absent, or the content it holds, which is also the SHA-256 R2 recorded with it (`store` puts every object under its archive's digest) |
| `archive[v][c]` | the ARCHIVES object under the row's own key, `publication::archive_key`: absent, or the content it holds |
| `pub[p]`, `ver[w]`, `cron`, `place[k]` | what a publish handler holds between its admission (or the read it converges from) and its confirmation, a verifier run between its listing and its `PATCH`, the cron's reservation pass between its object check and its write, and an archive placement of one listed row between its head of an empty ARCHIVES key and its copy, or its rejection |
| `gen[v][c]`, `decided[v][c]` | ghost history for the properties - the row's publish-event count, and the generation its terminal state was decided against; no guard reads them |
| `basis[v][c]` | ghost: what turned the row terminal - the inspector (`inspected`), an operator's reject resolution (`manual`), the cron releasing a reservation (`released`), the pending timeout (`timeout`), or a placement that found the upload without the verified bytes (`lost`); `none` while pending |

### Actions and the code they abstract

| Action | Rust | D1 / R2 |
| --- | --- | --- |
| `Sample` | `Preflight.now` as the admission sees it: the sample `publish` takes once it holds the body | none |
| `PubAdmit` | `package.rs` `admit_upload`, when its gate changes a row (a gate that changes none leaves no trace: the request decides again or answers) | `ADMIT_NEW_UPLOAD` (the gate, which is the user's daily charge), `CHARGE_SHARED_DAILY_UPLOAD`, `COUNT_ADMITTED_UPLOAD`, `ENSURE_PACKAGE`, `ENSURE_VERSION`, `INSERT_ADMITTED_REVISION`; or `REVIVE_REJECTED_UPLOAD` (the gate; `published_at < ?1`: the stamp must advance), `CHARGE_USER_DAILY_UPLOAD`, `CHARGE_SHARED_DAILY_UPLOAD`, `COUNT_USER_STORED_BYTES`; or `RESUME_RELEASED_UPLOAD` (the gate, with the same stamp rule), `COUNT_USER_STORED_BYTES` - one transaction, the gate evaluated against the rows as they are then, every later statement applying only by `changes()` |
| `PubConverge` | `attempt`'s `Converge` arm: the read of a reservation with the request's bytes | `EXISTING_REVISIONS`: snapshots `(upload id, published_at)` |
| `PubStore` | `store`: the put under the reservation's key, after a head when the operation resumes or converges (an object already there is kept) | `bucket.head`, `bucket.put` on `uploads/<upload id>.zip`, the put carrying the archive's SHA-256 |
| `PubConfirm` | `confirm`, `changed_rows == 0` as the reservation's generation being gone | `CONFIRM_UPLOAD_STORED`, guarded on the upload id and the `published_at` the request stored for |
| `PubDrop` | the request ends after its admission: a failed put or batch, or the Worker dying | none |
| `CronList` | `cron.rs` `settle_reservations`: one listed reservation and its object check | `EXPIRED_RESERVATIONS`: snapshots `(upload id, published_at)`; `bucket.head` |
| `CronSettle` | the confirmation or the release | `CONFIRM_UPLOAD_STORED`, or `RELEASE_EXPIRED_RESERVATION` then `REFUND_USER_STORED_BYTES` - one transaction, guarded on the listed generation |
| `CronTimeout` | `cron.rs` `sweep_uploads` | `REFUND_TIMED_OUT_UPLOADS` then `TIME_OUT_PENDING_UPLOADS` - one transaction over the stored `pending` rows |
| `CronForget` | the same batch | `PRUNE_SETTLED_UPLOAD_IDS`: a terminal row's `upload_id` is cleared, which frees the id |
| `VerList` | one verifier run over one listed entry: `admin_versions_response`, then the driver's `Pass::version` or `Pass::resolve` ("The verifier" below) | `REVISIONS_BY_VERIFICATION_STATUS('pending')`, which leaves reservations out: snapshots `(checksum, published_at)`; the artifact download reads the row's object |
| `VerApply` | the `PATCH`: `verdict_authn` (the run's OIDC JWT, single-use `jti`; not modeled), `verdict_response`, `apply_verdict`, `changed_rows == 0` as the lost race; a failed batch | `MARK_REVISION_VERIFIED`, or `MARK_REVISION_REJECTED` then `REFUND_USER_STORED_BYTES` - one transaction, `checksum = ?` and `published_at = ?` bound to the body's values (the listing's binding) |
| `PlaceList` | `publication.rs` `place_archives`: one listed revision, and `copy_archive`'s head of its ARCHIVES key finding no object; a row without an upload id is the lost source at once. A key that holds the archive is `PlaceMark`'s; one that holds anything else is an error the pass logs (no step) | `UNARCHIVED_REVISIONS`: snapshots `(upload id, published_at)`; `archives.head` |
| `PlaceCopy` | `copy_archive` past the head: the read of the listed upload's object and the put of its bytes; `Some(false)` as the lost source, `None` as the lost create race | `uploads.get`, compared by `publication::holds_archive`; `archives.put` under the row's SHA-256 with `onlyIf: etagDoesNotMatch "*"` |
| `PlaceDrop` | the placement ends before its copy: a failed R2 call, or the Worker dying | none |
| `PlaceMark` | `place_archive`'s write after `copy_archive` answered `Some(true)`: its head found the archive (`holds_archive`), or its put stored it | `MARK_REVISION_ARCHIVED`, guarded on `verified` and `archived_at IS NULL` and on no generation; the three statements after it in the batch count the change for the publish pass and write nothing the model holds |
| `PlaceReject` | `place_archive`'s write for a lost source, `changed_rows == 0` as the row having moved on; a failed batch, or the Worker dying before it | `REJECT_UNARCHIVED_REVISION` then `REFUND_USER_STORED_BYTES` - one transaction, guarded on `verified`, on `archived_at IS NULL`, and on the listed `published_at` |

The verdict handler's own read (`VERDICT_TARGET`) and
`verdict_binding_matches` can only refuse a `PATCH`, so the model lets
every `PATCH` a run renders reach the guarded update, which binds the
same values. The guarded updates do not test `stored_at`: `VerList`'s
`stored` conjunct stands for the listing's condition and for
`VERDICT_TARGET`'s, to which a reservation is a missing row. No action
deletes an object or a row, and none replaces an ARCHIVES object.

A placement slot (`Placers`) is one pass's work on one listed row,
from its head of an empty ARCHIVES key to its copy, and with a lost
source to its write. The pass a verdict runs when it verified a row
(`verdict_response`) and the cron's (`scheduled`) are the same code and
are not told apart: any slot may list any verified row that is not
archived, at any time, so passes overlap and a pass that listed several
rows is several placements (which no configuration has at work on two
rows at once: "Configurations and results"). `PlaceMark` belongs to no
slot. An object stays under its ARCHIVES key, so
`MARK_REVISION_ARCHIVED` lands exactly where a head taken at that
moment finds the archive; the model therefore does not tell the pass
that stored an archive from a later one that finds it, and lets the
write happen whenever its guard holds.

The read plane is modeled as the two predicates the invariants use:
`Current(v)` is the `current_revisions` view (the verified revision in
ARCHIVES with the greatest `(published_at, revision)`), and
`ArtifactReadable` is `verify::artifact_readable` over the stored rows
(`ARTIFACT_BY_REVISION`'s `stored_at IS NOT NULL`, and its `archived`
column). What a read returns is the object the handler opens: for a
verified revision the one under the row's ARCHIVES key
(`glue/read.rs`), which `PublicReadsAreArchived` compares with the row;
for a pending one, readable with the `verify` scope, the upload's
(`VerList`'s `bytes`).

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
  ends without a `PATCH`, which is the stutter. A verified row's object
  can be gone when a pass comes to read it: a run may download the
  object before it expires and send its `PATCH` after, so the verdict's
  own pass finds none, and so does a later pass when every earlier one
  failed. The pass that finds none rejects the row as lost
  (`publication_source_lost`), the outcome the model reaches through a
  pruned upload id and through an object that holds other bytes.
- **An upload id reused for other bytes** is the one way an UPLOADS
  object can disagree with the row that names it, and the model keeps
  it reachable rather than assuming it away (`NoStaleObject`,
  `NoVerifiedObjectReplaced`, and `NoReplacedObjectRejected` under
  "Non-vacuity"). An id leaves its row
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
  the run inspected while its object holds others. No read serves that
  object. The read plane serves a verified row only once it is
  archived, and then from the row's ARCHIVES key, which
  `ArchivedIsPlaced` holds to the row's own bytes: a placement copies
  an upload only when R2 recorded the row's SHA-256 with it, and R2
  checks the copy against that digest again (`ArchivesWriteOnce`). A
  placement that reads the replaced object rejects the row as
  `publication_source_lost`; one that read the object before the put
  landed archives the bytes the run inspected. It takes the reused id
  and a put outstanding across a release and a whole verification,
  which no client controls, and like the first form it costs only that
  client's own upload. Cabin draws a fresh random id for each upload,
  so both forms take a client that breaks that rule.
- **R2 behaves as measured.** A put that carries a SHA-256 stores
  nothing unless its body hashes to it; a put with
  `onlyIf: etagDoesNotMatch "*"` creates the object only while the key
  holds none, and otherwise changes nothing; a head or a get reports
  the SHA-256 an object was put with, and none for an object put
  without one. `PubStore`, `PlaceList`, `PlaceCopy`, and `PlaceMark`
  are written on these, and nothing here tests them. ARCHIVES has no
  lifecycle rule.
- **An ARCHIVES object no placement wrote** (`Stray`) is the one way an
  ARCHIVES key can hold anything but its row's bytes: for a content in
  `Stray`, a key starts either empty or holding other contents. No
  placement replaces such an object or counts it as placed, so its row
  stays verified and not archived - not served, not rejected, its bytes
  still charged - until an operator removes the object, which the model
  leaves out (`NoStrayBlocks` under "Non-vacuity"). `Stray` is empty in
  the configurations that do not exercise this.
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
  which is the stutter too, and so does a refusal of a placement's
  admission, which comes before the pass touches the row.
- Collapsed steps: the head and the put of one `store` call are one
  step, as are the cron's listing and its object check, a placement's
  listing and its head of the ARCHIVES key (no object leaves ARCHIVES,
  so a head that finds the key empty would have found it empty at the
  listing), and the head or put that finds or stores an archive and
  the write that records it (`PlaceMark`, above); a timeout or a prune
  takes one row per step where production takes every due row in one
  statement. None of the checked properties looks between those steps.
  A placement's read of the upload and its put are one step too, on
  the next item's argument. Its head of an empty key and its put stay
  two steps, because the create race lies between them, and so do its
  finding of a lost source and its rejection, because the row can
  return between them.
- Taking a placement's read and its put as one step (`PlaceCopy`) keeps
  what is stored, and not when. What: the put stores the bytes the read
  returned or, by R2's digest check, nothing, whatever the upload's
  object has become since. When: the model fills the key at the read,
  while in `copy_archive` the key stays empty until the streamed put
  completes, so another pass can head the still-empty key after the
  first one's read, and no step of the model is that head. The argument
  that no checked property depends on it is that such a run ends with a
  row and a key the model reaches another way, which takes two slots:
  with the other pass's listing taken before the copy, after which that
  pass loses the create race, or finds the source lost and rejects a
  row whose archive is, or comes to be, under the key
  (`NoEarlierGenerationArchive` under "Non-vacuity" continues from
  there). It is an argument, made for the simplest such runs, and not
  a result of TLC: no configuration has the read and the put as two
  steps.
- The verifier's runs that end without a `PATCH` (abstain, download or
  digest or upstream or inspector or mint failure) change nothing on the
  server, so they are the stutter; a run that does `PATCH` is one
  `decided` request. The model lets two runs overlap and a run's `PATCH`
  arrive arbitrarily late, which over-approximates the workflow's
  `concurrency` group (one run at a time): the server's guards are what
  is checked, not the group. One reservation pass of the cron runs at
  a time; placements overlap as far as `Placers` allows.
- Credentials are not modeled. The listing and the download carry the
  exchanged verify-scoped token (`has_verify_scope`); every `PATCH`
  carries a freshly minted OIDC JWT whose `jti` `verdict_authn` consumes
  in its own transaction before the business logic, so a replayed `PATCH`
  is refused before any read (the exchange consumes its own JWT's `jti`
  too, coupled to the token mint). That split is the structure of the workflow and the
  handlers, not a property TLC can establish here; `api/src/trustpub.rs`
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
| `PublicReadsAreArchived` | without the `verify` scope, an artifact is readable only when `verified` and archived, and the object that read opens, the one under the row's ARCHIVES key, holds the row's bytes; the served pointer names a revision such a read serves (that the row is archived follows from the two read predicates' definitions; that the object is the archive rests on `ArchivedIsPlaced`) |
| `ArchivedIsPlaced` | an archived row is `verified` (in D1 the column's `CHECK` holds that whatever the statements do; the model checks that none relies on it) and its ARCHIVES key holds its own bytes, whatever its upload's object holds: the second form of the upload-id reuse corner ("Assumptions") reaches no read |
| `TerminalDecidedOnCurrentGeneration` | a `verified` or `rejected` row was decided - by a verdict, a release, the timeout, or a placement's rejection - against its current generation (the ghost `gen`, not the stamp): no delayed verdict, release, or placement's rejection lands on a revival or a resume of the same bytes |
| `PendingUndecided` | a pending row carries no decision and no basis (every admission clears both) |
| `VerifiedByInspection` | a `verified` row was rendered by the inspector's answer on the listed bytes, never by the `reject` resolution and never on bytes the scan rejects (the abstain and failure outcomes are the stutter, so this constrains the two arms that `PATCH`) |
| `VerdictOnlyOnStored` | a verdict, the timeout, and the placement's rejection land only on stored uploads, and only a reservation is released: a verified row is stored, a row whose basis is `inspected`, `manual`, `timeout`, or `lost` is stored, and one whose basis is `released` is not |
| `UploadIdNamesOneRow` | a pending row names its operation, and no two rows are bound to one upload id (in D1 the unique index on `upload_id` holds the second whatever the gates do; the model checks that the gates never rely on it) |
| `StoredHasObject` | a stored upload awaiting its verdict has an object under its upload id |
| `StoredBytesExact` | each user's `stored_bytes` equals the archive sizes of the live (pending or verified, reservations included) rows they published - the same bytes on two versions twice - in every D1-visible state |
| `WithinLimits` | no user's pending rows (reservations included) exceed `PendingLimit`, no user's daily count exceeds `DailyLimit`, and no user's stored bytes exceed `StorageLimit`, however admissions interleave |
| `ArchivedImmutable` | an archived row stays archived and `verified`, at its stamp: nothing changes or withdraws a revision the registry serves |
| `VerifiedUnlessSourceLost` | a verified row keeps its stamp, and stops being `verified` only by the placement's rejection, which `ArchivedImmutable` keeps from an archived row |
| `ArchivesWriteOnce` | an ARCHIVES key changes only from empty to its row's own bytes: no placement replaces or removes an object, or stores other bytes under a key |
| `RejectedOnlyRevives` | a rejected row only ever returns to pending (never to verified) |
| `LiveGenerationStable` | a live row is never restamped, moved to another publisher, or un-stored, and a pending row keeps its upload id: no publish, retry, convergence, or revival rewrites it |
| `ServedChangesOnlyByPlacement` | what a version serves changes only when a row becomes archived, and to that row - a pending respin, a verdict, a rejection, a release, or a revival never disturbs it, and nothing withdraws it |
| `RespinNeedsOptIn` | a different-bytes revision joins a live sibling only through a request that carried `new-revision=true` |
| `ChargedOncePerOperation` | a user's daily count moves by exactly one for each row bound to an upload id it was not bound to: a resumed operation is not charged again, and nothing refunds a charge |
| `SpentIdStaysSpent` | an upload rejected after it was stored - by a verdict, the timeout, or the placement - returns to pending only under an upload id other than the one it holds, which `ChargedOncePerOperation` charges; only a reservation that never stored resumes for free |

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
`SpentIdStaysSpent`, `ServedChangesOnlyByPlacement`,
`VerifiedUnlessSourceLost`, `ArchivesWriteOnce`, and the timeout and
release halves of `VerdictOnlyOnStored` each follow step by step from
one atomic gate or write: they are tripwires for a weakened guard and
need no interleaving to hold, where the rest depend on how the
handlers' steps interleave.

## Configurations and results

All runs are exhaustive breadth-first checks under the pinned TLC with
symmetry over `Versions`, `Users`, `Uploads`, `Publishers`,
`Verifiers`, and `Placers`; "distinct" is the number of states checked
after symmetry reduction.

| Configuration | Constants | Distinct / generated states | Depth | Result | Local time (6 workers) |
| --- | --- | --- | --- | --- | --- |
| `RegistryLifecycle.cfg` (CI) | `Versions = {v1}`, `Checksums = {1, 2}`, `Users = {u1}`, `Uploads = {i1, i2}`, `Publishers = {p1, p2}`, `Verifiers = {w1}`, `Placers = {k1}`, `MaxStamps = 3`, `Malicious = {}`, `Stray = {}`, `PendingLimit = 1`, `DailyLimit = 2`, `StorageLimit = 3` | 1,868,426 / 12,197,509 | 32 | complete, no violation | 4 min 1 s |
| `RegistryLifecycleShared.cfg` (CI) | `Versions = {v1, v2}`, `Checksums = {1}`, `Users = {u1, u2}`, `Uploads = {i1, i2}`, `Publishers = {p1, p2}`, `Verifiers = {w1}`, `Placers = {}`, `MaxStamps = 3`, `Malicious = {}`, `Stray = {}`, `PendingLimit = 1`, `DailyLimit = 2`, `StorageLimit = 2` | 2,714,787 / 17,960,678 | 26 | complete, no violation | 12 min 44 s |
| `RegistryLifecycleScan.cfg` (CI) | `Versions = {v1}`, `Checksums = {1, 2}`, `Users = {u1}`, `Uploads = {i1, i2}`, `Publishers = {p1, p2}`, `Verifiers = {w1, w2}`, `Placers = {k1, k2}`, `MaxStamps = 2`, `Malicious = {2}`, `Stray = {1}`, `PendingLimit = 2`, `DailyLimit = 2`, `StorageLimit = 2` | 955,177 / 6,822,068 | 29 | complete, no violation | 4 min 15 s |

The first configuration exercises respins (two contents of one version,
both of which the inspector may verify, so the served revision switches
as each is archived), revivals beside a live sibling, the opt-in guard,
and one upload's retries, releases, and resumes, with one verifier run
and one archive placement at a time. There the pending limit and the
daily count bind (one pending upload at a time, two charged uploads of
three samples) and the storage cap holds both contents exactly. The
second has the same bytes under two versions and two publishing
accounts: two uploads with an object and a charge each, where a
rejection or a release must refund its own row's publisher and no
other. The third has the content the archive scan rejects (`Malicious`)
beside one it may verify, with two concurrent runs - the one
configuration where two verdicts race for a row - two concurrent
placements - the one where two passes race for an ARCHIVES key, and
where one pass's rejection goes stale behind another's - an ARCHIVES
key that may start out holding a stray object (`Stray`), and a storage
cap the two contents do not fit under together.

The placement is checked less widely than the rest. The second
configuration has no placement slot (`Placers = {}`), so no row there
is archived, served, or rejected by a placement, and the placement
properties hold there trivially; what it checks ends at the verdict.
And no configuration has two rows a placement can take and two slots at
once - the first has two such rows and one slot, the third two slots
and one such row - so two passes at work on different rows, which is
also how the model renders one pass that listed several rows, are never
exercised.

In the first two the inspector's rejections are of bytes it may also
verify, and operator resolutions reject as well. All three are
exhaustive finite-state checks of the model, not simulations and not
proofs of the unbounded system. The local times were taken with six
workers on a machine busy with other work, so an idle run is faster,
and they do not compare with one another. The table records no CI
times; each `tlc-*` job runs under a 30-minute timeout.

## Non-vacuity

`RegistryLifecycle_witness.cfg` lists traps: each is the negation of a
behavior the model must be able to reach, so TLC must report it
**violated**, and the printed trace is the witness. Its constants are
`RegistryLifecycleScan.cfg`'s with one verifier run at a time, which
is all any trap needs, and `StorageLimit = 3`, so both contents can be
live at once. `RegistryLifecycleShared_witness.cfg` holds
`NoSameBytesTwice`, which needs two versions carrying one checksum. TLC
stops at the first violation, so the `tlc-scan` job runs each trap
alone, and fails if TLC does not report one violated. From the
repository root:

```sh
cargo check-tlc-witnesses --jar api/proofs/tla2tools.jar \
  api/proofs/RegistryLifecycle.tla \
  api/proofs/RegistryLifecycle_witness.cfg \
  api/proofs/RegistryLifecycleShared_witness.cfg
```

To see one witness's trace, keep only its line in a copy of the
configuration. The lengths below are the states of the trace the
multi-worker search printed (not necessarily the shortest):

| Trap | Witnessed behavior | States |
| --- | --- | --- |
| `NoServed` | a revision is admitted, stored, confirmed, verified, archived, and served | 10 |
| `NoRevivedVerified` | a rejected revision returns - a revival or a resume - and is then verified | 11 |
| `NoStaleVerdictRefused` | a verdict is refused by its guarded update: the row was decided, or returned under a new stamp, since the listing | 8 |
| `NoLostConfirm` | a request stores for a reservation that is released, or taken again, before it confirms | 7 |
| `NoRespin` | two revisions of one version are live at once | 5 |
| `NoSameBytesTwice` | the same bytes are live under two versions, as two uploads | 5 |
| `NoConvergence` | a request completes a reservation another request left unstored, and is charged nothing | 7 |
| `NoCronConfirm` | the cron confirms a reservation whose request died after its put | 7 |
| `NoRelease` | the cron releases a reservation whose object never arrived, and refunds its bytes | 5 |
| `NoResume` | the released operation returns under its own id, is not charged again, and is verified | 11 |
| `NoLateObject` | a put lands under a reservation the cron already released (a resume then finds the object) | 7 |
| `NoStaleObject` | the upload-id reuse corner ("Assumptions"): a stored upload whose object holds other bytes | 11 |
| `NoVerifiedObjectReplaced` | the corner's second form: a late put under the reused id replaces a row's object after a run downloaded it, and the verified row's object holds other bytes | 13 |
| `NoReplacedObjectRejected` | where the second form ends: a placement finds the other bytes under the row's upload id and rejects the row, which was never served | 16 |
| `NoSourceLost` | a placement rejects a verified row whose upload no longer holds its bytes, and refunds them | 10 |
| `NoMarkWithoutSource` | a row is recorded as archived after its upload id was pruned or its object replaced: `MARK_REVISION_ARCHIVED` reads nothing from UPLOADS and rests on the object under the ARCHIVES key, whichever pass put it there (the model does not tell the pass that stored it from a later one whose head finds it) | 11 |
| `NoEarlierGenerationArchive` | a returned row is verified while its ARCHIVES key already holds its bytes, which a pass copied for an earlier generation that another pass rejected as lost. No pass copies for the returned one, and the trace ends where `PlaceMark` is enabled: the write that archives it, one step on, rests on that object, which `MARK_REVISION_ARCHIVED` allows by naming no generation | 18 |
| `NoCreateRaceLost` | two placements head one empty ARCHIVES key, and the second put finds it filled | 11 |
| `NoStaleLossRefused` | a placement's rejection is refused by its generation guard: since the listing the row was rejected by another placement, returned, and was verified again | 18 |
| `NoStrayBlocks` | a verified row whose ARCHIVES key holds an object that is not its archive, which no placement archives or rejects | 7 |
| `NoTimeout` | a stored pending upload times out | 6 |
| `NoInspectedRejection` | the inspector's rejection lands | 7 |
| `NoManualRejection` | an operator's reject resolution lands on bytes the scan accepts, nothing inspected | 8 |

## Sensitivity

Each mutation below was applied alone, the configurations re-run until
one reported a violation, and the model restored; the unmodified
properties must detect the weakened guard. The last two columns are
what TLC reported and in which configuration (the first violation a
breadth-first search reaches, so another property may also fail).

| Mutation | Weakened guard | Reported violation | Configuration |
| --- | --- | --- | --- |
| `VerApply`'s guard drops `pubAt[v][c] = ver[w].stamp` | `MARK_REVISION_*` without `published_at = ?` | `TerminalDecidedOnCurrentGeneration` | `RegistryLifecycleScan.cfg` |
| `VerApply`'s guard drops `status = "pending"` | `MARK_REVISION_*` without `verification = 'pending'` | `TypeOK` | `RegistryLifecycleScan.cfg` |
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
| `Current` drops `archived[v][c]` | `current_revisions` without `archived_at IS NOT NULL`: a package document names a revision the artifact route does not serve | `PublicReadsAreArchived` | `RegistryLifecycleScan.cfg` |
| `ArtifactReadable` serves every verified row | `artifact_readable` without its `archived` argument: a read opens an ARCHIVES key that holds no object | `PublicReadsAreArchived` | `RegistryLifecycleScan.cfg` |
| `PlaceMark` takes any object under the key for the archive | `copy_archive` without `holds_archive` on the head: a revision is served from an object no placement wrote | `PublicReadsAreArchived` | `RegistryLifecycleScan.cfg` |
| `PlaceCopy` copies whatever the upload holds | `copy_archive` without `holds_archive` on the upload and without the put's `sha256`; either alone still refuses | `ArchivesWriteOnce` | `RegistryLifecycleScan.cfg` |
| `PlaceMark` drops the verified test | `MARK_REVISION_ARCHIVED` without `verification = 'verified'` (in D1 the column's `CHECK` then fails the statement) | `ArchivedIsPlaced` | `RegistryLifecycleScan.cfg` |
| `PlaceReject` drops `pubAt[v][c] = place[k].stamp` | `REJECT_UNARCHIVED_REVISION` without `published_at = ?7`: a rejection listed before another pass rejected the row lands on the generation that returned and was verified since | `TerminalDecidedOnCurrentGeneration` | `RegistryLifecycleScan.cfg` |
| `PlaceReject` drops `~archived[v][c]` | `REJECT_UNARCHIVED_REVISION` without `archived_at IS NULL`: a served revision is rejected (in D1 the column's `CHECK` then fails the statement) | `ArchivedIsPlaced` | `RegistryLifecycleScan.cfg` |
| `PlaceReject` drops the verified test | `REJECT_UNARCHIVED_REVISION` without `verification = 'verified'` | `TypeOK` | `RegistryLifecycleScan.cfg` |
| the placement's rejection refunds nothing | `place_archive` without `REFUND_USER_STORED_BYTES` | `StoredBytesExact` | `RegistryLifecycleScan.cfg` |

The `ReviveGate` stamp row is the stamp guard's role made visible: the
verdict guards compare `published_at` exactly, so a revival that
repeated the rejected generation's stamp would let a delayed verdict of
either direction land on it; the revival gate refuses such a sample,
and without it the ghost generation detects what the stamp alone
cannot. The `ResumeGate` stamp row reports `TypeOK` for the same
reason: a resume that repeats its row's stamp adds a generation without
spending a sample, and the ghost count outruns the bound `MaxStamps`
gives it. Three rows test the driver model's arms, not a server
statement. `Inspections` allowing `verified` for every content makes it
inconsistent with `Malicious`. The run that inspects bytes other than
the listed content verifies a row from an object that holds other bytes
(reachable through the upload-id reuse corner). Only the `reject`
resolution that renders `verified` has a code anchor - `Pass::resolve`
routes `verify` through the full pass rather than a direct `PATCH`.

Of the placement's rows, `PlaceCopy`'s removes two checks at once
because either alone still refuses other bytes. The verified row
reports `TypeOK` because the second of two passes that found one
source lost rejects the row again, and the second refund takes the
tally below zero. Two guards have no row, because the model cannot tell
them from their absence. The put's `onlyIf: etagDoesNotMatch "*"`: a
pass puts only after its head found the key empty, the only object that
can come under the key meanwhile is another pass's copy of the same
bytes, and replacing an object by itself is no step here.
`MARK_REVISION_ARCHIVED`'s `archived_at IS NULL`: a second write would
change only the time, which the model does not carry.

## What this does and does not establish

TLC exhaustively checks the stated properties over the finite
configurations above, for the model. It does not prove the Rust, the
SQL, D1, R2, or the Durable Object. The correspondence table is the
abstraction boundary: a statement whose guards drift from the model's
(`NewGate`, `ReviveGate`, `ResumeGate`, the confirmation's and the
release's generation guards, `VerApply`'s guard, the guards of the
placement's two writes, its two `holds_archive` comparisons, the read
plane's `archived` conditions, and the `changes()` chain of the
admission and refund batches) silently invalidates the result, which
is why a change to any of them must keep the model faithful
(`api/AGENTS.md`, "Checks"). The SQL validation tests
(`tests/sql_validation/`) check the individual statements against the
schema; the Kani harnesses check the quota functions; `upload.rs`'s
unit tests check which outcome a publish picks; `publication.rs`'s
check `digest` and `holds_archive`; the layout of the ARCHIVES key is
asserted by the local smoke test, which reads the archive back under
the literal key (`crates/xtask-registry-smoke/src/legs/publish.rs`),
and by the path literals in the tests of `publish.rs` and
`documents.rs`, whose `source_path` is built on the same key; this
model checks that the gates compose under concurrency whatever a
publish picks, with the verifier's pass reduced to the outcomes its own
tests establish ("The verifier" above). Nothing here verifies the
inspection of archive bytes itself, the credentials, the yank column,
the publish pass (the generations, `publication_jobs`, and the INDEX
documents, argued in `src/glue/publication.rs`),
the byte and shared allowances, the package quotas, the governor, R2's
own behavior ("Assumptions"), that a row's ARCHIVES key is the path the
read plane opens, the responses a request is answered with, or anything
that depends on how long a step takes: that a verified revision is
eventually archived or rejected is not a property here, and with a
`Stray` object under its key it is neither.

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
