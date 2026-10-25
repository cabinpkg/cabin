------------------------- MODULE RegistryLifecycle -------------------------
(***************************************************************************)
(* The hosted registry's publish / verification lifecycle for one package  *)
(* (registry/docs/architecture.md, "The write path" and "The verification  *)
(* lifecycle"), as the D1 statements in registry/src/sql/ execute it, with *)
(* R2 and the governor ledger as the separate, non-transactional stores    *)
(* they are.  README.md maps every action to the Rust and SQL it abstracts *)
(* and lists what the model leaves out.                                    *)
(*                                                                         *)
(* A revision row is keyed by (version, checksum): the packaging-revision  *)
(* id is derived from the checksum, so the two name the same row.  Its    *)
(* production generation identity is the pair (checksum, published_at);   *)
(* `pubAt` stands for published_at as the handler sampled it from a clock  *)
(* that never repeats a value (the assumption README.md states), and the  *)
(* guards compare it exactly as the SQL compares the column.  `gen` and   *)
(* `decided` are ghost history for the properties only; no guard reads    *)
(* them.                                                                   *)
(*                                                                         *)
(* The external verifier (the registry-verify workflow's driver) is a     *)
(* participant on its own schedule: it lists, downloads, inspects, and    *)
(* PATCHes.  The archive inspector is the oracle `Inspections`: it rejects *)
(* Malicious bytes outright and is otherwise free (README.md, "The        *)
(* verifier").                                                             *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS
    Versions,    \* the version slots of one package
    Checksums,   \* archive contents, each named by the byte count it stores
    Publishers,  \* concurrent publish request slots
    Verifiers,   \* concurrent verdict request slots
    MaxStamps,   \* how many publish events one execution can stamp
    Malicious    \* the contents the archive inspector rejects (its result, abstracted)

ASSUME /\ Checksums \subseteq Nat \ {0}
       /\ MaxStamps \in Nat
       /\ Malicious \subseteq Checksums

Size(c) == c   \* revisions.archive_size, a function of the bytes

VARIABLES
    status,   \* [Versions -> [Checksums -> Status]]: revisions.verification, "absent" for no row
    pubAt,    \* [Versions -> [Checksums -> Nat]]: revisions.published_at, 0 for no row
    clock,    \* the last stamp now_iso8601() issued
    counter,  \* meta.total_stored_bytes
    blob,     \* [Checksums -> BOOLEAN]: the R2 object blobs/sha256/<checksum> exists
    ledger,   \* [Checksums -> Ledger]: the governor's primary storage entry for that key
    deletes,  \* SUBSET Checksums: R2 deletes decided but not yet landed or failed
    pub,      \* [Publishers -> publish request]
    ver,      \* [Verifiers -> verdict request]
    gen,      \* ghost: how many publish events (the insert, each revival) the row has had
    decided,  \* ghost: the generation a row's terminal verdict was computed against, 0 for none
    basis     \* ghost: what rendered a row's terminal verdict - the inspector, or an operator's reject resolution

vars == <<status, pubAt, clock, counter, blob, ledger, deletes, pub, ver, gen, decided, basis>>
rows == <<status, pubAt>>

Status == {"absent", "pending", "verified", "rejected"}
Live == {"pending", "verified"}
Terminal == {"verified", "rejected"}
Ledger == {"none", "reserved", "committed"}

\* Request slots: an idle slot carries nothing (a response, once sent, is
\* not registry state); a busy one carries what the handler holds.
Idle == [phase |-> "idle"]
PubPhase == {"idle", "disposed", "stored", "written", "lost"}
Disp == {"new", "revive", "noop", "refuse"}
VerPhase == {"idle", "decided", "apply", "reclaim"}
Verdicts == {"verified", "rejected"}
Basis == {"none", "inspected", "manual"}

\* Publishers, verifiers, and versions are interchangeable; checksums are
\* not (they carry sizes and break the served-revision tie).
Symmetry == Permutations(Versions) \cup Permutations(Publishers) \cup Permutations(Verifiers)

-----------------------------------------------------------------------------
(* Row predicates, each the SQL it names *)

IsLive(v, c) == status[v][c] \in Live

\* `... AND verification IN ('pending', 'verified') AND checksum <> ?`
LiveOtherBytes(v, c) == \E d \in Checksums \ {c} : IsLive(v, d)

\* COUNT_LIVE_BLOB_REFERENCES: `checksum = ? AND verification != 'rejected'`
\* (the column only ever holds the three states, so this is `IN (live)`)
LiveRefs(c) == Cardinality({v \in Versions : IsLive(v, c)})

\* revision_disposition (package.rs): what the publish preflight read says
Disposition(v, c, optIn) ==
    IF status[v][c] \in Live THEN "noop"
    ELSE IF LiveOtherBytes(v, c) /\ ~optIn THEN "refuse"
    ELSE IF status[v][c] = "rejected" THEN "revive"
    ELSE "new"

\* INSERT_REVISION's in-batch guards (one package: the version row exists)
NewGuard(v, c, optIn) ==
    /\ status[v][c] = "absent"
    /\ optIn \/ ~LiveOtherBytes(v, c)

\* REVIVE_REJECTED_REVISION's guards; the checksum is the row's own
ReviveGuard(v, c, optIn) ==
    /\ status[v][c] = "rejected"
    /\ optIn \/ ~LiveOtherBytes(v, c)

\* COUNT_STORED_BYTES_ON_PUBLISH / _ON_REVIVAL: the sole live reference counts
CountOnWrite(c) == IF LiveRefs(c) = 0 THEN Size(c) ELSE 0

\* REFUND_STORED_BYTES_ON_REJECTION, decided before the flip
RefundOnReject(c) == IF LiveRefs(c) = 1 THEN Size(c) ELSE 0

\* heal_blob_if_reclaimed / heal_blobs_on_retry: head, reserve, re-put -
\* or the governor refuses the reservation and the repair is skipped
Heal(c) ==
    \/ /\ blob' = [blob EXCEPT ![c] = TRUE]
       /\ ledger' = [ledger EXCEPT ![c] = IF ledger[c] = "none" THEN "reserved" ELSE ledger[c]]
    \/ UNCHANGED <<blob, ledger>>

-----------------------------------------------------------------------------
(* The read plane *)

\* current_revisions: the verified revision with the greatest (published_at, revision)
Current(v) ==
    LET served == {c \in Checksums : status[v][c] = "verified"}
    IN IF served = {} THEN 0
       ELSE CHOOSE c \in served :
                \A d \in served \ {c} :
                    \/ pubAt[v][c] > pubAt[v][d]
                    \/ pubAt[v][c] = pubAt[v][d] /\ c > d

\* verify.rs artifact_readable
ArtifactReadable(v, c, verifyScope) ==
    CASE status[v][c] = "verified" -> TRUE
      [] status[v][c] = "pending" -> verifyScope
      [] OTHER -> FALSE

-----------------------------------------------------------------------------
(* The verifier *)

\* cabin-registry-verify's possible verdicts on an archive.  The archive
\* scan rejects Malicious bytes whatever else is true; for other bytes the
\* verdict also depends on the generation's stored metadata (a revival may
\* carry corrected metadata for the same bytes) and on the pinned upstream
\* fetch, which the model leaves free.
Inspections(c) == IF c \in Malicious THEN {"rejected"} ELSE Verdicts

-----------------------------------------------------------------------------
Init ==
    /\ status = [v \in Versions |-> [c \in Checksums |-> "absent"]]
    /\ pubAt = [v \in Versions |-> [c \in Checksums |-> 0]]
    /\ clock = 0
    /\ counter = 0
    /\ blob = [c \in Checksums |-> FALSE]
    /\ ledger = [c \in Checksums |-> "none"]
    /\ deletes = {}
    /\ pub = [p \in Publishers |-> Idle]
    /\ ver = [w \in Verifiers |-> Idle]
    /\ gen = [v \in Versions |-> [c \in Checksums |-> 0]]
    /\ decided = [v \in Versions |-> [c \in Checksums |-> 0]]
    /\ basis = [v \in Versions |-> [c \in Checksums |-> "none"]]

-----------------------------------------------------------------------------
(* Publish: PUT /api/v1/packages/<scope>/<name>/<version>[?new-revision=true] *)

\* The preflight read (EXISTING_REVISIONS) decides the request's shape.
PubStart(p) ==
    /\ pub[p].phase = "idle"
    /\ \E v \in Versions, c \in Checksums, optIn \in BOOLEAN :
        pub' = [pub EXCEPT ![p] = [phase |-> "disposed", v |-> v, c |-> c, optIn |-> optIn,
                                   disp |-> Disposition(v, c, optIn)]]
    /\ UNCHANGED <<status, pubAt, clock, counter, blob, ledger, deletes, ver, gen, decided, basis>>

\* Answered from the preflight: the byte-identical 200 no-op (which heals
\* its blob) or the 409 new-revision-required refusal.  Nothing is written.
PubAnswer(p) ==
    /\ pub[p].phase = "disposed"
    /\ pub[p].disp \in {"noop", "refuse"}
    /\ IF pub[p].disp = "noop"
       THEN Heal(pub[p].c) /\ pub' = [pub EXCEPT ![p] = Idle]
       ELSE UNCHANGED <<blob, ledger>> /\ pub' = [pub EXCEPT ![p] = Idle]
    /\ UNCHANGED <<status, pubAt, clock, counter, deletes, ver, gen, decided, basis>>

\* The handler samples now_iso8601() here - before the quota reads and the
\* storage work, so a slow request commits with an older stamp than a
\* faster one that sampled later - then R2 first: a new revision uploads
\* unless the content-addressed object is already there; a revival always
\* re-puts.  The upload reserves its bytes in the ledger (idempotent per key).
\* The request can also end here without reaching D1: the governor refuses
\* admission or the reservation (nothing written), or the put fails after
\* the reservation (a conservative reserved entry with no object).
PubStore(p) ==
    /\ pub[p].phase = "disposed"
    /\ pub[p].disp \in {"new", "revive"}
    /\ LET c == pub[p].c
           reserve == [ledger EXCEPT ![c] = IF ledger[c] = "none" THEN "reserved" ELSE ledger[c]]
       IN \/ /\ clock < MaxStamps
             /\ IF pub[p].disp = "new" /\ blob[c]
                THEN UNCHANGED <<blob, ledger>>
                ELSE blob' = [blob EXCEPT ![c] = TRUE] /\ ledger' = reserve
             /\ clock' = clock + 1
             /\ pub' = [pub EXCEPT ![p] = [phase |-> "stored", v |-> pub[p].v, c |-> c,
                                        optIn |-> pub[p].optIn, disp |-> pub[p].disp,
                                        stamp |-> clock + 1]]
          \/ /\ ledger' \in {ledger, reserve}
             /\ pub' = [pub EXCEPT ![p] = Idle]
             /\ UNCHANGED <<blob, clock>>
    /\ UNCHANGED <<status, pubAt, counter, deletes, ver, gen, decided, basis>>

\* One D1 batch: the guarded insert or revival plus the stored-bytes bump,
\* evaluated against the rows as they are now, not as the preflight saw
\* them.  Zero changed rows is a lost race; the batch can also fail
\* outright (or the request die after the upload), committing nothing.
PubBatch(p) ==
    /\ pub[p].phase = "stored"
    /\ LET v == pub[p].v
           c == pub[p].c
           guard == IF pub[p].disp = "new"
                    THEN NewGuard(v, c, pub[p].optIn)
                    ELSE ReviveGuard(v, c, pub[p].optIn)
       IN \/ /\ guard
             /\ status' = [status EXCEPT ![v][c] = "pending"]
             /\ pubAt' = [pubAt EXCEPT ![v][c] = pub[p].stamp]
             /\ counter' = counter + CountOnWrite(c)
             /\ gen' = [gen EXCEPT ![v][c] = gen[v][c] + 1]
             /\ decided' = [decided EXCEPT ![v][c] = 0]
             /\ basis' = [basis EXCEPT ![v][c] = "none"]
             /\ pub' = [pub EXCEPT ![p].phase = "written"]
          \/ /\ ~guard
             /\ pub' = [pub EXCEPT ![p].phase = "lost"]
             /\ UNCHANGED <<status, pubAt, counter, gen, decided, basis>>
          \/ /\ pub' = [pub EXCEPT ![p] = Idle]
             /\ UNCHANGED <<status, pubAt, counter, gen, decided, basis>>
    /\ UNCHANGED <<clock, blob, ledger, deletes, ver>>

\* After a lost race the handler re-reads and answers as if it had arrived
\* after the winner: the no-op (healing the blob) or a 409.
PubReread(p) ==
    /\ pub[p].phase = "lost"
    /\ LET d == Disposition(pub[p].v, pub[p].c, pub[p].optIn) IN
       IF d = "noop"
       THEN Heal(pub[p].c) /\ pub' = [pub EXCEPT ![p] = Idle]
       ELSE UNCHANGED <<blob, ledger>> /\ pub' = [pub EXCEPT ![p] = Idle]
    /\ UNCHANGED <<status, pubAt, clock, counter, deletes, ver, gen, decided, basis>>

\* After the commit: settle the reservation (best-effort - a lost settle
\* leaves the conservative reserved entry), then heal the blob if a
\* reclaim delete beat the batch.
PubFinish(p) ==
    /\ pub[p].phase = "written"
    /\ LET c == pub[p].c
           settled == [ledger EXCEPT ![c] = "committed"]
       IN \/ /\ blob' = [blob EXCEPT ![c] = TRUE]
             /\ ledger' \in {settled, ledger}
          \/ UNCHANGED blob /\ ledger' \in {settled, ledger}
    /\ pub' = [pub EXCEPT ![p] = Idle]
    /\ UNCHANGED <<status, pubAt, clock, counter, deletes, ver, gen, decided, basis>>

-----------------------------------------------------------------------------
(* Verdicts: GET /api/v1/admin/versions?status=pending, then                *)
(* PATCH /api/v1/admin/versions/<scope>/<name>/<version>                    *)

\* One verifier run over one listed entry (xtask-registry-admin verify.rs,
\* Pass::version): the listing (admin_versions_response, read with the
\* run's verify-scoped token) snapshots the pending row's binding
\* (checksum, published_at) - the ghost `gen` remembers which generation
\* that was - then the name advisories, the artifact download with the
\* same token (a pending row is readable to the verify scope, and the
\* download needs its blob), the digest check against the listed checksum,
\* the optional upstream fetch, the inspector child, and the OIDC mint for
\* the PATCH.  The advisory's abstain, a digest mismatch, and every failure
\* end the run for this entry without a PATCH: the row stays pending and
\* nothing else changes, so those outcomes are the stutter.  An operator's
\* `reject` resolution (Pass::resolve) composes the advisory rejection
\* from the listing entry and inspects nothing; the `verify` resolution is
\* the same pass with the advisory gate waived, so the inspector still
\* renders its verdict.
\*
\* The listing and the download are one step although the driver reads
\* them minutes apart: a PATCH lands only while the row is still pending
\* at the listed stamp, the download needs the blob while the row is
\* readable, and both hold at the download instant of any landing PATCH -
\* when this step, taken then, snapshots the same stamp and renders the
\* same request; a PATCH that would be refused changes nothing either way.
VerList(w) ==
    /\ ver[w].phase = "idle"
    /\ \E v \in Versions, c \in Checksums :
        /\ status[v][c] = "pending"
        /\ LET render(verdict, how) ==
                   ver' = [ver EXCEPT ![w] = [phase |-> "decided", v |-> v, c |-> c,
                                              stamp |-> pubAt[v][c], gen |-> gen[v][c],
                                              verdict |-> verdict, basis |-> how]]
           IN \/ /\ blob[c]
                 /\ \E bytes \in Checksums : \E verdict \in Inspections(bytes) :
                     /\ bytes = c   \* undeclared_digest and the inspector's own hash (a mutation hook, README "Sensitivity")
                     /\ render(verdict, "inspected")
              \/ render("rejected", "manual")
    /\ UNCHANGED <<status, pubAt, clock, counter, blob, ledger, deletes, pub, gen, decided, basis>>

\* The PATCH arrives: verdict_authn consumes its single-use OIDC jti in
\* its own transaction first (a replayed PATCH is refused there and never
\* reaches this read; not modeled), then the handler's VERDICT_TARGET
\* read: verdict_binding_matches compares the listing's (checksum,
\* published_at) against the row it just read, then the transition table
\* (verify.rs) decides.  What the handler carries into its guarded UPDATE
\* is the row's stamp as *it* read it - the SQL binds target.published_at,
\* not the listing's - so the listing-to-row binding is enforced here, in
\* Rust, and nowhere else.  A repeat rejection re-drives the reclaim.
VerRead(w) ==
    /\ ver[w].phase = "decided"
    /\ LET v == ver[w].v
           c == ver[w].c
           cur == status[v][c]
           verdict == ver[w].verdict
       IN IF pubAt[v][c] # ver[w].stamp
          THEN ver' = [ver EXCEPT ![w] = Idle] /\ UNCHANGED deletes
          ELSE CASE cur = "pending" ->
                       \* the stamp carried on is the row's as read (equal here, by the check above)
                       /\ ver' = [ver EXCEPT ![w].phase = "apply", ![w].stamp = pubAt[v][c]]
                       /\ UNCHANGED deletes
                 [] cur = verdict ->
                       /\ ver' = [ver EXCEPT ![w] = Idle]
                       /\ deletes' = IF verdict = "rejected" /\ LiveRefs(c) = 0
                                     THEN deletes \cup {c} ELSE deletes
                 [] OTHER ->
                       ver' = [ver EXCEPT ![w] = Idle] /\ UNCHANGED deletes
    /\ UNCHANGED <<status, pubAt, clock, counter, blob, ledger, pub, gen, decided, basis>>

\* One D1 batch: MARK_REVISION_VERIFIED, or REFUND_STORED_BYTES_ON_REJECTION
\* then MARK_REVISION_REJECTED, guarded on the row still being pending with
\* the stamp the handler read (the read-to-update race), evaluated now.
VerApply(w) ==
    /\ ver[w].phase = "apply"
    /\ LET v == ver[w].v
           c == ver[w].c
           s == ver[w].stamp
           guard == status[v][c] = "pending" /\ pubAt[v][c] = s
       IN \/ /\ guard /\ ver[w].verdict = "verified"
             /\ status' = [status EXCEPT ![v][c] = "verified"]
             /\ decided' = [decided EXCEPT ![v][c] = ver[w].gen]
             /\ basis' = [basis EXCEPT ![v][c] = ver[w].basis]
             /\ ver' = [ver EXCEPT ![w] = Idle]
             /\ UNCHANGED counter
          \/ /\ guard /\ ver[w].verdict = "rejected"
             /\ LET refund == RefundOnReject(c) IN
                counter' = IF counter > refund THEN counter - refund ELSE 0
             /\ status' = [status EXCEPT ![v][c] = "rejected"]
             /\ decided' = [decided EXCEPT ![v][c] = ver[w].gen]
             /\ basis' = [basis EXCEPT ![v][c] = ver[w].basis]
             /\ ver' = [ver EXCEPT ![w].phase = "reclaim"]
          \/ /\ ~guard
             /\ ver' = [ver EXCEPT ![w] = Idle]
             /\ UNCHANGED <<status, counter, decided, basis>>
          \/ /\ ver' = [ver EXCEPT ![w] = Idle]
             /\ UNCHANGED <<status, counter, decided, basis>>
    /\ UNCHANGED <<pubAt, clock, blob, ledger, deletes, pub, gen>>

\* delete_blob_if_unreferenced after the flip: a separate refcount read
\* decides the R2 delete, which lands (or fails) later.  The request can
\* also die here, leaving the blob an orphan.
VerReclaim(w) ==
    /\ ver[w].phase = "reclaim"
    /\ \/ /\ deletes' = IF LiveRefs(ver[w].c) = 0 THEN deletes \cup {ver[w].c} ELSE deletes
          /\ ver' = [ver EXCEPT ![w] = Idle]
       \/ /\ UNCHANGED deletes
          /\ ver' = [ver EXCEPT ![w] = Idle]
    /\ UNCHANGED <<status, pubAt, clock, counter, blob, ledger, pub, gen, decided, basis>>

-----------------------------------------------------------------------------
(* R2: a decided delete lands whenever, or fails and leaves an orphan.  *)

LandDelete(c) ==
    /\ c \in deletes
    /\ blob' = [blob EXCEPT ![c] = FALSE]
    /\ deletes' = deletes \ {c}
    /\ UNCHANGED <<status, pubAt, clock, counter, ledger, pub, ver, gen, decided, basis>>

DropDelete(c) ==
    /\ c \in deletes
    /\ deletes' = deletes \ {c}
    /\ UNCHANGED <<status, pubAt, clock, counter, blob, ledger, pub, ver, gen, decided, basis>>

-----------------------------------------------------------------------------
Next ==
    \/ \E p \in Publishers :
        PubStart(p) \/ PubAnswer(p) \/ PubStore(p) \/ PubBatch(p) \/ PubReread(p) \/ PubFinish(p)
    \/ \E w \in Verifiers : VerList(w) \/ VerRead(w) \/ VerApply(w) \/ VerReclaim(w)
    \/ \E c \in Checksums : LandDelete(c) \/ DropDelete(c)

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* Invariants *)

TypeOK ==
    /\ status \in [Versions -> [Checksums -> Status]]
    /\ pubAt \in [Versions -> [Checksums -> 0..MaxStamps]]
    /\ clock \in 0..MaxStamps
    /\ counter \in Nat
    /\ blob \in [Checksums -> BOOLEAN]
    /\ ledger \in [Checksums -> Ledger]
    /\ deletes \subseteq Checksums
    /\ \A p \in Publishers :
        /\ pub[p].phase \in PubPhase
        /\ pub[p].phase # "idle" =>
            /\ pub[p].v \in Versions /\ pub[p].c \in Checksums /\ pub[p].optIn \in BOOLEAN
            /\ pub[p].disp \in Disp
            /\ pub[p].phase \in {"stored", "written", "lost"} =>
                pub[p].disp \in {"new", "revive"} /\ pub[p].stamp \in 1..MaxStamps
    /\ \A w \in Verifiers :
        /\ ver[w].phase \in VerPhase
        /\ ver[w].phase # "idle" =>
            /\ ver[w].v \in Versions /\ ver[w].c \in Checksums
            /\ ver[w].stamp \in 1..MaxStamps /\ ver[w].gen \in 1..MaxStamps
        /\ ver[w].phase \in {"decided", "apply", "reclaim"} =>
            ver[w].verdict \in Verdicts /\ ver[w].basis \in Basis \ {"none"}
    /\ gen \in [Versions -> [Checksums -> 0..MaxStamps]]
    /\ decided \in [Versions -> [Checksums -> 0..MaxStamps]]
    /\ basis \in [Versions -> [Checksums -> Basis]]

\* A row exists exactly when it has been stamped; stamps are unique (each
\* sample advances the clock, so a sample never repeats).
RowsStamped ==
    /\ \A v \in Versions, c \in Checksums :
        /\ (status[v][c] = "absent") <=> (pubAt[v][c] = 0)
        /\ (status[v][c] = "absent") <=> (gen[v][c] = 0)
    /\ \A v, u \in Versions, c, d \in Checksums :
        (pubAt[v][c] = pubAt[u][d] /\ pubAt[v][c] # 0) => (v = u /\ c = d)

\* Ordinary (verify-scope-less) reads expose verified revisions only, and
\* the served pointer names a verified row.
PublicReadsAreVerified ==
    /\ \A v \in Versions, c \in Checksums : ArtifactReadable(v, c, FALSE) => status[v][c] = "verified"
    /\ \A v \in Versions : Current(v) # 0 => status[v][Current(v)] = "verified"

\* A terminal status was decided by a verdict computed against the row's
\* current generation: no delayed verdict lands on a revival (same bytes,
\* new publish event).  The ghost generation, not the stamp the guards
\* compare, is what lets this detect a stamp the clock reused.
TerminalDecidedOnCurrentGeneration ==
    \A v \in Versions, c \in Checksums :
        status[v][c] \in Terminal => decided[v][c] = gen[v][c]

\* A pending row carries no verdict; a revival clears the old one.
PendingUndecided ==
    \A v \in Versions, c \in Checksums :
        status[v][c] = "pending" => decided[v][c] = 0 /\ basis[v][c] = "none"

\* A verified row was rendered by the inspector, and never on bytes the
\* scan rejects: no operator resolution, abstain, failed download,
\* mismatched digest, or inspector failure produces `verified` (each
\* leaves the row pending).
VerifiedByInspection ==
    \A v \in Versions, c \in Checksums :
        status[v][c] = "verified" => basis[v][c] = "inspected" /\ c \notin Malicious

RECURSIVE Sum(_)
Sum(S) == IF S = {} THEN 0 ELSE LET c == CHOOSE x \in S : TRUE IN Size(c) + Sum(S \ {c})

\* meta.total_stored_bytes is exactly one archive size per live checksum
\* in every D1-visible state: every change rides the batch that changes
\* the referencing rows.
CounterExact == counter = Sum({c \in Checksums : LiveRefs(c) > 0})

\* Every R2 object is represented in the ledger (reservation before put,
\* never auto-released): an orphan is conservative spend, never unaccounted.
LedgerConservative == \A c \in Checksums : blob[c] => ledger[c] # "none"

-----------------------------------------------------------------------------
(* Action properties: what one step may change *)

\* Immutability: a verified row never changes status or generation.
VerifiedImmutable ==
    [][\A v \in Versions, c \in Checksums :
        status[v][c] = "verified" => status'[v][c] = "verified" /\ pubAt'[v][c] = pubAt[v][c]]_vars

\* A rejected row only ever revives (to pending); no verdict verifies it.
RejectedOnlyRevives ==
    [][\A v \in Versions, c \in Checksums :
        status[v][c] = "rejected" => status'[v][c] \in {"rejected", "pending"}]_vars

\* A live row's generation is fixed: no publish, retry, or revival restamps it.
LiveGenerationStable ==
    [][\A v \in Versions, c \in Checksums : IsLive(v, c) => pubAt'[v][c] = pubAt[v][c]]_vars

\* What a version serves changes only when a row becomes verified: a
\* pending respin, a rejection, or a revival never disturbs the served revision.
ServedChangesOnlyByVerification ==
    [][\A v \in Versions :
        Current(v)' # Current(v) =>
            status[v][Current(v)'] # "verified" /\ status'[v][Current(v)'] = "verified"]_vars

\* A different-bytes revision joins a live sibling only through a request
\* that carried the new-revision opt-in (INSERT_REVISION's `?10`,
\* REVIVE_REJECTED_REVISION's `?6`).
RespinNeedsOptIn ==
    [][\A v \in Versions, c \in Checksums :
        (status[v][c] # "pending" /\ status'[v][c] = "pending" /\ LiveOtherBytes(v, c))
            => \E p \in Publishers :
                /\ pub[p].phase = "stored" /\ pub'[p].phase = "written"
                /\ pub[p].v = v /\ pub[p].c = c /\ pub[p].optIn]_vars

\* Idempotence: a request answered without committing a batch - the 200
\* no-op, every 409, a failed batch, a refused or repeated verdict - changes
\* no row and no accounting.
AnsweredWithoutCommitWritesNothing ==
    [][/\ \A p \in Publishers :
            (pub[p].phase \in {"disposed", "stored", "lost"} /\ pub'[p].phase = "idle")
                => UNCHANGED <<rows, counter>>
       /\ \A w \in Verifiers :
            (ver[w].phase = "decided" /\ ver'[w].phase = "idle")
                => UNCHANGED <<rows, counter>>]_vars

-----------------------------------------------------------------------------
(* Witness traps: each is the negation of a behavior the model must reach, *)
(* so TLC must report it violated, and the printed trace is the witness    *)
(* (RegistryLifecycle_witness.cfg; README.md "Non-vacuity").               *)

\* a revision becomes verified and is served
NoVerified == \A v \in Versions : Current(v) = 0
\* a rejected revision is revived (same bytes, new stamp) and then verified
NoRevivedVerified == \A v \in Versions, c \in Checksums : ~(gen[v][c] > 1 /\ status[v][c] = "verified")
\* a verdict is refused at the read because its generation binding went stale
NoStaleVerdictRefused ==
    [][\A w \in Verifiers :
        ~(ver[w].phase = "decided" /\ ver'[w].phase = "idle"
          /\ pubAt[ver[w].v][ver[w].c] # ver[w].stamp)]_vars
\* a verdict loses the race between its read and its guarded update
NoVerdictLostRace ==
    [][\A w \in Verifiers :
        ~(ver[w].phase = "apply" /\ ver'[w].phase = "idle"
          /\ ~(status[ver[w].v][ver[w].c] = "pending" /\ pubAt[ver[w].v][ver[w].c] = ver[w].stamp))]_vars
\* a publish loses its in-batch race to a concurrent writer and is re-read
NoLostRace == \A p \in Publishers : pub[p].phase # "lost"
\* two revisions of one version are live at once (a new-revision respin)
NoRespin == \A v \in Versions : Cardinality({c \in Checksums : IsLive(v, c)}) < 2
\* a blob is shared by two live rows (the sole-reference accounting matters)
NoSharedBlob == \A c \in Checksums : LiveRefs(c) < 2
\* a rejection is applied (and its reclaim decided)
NoRejection == \A w \in Verifiers : ver[w].phase # "reclaim"
\* the inspector rejects the bytes it rejects
NoInspectedRejection ==
    \A v \in Versions, c \in Checksums : ~(status[v][c] = "rejected" /\ basis[v][c] = "inspected")
\* an operator's reject resolution lands on bytes the scan accepts: the
\* advisory rejection, nothing inspected
NoManualRejection ==
    \A v \in Versions, c \in Checksums :
        ~(status[v][c] = "rejected" /\ basis[v][c] = "manual" /\ c \notin Malicious)
\* the documented reclaim/revival window: a live row whose blob a late
\* delete removed after its publish healed, with nothing in flight for it
\* (runbook, "Orphaned R2 blobs"; verifier.rs delete_blob_if_unreferenced)
LiveBlobPresent ==
    \A v \in Versions, c \in Checksums :
        /\ IsLive(v, c)
        /\ deletes = {}
        /\ \A p \in Publishers : pub[p].phase = "idle" \/ pub[p].c # c
        => blob[c]

=============================================================================
