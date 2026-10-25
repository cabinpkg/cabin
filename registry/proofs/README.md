# Registry lifecycle model

`RegistryLifecycle.tla` is a TLA+ specification of the hosted registry's
publish / verification lifecycle for one package, checked with the TLC
model checker. It covers what the Kani harnesses (single functions) and
the SQL validation tests (single statements) cannot: how the preflight
reads, the guarded D1 batches, the R2 writes, and the governor ledger
interact under concurrent publishes, respins, verdicts, rejections,
revivals, retries, lost races, failed batches, and delayed R2 deletes.
The external verifier is a participant: its listing, artifact download,
digest check, archive inspection (an oracle), operator resolutions, and
verdict `PATCH` progress on their own schedule against the server's
rules.

CI runs the configurations marked below on every registry-relevant change
(`.github/workflows/registry.yml`, job `tlc`, feeding the required
`registry-build` aggregate). A missing tool, a digest mismatch, a
violation, or the job timeout fails the check. The job then runs each
witness trap alone, where the reverse holds: a trap TLC does not report
violated fails it ("Non-vacuity").

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
`registry/src/verify.rs`, `registry/src/sql/packages.rs`, `registry/src/sql/meta.rs`,
`registry/src/sql/downloads.rs`, `registry/src/sql/backup.rs`,
`registry/migrations/0001_init.sql`) and the verifier
(`crates/xtask-registry-admin/src/verify.rs`, `crates/cabin-registry-verify/src/lib.rs`,
`.github/workflows/registry-verify.yml`).

One package; `Versions` version slots; `Checksums` distinct archive
contents. A revision row is keyed by `(version, checksum)`, because the
packaging-revision id is the checksum's leading hex and names the same
row (the astronomically unlikely prefix collision, answered `409` in
`revision_disposition`, is excluded). Each content's `archive_size` is the
number that names it (`Size(c) == c`), so the stored-bytes counter is
discriminating.

State:

| Variable | Production state |
| --- | --- |
| `status[v][c]` | `revisions.verification`, or `absent` when no row exists |
| `pubAt[v][c]` | `revisions.published_at`: the `now_iso8601()` sample of the publish event that wrote the row |
| `clock` | the greatest stamp sampled so far (a revival's repeated sample does not advance it) |
| `counter` | `meta.total_stored_bytes` |
| `blob[c]` | the R2 object `blobs/sha256/<c>` exists |
| `ledger[c]` | the governor's primary-pool entry for that key: none, reserved, committed |
| `deletes` | R2 deletes decided (refcount read zero) but not yet landed or failed |
| `pub[p]`, `ver[w]` | what an in-flight publish handler, or a verifier run and the verdict handler serving its `PATCH`, holds between reads and writes |
| `gen[v][c]`, `decided[v][c]` | ghost history for the properties - the row's publish-event count, and the generation its terminal verdict was computed against; no guard reads them |
| `basis[v][c]` | ghost: what rendered the row's terminal verdict - the inspector (`inspected`) or an operator's reject resolution (`manual`); `none` while pending |

### Actions and the code they abstract

| Action | Rust | D1 / R2 / ledger |
| --- | --- | --- |
| `PubStart` | `package.rs` `revision_disposition` | `EXISTING_REVISIONS`; decides no-op `200`, `409` new-revision-required, revive, or new |
| `PubAnswer` | the `Answered` arms; `heal_blobs_on_retry` on the `200` | no D1 write; the no-op heads and re-puts its blob |
| `PubStore` | `now_iso8601()` sampled (before the quota reads and the storage work; a revival may sample its row's current stamp again, the clock-skew case `ReviveGuard` refuses), then `persist_new_revision` head-then-put or `revive_rejected_revision` unconditional put; or the governor refuses admission or the reservation, or the put fails | governor reserve (idempotent per key), `bucket.put`; a reservation taken before a failed put stays |
| `PubBatch` | the `db.batch` in both write paths, `changed_rows == 0` as `Persist::Lost`; a failed batch or a request that dies after the upload | `INSERT_PACKAGE`, `COUNT_USER_PACKAGE`, `INSERT_VERSION_ROW`, `COUNT_STORED_BYTES_ON_PUBLISH`, `INSERT_REVISION`, `COUNT_USER_STORED_BYTES`; or `COUNT_STORED_BYTES_ON_REVIVAL`, `REVIVE_REJECTED_REVISION` (`published_at < ?`: the stamp must advance), `COUNT_USER_STORED_BYTES` - one transaction, guards evaluated against the rows as they are then |
| `PubReread` | the `Persist::Lost` re-read | `EXISTING_REVISIONS` again; `200` (healing) or `409` |
| `PubFinish` | `governor_client::settle` (best-effort), `heal_blob_if_reclaimed` | ledger commit; head, reserve, re-put if the blob is gone - or the reservation is refused and the repair skipped |
| `VerList` | one verifier run over one listed entry: `admin_versions_response`, then the driver's `Pass::version` or `Pass::resolve` ("The verifier" below) | `REVISIONS_BY_VERIFICATION_STATUS('pending')`: snapshots `(checksum, published_at)`; the artifact download reads the blob |
| `VerRead` | the `PATCH` arrives: `verdict_authn` (the run's OIDC JWT, single-use `jti`; not modeled), then `verdict_response`: `verdict_binding_matches` compares the listing's `(checksum, published_at)` with the row just read, then `verify::transition`; a repeat rejection re-drives `delete_blob_if_unreferenced` | `VERDICT_TARGET` |
| `VerApply` | `apply_verdict` / `apply_rejection`, `changed_rows == 0` as the lost race; a failed batch | `MARK_REVISION_VERIFIED` (+ `ENQUEUE_VERIFIED_BACKUP`), or `REFUND_STORED_BYTES_ON_REJECTION` then `MARK_REVISION_REJECTED` then `REFUND_USER_STORED_BYTES` - one transaction, `checksum = ?` and `published_at = ?` bound to the body's values (the listing's binding, which the check at the read found equal to the row's) |
| `VerReclaim` | `delete_blob_if_unreferenced` after the rejection batch; or the request dying first | `COUNT_LIVE_BLOB_REFERENCES`, then the delete is in flight |
| `LandDelete`, `DropDelete` | the R2 delete landing, or failing into an orphan | `bucket.delete` |

The read plane is modeled as the two predicates the invariants use:
`Current(v)` is the `current_revisions` view (the verified revision with
the greatest `(published_at, revision)`), and `ArtifactReadable` is
`verify::artifact_readable`.

### The verifier

`cargo registry-verify` (the `registry-verify.yml` workflow's verify
step) is the only writer of verdicts in production: the verdict route
accepts the OIDC identity of that workflow alone. One `VerList` step is
one run's work on one listed entry; what the driver does between the
listing and the `PATCH` maps as follows.

| Driver step (`xtask-registry-admin/src/verify.rs`) | Model |
| --- | --- |
| `exchange`: mint an OIDC JWT for the exchange audience, trade it for a verify-scoped registry token | not modeled (see "Assumptions") |
| `GET /api/v1/admin/versions?status=pending` with that token | the listing snapshot `(checksum, published_at)` of a pending row |
| `name_advisories` (`cabin-registry-verify --name-advisories`): `abstain` leaves the row pending for an operator | the run ends without a `PATCH` (no step) |
| `download` of the artifact by revision with the same token; a failure leaves it pending | `blob[c]` must hold, else the run ends without a `PATCH` |
| `undeclared_digest`: the bytes must hash to the listed checksum, else pending | `bytes = c` |
| the optional upstream fetch; a stored cleartext URL leaves it pending, a failed download continues without the file (the inspector can still reject, and a version whose metadata pins an upstream then fails operationally: pending) | folded into the inspector's result |
| `cabin-registry-verify <archive> <entry>`: `verified` or `rejected(reasons)`; an operational failure or an unparsable answer leaves it pending | a verdict from `Inspections(bytes)`: only `rejected` for `bytes \in Malicious`, either verdict otherwise; failures end the run without a `PATCH` |
| `Minter::mint` a fresh OIDC JWT for the verifier audience, then `PATCH` the verdict with the listing's `checksum` and `published_at` | the `decided` request, served by `VerRead` and `VerApply` |
| `Pass::resolve`, `verify`: the same pass with the advisory gate waived | the inspected arm (waiving the gate changes no later step) |
| `Pass::resolve`, `reject`: `PATCH` `rejected` with `name_advisory: <rule>` (the rule from the dispatch input, the `checksum` and `published_at` binding from the listing entry), inspecting nothing | the `manual` arm |

The inspector is an oracle. `Malicious` names the contents its archive
scan rejects whatever else is true (traversal, bombs, forbidden zip
features: `scan.rs`); for every other content the verdict is left free,
because it also depends on the generation's stored metadata - a revival
of the same bytes may carry corrected metadata (`REVIVE_REJECTED_REVISION`
replaces `metadata_json`), so identical bytes can be rejected and later
verified - on the bytes the pinned upstream host serves that run, on the
listing row's own `name` and `version` (the archive's manifest must
agree, so one content verifies under at most one version: the shared-blob
configuration, where the model lets both rows verify, admits more than
production, which is safe for the properties checked), and on the caps
the workflow's repository variables set. Whether `cabin-registry-verify`
rejects what it should is its own test suite's claim
(`crates/cabin-registry-verify/tests/verify.rs`, and `advise.rs` for the
advisory gate, which is a stutter here), not this model's. The digest check appears twice in production - the driver's
`undeclared_digest` before the inspector runs, and the inspector's own
hash of the archive against the listing (`checksum_mismatch_is_rejected`)
- and `bytes = c` stands for both.

The listing and the pass are one step although the driver performs them
minutes apart. A `PATCH` lands only while the row is still pending at the
listed stamp; the download needs the blob while the row is readable; at
the download instant of any `PATCH` that lands, both hold, and the merged
step taken then snapshots the same stamp and renders the same request. A
`PATCH` the server refuses changes nothing in either shape. A run whose
download finds no blob ends without a `PATCH`, like every other failure. The driver's parsing of the listing and of the child answers,
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
  `published_at` is `now_iso8601()`, sampled after the disposition read
  and before the storage work, so a slow request commits with an older
  stamp than a faster one that sampled later; the model reproduces that
  (across rows, commit order need not follow stamp order, and
  `current_revisions` serves by stamp, not commit order). Within one row the stamp is the generation identity, and the
  model does not assume the clock makes it unique: a revival may sample
  its row's current stamp again (two publish events of one row in one
  millisecond, or Worker clocks that disagree by at least the gap
  between them), and `REVIVE_REJECTED_REVISION`'s `published_at < ?`
  refuses that revival as a lost race, which the third sensitivity row
  shows to be load-bearing. The guards otherwise compare the stamp as
  the SQL compares the column; no generation counter replaces the
  production identity. Equal stamps across different rows would only
  reach the `(published_at, revision)` tie-break of
  `current_revisions`, which the model does not exercise.
- One package: `INSERT_PACKAGE`, `INSERT_VERSION_ROW`, and the `-`/`_`
  twin guard always succeed; the twin `400` is unreachable.
- Resolver-metadata invariance (`dependencies`, `features`, `standards`,
  `links`) is not modeled: the model's `409`s are the opt-in and
  immutability refusals only.
- The per-user quota tallies (`COUNT_USER_PACKAGE`,
  `COUNT_USER_STORED_BYTES`, `REFUND_USER_STORED_BYTES`) are not
  modeled: they write only `users` columns no guard reads, and only the
  quota preflight reads them (`tests/sql_validation/quota.rs` checks
  them against the rows they count).
- Governor refusals, the breaker's write gate, scope membership, rate
  limiting, and the quota preflight are not distinguished: each ends a
  publish before any revision or stored-bytes write (the preflight's own
  token-bucket update is not modeled), which `PubStore`'s failure arm
  stands for.
  The one refusal after a commit, of the opportunistic heal's
  reservation, is modeled as the repair being skipped.
- Collapsed steps: the reservation and the put are one step, as are a
  repeat rejection's target and refcount reads; `deletes` is a set, so
  overlapping deletes of one key coalesce. None of the checked properties
  looks between those steps.
- The verifier's runs that end without a `PATCH` (abstain, download or
  digest or upstream or inspector or mint failure) change nothing on the
  server, so they are the stutter; a run that does `PATCH` is one
  `decided` request. The model lets two runs overlap and a run's `PATCH`
  arrive arbitrarily late, which over-approximates the workflow's
  `concurrency` group (one run at a time): the server's guards are what
  is checked, not the group.
- Credentials are not modeled. The listing and the download carry the
  exchanged verify-scoped token (`has_verify_scope`); every `PATCH`
  carries a freshly minted OIDC JWT whose `jti` `verdict_authn` consumes
  in its own transaction before the business logic, so a replayed `PATCH`
  is refused before any read (the exchange consumes its own JWT's `jti`
  too, coupled to the token mint). That split is the structure of the workflow and the
  handlers, not a property TLC can establish here; `registry/src/trustpub.rs`
  and the `check_oidc` diagnostic test it.
- `MaxStamps` bounds the stamps one execution samples; a publish that
  would sample past the bound can still fail or be answered from its
  preflight, never reach the storage work. A sample spent by a publish
  that fails before D1 is elided (the model leaves the clock untouched),
  so the bound counts samples that can reach a row. A revival's repeated
  sample is offered under the same bound, so at the bound it is not
  explored; such a revival can only lose its race (the row's stamp only
  rises), so the bound cuts lost-race traces, not commits.

## Properties

Checked as `INVARIANTS` and action `PROPERTIES` on every state and step
of every configuration:

| Property | Statement |
| --- | --- |
| `TypeOK`, `RowsStamped` | domains; a row exists iff stamped, and stamps are unique (fresh samples advance the clock and the revival guard refuses the repeated one - a sanity check on the model, not a production property) |
| `PublicReadsAreVerified` | without the `verify` scope, an artifact is readable only when `verified`; the served pointer names a verified row (both follow from the two read predicates' definitions: this documents the gate rather than tests the write path) |
| `TerminalDecidedOnCurrentGeneration` | a `verified` or `rejected` row was decided by a verdict computed against its current generation (the ghost `gen`, not the stamp): no delayed verdict lands on a revival of the same bytes |
| `PendingUndecided` | a pending row carries no verdict and no basis (revival clears both) |
| `VerifiedByInspection` | a `verified` row was rendered by the inspector's answer on the listed bytes, never by the `reject` resolution and never on bytes the scan rejects (the abstain and failure outcomes are the stutter, so this constrains the two arms that `PATCH`) |
| `CounterExact` | `total_stored_bytes` equals one archive size per checksum with a live (pending or verified) row, in every D1-visible state |
| `LedgerConservative` | an existing R2 object always has a ledger entry: orphans are conservative spend |
| `VerifiedImmutable` | a verified row never changes status or stamp |
| `RejectedOnlyRevives` | a rejected row only ever returns to pending (never to verified) |
| `LiveGenerationStable` | a live row is never restamped: no publish, retry, or revival rewrites it |
| `ServedChangesOnlyByVerification` | what a version serves changes only when a row becomes verified - a pending respin, a rejection, or a revival never disturbs it |
| `RespinNeedsOptIn` | a different-bytes revision joins a live sibling only through a request that carried `new-revision=true` |
| `AnsweredWithoutCommitWritesNothing` | the `200` no-op, the preflight `409`s, a failed publish batch, and a verdict refused or repeated at its read change no row and no accounting |

`PendingUndecided`, `AnsweredWithoutCommitWritesNothing`, and
`VerifiedByInspection` likewise follow from how the actions are written;
they pin the model's shape against later edits rather than test a rule of
the server. `VerifiedByInspection` in particular is a fact about the
driver as modeled, not a server rule: the verdict route cannot tell an
inspected verdict from any other `PATCH` by the pinned OIDC identity, so
whoever holds that identity can verify anything, and no change to
`verdict_binding_matches`, the transition table, or the guarded updates
can make this invariant fire. The remaining properties are stated over
the rows and the ghost history, not over the actions' own guard
expressions, so weakening a guard breaks them (see "Sensitivity").

## Configurations and results

All runs are exhaustive breadth-first checks under the pinned TLC with
symmetry over `Versions`, `Publishers`, and `Verifiers`; "distinct" is
the number of states checked after symmetry reduction.

| Configuration | Constants | Distinct / generated states | Depth | Result | Local time (10 cores) |
| --- | --- | --- | --- | --- | --- |
| `RegistryLifecycle.cfg` (CI) | `Versions = {v1}`, `Checksums = {1, 2}`, `Publishers = {p1, p2}`, `Verifiers = {w1}`, `MaxStamps = 3`, `Malicious = {}` | 3,637,794 / 22,537,331 | 32 | complete, no violation | 34 s |
| `RegistryLifecycleShared.cfg` (CI) | `Versions = {v1, v2}`, `Checksums = {1}`, `Publishers = {p1, p2}`, `Verifiers = {w1, w2}`, `MaxStamps = 3`, `Malicious = {}` | 9,237,089 / 71,498,443 | 32 | complete, no violation | 2 min 18 s |
| `RegistryLifecycleScan.cfg` (CI) | `Versions = {v1}`, `Checksums = {1, 2}`, `Publishers = {p1, p2}`, `Verifiers = {w1, w2}`, `MaxStamps = 2`, `Malicious = {2}` | 2,627,569 / 19,073,169 | 27 | complete, no violation | 29 s |

The first configuration exercises respins (two contents of one version,
both of which the inspector may verify, so the served revision switches),
revivals beside a live sibling, and the opt-in guards, with one verifier
run at a time: a second run there multiplies the states past what CI can
afford (36.9 million distinct, 7 minutes locally), and the races two runs
produce - conflicting verdicts, a verdict losing its guarded update, a
listing going stale across a revival - are single-row scenarios the other
two configurations check. The second exercises one blob referenced by two
rows, where the sole-live-reference accounting and the reclaim refcount
decide. The third has the content the archive scan rejects
(`Malicious = {2}`) beside one it may verify, with two concurrent runs, at
two stamps. In the first two the inspector's rejections are of bytes it
may also verify, and operator resolutions reject as well. All three
are exhaustive finite-state checks of the model, not simulations and not
proofs of the unbounded system; `MaxStamps = 3` admits publish, reject,
revive, and one respin in a single execution when no sample is spent on a
publish that then fails or loses its race. The GitHub runner takes about
three times the local figures (the previous model's first configuration:
2 min 3 s locally, 6 min 30 s in the merge-group run of #1932), which is
what the `tlc` job's 30-minute timeout is sized against.

## Non-vacuity

`RegistryLifecycle_witness.cfg` lists traps: each is the negation of a
behavior the model must be able to reach, so TLC must report it
**violated**, and the printed trace is the witness.
`RegistryLifecycleShared_witness.cfg` holds `NoSharedBlob`, which needs two
versions sharing one checksum. TLC stops at the first violation, so the
`tlc` job runs each trap alone, and fails if TLC does not report one
violated. From the repository root:

```sh
cargo check-tlc-witnesses --jar registry/proofs/tla2tools.jar \
  registry/proofs/RegistryLifecycle.tla \
  registry/proofs/RegistryLifecycle_witness.cfg \
  registry/proofs/RegistryLifecycleShared_witness.cfg
```

To see one witness's trace, keep only its line in a copy of the
configuration. The depths below are as the multi-worker search printed
them (not necessarily the shortest), and the action sequences are the
printed traces:

| Trap | Witnessed behavior | Depth | Trace |
| --- | --- | --- | --- |
| `NoVerified` | a revision is verified and served | 7 | publish, list, verdict |
| `NoRevivedVerified` | reject, revive by identical bytes, verify | 13 | publish, reject, revive (new stamp), list, verify |
| `NoStaleVerdictRefused` | a verdict listed against one generation is refused at its read after a revival restamped the row | 12 | publish, two listings, one rejects, revival, the other's read answers 409 |
| `NoVerdictLostRace` | a verdict's guarded update loses to a conflicting verdict between its read and its write | 10 | publish, two listings and reads, one applies, the other's update changes zero rows |
| `NoLostRace` | a publish's batch changes zero rows and the handler re-reads | 7 | two publishes of the same bytes, the second batch loses |
| `NoRespin` | two revisions of one version are live at once | 7 | publish, publish different bytes with the opt-in |
| `NoSharedBlob` | one blob is referenced by two live rows | 8 | the same bytes published under two versions |
| `NoRejection` | a rejection is applied and its reclaim decided | 7 | publish, list, reject |
| `NoInspectedRejection` | the inspector's rejection lands | 7 | publish the malicious content, list and inspect, reject |
| `NoManualRejection` | an operator's reject resolution lands on bytes the scan accepts, nothing inspected | 7 | publish, resolve `reject`, apply |
| `LiveBlobPresent` | see below | 14 | publish, reject, revival puts and commits and heals, then the reclaim's delete lands |

`LiveBlobPresent` (a live row with no in-flight delete always has its
blob) is **expected to fail**: the trace is the window the code documents
(`verifier.rs` `delete_blob_if_unreferenced`, `package.rs`
`revive_rejected_revision`, runbook "Orphaned R2 blobs"). A rejection
reads a zero refcount and decides a delete; a revival (or a deduplicating
publish of the same bytes) puts the blob, commits, and heals while the
delete is still in flight; the delete then lands. The row is live, its
blob is gone, and the artifact route answers the loud missing-blob error
until a byte-identical republish heals it (a heal whose reservation the
governor refuses leaves the same window). The model reproduces this
accepted behavior; it is not a new finding, and no invariant claims
otherwise.

## Sensitivity

Each mutation below was applied alone, the CI configuration re-run, and
the model restored. TLC reported the listed violation each time; the
unmodified properties detect the weakened guard.

Run on the smaller `MaxStamps = 2` constants of the witness configuration
(the shared-blob constants, with three stamps, for the two accounting
mutations and for the revival mutation, which needs two revivals in
flight), where each violation surfaces within seconds.

| Mutation | Weakened guard | Violated |
| --- | --- | --- |
| `VerApply` guard drops `pubAt[v][c] = s` | `MARK_REVISION_*` without `published_at = ?` | `TerminalDecidedOnCurrentGeneration` |
| `VerApply` guard drops `status = "pending"` | `MARK_REVISION_*` without `verification = 'pending'` | `VerifiedImmutable` (a late rejection flips a verified row) |
| `ReviveGuard` drops `stamp > pubAt[v][c]` | `REVIVE_REJECTED_REVISION` without `published_at < ?`: a revival that sampled the rejected generation's stamp lands | `TerminalDecidedOnCurrentGeneration` (the stale verdict lands on the revival) |
| `RefundOnReject` ignores the sole-reference count | `REFUND_STORED_BYTES_ON_REJECTION` without `COUNT(*) ... = 1` | `CounterExact` |
| `NewGuard` drops the opt-in conjunct | `INSERT_REVISION` without the `?10 OR NOT EXISTS` guard | `RespinNeedsOptIn` |
| `CountOnWrite` ignores existing live references | `COUNT_STORED_BYTES_ON_PUBLISH` without `COUNT(*) ... = 0` | `CounterExact` |
| `ReviveGuard` drops the rejected test | `REVIVE_REJECTED_REVISION` without `verification = 'rejected'` | `LiveGenerationStable` (a slow second revival restamps the first's live row) |
| `VerRead` skips the binding check and carries the row's stamp as the handler read it into `VerApply` | `verdict_binding_matches` removed and `MARK_REVISION_*` bound to the handler's `VERDICT_TARGET` read instead of the body | `TerminalDecidedOnCurrentGeneration` (a verdict listed against the rejected generation lands on its revival) |
| `Inspections` allows `verified` for every content | the archive scan bypassed | `VerifiedByInspection` (the malicious content verifies) |
| the run inspects bytes other than the listed content | both checksum checks removed, the driver's `undeclared_digest` and the inspector's own hash; either alone still refuses | `VerifiedByInspection` (benign bytes verify the malicious row) |
| the `reject` resolution renders `verified` | an operator verifying from the name alone | `VerifiedByInspection` (a `manual` basis on a verified row) |

The third row is the stamp guard's role made visible: the verdict guards
compare `published_at` exactly, so a revival that repeated the rejected
generation's stamp would let a delayed verdict of either direction land
on it; the revival guard refuses such a sample, and without it the ghost
generation detects what the stamp alone cannot. The
`VerRead` row is a double mutation because the listing binding is
enforced twice: `verdict_binding_matches` refuses a stale or mismatched
verdict loudly at the read, and the guarded `UPDATE` binds the same
body values, so a stale verdict that would apply is refused again by
D1 (as the lost-race `409`) if the Rust check were gone, while a stale
no-op or conflict answers as its transition says - removing the check
alone changes no row and is not a detected mutation. Binding the handler's own read instead, as the
code once did, would make the Rust check the only enforcement. The
three verifier rows test the driver model's arms, not a server statement:
the first two make `Inspections` or the digest hook inconsistent with
`Malicious` (the second against the skew/corruption threat the model
otherwise excludes, since the artifact route is content-addressed), and
only the third has a code anchor - `Pass::resolve` routes `verify`
through the full pass rather than a direct `PATCH`.

## What this does and does not establish

TLC exhaustively checks the stated properties over the finite
configurations above, for the model. It does not prove the Rust, the
SQL, D1, R2, or the Durable Object. The correspondence table is the
abstraction boundary: a statement whose guards drift from the model's
(`NewGuard`, `ReviveGuard`, `VerApply`'s guard, `CountOnWrite`,
`RefundOnReject`, `LiveRefs`, and the Rust `verdict_binding_matches`)
silently invalidates the result, which is
why `registry/AGENTS.md` asks a change to those statements to keep the
model faithful. The SQL validation tests (`tests/sql_validation/`) check
the individual statements against the schema; the Kani harnesses check
the quota functions; this model checks their composition under
concurrency, with the verifier's pass reduced to the outcomes its own
tests establish ("The verifier" above). Nothing here verifies the
inspection of archive bytes itself, the credentials, the yank column, the
backup queue, or the resolver-metadata invariance guards.

## Interpreting a counterexample

TLC prints the violated property and a trace: the initial state, then
each state with the action that produced it (`<PubBatch line N, col M>`,
and so on). Read the request slots to see which handler was between its
read and its write when another one committed. Decide whether the trace
is a modeling mistake (an action that does not match its row of the
table above), a stated assumption being false, or a real interleaving
the code permits; only the last is an implementation finding, and it
belongs in its own fix PR with a regression test, never in a weakened
property.
