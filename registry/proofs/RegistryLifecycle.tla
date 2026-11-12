------------------------- MODULE RegistryLifecycle -------------------------
(***************************************************************************)
(* The hosted registry's publish / verification lifecycle for one package  *)
(* (docs/remote-registry.md, "Publish" and "Verification lifecycle"), as   *)
(* the D1 statements in registry/src/sql/ execute it, with the UPLOADS and *)
(* ARCHIVES buckets as the separate, non-transactional stores they are.    *)
(* README.md maps every action to the Rust and SQL it abstracts and lists  *)
(* what the model leaves out.                                              *)
(*                                                                         *)
(* A revision row is keyed by (version, checksum): the packaging-revision  *)
(* id is derived from the checksum, so the two name the same row.  Its    *)
(* production generation identity is the pair (checksum, published_at);   *)
(* `pubAt` stands for published_at as the request sampled it: a fresh      *)
(* sample advances the clock, and a request returning to a rejected row    *)
(* may repeat that row's stamp (README.md, "Assumptions"), which the       *)
(* gates refuse; the guards compare it exactly as the SQL compares the     *)
(* column.  `gen`, `decided`, and `basis` are ghost history for the        *)
(* properties only; no guard reads them.                                   *)
(*                                                                         *)
(* An upload operation is named by its Cabin-Upload-Id, and its archive    *)
(* is the one object uploads/<id>.zip.  A row is a reservation from its    *)
(* admission until the object is confirmed stored; the cron settles the    *)
(* reservations no request confirmed.                                      *)
(*                                                                         *)
(* A verified row is served once it is archived: a placement pass copies   *)
(* its archive from the upload's object to the row's own ARCHIVES key,     *)
(* and a separate write records it.  The key names one byte sequence, so   *)
(* a pass that finds the archive under it converges on that object, and    *)
(* no pass replaces one.                                                   *)
(*                                                                         *)
(* What a publish request decides to attempt (upload::dispose, from a      *)
(* read that may be stale by the time it writes) is left free: a request   *)
(* may run any of the three admission gates with any upload id, or         *)
(* converge on any reservation it read.  The properties therefore hold     *)
(* whatever the handler decides: the D1 gates and a request's              *)
(* store-then-confirm order carry them.                                    *)
(*                                                                         *)
(* The external verifier (the registry-verify workflow's driver) is a     *)
(* participant on its own schedule: it lists, downloads, inspects, and    *)
(* PATCHes.  The archive inspector is the oracle `Inspections`: it rejects *)
(* Malicious bytes outright and is otherwise free (README.md, "The        *)
(* verifier").                                                             *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS
    Versions,      \* the version slots of one package
    Checksums,     \* archive contents, each named by the byte count it stores
    Users,         \* publishers' accounts
    Uploads,       \* Cabin-Upload-Id values
    Publishers,    \* concurrent publish request slots
    Verifiers,     \* concurrent verdict request slots
    Placers,       \* concurrent archive placements, each of one listed row
    MaxStamps,     \* how many publish events one execution can stamp
    Malicious,     \* the contents the archive inspector rejects (its result, abstracted)
    Stray,         \* the contents whose ARCHIVES key may hold an object no placement wrote
    PendingLimit,  \* quotas.max_pending_uploads
    DailyLimit,    \* quotas.max_uploads_per_day
    StorageLimit   \* quotas.max_total_bytes_per_user

ASSUME /\ Checksums \subseteq Nat \ {0}
       /\ MaxStamps \in Nat
       /\ Malicious \subseteq Checksums
       /\ Stray \subseteq Checksums
       /\ PendingLimit \in Nat /\ DailyLimit \in Nat /\ StorageLimit \in Nat

Size(c) == c   \* revisions.archive_size, a function of the bytes

VARIABLES
    status,        \* [Versions -> [Checksums -> Status]]: revisions.verification, "absent" for no row
    pubAt,         \* [Versions -> [Checksums -> Nat]]: revisions.published_at, 0 for no row
    clock,         \* the greatest stamp now_iso8601() has issued (a repeated sample leaves it)
    spare,         \* SUBSET Nat: stamps requests sampled and have not committed
    stored,        \* [Versions -> [Checksums -> BOOLEAN]]: revisions.stored_at IS NOT NULL
    archived,      \* [Versions -> [Checksums -> BOOLEAN]]: revisions.archived_at IS NOT NULL
    opOf,          \* [Versions -> [Checksums -> Uploads \cup {None}]]: revisions.upload_id
    owner,         \* [Versions -> [Checksums -> Users \cup {None}]]: revisions.published_by
    storedBytes,   \* [Users -> Nat]: users.stored_bytes
    dailyUploads,  \* [Users -> Nat]: user_daily_upload_usage.uploads, within one UTC day
    object,        \* [Uploads -> Checksums \cup {0}]: the bytes of uploads/<id>.zip, 0 for no object
    archive,       \* [Versions -> [Checksums -> Checksums \cup {0}]]: the bytes under the row's ARCHIVES key
    pub,           \* [Publishers -> publish request]
    ver,           \* [Verifiers -> verdict request]
    cron,          \* the cron's reservation pass
    place,         \* [Placers -> archive placement]
    gen,           \* ghost: how many publish events (the admission, each return) the row has had
    decided,       \* ghost: the generation a row's terminal state was decided against, 0 for none
    basis          \* ghost: what rendered a row's terminal state

vars == <<status, pubAt, clock, spare, stored, archived, opOf, owner, storedBytes, dailyUploads,
          object, archive, pub, ver, cron, place, gen, decided, basis>>

None == "none"
Status == {"absent", "pending", "verified", "rejected"}
Live == {"pending", "verified"}
Terminal == {"verified", "rejected"}

\* Request slots: an idle slot carries nothing (a response, once sent, is
\* not registry state); a busy one carries what the handler holds.
Idle == [phase |-> "idle"]
PubPhase == {"idle", "admitted", "stored"}
Admissions == {"new", "revive", "resume"}
VerPhase == {"idle", "decided"}
Verdicts == {"verified", "rejected"}
PlacePhase == {"idle", "headed", "lost"}
Basis == {"none", "inspected", "manual", "released", "timeout", "lost"}

\* Everything but the checksums is interchangeable (checksums carry sizes
\* and break the served-revision tie).
Symmetry == Permutations(Versions) \cup Permutations(Users) \cup Permutations(Uploads)
            \cup Permutations(Publishers) \cup Permutations(Verifiers)
            \cup Permutations(Placers)

-----------------------------------------------------------------------------
(* Row predicates, each the SQL it names *)

IsLive(v, c) == status[v][c] \in Live

\* `... AND verification IN ('pending', 'verified') AND checksum <> ?`
LiveOtherBytes(v, c) == \E d \in Checksums \ {c} : IsLive(v, d)

\* `NOT EXISTS (SELECT 1 FROM revisions WHERE upload_id = ?)`
Bound(i) == \E v \in Versions, c \in Checksums : opOf[v][c] = i

\* `(SELECT COUNT(*) FROM revisions WHERE published_by = ? AND verification = 'pending')`
PendingOf(u) ==
    Cardinality({x \in Versions \X Checksums :
                    status[x[1]][x[2]] = "pending" /\ owner[x[1]][x[2]] = u})

\* The conditions the three gates share: the respin opt-in, the
\* publisher's storage, and their pending uploads.
GateCommon(u, v, c, optIn) ==
    /\ optIn \/ ~LiveOtherBytes(v, c)
    /\ storedBytes[u] + Size(c) <= StorageLimit
    /\ PendingOf(u) < PendingLimit

\* ADMIT_NEW_UPLOAD (one package: no package or version conditions)
NewGate(u, v, c, optIn, i) ==
    /\ status[v][c] = "absent"
    /\ ~Bound(i)
    /\ dailyUploads[u] < DailyLimit
    /\ GateCommon(u, v, c, optIn)

\* REVIVE_REJECTED_UPLOAD: `published_at < ?1` makes the revival a new
\* generation by stamp alone
ReviveGate(u, v, c, optIn, i, stamp) ==
    /\ status[v][c] = "rejected"
    /\ pubAt[v][c] < stamp
    /\ ~Bound(i)
    /\ dailyUploads[u] < DailyLimit
    /\ GateCommon(u, v, c, optIn)

\* RESUME_RELEASED_UPLOAD: the operation's own released reservation, and
\* no daily allowance.  Its retry window only ever refuses, so the model
\* leaves it out (README.md, "Time").
ResumeGate(u, v, c, optIn, i, stamp) ==
    /\ status[v][c] = "rejected"
    /\ ~stored[v][c]
    /\ opOf[v][c] = i
    /\ owner[v][c] = u
    /\ pubAt[v][c] < stamp
    /\ GateCommon(u, v, c, optIn)

Gate(kind, u, v, c, optIn, i, stamp) ==
    CASE kind = "new" -> NewGate(u, v, c, optIn, i)
      [] kind = "revive" -> ReviveGate(u, v, c, optIn, i, stamp)
      [] OTHER -> ResumeGate(u, v, c, optIn, i, stamp)

-----------------------------------------------------------------------------
(* The read plane *)

\* current_revisions: the verified revision in ARCHIVES with the greatest
\* (published_at, revision)
Current(v) ==
    LET served == {c \in Checksums : status[v][c] = "verified" /\ archived[v][c]}
    IN IF served = {} THEN 0
       ELSE CHOOSE c \in served :
                \A d \in served \ {c} :
                    \/ pubAt[v][c] > pubAt[v][d]
                    \/ pubAt[v][c] = pubAt[v][d] /\ c > d

\* ARTIFACT_BY_REVISION, then verify.rs artifact_readable: a reservation
\* is a missing row
ArtifactReadable(v, c, verifyScope) ==
    /\ stored[v][c]
    /\ CASE status[v][c] = "verified" -> archived[v][c]
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
    /\ spare = {}
    /\ stored = [v \in Versions |-> [c \in Checksums |-> FALSE]]
    /\ archived = [v \in Versions |-> [c \in Checksums |-> FALSE]]
    /\ opOf = [v \in Versions |-> [c \in Checksums |-> None]]
    /\ owner = [v \in Versions |-> [c \in Checksums |-> None]]
    /\ storedBytes = [u \in Users |-> 0]
    /\ dailyUploads = [u \in Users |-> 0]
    /\ object = [i \in Uploads |-> 0]
    \* An ARCHIVES key starts empty or, for a Stray content, holding an
    \* object that is not the key's archive.
    /\ archive \in [Versions -> [Checksums -> Checksums \cup {0}]]
    /\ \A v \in Versions, c \in Checksums :
        archive[v][c] = 0 \/ (c \in Stray /\ archive[v][c] # c)
    /\ pub = [p \in Publishers |-> Idle]
    /\ ver = [w \in Verifiers |-> Idle]
    /\ cron = Idle
    /\ place = [k \in Placers |-> Idle]
    /\ gen = [v \in Versions |-> [c \in Checksums |-> 0]]
    /\ decided = [v \in Versions |-> [c \in Checksums |-> 0]]
    /\ basis = [v \in Versions |-> [c \in Checksums |-> "none"]]

-----------------------------------------------------------------------------
(* Publish: PUT /api/v1/packages/<scope>/<name>/<version>[?new-revision=true] *)

\* A request samples its clock once, with the body in hand and before it
\* reads any revision row (Preflight.now), and may write with it much
\* later - after a faster request that sampled later - or never.  `spare`
\* holds the samples not yet committed.
Sample ==
    /\ clock < MaxStamps
    /\ clock' = clock + 1
    /\ spare' = spare \cup {clock + 1}
    /\ UNCHANGED <<status, pubAt, stored, archived, opOf, owner, storedBytes, dailyUploads,
                   object, archive, pub, ver, cron, place, gen, decided, basis>>

\* One D1 batch (package.rs admit_upload): a gate, evaluated against the
\* rows as they are now, and behind it the charges and the reservation.
\* Which gate, for which user, bytes, and upload id, is the request's
\* choice; a gate that changes nothing leaves no trace (the request
\* decides again or answers), so only the admitted case is a step.
\*
\* The clock is not trusted where the stamp carries the generation: a
\* request returning to a rejected row may also hold that row's current
\* stamp (a Worker clock behind the one that stamped the rejected
\* generation, or two publish events in one millisecond; an earlier
\* sample fails the same guards), which the gates refuse.
\*
\* The opt-in is only read beside live different bytes, so it is only
\* chosen there.
PubAdmit(p) ==
    /\ pub[p].phase = "idle"
    /\ \E u \in Users, v \in Versions, c \in Checksums, i \in Uploads, kind \in Admissions :
       \E optIn \in IF LiveOtherBytes(v, c) THEN BOOLEAN ELSE {FALSE} :
       \E s \in spare \cup (IF status[v][c] = "rejected" THEN {pubAt[v][c]} ELSE {}) :
        /\ Gate(kind, u, v, c, optIn, i, s)
        /\ status' = [status EXCEPT ![v][c] = "pending"]
        /\ pubAt' = [pubAt EXCEPT ![v][c] = s]
        /\ spare' = spare \ {s}
        \* RESUME_RELEASED_UPLOAD writes none of the three: its gate found
        \* them as the request has them.
        /\ stored' = IF kind = "resume" THEN stored ELSE [stored EXCEPT ![v][c] = FALSE]
        /\ opOf' = IF kind = "resume" THEN opOf ELSE [opOf EXCEPT ![v][c] = i]
        /\ owner' = IF kind = "resume" THEN owner ELSE [owner EXCEPT ![v][c] = u]
        /\ storedBytes' = [storedBytes EXCEPT ![u] = @ + Size(c)]
        /\ dailyUploads' = [dailyUploads EXCEPT ![u] = @ + IF kind = "resume" THEN 0 ELSE 1]
        /\ gen' = [gen EXCEPT ![v][c] = @ + 1]
        /\ decided' = [decided EXCEPT ![v][c] = 0]
        /\ basis' = [basis EXCEPT ![v][c] = "none"]
        /\ pub' = [pub EXCEPT ![p] = [phase |-> "admitted", v |-> v, c |-> c, id |-> i,
                                      stamp |-> s, optIn |-> optIn,
                                      how |-> IF kind = "resume" THEN "resume" ELSE "put"]]
    /\ UNCHANGED <<clock, archived, object, archive, ver, cron, place>>

\* A request whose bytes are already a reservation - another request's,
\* or its own earlier attempt's - converges on it: nothing is admitted or
\* charged, and it stores and confirms for the generation it read
\* (EXISTING_REVISIONS).
PubConverge(p) ==
    /\ pub[p].phase = "idle"
    /\ \E v \in Versions, c \in Checksums :
        /\ status[v][c] = "pending" /\ ~stored[v][c]
        /\ pub' = [pub EXCEPT ![p] = [phase |-> "admitted", v |-> v, c |-> c, id |-> opOf[v][c],
                                      stamp |-> pubAt[v][c], optIn |-> FALSE,
                                      how |-> "converge"]]
    /\ UNCHANGED <<status, pubAt, clock, spare, stored, archived, opOf, owner, storedBytes,
                   dailyUploads, object, archive, ver, cron, place, gen, decided, basis>>

\* package.rs `store`: the request's bytes go to the reservation's key.
\* A new or revived operation always puts; a resumed or converging one
\* heads first and keeps an object that is already there, whatever it
\* holds.
PubStore(p) ==
    /\ pub[p].phase = "admitted"
    /\ LET i == pub[p].id IN
       object' = IF pub[p].how # "put" /\ object[i] # 0 THEN object
                 ELSE [object EXCEPT ![i] = pub[p].c]
    /\ pub' = [pub EXCEPT ![p].phase = "stored"]
    /\ UNCHANGED <<status, pubAt, clock, spare, stored, archived, opOf, owner, storedBytes,
                   dailyUploads, archive, ver, cron, place, gen, decided, basis>>

\* CONFIRM_UPLOAD_STORED, guarded on the generation stored for.  A
\* reservation released or taken again in the meantime is left alone.
Confirmable(v, c, i, s) == status[v][c] = "pending" /\ opOf[v][c] = i /\ pubAt[v][c] = s

PubConfirm(p) ==
    /\ pub[p].phase = "stored"
    /\ LET v == pub[p].v
           c == pub[p].c
       IN stored' = IF Confirmable(v, c, pub[p].id, pub[p].stamp)
                    THEN [stored EXCEPT ![v][c] = TRUE] ELSE stored
    /\ pub' = [pub EXCEPT ![p] = Idle]
    /\ UNCHANGED <<status, pubAt, clock, spare, archived, opOf, owner, storedBytes, dailyUploads,
                   object, archive, ver, cron, place, gen, decided, basis>>

\* The request ends after its admission, writing nothing more: a failed
\* put or batch, or the Worker dying.  It leaves a reservation for the
\* cron.
PubDrop(p) ==
    /\ pub[p].phase \in {"admitted", "stored"}
    /\ pub' = [pub EXCEPT ![p] = Idle]
    /\ UNCHANGED <<status, pubAt, clock, spare, stored, archived, opOf, owner, storedBytes,
                   dailyUploads, object, archive, ver, cron, place, gen, decided, basis>>

-----------------------------------------------------------------------------
(* The cron (glue/cron.rs) *)

\* settle_reservations: EXPIRED_RESERVATIONS lists a reservation's
\* generation, then the pass heads its object.  The listing and the head
\* are one step: both writes below re-check the generation, so only the
\* head's answer is carried.  Any reservation may be listed (the model
\* has no durations; README.md, "Time").
CronList ==
    /\ cron.phase = "idle"
    /\ \E v \in Versions, c \in Checksums :
        /\ status[v][c] = "pending" /\ ~stored[v][c]
        /\ cron' = [phase |-> "headed", v |-> v, c |-> c, id |-> opOf[v][c],
                    stamp |-> pubAt[v][c], gen |-> gen[v][c],
                    present |-> object[opOf[v][c]] # 0]
    /\ UNCHANGED <<status, pubAt, clock, spare, stored, archived, opOf, owner, storedBytes,
                   dailyUploads, object, archive, pub, ver, place, gen, decided, basis>>

\* CONFIRM_UPLOAD_STORED when the object was there; otherwise
\* RELEASE_EXPIRED_RESERVATION and REFUND_USER_STORED_BYTES in one batch.
\* Either write may change nothing (the generation moved on) or fail.
CronSettle ==
    /\ cron.phase = "headed"
    /\ LET v == cron.v
           c == cron.c
       IN \/ /\ cron.present /\ Confirmable(v, c, cron.id, cron.stamp)
             /\ stored' = [stored EXCEPT ![v][c] = TRUE]
             /\ UNCHANGED <<status, storedBytes, decided, basis>>
          \/ /\ ~cron.present
             /\ status[v][c] = "pending" /\ ~stored[v][c] /\ pubAt[v][c] = cron.stamp
             /\ status' = [status EXCEPT ![v][c] = "rejected"]
             /\ storedBytes' = [storedBytes EXCEPT ![owner[v][c]] = @ - Size(c)]
             /\ decided' = [decided EXCEPT ![v][c] = cron.gen]
             /\ basis' = [basis EXCEPT ![v][c] = "released"]
             /\ UNCHANGED stored
          \/ UNCHANGED <<status, stored, storedBytes, decided, basis>>
    /\ cron' = Idle
    /\ UNCHANGED <<pubAt, clock, spare, archived, opOf, owner, dailyUploads, object, archive,
                   pub, ver, place, gen>>

\* REFUND_TIMED_OUT_UPLOADS and TIME_OUT_PENDING_UPLOADS in one batch: a
\* stored upload no verdict reached.  The statements take every such row
\* at once; one row per step reaches the same states.
CronTimeout ==
    /\ \E v \in Versions, c \in Checksums :
        /\ status[v][c] = "pending" /\ stored[v][c]
        /\ status' = [status EXCEPT ![v][c] = "rejected"]
        /\ storedBytes' = [storedBytes EXCEPT ![owner[v][c]] = @ - Size(c)]
        /\ decided' = [decided EXCEPT ![v][c] = gen[v][c]]
        /\ basis' = [basis EXCEPT ![v][c] = "timeout"]
    /\ UNCHANGED <<pubAt, clock, spare, stored, archived, opOf, owner, dailyUploads, object,
                   archive, pub, ver, cron, place, gen>>

\* PRUNE_SETTLED_UPLOAD_IDS: a terminal row forgets its operation, which
\* frees the upload id.
CronForget ==
    /\ \E v \in Versions, c \in Checksums :
        /\ status[v][c] \in Terminal /\ opOf[v][c] # None
        /\ opOf' = [opOf EXCEPT ![v][c] = None]
    /\ UNCHANGED <<status, pubAt, clock, spare, stored, archived, owner, storedBytes,
                   dailyUploads, object, archive, pub, ver, cron, place, gen, decided, basis>>

-----------------------------------------------------------------------------
(* Verdicts: GET /api/v1/admin/versions?status=pending, then                *)
(* PATCH /api/v1/admin/versions/<scope>/<name>/<version>                    *)

\* One verifier run over one listed entry (xtask-registry-admin verify.rs,
\* Pass::version): the listing (REVISIONS_BY_VERIFICATION_STATUS, which
\* never lists a reservation) snapshots the pending row's binding
\* (checksum, published_at) - the ghost `gen` remembers which generation
\* that was - then the name advisories, the artifact download with the
\* same token (the object under the row's upload id), the digest check
\* against the listed checksum, the optional upstream fetch, the
\* inspector child, and the OIDC mint for the PATCH.  The advisory's
\* abstain, a digest mismatch, and every failure end the run for this
\* entry without a PATCH: the row stays pending and nothing else changes,
\* so those outcomes are the stutter.  An operator's `reject` resolution
\* (Pass::resolve) composes the advisory rejection from the listing entry
\* and inspects nothing; the `verify` resolution is the same pass with
\* the advisory gate waived, so the inspector still renders its verdict.
\*
\* The listing and the download are one step although the driver reads
\* them minutes apart: a PATCH lands only while the row is still pending
\* at the listed stamp, a generation's upload id never changes, and its
\* object, once confirmed, is only ever replaced by a put under the same
\* id (README.md, "Assumptions") - so this step, taken at the download
\* instant of any landing PATCH, snapshots the same stamp and reads the
\* same bytes; a PATCH that would be refused changes nothing either way.
VerList(w) ==
    /\ ver[w].phase = "idle"
    /\ \E v \in Versions, c \in Checksums :
        /\ status[v][c] = "pending" /\ stored[v][c]
        /\ LET bytes == object[opOf[v][c]]
               render(verdict, how) ==
                   ver' = [ver EXCEPT ![w] = [phase |-> "decided", v |-> v, c |-> c,
                                              stamp |-> pubAt[v][c], gen |-> gen[v][c],
                                              verdict |-> verdict, basis |-> how]]
           IN \/ /\ bytes # 0
                 /\ bytes = c   \* undeclared_digest and the inspector's own hash (a mutation hook, README "Sensitivity")
                 /\ \E verdict \in Inspections(bytes) : render(verdict, "inspected")
              \/ render("rejected", "manual")
    /\ UNCHANGED <<status, pubAt, clock, spare, stored, archived, opOf, owner, storedBytes,
                   dailyUploads, object, archive, pub, cron, place, gen, decided, basis>>

\* The PATCH arrives: verdict_authn consumes its single-use OIDC jti in
\* its own transaction first (a replayed PATCH is refused there and never
\* reaches D1 again; not modeled).  The handler's VERDICT_TARGET read -
\* to which a reservation is a missing row - and verdict_binding_matches
\* can only refuse the PATCH, so the model lets every PATCH reach the
\* write: one D1 batch, MARK_REVISION_VERIFIED, or MARK_REVISION_REJECTED
\* then REFUND_USER_STORED_BYTES, guarded on the row still being pending
\* at the listing's (checksum, published_at), evaluated now.  A refused
\* or failed batch changes nothing.
VerApply(w) ==
    /\ ver[w].phase = "decided"
    /\ LET v == ver[w].v
           c == ver[w].c
           guard == status[v][c] = "pending" /\ pubAt[v][c] = ver[w].stamp
       IN \/ /\ guard
             /\ status' = [status EXCEPT ![v][c] = ver[w].verdict]
             /\ storedBytes' = IF ver[w].verdict = "rejected"
                               THEN [storedBytes EXCEPT ![owner[v][c]] = @ - Size(c)]
                               ELSE storedBytes
             /\ decided' = [decided EXCEPT ![v][c] = ver[w].gen]
             /\ basis' = [basis EXCEPT ![v][c] = ver[w].basis]
          \/ UNCHANGED <<status, storedBytes, decided, basis>>
    /\ ver' = [ver EXCEPT ![w] = Idle]
    /\ UNCHANGED <<pubAt, clock, spare, stored, archived, opOf, owner, dailyUploads, object,
                   archive, pub, cron, place, gen>>

-----------------------------------------------------------------------------
(* Archive placement (glue/publication.rs): the pass a verdict that         *)
(* verified a row runs before it answers, and the cron's                    *)

\* place_archives lists a verified row that is not archived, with its
\* upload id and published_at (UNARCHIVED_REVISIONS), and copy_archive
\* heads the row's ARCHIVES key and finds it empty.  The listing and the
\* head are one step: no object leaves ARCHIVES, so the key was empty at
\* the listing too.  A row that outlived its upload id has no object to
\* read, so the empty key settles it as the lost source.  A key that
\* holds the archive is PlaceMark's; one that holds anything else is an
\* error the pass logs, which writes nothing and leaves the row listed:
\* the stutter.  The ghost `gen` remembers which generation was listed.
PlaceList(k) ==
    /\ place[k].phase = "idle"
    /\ \E v \in Versions, c \in Checksums :
        /\ status[v][c] = "verified" /\ ~archived[v][c] /\ archive[v][c] = 0
        /\ place' = [place EXCEPT ![k] =
                       [phase |-> IF opOf[v][c] = None THEN "lost" ELSE "headed",
                        v |-> v, c |-> c, id |-> opOf[v][c],
                        stamp |-> pubAt[v][c], gen |-> gen[v][c]]]
    /\ UNCHANGED <<status, pubAt, clock, spare, stored, archived, opOf, owner, storedBytes,
                   dailyUploads, object, archive, pub, ver, cron, gen, decided, basis>>

\* copy_archive past the head: the read of the listed upload's object and
\* the put of its bytes under the row's SHA-256, which creates the key
\* only while it is empty.  A missing object and one recording another
\* digest are the lost source.  A put that finds the key filled lost the
\* create race to another pass and writes nothing more; after the put
\* that fills it, the write is PlaceMark's.  The read and the put are one
\* step, taken at the read.  That keeps what is stored (the bytes the
\* read returned, or by R2's digest check nothing) but not when: the key
\* fills here, where copy_archive leaves it empty until the streamed put
\* completes, so no step is another pass's head of the key in between.
\* Why no checked property depends on that is an argument, not a checked
\* fact (README.md, "Assumptions").
PlaceCopy(k) ==
    /\ place[k].phase = "headed"
    /\ LET v == place[k].v
           c == place[k].c
           bytes == object[place[k].id]
       IN IF bytes # c   \* holds_archive, and R2's check of the put against the digest (a mutation hook)
          THEN /\ place' = [place EXCEPT ![k].phase = "lost"]
               /\ UNCHANGED archive
          ELSE /\ place' = [place EXCEPT ![k] = Idle]
               /\ archive' = IF archive[v][c] = 0 THEN [archive EXCEPT ![v][c] = bytes]
                             ELSE archive
    /\ UNCHANGED <<status, pubAt, clock, spare, stored, archived, opOf, owner, storedBytes,
                   dailyUploads, object, pub, ver, cron, gen, decided, basis>>

\* The pass ends before its copy: a failed R2 call, or the Worker dying.
PlaceDrop(k) ==
    /\ place[k].phase = "headed"
    /\ place' = [place EXCEPT ![k] = Idle]
    /\ UNCHANGED <<status, pubAt, clock, spare, stored, archived, opOf, owner, storedBytes,
                   dailyUploads, object, archive, pub, ver, cron, gen, decided, basis>>

\* MARK_REVISION_ARCHIVED, by a pass whose head found the archive under
\* the key or whose put stored it: guarded on the row being verified and
\* not archived, and on no generation, because the key and the bytes
\* under it follow from the row key.  The head or the put and the write
\* are one step, and no slot's: the object stays under its key, so the
\* write lands exactly where a head taken at that moment finds the
\* archive, whichever pass makes it and however long ago it looked.
PlaceMark ==
    /\ \E v \in Versions, c \in Checksums :
        /\ status[v][c] = "verified" /\ ~archived[v][c]
        /\ archive[v][c] = c   \* publication.rs holds_archive (a mutation hook, README "Sensitivity")
        /\ archived' = [archived EXCEPT ![v][c] = TRUE]
    /\ UNCHANGED <<status, pubAt, clock, spare, stored, opOf, owner, storedBytes, dailyUploads,
                   object, archive, pub, ver, cron, place, gen, decided, basis>>

\* place_archive's write for a lost source: REJECT_UNARCHIVED_REVISION
\* then REFUND_USER_STORED_BYTES, one D1 batch evaluated now, guarded on
\* the row being verified and not archived at the listed published_at.
\* A refused or failed batch, or a pass that dies before it, changes
\* nothing.
PlaceReject(k) ==
    /\ place[k].phase = "lost"
    /\ LET v == place[k].v
           c == place[k].c
       IN \/ /\ status[v][c] = "verified" /\ ~archived[v][c] /\ pubAt[v][c] = place[k].stamp
             /\ status' = [status EXCEPT ![v][c] = "rejected"]
             /\ storedBytes' = [storedBytes EXCEPT ![owner[v][c]] = @ - Size(c)]
             /\ decided' = [decided EXCEPT ![v][c] = place[k].gen]
             /\ basis' = [basis EXCEPT ![v][c] = "lost"]
          \/ UNCHANGED <<status, storedBytes, decided, basis>>
    /\ place' = [place EXCEPT ![k] = Idle]
    /\ UNCHANGED <<pubAt, clock, spare, stored, archived, opOf, owner, dailyUploads, object,
                   archive, pub, ver, cron, gen>>

-----------------------------------------------------------------------------
Next ==
    \/ Sample
    \/ \E p \in Publishers :
        PubAdmit(p) \/ PubConverge(p) \/ PubStore(p) \/ PubConfirm(p) \/ PubDrop(p)
    \/ \E w \in Verifiers : VerList(w) \/ VerApply(w)
    \/ CronList \/ CronSettle \/ CronTimeout \/ CronForget
    \/ \E k \in Placers : PlaceList(k) \/ PlaceCopy(k) \/ PlaceDrop(k) \/ PlaceReject(k)
    \/ PlaceMark
    \* The stamp bound ends an execution: with the clock spent an
    \* execution may have nothing left to do, which is the model's bound.
    \* This stutter keeps TLC from reporting that end as a deadlock; before
    \* it `Sample` is always enabled, so no deadlock is ever reported.
    \/ clock = MaxStamps /\ UNCHANGED vars

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* Invariants *)

TypeOK ==
    /\ status \in [Versions -> [Checksums -> Status]]
    /\ pubAt \in [Versions -> [Checksums -> 0..MaxStamps]]
    /\ clock \in 0..MaxStamps
    /\ spare \subseteq 1..clock
    /\ stored \in [Versions -> [Checksums -> BOOLEAN]]
    /\ archived \in [Versions -> [Checksums -> BOOLEAN]]
    /\ opOf \in [Versions -> [Checksums -> Uploads \cup {None}]]
    /\ owner \in [Versions -> [Checksums -> Users \cup {None}]]
    /\ storedBytes \in [Users -> Nat]
    /\ dailyUploads \in [Users -> Nat]
    /\ object \in [Uploads -> Checksums \cup {0}]
    /\ archive \in [Versions -> [Checksums -> Checksums \cup {0}]]
    /\ \A p \in Publishers :
        /\ pub[p].phase \in PubPhase
        /\ pub[p].phase # "idle" =>
            /\ pub[p].v \in Versions /\ pub[p].c \in Checksums /\ pub[p].id \in Uploads
            /\ pub[p].stamp \in 1..MaxStamps /\ pub[p].optIn \in BOOLEAN
            /\ pub[p].how \in {"put", "resume", "converge"}
    /\ \A w \in Verifiers :
        /\ ver[w].phase \in VerPhase
        /\ ver[w].phase # "idle" =>
            /\ ver[w].v \in Versions /\ ver[w].c \in Checksums
            /\ ver[w].stamp \in 1..MaxStamps /\ ver[w].gen \in 1..MaxStamps
            /\ ver[w].verdict \in Verdicts /\ ver[w].basis \in {"inspected", "manual"}
    /\ cron.phase \in {"idle", "headed"}
    /\ cron.phase = "headed" =>
        /\ cron.v \in Versions /\ cron.c \in Checksums /\ cron.id \in Uploads
        /\ cron.stamp \in 1..MaxStamps /\ cron.gen \in 1..MaxStamps
        /\ cron.present \in BOOLEAN
    /\ \A k \in Placers :
        /\ place[k].phase \in PlacePhase
        /\ place[k].phase # "idle" =>
            /\ place[k].v \in Versions /\ place[k].c \in Checksums
            /\ place[k].id \in Uploads \cup {None}
            /\ place[k].stamp \in 1..MaxStamps /\ place[k].gen \in 1..MaxStamps
            /\ place[k].phase = "headed" => place[k].id # None
    /\ gen \in [Versions -> [Checksums -> 0..MaxStamps]]
    /\ decided \in [Versions -> [Checksums -> 0..MaxStamps]]
    /\ basis \in [Versions -> [Checksums -> Basis]]

\* A row exists exactly when it has been stamped; stamps are unique (a
\* fresh sample advances the clock, and the one repeated sample the model
\* admits, of a rejected row's own stamp, is refused by every gate).
RowsStamped ==
    /\ \A v \in Versions, c \in Checksums :
        /\ (status[v][c] = "absent") <=> (pubAt[v][c] = 0)
        /\ (status[v][c] = "absent") <=> (gen[v][c] = 0)
    /\ \A v, u \in Versions, c, d \in Checksums :
        (pubAt[v][c] = pubAt[u][d] /\ pubAt[v][c] # 0) => (v = u /\ c = d)

\* Ordinary (verify-scope-less) reads expose archived revisions only and
\* find the revision's own bytes where read.rs reads them, under its
\* ARCHIVES key; the served pointer names a revision such a read serves.
PublicReadsAreArchived ==
    /\ \A v \in Versions, c \in Checksums :
        ArtifactReadable(v, c, FALSE) =>
            status[v][c] = "verified" /\ archived[v][c] /\ archive[v][c] = c
    /\ \A v \in Versions : Current(v) # 0 => ArtifactReadable(v, Current(v), FALSE)

\* An archived row is verified (the column's CHECK) and its ARCHIVES key
\* holds its bytes, whatever became of the upload's object.
ArchivedIsPlaced ==
    \A v \in Versions, c \in Checksums :
        archived[v][c] => status[v][c] = "verified" /\ archive[v][c] = c

\* A terminal status was decided against the row's current generation: no
\* delayed verdict, release, or placement's rejection lands on a later
\* publish event of the same bytes.  The ghost generation, not the stamp
\* the guards compare, is what lets this detect a stamp the clock reused.
TerminalDecidedOnCurrentGeneration ==
    \A v \in Versions, c \in Checksums :
        status[v][c] \in Terminal => decided[v][c] = gen[v][c]

\* A pending row carries no verdict; a returning operation clears the old one.
PendingUndecided ==
    \A v \in Versions, c \in Checksums :
        status[v][c] = "pending" => decided[v][c] = 0 /\ basis[v][c] = "none"

\* A verified row was rendered by the inspector, and never on bytes the
\* scan rejects: no reject resolution produces `verified` (it rejects),
\* and no abstain, failed artifact download, mismatched digest, or inspector
\* failure renders any verdict (each leaves the row pending).
VerifiedByInspection ==
    \A v \in Versions, c \in Checksums :
        status[v][c] = "verified" => basis[v][c] = "inspected" /\ c \notin Malicious

\* The verifier's verdicts, the timeout, and the placement's rejection
\* land only on stored uploads, and only a reservation is released: the
\* listing and VERDICT_TARGET hide reservations, and a generation, once
\* stored, stays stored.
VerdictOnlyOnStored ==
    \A v \in Versions, c \in Checksums :
        /\ status[v][c] = "verified" => stored[v][c]
        /\ basis[v][c] \in {"inspected", "manual", "timeout", "lost"} => stored[v][c]
        /\ basis[v][c] = "released" => ~stored[v][c]

\* Every row that is or was admitted names its publisher; a pending row
\* names its operation, whose id names no other row.
UploadIdNamesOneRow ==
    /\ \A v \in Versions, c \in Checksums :
        /\ (status[v][c] = "absent") <=> (owner[v][c] = None)
        /\ status[v][c] = "absent" => opOf[v][c] = None
        /\ status[v][c] = "pending" => opOf[v][c] # None
    /\ \A v, w \in Versions, c, d \in Checksums :
        (opOf[v][c] = opOf[w][d] /\ opOf[v][c] # None) => (v = w /\ c = d)

\* A stored upload awaiting its verdict has an object under its id.
\* (What that object holds is the NoStaleObject trap's business.)
StoredHasObject ==
    \A v \in Versions, c \in Checksums :
        (status[v][c] = "pending" /\ stored[v][c]) => object[opOf[v][c]] # 0

RECURSIVE SumOver(_, _)
SumOver(f(_), S) == IF S = {} THEN 0
                    ELSE LET c == CHOOSE x \in S : TRUE IN f(c) + SumOver(f, S \ {c})

\* users.stored_bytes is exactly one archive size per live row the user
\* published - reservations included, the same bytes on two versions
\* twice - in every D1-visible state: every change rides the batch that
\* changes the row.
StoredBytesExact ==
    \A u \in Users :
        LET held(c) == Size(c) * Cardinality({v \in Versions : IsLive(v, c) /\ owner[v][c] = u})
        IN storedBytes[u] = SumOver(held, Checksums)

\* No interleaving of admissions overshoots a limit: each gate reads the
\* usage it adds to inside its own batch.
WithinLimits ==
    \A u \in Users :
        /\ PendingOf(u) <= PendingLimit
        /\ dailyUploads[u] <= DailyLimit
        /\ storedBytes[u] <= StorageLimit

-----------------------------------------------------------------------------
(* Action properties: what one step may change *)

\* Immutability: an archived row stays archived and verified, and never
\* changes generation.
ArchivedImmutable ==
    [][\A v \in Versions, c \in Checksums :
        archived[v][c] => /\ archived'[v][c]
                          /\ status'[v][c] = "verified"
                          /\ pubAt'[v][c] = pubAt[v][c]]_vars

\* Until it is archived a verified row keeps its generation too, and
\* leaves `verified` only by the placement's rejection.
VerifiedUnlessSourceLost ==
    [][\A v \in Versions, c \in Checksums :
        status[v][c] = "verified" =>
            /\ pubAt'[v][c] = pubAt[v][c]
            /\ \/ status'[v][c] = "verified"
               \/ status'[v][c] = "rejected" /\ basis'[v][c] = "lost"]_vars

\* No object in ARCHIVES is replaced or removed, and a key is only ever
\* filled with its row's own bytes.
ArchivesWriteOnce ==
    [][\A v \in Versions, c \in Checksums :
        archive'[v][c] # archive[v][c] => archive[v][c] = 0 /\ archive'[v][c] = c]_vars

\* A rejected row only ever returns to pending; no verdict verifies it.
RejectedOnlyRevives ==
    [][\A v \in Versions, c \in Checksums :
        status[v][c] = "rejected" => status'[v][c] \in {"rejected", "pending"}]_vars

\* A live row's generation is fixed: no publish, retry, or return restamps
\* it, moves it to another operation or publisher, or un-stores it.
LiveGenerationStable ==
    [][\A v \in Versions, c \in Checksums :
        IsLive(v, c) => /\ pubAt'[v][c] = pubAt[v][c]
                        /\ owner'[v][c] = owner[v][c]
                        /\ stored[v][c] => stored'[v][c]
                        /\ status[v][c] = "pending" => opOf'[v][c] = opOf[v][c]]_vars

\* What a version serves changes only when a row is archived, and to
\* that row: a pending respin, a verdict, a rejection, or a revival
\* never disturbs the served revision, and nothing withdraws it.
ServedChangesOnlyByPlacement ==
    [][\A v \in Versions :
        Current(v)' # Current(v) =>
            /\ Current(v)' # 0
            /\ ~archived[v][Current(v)'] /\ archived'[v][Current(v)']]_vars

\* A different-bytes revision joins a live sibling only through a request
\* that carried the new-revision opt-in (`?9` of the three gates).
RespinNeedsOptIn ==
    [][\A v \in Versions, c \in Checksums :
        (status[v][c] # "pending" /\ status'[v][c] = "pending" /\ LiveOtherBytes(v, c))
            => \E p \in Publishers :
                /\ pub[p].phase = "idle" /\ pub'[p].phase = "admitted"
                /\ pub'[p].v = v /\ pub'[p].c = c /\ pub'[p].optIn]_vars

\* The daily allowance moves by exactly one per operation a user starts
\* - a row bound to an upload id it was not bound to - so a resumed
\* operation is not charged again and nothing refunds a charge.
ChargedOncePerOperation ==
    [][\A u \in Users :
        dailyUploads'[u] = dailyUploads[u] +
            Cardinality({x \in Versions \X Checksums :
                            /\ owner'[x[1]][x[2]] = u
                            /\ opOf'[x[1]][x[2]] # None
                            /\ opOf'[x[1]][x[2]] # opOf[x[1]][x[2]]})]_vars

\* The id of an upload rejected after it was stored - by a verdict, the
\* timeout, or the placement - is spent: the bytes return only as a new
\* operation, which ChargedOncePerOperation charges.  Only a reservation
\* that never stored resumes for free.
SpentIdStaysSpent ==
    [][\A v \in Versions, c \in Checksums :
        (status[v][c] = "rejected" /\ stored[v][c] /\ status'[v][c] = "pending")
            => opOf'[v][c] # opOf[v][c]]_vars

-----------------------------------------------------------------------------
(* Witness traps: each is the negation of a behavior the model must reach, *)
(* so TLC must report it violated, and the printed trace is the witness    *)
(* (RegistryLifecycle_witness.cfg, and RegistryLifecycleShared_witness.cfg *)
(* for NoSameBytesTwice; README.md "Non-vacuity").                         *)

\* a revision is verified, archived, and served
NoServed == \A v \in Versions : Current(v) = 0
\* a rejected revision returns (same bytes, new stamp) and is then verified
NoRevivedVerified == \A v \in Versions, c \in Checksums : ~(gen[v][c] > 1 /\ status[v][c] = "verified")
\* a verdict is refused because its generation binding went stale: the
\* row was decided, or returned under a new stamp, since the listing
NoStaleVerdictRefused ==
    [][\A w \in Verifiers :
        ~(ver[w].phase = "decided" /\ ver'[w].phase = "idle"
          /\ ~(status[ver[w].v][ver[w].c] = "pending"
               /\ pubAt[ver[w].v][ver[w].c] = ver[w].stamp))]_vars
\* a request stores for a reservation that is released or taken again
\* before it confirms
NoLostConfirm ==
    [][\A p \in Publishers :
        ~(pub[p].phase = "stored" /\ pub'[p].phase = "idle"
          /\ ~Confirmable(pub[p].v, pub[p].c, pub[p].id, pub[p].stamp))]_vars
\* two revisions of one version are live at once (a new-revision respin)
NoRespin == \A v \in Versions : Cardinality({c \in Checksums : IsLive(v, c)}) < 2
\* the same bytes are live on two versions, and charged for both
NoSameBytesTwice == \A c \in Checksums : Cardinality({v \in Versions : IsLive(v, c)}) < 2
\* the inspector rejects the bytes it rejects
NoInspectedRejection ==
    \A v \in Versions, c \in Checksums : ~(status[v][c] = "rejected" /\ basis[v][c] = "inspected")
\* an operator's reject resolution lands on bytes the scan accepts: the
\* advisory rejection, nothing inspected
NoManualRejection ==
    \A v \in Versions, c \in Checksums :
        ~(status[v][c] = "rejected" /\ basis[v][c] = "manual" /\ c \notin Malicious)
\* a request confirms a reservation another upload id took (a convergence)
NoConvergence ==
    [][\A p \in Publishers :
        ~(pub[p].phase = "stored" /\ pub'[p].phase = "idle" /\ pub[p].how = "converge"
          /\ stored'[pub[p].v][pub[p].c] /\ ~stored[pub[p].v][pub[p].c])]_vars
\* the cron confirms a reservation whose request stored and never confirmed
NoCronConfirm ==
    [][\A v \in Versions, c \in Checksums :
        ~(cron.phase = "headed" /\ cron'.phase = "idle" /\ stored'[v][c] /\ ~stored[v][c])]_vars
\* the cron releases a reservation
NoRelease == \A v \in Versions, c \in Checksums : basis[v][c] # "released"
\* an operation returns to its released reservation and is verified
NoResume ==
    \A v \in Versions, c \in Checksums :
        ~(status[v][c] = "verified" /\ gen[v][c] > 1 /\ dailyUploads[owner[v][c]] = 1)
\* a stored upload times out unverified
NoTimeout == \A v \in Versions, c \in Checksums : basis[v][c] # "timeout"
\* a put lands under a reservation the cron already released
NoLateObject ==
    [][\A v \in Versions, c \in Checksums :
        ~(/\ basis[v][c] = "released" /\ opOf[v][c] # None
          /\ object[opOf[v][c]] = 0 /\ object'[opOf[v][c]] # 0)]_vars
\* the upload-id reuse corner (README.md, "Assumptions"): a stored upload
\* whose object holds other bytes; a run that downloads them does not
\* verify it
NoStaleObject ==
    \A v \in Versions, c \in Checksums :
        (status[v][c] = "pending" /\ stored[v][c]) => object[opOf[v][c]] = c
\* the same corner after the download: a late put of an earlier operation
\* under the reused id replaces the object of a row a run then verifies
NoVerifiedObjectReplaced ==
    \A v \in Versions, c \in Checksums :
        (status[v][c] = "verified" /\ opOf[v][c] # None) => object[opOf[v][c]] = c
\* and where that form ends: the placement finds other bytes under the
\* row's upload id and rejects the row, which was never served
NoReplacedObjectRejected ==
    \A v \in Versions, c \in Checksums :
        ~(basis[v][c] = "lost" /\ opOf[v][c] # None /\ object[opOf[v][c]] \notin {0, c})
\* the placement rejects a verified row whose upload no longer holds its
\* bytes
NoSourceLost == \A v \in Versions, c \in Checksums : basis[v][c] # "lost"
\* a row is recorded as archived after its upload stopped holding its
\* bytes (the id pruned, or the object replaced): the write reads nothing
\* from UPLOADS and rests on the object under the ARCHIVES key
NoMarkWithoutSource ==
    [][\A v \in Versions, c \in Checksums :
        ~(/\ archived'[v][c] /\ ~archived[v][c]
          /\ (opOf[v][c] = None \/ object[opOf[v][c]] # c))]_vars
\* a returned row is verified while its ARCHIVES key already holds its
\* bytes, which a pass copied for an earlier generation that another pass
\* rejected as lost.  No pass copies for the returned one (PlaceList), and
\* the state the trace ends in satisfies PlaceMark's guard: the write that
\* archives it, one step on, rests on that object, which is what
\* MARK_REVISION_ARCHIVED's missing generation guard allows
NoEarlierGenerationArchive ==
    [][\A v \in Versions, c \in Checksums :
        ~(/\ status[v][c] = "pending" /\ status'[v][c] = "verified"
          /\ archive[v][c] = c)]_vars
\* two passes head one empty key, and the second put finds it filled
NoCreateRaceLost ==
    [][\A k \in Placers :
        ~(/\ place[k].phase = "headed" /\ place'[k].phase = "idle"
          /\ archive[place[k].v][place[k].c] # 0
          /\ object[place[k].id] = place[k].c)]_vars
\* a rejection is refused by its generation guard: since the listing the
\* row was rejected by another pass, returned, and was verified again
NoStaleLossRefused ==
    [][\A k \in Placers :
        ~(/\ place[k].phase = "lost" /\ place'[k].phase = "idle"
          /\ status[place[k].v][place[k].c] = "verified" /\ ~archived[place[k].v][place[k].c]
          /\ pubAt[place[k].v][place[k].c] # place[k].stamp)]_vars
\* a verified row whose ARCHIVES key holds an object that is not its
\* archive: no pass places it or rejects it
NoStrayBlocks ==
    \A v \in Versions, c \in Checksums :
        ~(status[v][c] = "verified" /\ archive[v][c] \notin {0, c})

=============================================================================
