# Registry lifecycle model

`RegistryLifecycle.tla` is a TLA+ specification of the hosted registry's
publish / verification lifecycle for one package, checked with the TLC
model checker. It covers what the Kani harnesses (single functions) and
the SQL validation tests (single statements) cannot: how the preflight
reads, the guarded D1 batches, the R2 writes, and the governor ledger
interact under concurrent publishes, respins, verdicts, rejections,
revivals, retries, lost races, failed batches, and delayed R2 deletes.

CI runs the configurations marked below on every registry-relevant change
(`.github/workflows/registry.yml`, job `tlc`, feeding the required
`registry-build` aggregate). A missing tool, a digest mismatch, a
violation, or the job timeout fails the check.

## Running it

```sh
cd registry/proofs
curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
  --output tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar
sha256sum --check tla2tools.jar.sha256
java -XX:+UseParallelGC -jar tla2tools.jar -workers auto -cleanup -config RegistryLifecycle.cfg RegistryLifecycle.tla
java -XX:+UseParallelGC -jar tla2tools.jar -workers auto -cleanup -config RegistryLifecycleShared.cfg RegistryLifecycle.tla
```

The pinned tool is the TLA+ `v1.7.4` release (TLC 2.19, 2024-08-08); its
`tla2tools.jar` digest is `tla2tools.jar.sha256`, and the release page's
SHA-1 for the same file is `bee4a54f3ee3d4afc347c3240ec2d9e93b075104`. Any
Java 11 or later runs it. TLC exits 0 only when the check completed with
no violation.

## What is modeled

The model follows the registry at commit `3cc279547afa134c87f562736bdcdd66cde958d3`
(`registry/src/glue/bearer/package.rs`, `registry/src/glue/bearer/verifier.rs`,
`registry/src/verify.rs`, `registry/src/sql/packages.rs`, `registry/src/sql/meta.rs`,
`registry/src/sql/downloads.rs`, `registry/src/sql/backup.rs`,
`registry/migrations/0001_init.sql`).

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
| `clock` | the last stamp sampled |
| `counter` | `meta.total_stored_bytes` |
| `blob[c]` | the R2 object `blobs/sha256/<c>` exists |
| `ledger[c]` | the governor's primary-pool entry for that key: none, reserved, committed |
| `deletes` | R2 deletes decided (refcount read zero) but not yet landed or failed |
| `pub[p]`, `ver[w]` | what an in-flight publish or verdict handler holds between its reads and writes |
| `gen[v][c]`, `decided[v][c]` | ghost history for the properties - the row's publish-event count, and the generation its terminal verdict was computed against; no guard reads them |

### Actions and the code they abstract

| Action | Rust | D1 / R2 / ledger |
| --- | --- | --- |
| `PubStart` | `package.rs` `revision_disposition` | `EXISTING_REVISIONS`; decides no-op `200`, `409` new-revision-required, revive, or new |
| `PubAnswer` | the `Answered` arms; `heal_blobs_on_retry` on the `200` | no D1 write; the no-op heads and re-puts its blob |
| `PubStore` | `now_iso8601()` sampled (before the quota reads and the storage work), then `persist_new_revision` head-then-put or `revive_rejected_revision` unconditional put; or the governor refuses admission or the reservation, or the put fails | governor reserve (idempotent per key), `bucket.put`; a reservation taken before a failed put stays |
| `PubBatch` | the `db.batch` in both write paths, `changed_rows == 0` as `Persist::Lost`; a failed batch or a request that dies after the upload | `INSERT_PACKAGE`, `INSERT_VERSION_ROW`, `COUNT_STORED_BYTES_ON_PUBLISH`, `INSERT_REVISION`; or `COUNT_STORED_BYTES_ON_REVIVAL`, `REVIVE_REJECTED_REVISION` - one transaction, guards evaluated against the rows as they are then |
| `PubReread` | the `Persist::Lost` re-read | `EXISTING_REVISIONS` again; `200` (healing) or `409` |
| `PubFinish` | `governor_client::settle` (best-effort), `heal_blob_if_reclaimed` | ledger commit; head, reserve, re-put if the blob is gone - or the reservation is refused and the repair skipped |
| `VerList` | `admin_versions_response` | `REVISIONS_BY_VERIFICATION_STATUS('pending')`: snapshots `(checksum, published_at)` |
| `VerRead` | `verdict_response`: `verdict_binding_matches` compares the listing's `(checksum, published_at)` with the row just read, then `verify::transition`; a repeat rejection re-drives `delete_blob_if_unreferenced` | `VERDICT_TARGET`; the row's stamp as read here is what the update below binds |
| `VerApply` | `apply_verdict` / `apply_rejection`, `changed_rows == 0` as the lost race; a failed batch | `MARK_REVISION_VERIFIED` (+ `ENQUEUE_VERIFIED_BACKUP`), or `REFUND_STORED_BYTES_ON_REJECTION` then `MARK_REVISION_REJECTED` - one transaction, `published_at = ?` bound to `target.published_at` (the handler's own read, not the listing's) |
| `VerReclaim` | `delete_blob_if_unreferenced` after the rejection batch; or the request dying first | `COUNT_LIVE_BLOB_REFERENCES`, then the delete is in flight |
| `LandDelete`, `DropDelete` | the R2 delete landing, or failing into an orphan | `bucket.delete` |

The read plane is modeled as the two predicates the invariants use:
`Current(v)` is the `current_revisions` view (the verified revision with
the greatest `(published_at, revision)`), and `ArtifactReadable` is
`verify::artifact_readable`.

### Assumptions

- **Every publish event samples a distinct stamp.** `published_at` is
  `now_iso8601()`, sampled after the disposition read and before the
  storage work, so a slow request commits with an older stamp than a
  faster one that sampled later; the model reproduces that (a row's
  generations need not carry increasing stamps, and `current_revisions`
  serves by stamp, not commit order). What the model's clock adds is that
  no two samples are equal, and the generation-isolation results rest on
  exactly that: the guards compare the stamp as the SQL compares the
  column - no generation counter replaces the production identity - and
  `REVIVE_REJECTED_REVISION` binds no stamp the reviver observed, so two
  publish events of one row that sampled the same millisecond would be
  one generation to the SQL, and a verdict listed against the first could
  land on the second (the third sensitivity row shows the consequence).
  In production that needs two publish requests for the same row to
  sample the same millisecond: two revivals of one rejected row racing
  through the same Worker, or Worker clocks that disagree by at least the
  gap between them. The model excludes it rather than argues it away.
  Equal stamps across different rows would only reach the
  `(published_at, revision)` tie-break of `current_revisions`, which the
  model therefore never exercises.
- One package: `INSERT_PACKAGE`, `INSERT_VERSION_ROW`, and the `-`/`_`
  twin guard always succeed; the twin `400` is unreachable.
- Resolver-metadata invariance (`dependencies`, `features`, `standards`,
  `links`) is not modeled: the model's `409`s are the opt-in and
  immutability refusals only.
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
- The verifier's own pipeline is abstract: a verdict request chooses
  `verified` or `rejected` freely at `VerRead`. The server-side rules are
  what is checked.
- `MaxStamps` bounds the stamps one execution samples; a publish that
  would sample past the bound can still fail or be answered from its
  preflight, never reach the storage work. A sample spent by a publish
  that fails before D1 is elided (the model leaves the clock untouched),
  so the bound counts samples that can reach a row.

## Properties

Checked as `INVARIANTS` and action `PROPERTIES` on every state and step
of every configuration:

| Property | Statement |
| --- | --- |
| `TypeOK`, `RowsStamped` | domains; a row exists iff stamped, and stamps are unique (the clock's consequence - a sanity check on the model, not a production property) |
| `PublicReadsAreVerified` | without the `verify` scope, an artifact is readable only when `verified`; the served pointer names a verified row (both follow from the two read predicates' definitions: this documents the gate rather than tests the write path) |
| `TerminalDecidedOnCurrentGeneration` | a `verified` or `rejected` row was decided by a verdict computed against its current generation (the ghost `gen`, not the stamp): no delayed verdict lands on a revival of the same bytes |
| `PendingUndecided` | a pending row carries no verdict (revival clears the old one) |
| `CounterExact` | `total_stored_bytes` equals one archive size per checksum with a live (pending or verified) row, in every D1-visible state |
| `LedgerConservative` | an existing R2 object always has a ledger entry: orphans are conservative spend |
| `VerifiedImmutable` | a verified row never changes status or stamp |
| `RejectedOnlyRevives` | a rejected row only ever returns to pending (never to verified) |
| `LiveGenerationStable` | a live row is never restamped: no publish, retry, or revival rewrites it |
| `ServedChangesOnlyByVerification` | what a version serves changes only when a row becomes verified - a pending respin, a rejection, or a revival never disturbs it |
| `RespinNeedsOptIn` | a different-bytes revision joins a live sibling only through a request that carried `new-revision=true` |
| `AnsweredWithoutCommitWritesNothing` | the `200` no-op, the preflight `409`s, a failed publish batch, and a verdict refused or repeated at its read change no row and no accounting |

`PendingUndecided` and `AnsweredWithoutCommitWritesNothing` likewise
follow from how the actions are written; they pin the model's shape
against later edits rather than test a rule of the code. The remaining
properties are stated over the rows and the ghost history, not over the
actions' own guard expressions, so weakening a guard breaks them (see
"Sensitivity").

## Configurations and results

All runs are exhaustive breadth-first checks under the pinned TLC with
symmetry over `Versions`, `Publishers`, and `Verifiers`; "distinct" is
the number of states checked after symmetry reduction.

| Configuration | Constants | Distinct / generated states | Depth | Result | Local time (10 cores) |
| --- | --- | --- | --- | --- | --- |
| `RegistryLifecycle.cfg` (CI) | `Versions = {v1}`, `Checksums = {1, 2}`, `Publishers = {p1, p2}`, `Verifiers = {w1, w2}`, `MaxStamps = 3` | 9,806,544 / 82,180,055 | 34 | complete, no violation | 2 min 3 s |
| `RegistryLifecycleShared.cfg` (CI) | `Versions = {v1, v2}`, `Checksums = {1}`, `Publishers = {p1, p2}`, `Verifiers = {w1, w2}`, `MaxStamps = 3` | 1,539,799 / 12,848,429 | 31 | complete, no violation | 27 s |

The first configuration exercises respins (two contents of one version),
revivals beside a live sibling, and stale or conflicting verdicts; the
second exercises one blob referenced by two rows, where the
sole-live-reference accounting and the reclaim refcount decide. Both
are exhaustive finite-state checks of the model, not simulations and not
proofs of the unbounded system; `MaxStamps = 3` admits publish, reject,
revive, and one respin in a single execution when no sample is spent on a
publish that then fails or loses its race. The two-version,
two-content configuration also completes but takes long enough that CI
does not run it.

## Non-vacuity

`RegistryLifecycle_witness.cfg` lists traps: each is the negation of a
behavior the model must be able to reach, so TLC must report it
**violated**, and the printed trace is the witness. TLC stops at the first
violation, so to see one particular witness keep only its line (and for
`NoSharedBlob` use two versions and one checksum). Every trap was run
alone against the final model; the trace depths and action sequences:

```sh
java -jar tla2tools.jar -workers auto -cleanup -config RegistryLifecycle_witness.cfg RegistryLifecycle.tla
```

| Trap | Witnessed behavior | Depth | Trace |
| --- | --- | --- | --- |
| `NoVerified` | a revision is verified and served | 7 | publish, list, verdict |
| `NoRevivedVerified` | reject, revive by identical bytes, verify | 14 | publish, reject, revive (new stamp), list, verify |
| `NoStaleVerdictRefused` | a verdict listed against one generation is refused at its read after a revival restamped the row | 12 | publish, two listings, one rejects, revival, the other's read answers 409 |
| `NoVerdictLostRace` | a verdict's guarded update loses to a conflicting verdict between its read and its write | 10 | publish, two listings and reads, one applies, the other's update changes zero rows |
| `NoLostRace` | a publish's batch changes zero rows and the handler re-reads | 7 | two publishes of the same bytes, the second batch loses |
| `NoRespin` | two revisions of one version are live at once | 7 | publish, publish different bytes with the opt-in |
| `NoSharedBlob` | one blob is referenced by two live rows | 8 | the same bytes published under two versions |
| `NoRejection` | a rejection is applied and its reclaim decided | 7 | publish, list, reject |
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
| a revival keeps the rejected generation's stamp | the clock assumption fails: two publish events of one row sample the same `now_iso8601()` | `TerminalDecidedOnCurrentGeneration` (the stale verdict lands on the revival) |
| `RefundOnReject` ignores the sole-reference count | `REFUND_STORED_BYTES_ON_REJECTION` without `COUNT(*) ... = 1` | `CounterExact` |
| `NewGuard` drops the opt-in conjunct | `INSERT_REVISION` without the `?10 OR NOT EXISTS` guard | `RespinNeedsOptIn` |
| `CountOnWrite` ignores existing live references | `COUNT_STORED_BYTES_ON_PUBLISH` without `COUNT(*) ... = 0` | `CounterExact` |
| `ReviveGuard` drops the rejected test | `REVIVE_REJECTED_REVISION` without `verification = 'rejected'` | `LiveGenerationStable` (a slow second revival restamps the first's live row) |
| `VerRead` skips the `published_at` binding check | `verdict_binding_matches` removed | `TerminalDecidedOnCurrentGeneration` (a verdict listed against the rejected generation lands on its revival) |

The third row is the clock assumption's role made visible: the production
guards compare `published_at` exactly, so a revival whose stamp equals the
rejected generation's would let a delayed verdict of either direction land
on it. The ghost generation detects it; the stamp alone cannot. The last
row shows that the Rust check is load-bearing: the guarded `UPDATE` binds
the stamp the handler itself just read, which closes the read-to-update
race but says nothing about the listing the verdict was computed from,
so the listing-to-row binding is enforced by `verdict_binding_matches`
alone. (Binding the listing's `published_at` into the `UPDATE` instead
would make D1 enforce both; the model records the code as it is.)

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
concurrency. Nothing here verifies the external verifier's inspection of
archive bytes, the yank column, the backup queue, or the resolver-metadata
invariance guards.

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
