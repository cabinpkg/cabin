//! The package mutations: publish and yank. Publish's validation and
//! policy live in `crate::publish` and `crate::quota`; this is the
//! runtime pipeline around them - the rate limit, the quota preflight,
//! the revision disposition, and the R2-then-D1 write phase with its
//! blob self-heal.

use serde::Deserialize;
use worker::{D1Database, Env, Request, Response, console_error, console_log};

use crate::auth::{AuthContext, Scope};
use crate::error;
use crate::glue::{
    CountRecord, MAX_MUTATION_BODY_BYTES, bounded_body, bucket_from_columns, changed_rows,
    commit_object, consume_one, error_response, governor_refusal_response, js_int,
    json_response_with_status, non_negative, now_epoch_ms, now_iso8601, write_gate,
};
use crate::governor::{Consume, Decision, OpPool, Reserve, StoragePool};
use crate::governor_client::{self, Gate};
use crate::publish;
use crate::{quota, sql, verify};

use super::denial_response;

#[derive(Deserialize)]
struct MetadataRecord {
    metadata_json: String,
}

#[derive(Deserialize)]
struct StoredRevisionRecord {
    revision: String,
    checksum: String,
    verification: String,
}

#[derive(Deserialize)]
struct YankedRecord {
    yanked: i64,
    /// Whether any revision of the version is verified (the
    /// `EXISTS(...) AS verified` column): yank applies to versions
    /// the registry actually serves.
    verified: i64,
}

/// The yank request body, exactly `{"yanked": <bool>}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct YankBody {
    yanked: bool,
}

/// `PUT /api/v1/packages/<scope>/<name>/<version>`: the publish route
/// (`docs/remote-registry.md`, "Publish"). Validation order and status
/// mapping follow `crate::publish`, preceded by the budget gate (`503`),
/// the publish rate limit (`429`), and the scope-membership gate (the
/// uniform `403` - publishing under a scope creates the package row, so
/// membership alone decides, and a scope that does not exist answers
/// exactly like one the user is not a member of), and followed - for
/// genuinely new versions and replacements of rejected ones only - by
/// the archive-size cap (`413`), the `-`/`_` twin-name reject for new
/// packages (`400`, before the quotas - name validity does not depend
/// on quota state), and the per-user quota checks (`403`);
/// on success the archive lands in R2 first (an orphaned blob from a
/// crash between the two writes stays conservatively represented by
/// its governor reservation - see `docs/runbook.md`), then one atomic
/// D1 batch inserts (or, for a
/// rejected row, replaces) the package and version rows and bumps the
/// storage self-accounting. New rows start `pending` and the `201`
/// reports it: nothing becomes resolvable before the verifier says so.
// The route triple plus the request plumbing exceeds the argument lint,
// and the publish pipeline is one deliberately linear sequence of gate
// checks in documented order, so it also runs long; splitting either
// would scatter that structure across helpers rather than clarify it.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn publish_response(
    req: &mut Request,
    env: &Env,
    db: &D1Database,
    auth: &AuthContext,
    scope: &str,
    name: &str,
    version: &str,
) -> worker::Result<Response> {
    if let Some(blocked) = write_gate(env, db).await? {
        return Ok(blocked);
    }
    if !auth.scopes.contains(&Scope::Publish) {
        return error_response(403, error::PUBLISH_SCOPE_REQUIRED);
    }

    let quotas = quota::quotas_for_class(&auth.quota_class);
    let bucket_quotas = quota::quotas_for_class(&auth.user_quota_class);
    if let Some(limited) = publish_rate_limit(env, db, auth, &bucket_quotas).await? {
        return Ok(limited);
    }

    // After the rate limit, so probing scopes is throttled like any
    // other publish attempt; before the body is buffered, like every
    // other authorization check. The scope-limit check shares the
    // membership gate's byte-identical refusal so a scope-limited
    // token's denials look like anyone else's.
    if auth.scope_limit_refuses(scope) || !is_scope_member(db, scope, auth.user_id).await? {
        return error_response(403, error::SCOPE_MEMBERSHIP_REQUIRED);
    }

    // The `new-revision` opt-in rides as a query parameter; any other
    // value than the literal `true` is a malformed request, so a typo
    // can never silently drop the opt-in.
    let new_revision = {
        let url = req.url()?;
        let mut flag = false;
        for (key, value) in url.query_pairs() {
            if key == "new-revision" {
                if value != "true" {
                    return error_response(400, error::INVALID_NEW_REVISION_QUERY);
                }
                flag = true;
            }
        }
        flag
    };
    let Some(body) = bounded_body(req, publish::MAX_BODY_BYTES).await? else {
        return error_response(400, publish::BODY_TOO_LARGE);
    };
    let frame = match publish::decode_frame(&body) {
        Ok(frame) => frame,
        Err(detail) => return error_response(400, detail),
    };
    let archive_bytes = frame.archive.len() as u64;
    // Reject a body that cannot be a profile zip before hashing it; the
    // full profile is checked later by the async verifier.
    if let Err(detail) = publish::sanity_check_zip(frame.archive) {
        return error_response(400, detail);
    }
    // The digest comes before metadata validation: the revision id is
    // its leading hex prefix, and the canonical source path the
    // metadata must carry embeds it.
    let checksum = crate::checksum::from_hex(&sha256_hex(frame.archive).await?);
    let revision = crate::checksum::revision_id(&checksum);
    let metadata = match publish::validate_metadata(scope, name, version, revision, frame.metadata)
    {
        Ok(metadata) => metadata,
        Err(detail) => return error_response(400, detail),
    };
    if let Err(detail) = publish::verify_checksum(&metadata, &checksum) {
        return error_response(400, detail);
    }
    // The frame parsed as JSON, so it is valid UTF-8; the stored column
    // is the uploaded document verbatim.
    let Ok(metadata_text) = std::str::from_utf8(frame.metadata) else {
        return error_response(400, publish::METADATA_NOT_JSON);
    };

    let revive = match revision_disposition(
        db,
        scope,
        name,
        version,
        revision,
        &checksum,
        new_revision,
        metadata_text,
    )
    .await?
    {
        RevisionDisposition::Answered(response) => {
            // The idempotent no-op (200) is a retry of a committed
            // publish that still holds the row's exact bytes, so it is
            // the one chance to self-heal a primary blob a reclaim
            // race deleted. The 409 arms get no heal: their uploaded
            // bytes were refused.
            if response.status_code() == 200 {
                heal_blobs_on_retry(env, &checksum, frame.archive).await?;
            }
            return Ok(response);
        }
        RevisionDisposition::Revive => true,
        RevisionDisposition::New { .. } => false,
    };

    // The archive-size cap and the per-user quotas gate genuinely new
    // versions - including a replacement of a rejected one, whose new
    // archive consumes quota like any other (the rejected row's own
    // bytes were refunded at rejection): a byte-identical re-publish of
    // an already-stored archive (even one grandfathered above the
    // current cap) stays the idempotent no-op above and never consumes
    // quota.
    if let Err(denial) = quota::check_archive_size(archive_bytes, &quotas) {
        return denial_response(env, &denial, None);
    }
    // ponytail: the quota counts below are a preflight, not a serialized
    // transaction - concurrent publishes can each pass the same
    // near-limit check and overshoot a quota by up to the in-flight
    // request count. The CAS'd rate limit bounds that per user at the
    // bucket burst, however many tokens the user holds; the budget
    // headroom and the breaker absorb the transient. Move the checks
    // into conditional inserts if that ever stops holding.
    let now = now_iso8601();
    let Some(day_prefix) = quota::utc_day_prefix(&now).map(str::to_owned) else {
        console_error!("clock produced a non-ISO timestamp: {now}");
        return error_response(500, error::INTERNAL);
    };
    let (counts, twin_exists) = publish_counts(db, auth.user_id, scope, name, &day_prefix).await?;
    // The deterministic `-`/`_` twin reject (`docs/architecture.md`,
    // "Name fidelity") gates new packages only, and answers before the
    // quota 403s: whether a name can exist does not depend on the
    // publisher's quota state.
    if !counts.package_exists && twin_exists {
        return error_response(400, publish::NAME_TWIN_CONFLICT);
    }
    if let Err(denial) = quota::check_publish(archive_bytes, &counts, &quotas) {
        return denial_response(env, &denial, None);
    }

    let new = NewRevision {
        scope,
        name,
        version,
        revision,
        checksum: &checksum,
        metadata_text,
        published_at: &now,
        archive: frame.archive,
        user_id: auth.user_id,
        opt_in: new_revision,
    };
    let persisted = if revive {
        revive_rejected_revision(env, db, &new).await?
    } else {
        persist_new_revision(env, db, &new).await?
    };
    match persisted {
        Persist::Done => {}
        Persist::Refused(response) => return Ok(response),
        Persist::Lost => {
            // The batch's own guards suppressed the write: a twin
            // publish, a concurrent revision, or a verdict moved
            // first. Re-read and answer exactly as if this request
            // had arrived after the winner; a vanished version row
            // (the twin case) answers the twin `400` the preflight
            // would have.
            return match revision_disposition(
                db,
                scope,
                name,
                version,
                revision,
                &checksum,
                new_revision,
                metadata_text,
            )
            .await?
            {
                RevisionDisposition::Answered(response) => {
                    if response.status_code() == 200 {
                        heal_blobs_on_retry(env, &checksum, frame.archive).await?;
                    }
                    Ok(response)
                }
                RevisionDisposition::Revive => {
                    // Rejected again (a third racer): the conservative
                    // refusal; a retry resolves it.
                    error_response(409, error::VERSION_IMMUTABLE)
                }
                // No revision rows at all means the twin guard
                // suppressed the package; rows without a blocking
                // sibling mean the losing guard's cause (a concurrent
                // revision or verdict) has already moved on - a
                // transient the conservative refusal covers, and a
                // retry resolves cleanly.
                RevisionDisposition::New {
                    version_has_revisions: false,
                } => error_response(400, publish::NAME_TWIN_CONFLICT),
                RevisionDisposition::New {
                    version_has_revisions: true,
                } => error_response(409, error::VERSION_IMMUTABLE),
            };
        }
    }

    json_response_with_status(
        201,
        &serde_json::json!({
            "ok": true,
            "name": format!("{scope}/{name}"),
            "version": version,
            "checksum": metadata.checksum,
            "revision": revision,
            "verification": "pending",
        })
        .to_string(),
    )
}

/// Whether the token's user is a member (any role) of `scope`. A scope
/// that does not exist has no members, so the caller's uniform refusal
/// needs no separate existence check.
async fn is_scope_member(db: &D1Database, scope: &str, user_id: i64) -> worker::Result<bool> {
    let membership: CountRecord = db
        .prepare(sql::SCOPE_MEMBERSHIP)
        .bind(&[scope.into(), js_int(user_id)])?
        .first(None)
        .await?
        .ok_or_else(|| worker::Error::RustError("empty COUNT(*) result".to_owned()))?;
    Ok(membership.n > 0)
}

/// `PATCH /api/v1/packages/<scope>/<name>/<version>/yank`
/// (`docs/remote-registry.md`, "Yank"): idempotent, and the row's
/// `yanked` column is the single home of yank state - the read path
/// overrides the stored metadata's field from it. Gated by the budget
/// breaker (`503`) like publish; yank has no rate limit or quota.
/// The scope-membership gate (the uniform `403`) answers before the
/// version lookup, so a non-member can never probe which versions exist
/// under a foreign scope. Yank applies to **verified** versions only: a
/// pending or rejected version was never part of the registry's
/// resolvable surface, so there is nothing to retract and the triple
/// answers an authenticated 404.
pub(super) async fn yank_response(
    req: &mut Request,
    env: &Env,
    db: &D1Database,
    auth: &AuthContext,
    scope: &str,
    name: &str,
    version: &str,
) -> worker::Result<Response> {
    if let Some(blocked) = write_gate(env, db).await? {
        return Ok(blocked);
    }
    if !auth.scopes.contains(&Scope::Yank) {
        return error_response(403, error::YANK_SCOPE_REQUIRED);
    }
    if auth.scope_limit_refuses(scope) || !is_scope_member(db, scope, auth.user_id).await? {
        return error_response(403, error::SCOPE_MEMBERSHIP_REQUIRED);
    }
    let Some(body) = bounded_body(req, MAX_MUTATION_BODY_BYTES).await? else {
        return error_response(400, error::INVALID_YANK_BODY);
    };
    let Ok(YankBody { yanked }) = serde_json::from_slice(&body) else {
        return error_response(400, error::INVALID_YANK_BODY);
    };

    let existing: Option<YankedRecord> = db
        .prepare(sql::VERSION_YANK_STATE)
        .bind(&[scope.into(), name.into(), version.into()])?
        .first(None)
        .await?;
    let Some(existing) = existing else {
        return error_response(404, error::NOT_FOUND);
    };
    if existing.verified == 0 {
        return error_response(404, error::NOT_FOUND);
    }
    let changed = (existing.yanked != 0) != yanked;
    if changed {
        db.prepare(sql::SET_VERSION_YANKED)
            .bind(&[
                i32::from(yanked).into(),
                scope.into(),
                name.into(),
                version.into(),
            ])?
            .run()
            .await?;
    }
    // The resulting state, plus whether this request changed it (the
    // idempotent no-op reports `changed: false`).
    json_response_with_status(
        200,
        &serde_json::json!({ "ok": true, "yanked": yanked, "changed": changed }).to_string(),
    )
}

/// Lowercase SHA-256 hex of `bytes` via the runtime's native
/// `SubtleCrypto` digest - hashing multi-MiB archives with a wasm `sha2`
/// would burn CPU budget instead. If per-request CPU ever nears the free
/// plan's limit anyway, the next step is the runtime's `DigestStream`
/// (hash while the body streams in, no full buffer); measurement is
/// deferred to the load-testing step, do not rework speculatively.
async fn sha256_hex(bytes: &[u8]) -> worker::Result<String> {
    use worker::js_sys::{Function, Promise, Reflect, Uint8Array};
    use worker::wasm_bindgen::{JsCast, JsValue};
    use worker::wasm_bindgen_futures::JsFuture;

    let crypto = Reflect::get(&worker::js_sys::global(), &JsValue::from_str("crypto"))?;
    let subtle = Reflect::get(&crypto, &JsValue::from_str("subtle"))?;
    let digest: Function = Reflect::get(&subtle, &JsValue::from_str("digest"))?.dyn_into()?;
    let promise: Promise = digest
        .call2(
            &subtle,
            &JsValue::from_str("SHA-256"),
            &Uint8Array::from(bytes),
        )?
        .dyn_into()?;
    let buffer = JsFuture::from(promise).await?;
    Ok(crate::auth::hex(&Uint8Array::new(&buffer).to_vec()))
}

#[derive(Deserialize)]
struct UserUsageRecord {
    stored_bytes: i64,
}

#[derive(Deserialize)]
struct PackageCountsRecord {
    package_count: i64,
    new_today: i64,
}

/// Everything the write phase of a validated, quota-cleared publish
/// needs.
struct NewRevision<'a> {
    scope: &'a str,
    name: &'a str,
    version: &'a str,
    /// The packaging-revision id: the checksum's leading hex prefix.
    revision: &'a str,
    checksum: &'a str,
    metadata_text: &'a str,
    published_at: &'a str,
    archive: &'a [u8],
    user_id: i64,
    /// The `new-revision` opt-in, re-enforced inside the batch guards.
    opt_in: bool,
}

/// The write phase's outcome: persisted, lost its guarded race, or
/// refused by the governor before any billable R2 call.
enum Persist {
    Done,
    Lost,
    Refused(Response),
}

/// The publish write phase: R2 before D1, skipping the upload when the
/// content-addressed blob is already there (e.g. the same archive
/// published under a name it was yanked from, or a retry after a crash
/// between the two writes), then one atomic D1 batch for the package and
/// version rows plus the storage self-accounting. The row starts
/// `pending`: it becomes resolvable only once the verifier says so.
///
/// Every billable R2 call is governed (`docs/architecture.md`, "The
/// cost governor"): the existence head consumes a publish-plane Class B
/// op, and a fresh upload consumes a Class A op plus a storage
/// reservation keyed by the content-addressed object key - so retries
/// and concurrent identical publishes share one reservation instead of
/// double-counting, and a crash after the put leaves the reservation
/// conservatively held (never auto-released; reconciliation settles it
/// once the D1 rows prove the blob live, and reports it otherwise).
/// After the batch commits the reservation settles into committed
/// usage.
///
/// The accounting decision lives inside the batch (one transaction): the
/// meta bump counts the archive only when the row just inserted is the
/// checksum's sole **live** (non-rejected) reference - a rejected row's
/// bytes were refunded when its blob was reclaimed, so it must not
/// suppress re-counting a re-uploaded blob. That way the crash-retry
/// path - blob already uploaded but never counted - still accounts for
/// it, a second name sharing the blob never double-counts it, and two
/// concurrent first publishes of the same archive serialize on the
/// transaction so exactly one of them counts it. Backup replication no
/// longer rides publish at all: only versions that become **verified**
/// enter the durable backup queue ([`sql::ENQUEUE_VERIFIED_BACKUP`]).
///
/// [`Persist::Lost`] means the batch's `-`/`_` twin guard suppressed
/// both inserts - a twin publish won the race after this request's
/// preflight - and nothing was persisted (the uploaded blob stays
/// behind exactly like a crash between the two writes: an orphan the
/// ledger keeps conservatively represented); the caller answers the
/// twin `400`.
async fn persist_new_revision(
    env: &Env,
    db: &D1Database,
    new: &NewRevision<'_>,
) -> worker::Result<Persist> {
    let key = format!("blobs/sha256/{}", crate::checksum::hex(new.checksum));
    let bucket = env.bucket("BLOBS")?;
    match governor_client::decide(env, &consume_one(OpPool::BPublish)).await {
        Gate::Allowed => {}
        Gate::Refused(refusal) => {
            return Ok(Persist::Refused(governor_refusal_response(
                refusal.as_ref(),
                true,
            )?));
        }
    }
    if bucket.head(&key).await?.is_none() {
        let admit = Decision {
            consume: vec![Consume {
                pool: OpPool::APublish,
                n: 1,
                principal: None,
                principal_cap: None,
            }],
            reserve: vec![Reserve {
                pool: StoragePool::Primary,
                key: key.clone(),
                bytes: new.archive.len() as u64,
            }],
            ..Decision::default()
        };
        match governor_client::decide(env, &admit).await {
            Gate::Allowed => {}
            Gate::Refused(refusal) => {
                return Ok(Persist::Refused(governor_refusal_response(
                    refusal.as_ref(),
                    true,
                )?));
            }
        }
        bucket.put(&key, new.archive.to_vec()).execute().await?;
    }

    let archive_size = js_int(i64::try_from(new.archive.len()).unwrap_or(i64::MAX));
    let opt_in = js_int(i64::from(new.opt_in));
    let results = db
        .batch(vec![
            db.prepare(sql::INSERT_PACKAGE).bind(&[
                new.scope.into(),
                new.name.into(),
                new.published_at.into(),
                js_int(new.user_id),
            ])?,
            db.prepare(sql::INSERT_VERSION_ROW).bind(&[
                new.scope.into(),
                new.name.into(),
                new.version.into(),
            ])?,
            db.prepare(sql::COUNT_STORED_BYTES_ON_PUBLISH).bind(&[
                new.checksum.into(),
                archive_size.clone(),
                new.scope.into(),
                new.name.into(),
                new.version.into(),
                new.revision.into(),
                new.metadata_text.into(),
                opt_in.clone(),
            ])?,
            db.prepare(sql::INSERT_REVISION).bind(&[
                new.scope.into(),
                new.name.into(),
                new.version.into(),
                new.revision.into(),
                new.checksum.into(),
                new.metadata_text.into(),
                new.published_at.into(),
                archive_size,
                js_int(new.user_id),
                opt_in,
            ])?,
        ])
        .await?;
    // The revision insert changes zero rows under the in-batch guards
    // - the twin guard suppressed the package (and so the version
    // row), a racing byte-identical publish committed the same
    // revision key first, or the opt-in guard refused an unflagged
    // respin racing a live sibling; the accounting statement ran
    // just before it against the same pre-insert state under the
    // same guards, so it added nothing then either.  The caller
    // re-reads to tell the cases apart.
    let revision_insert = results
        .get(3)
        .ok_or_else(|| worker::Error::RustError("missing batch result 3".to_owned()))?;
    if changed_rows(revision_insert.meta()?) == 0 {
        return Ok(Persist::Lost);
    }

    // The row now references the blob: settle the reservation into
    // committed usage (best-effort - a lost settle leaves conservative
    // reserved state for reconciliation, never unaccounted spend).
    governor_client::settle(
        env,
        &commit_object(StoragePool::Primary, &key, new.archive.len() as u64),
    )
    .await;

    heal_blob_if_reclaimed(env, &bucket, &key, new.archive).await?;
    Ok(Persist::Done)
}

/// Self-heal for the head-skip/reclaim race: a reclaim delete whose
/// refcount was read before the publish batch committed can land after
/// the earlier head, leaving the just-inserted row's blob missing; the
/// request still holds the bytes, so one more head buys the repair.
/// Every call here is billable, so the whole repair is opportunistic:
/// a governor refusal skips it (the publish already succeeded, and the
/// missing-blob case stays loud on the artifact route) rather than
/// initiating unpaid R2 work.
async fn heal_blob_if_reclaimed(
    env: &Env,
    bucket: &worker::Bucket,
    key: &str,
    archive: &[u8],
) -> worker::Result<()> {
    match governor_client::decide(env, &consume_one(OpPool::BPublish)).await {
        Gate::Allowed => {}
        Gate::Refused(_) => {
            console_log!("governor refused the post-publish heal head for {key}; skipping");
            return Ok(());
        }
    }
    if bucket.head(key).await?.is_some() {
        return Ok(());
    }
    let admit = Decision {
        consume: vec![Consume {
            pool: OpPool::APublish,
            n: 1,
            principal: None,
            principal_cap: None,
        }],
        // Idempotent against the committed row: same key, same bytes.
        reserve: vec![Reserve {
            pool: StoragePool::Primary,
            key: key.to_owned(),
            bytes: archive.len() as u64,
        }],
        ..Decision::default()
    };
    match governor_client::decide(env, &admit).await {
        Gate::Allowed => {}
        Gate::Refused(_) => {
            console_log!("governor refused the post-publish heal put for {key}; skipping");
            return Ok(());
        }
    }
    bucket.put(key, archive.to_vec()).execute().await?;
    Ok(())
}

/// The write phase for a publish that revives a **rejected** revision
/// (`docs/remote-registry.md`, "Verification lifecycle"): the rejected
/// revision never became part of the registry, and the revision id
/// derives from the bytes, so a revival is always byte-identical -
/// fresh metadata, publisher, and timestamp, verification back to
/// `pending` with the old verdict cleared. R2 first, like
/// [`persist_new_revision`]. Both statements are guarded on the row
/// still being the rejected generation this request read (plus the
/// in-batch `new-revision` opt-in guard) - `false` means a concurrent
/// revival or verdict moved it first and nothing was changed (a stale
/// revival must never rewrite a live row, least of all drag a
/// verified one back to pending). The accounting is decided **before**
/// the row flips, mirroring the verifier's `apply_rejection`: the counter regains
/// the archive's bytes exactly when the guards will let the flip apply
/// and no other live row references the checksum - the rejection
/// refunded them, so the revived row is about to become the blob's
/// sole live reference and is re-counted exactly once.
async fn revive_rejected_revision(
    env: &Env,
    db: &D1Database,
    new: &NewRevision<'_>,
) -> worker::Result<Persist> {
    let key = format!("blobs/sha256/{}", crate::checksum::hex(new.checksum));
    let bucket = env.bucket("BLOBS")?;
    // The unconditional put is one Class A op plus a storage
    // reservation; the reservation is idempotent when the ledger
    // already carries the content-addressed key (same key means the
    // same bytes).
    let admit = Decision {
        consume: vec![Consume {
            pool: OpPool::APublish,
            n: 1,
            principal: None,
            principal_cap: None,
        }],
        reserve: vec![Reserve {
            pool: StoragePool::Primary,
            key: key.clone(),
            bytes: new.archive.len() as u64,
        }],
        ..Decision::default()
    };
    match governor_client::decide(env, &admit).await {
        Gate::Allowed => {}
        Gate::Refused(refusal) => {
            return Ok(Persist::Refused(governor_refusal_response(
                refusal.as_ref(),
                true,
            )?));
        }
    }
    // Unconditional put, unlike persist_new_revision's head-first skip:
    // the revival re-uses the rejected bytes, so the rejecting
    // verdict's reclaim delete may still be in flight, and a head could
    // observe the object right before that delete lands - skipping the
    // upload would then leave a pending row whose blob is gone.
    // ponytail: a delete decided before this batch can still land after
    // this put (two stores, no shared transaction); that residual
    // window needs the same version's verdict and replacement in flight
    // simultaneously, fails loudly (the artifact route's missing-blob
    // 500), and the verified-only BACKUP replica holds the bytes for
    // recovery when the version had ever been verified.
    bucket.put(&key, new.archive.to_vec()).execute().await?;

    let archive_size = js_int(i64::try_from(new.archive.len()).unwrap_or(i64::MAX));
    let opt_in = js_int(i64::from(new.opt_in));
    let results = db
        .batch(vec![
            db.prepare(sql::COUNT_STORED_BYTES_ON_REVIVAL).bind(&[
                new.scope.into(),
                new.name.into(),
                new.version.into(),
                new.checksum.into(),
                new.checksum.into(),
                archive_size,
                new.revision.into(),
                opt_in.clone(),
                new.metadata_text.into(),
            ])?,
            db.prepare(sql::REVIVE_REJECTED_REVISION).bind(&[
                new.metadata_text.into(),
                new.published_at.into(),
                js_int(new.user_id),
                new.scope.into(),
                new.name.into(),
                opt_in,
                new.version.into(),
                new.revision.into(),
                new.checksum.into(),
            ])?,
        ])
        .await?;
    let row_flip = results
        .get(1)
        .ok_or_else(|| worker::Error::RustError("missing batch result 1".to_owned()))?;
    if changed_rows(row_flip.meta()?) == 0 {
        // Lost the race; the blob uploaded above is at worst an
        // unreferenced orphan (see docs/runbook.md), which the kept
        // reservation represents conservatively.
        return Ok(Persist::Lost);
    }

    governor_client::settle(
        env,
        &commit_object(StoragePool::Primary, &key, new.archive.len() as u64),
    )
    .await;

    // Same self-heal as persist_new_revision: repair the blob if a
    // reclaim delete landed between the put above and the batch commit.
    heal_blob_if_reclaimed(env, &bucket, &key, new.archive).await?;
    Ok(Persist::Done)
}

/// The idempotent no-op's self-heal: the retry holds the row's exact
/// bytes, so it repairs a primary blob a reclaim race deleted. Like
/// every repair path it is governed and opportunistic - a refusal
/// skips it without failing the (already correct) response. Backup
/// replication no longer rides retries: the verified-backup queue is
/// durable on its own.
async fn heal_blobs_on_retry(env: &Env, checksum: &str, archive: &[u8]) -> worker::Result<()> {
    let key = format!("blobs/sha256/{}", crate::checksum::hex(checksum));
    let bucket = env.bucket("BLOBS")?;
    heal_blob_if_reclaimed(env, &bucket, &key, archive).await
}

/// What the publish handler decided from the version's existing
/// revision rows.
enum RevisionDisposition {
    /// The request is answered directly: a byte-identical
    /// republication of a pending or verified revision is the `200`
    /// no-op reporting that revision's verification status (the
    /// revision id derives from the bytes, so equal checksums are the
    /// whole test - stored metadata is preserved, never overwritten);
    /// a same-prefix different-bytes collision and a different-bytes
    /// publish without the `new-revision` opt-in are `409`s.
    Answered(Response),
    /// A rejected revision with exactly these bytes exists: revive it
    /// in place ([`revive_rejected_revision`]); the guarded update
    /// re-checks the rejected state inside the batch.
    Revive,
    /// No revision with these bytes exists and the rules allow a new
    /// one: insert it ([`persist_new_revision`]).  Whether the
    /// version already carries revision rows disambiguates a lost
    /// in-batch race afterwards: with none, the loss was the twin
    /// guard; with some, it was a concurrent revision or verdict
    /// (transient - a retry resolves it).
    New { version_has_revisions: bool },
}

/// Idempotency, immutability, the `new-revision` opt-in, and the
/// rejected-revival carve-out for the existing revisions of
/// `(scope, name, version)`.  The same checks are enforced *inside*
/// the write batches ([`sql::INSERT_REVISION`] /
/// [`sql::REVIVE_REJECTED_REVISION`]), so this preflight only shapes
/// responses - a racer that slips between the read and the batch loses
/// the guarded write, and the caller re-runs this function against the
/// winner's state.
#[allow(clippy::too_many_arguments)] // the revision quad plus the request plumbing
async fn revision_disposition(
    db: &D1Database,
    scope: &str,
    name: &str,
    version: &str,
    revision: &str,
    checksum: &str,
    new_revision: bool,
    metadata_text: &str,
) -> worker::Result<RevisionDisposition> {
    let existing: Vec<StoredRevisionRecord> = db
        .prepare(sql::EXISTING_REVISIONS)
        .bind(&[scope.into(), name.into(), version.into()])?
        .all()
        .await?
        .results()?;
    let live_other_bytes = existing.iter().any(|row| {
        row.checksum != checksum
            && matches!(
                verify::Status::parse(&row.verification),
                Some(verify::Status::Pending | verify::Status::Verified)
            )
    });
    if let Some(row) = existing.iter().find(|row| row.revision == revision) {
        let Some(status) = verify::Status::parse(&row.verification) else {
            // An invariant break (the schema never writes other
            // values); fail safe by refusing rather than guessing a
            // transition.
            console_error!(
                "stored verification for {scope}/{name}@{version}#{revision} is invalid: {}",
                row.verification
            );
            return error_response(500, error::INTERNAL).map(RevisionDisposition::Answered);
        };
        if row.checksum != checksum {
            // Two different archives whose digests share the 16-hex
            // prefix: astronomically unlikely, and silently replacing
            // either side would break immutability - fail loudly.
            return error_response(409, error::REVISION_COLLISION)
                .map(RevisionDisposition::Answered);
        }
        if status == verify::Status::Rejected {
            // Byte-identical revival of a rejected revision - but a
            // live sibling with different bytes still demands the
            // opt-in, or an unflagged retry could sneak a superseded
            // respin back beside what is currently served.
            if live_other_bytes && !new_revision {
                return error_response(409, error::NEW_REVISION_REQUIRED)
                    .map(RevisionDisposition::Answered);
            }
            // A revival re-enters the live set, so the invariance
            // check applies exactly as for a new revision: the
            // rejected document never constrained anyone, and a
            // sibling published since the rejection may carry
            // different resolver metadata this revival must not
            // contradict.
            if live_other_bytes
                && let Some(response) =
                    live_metadata_conflict(db, scope, name, version, metadata_text).await?
            {
                return Ok(RevisionDisposition::Answered(response));
            }
            return Ok(RevisionDisposition::Revive);
        }
        return json_response_with_status(
            200,
            &serde_json::json!({
                "ok": true,
                "no_op": true,
                "revision": revision,
                "verification": status.as_str(),
            })
            .to_string(),
        )
        .map(RevisionDisposition::Answered);
    }
    if live_other_bytes && !new_revision {
        return error_response(409, error::NEW_REVISION_REQUIRED)
            .map(RevisionDisposition::Answered);
    }
    if live_other_bytes
        && let Some(response) =
            live_metadata_conflict(db, scope, name, version, metadata_text).await?
    {
        return Ok(RevisionDisposition::Answered(response));
    }
    Ok(RevisionDisposition::New {
        version_has_revisions: !existing.is_empty(),
    })
}

/// The revision contract's preflight: a respin (or a revival) must
/// not change what resolution consumes.  One live sibling represents
/// the set (they agree by induction), and [`sql::INSERT_REVISION`] /
/// [`sql::REVIVE_REJECTED_REVISION`] re-enforce the rule inside their
/// transactions - this read only shapes the diagnostic.  `None` means
/// no conflict.
async fn live_metadata_conflict(
    db: &D1Database,
    scope: &str,
    name: &str,
    version: &str,
    metadata_text: &str,
) -> worker::Result<Option<Response>> {
    let sibling: Option<MetadataRecord> = db
        .prepare(sql::LIVE_REVISION_METADATA)
        .bind(&[scope.into(), name.into(), version.into()])?
        .first(None)
        .await?;
    let Some(sibling) = sibling else {
        return Ok(None);
    };
    match publish::resolver_metadata_conflict(&sibling.metadata_json, metadata_text) {
        Ok(None) => Ok(None),
        Ok(Some(_field)) => {
            error_response(409, error::REVISION_CHANGES_RESOLVER_METADATA).map(Some)
        }
        Err(_) => {
            // A stored document that no longer parses is an
            // invariant break; refuse rather than guess.
            console_error!(
                "stored metadata for a live revision of {scope}/{name}@{version} is invalid"
            );
            error_response(500, error::INTERNAL).map(Some)
        }
    }
}

/// The publish token bucket (`429`), charged per publish attempt - valid
/// or not - before the body is even buffered. On an allowed take the new
/// bucket state is persisted as a compare-and-swap against the state the
/// take was computed from, so concurrent requests as one user cannot
/// all spend the same snapshot; a loser re-reads the row and retries
/// once. On a denial the stored state is left untouched, so refill keeps
/// accruing from the last persisted take, and the response carries
/// `Retry-After`.
async fn publish_rate_limit(
    env: &Env,
    db: &D1Database,
    auth: &AuthContext,
    quotas: &quota::ClassQuotas,
) -> worker::Result<Option<Response>> {
    // Enough attempts to drain a full burst even when every one of them
    // loses a race to a parallel publisher as the same user.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // small quota constant
    let attempts = quotas.publish_burst.ceil() as usize + 1;
    let mut bucket = auth.bucket;
    for _ in 0..attempts {
        let outcome = quota::take_publish_token(bucket, now_epoch_ms(), quotas);
        if !outcome.allowed {
            return Ok(Some(denial_response(
                env,
                &quota::RATE_LIMITED,
                Some(outcome.retry_after_secs),
            )?));
        }
        if cas_bucket(db, auth.user_id, bucket, outcome.bucket).await? {
            return Ok(None);
        }
        bucket = read_bucket(db, auth.user_id).await?;
    }
    // Losing a burst's worth of races in a row means the user's bucket is
    // being spent concurrently right now; refusing the attempt is the limiter
    // working. The bucket refills within a minute, hence the short
    // Retry-After.
    denial_response(env, &quota::RATE_LIMITED, Some(1)).map(Some)
}

#[derive(Deserialize)]
struct BucketRecord {
    rl_tokens: Option<f64>,
    rl_updated_at: Option<String>,
}

/// The current bucket state straight from the user row.
async fn read_bucket(db: &D1Database, user_id: i64) -> worker::Result<Option<quota::Bucket>> {
    let record: Option<BucketRecord> = db
        .prepare(sql::USER_BUCKET)
        .bind(&[js_int(user_id)])?
        .first(None)
        .await?;
    Ok(record
        .and_then(|record| bucket_from_columns(record.rl_tokens, record.rl_updated_at.as_deref())))
}

/// Persists a bucket take iff the row still holds `prev` (`IS` makes the
/// comparison NULL-safe for a user that has never published). `false`
/// means a concurrent request won the race. Round-trip exactness holds:
/// the stored text and REAL came from these same f64 values.
async fn cas_bucket(
    db: &D1Database,
    user_id: i64,
    prev: Option<quota::Bucket>,
    next: quota::Bucket,
) -> worker::Result<bool> {
    use worker::wasm_bindgen::JsValue;
    let (prev_tokens, prev_updated_at) = match prev {
        Some(prev) => (
            JsValue::from_f64(prev.tokens),
            prev.updated_at_ms.to_string().into(),
        ),
        None => (JsValue::NULL, JsValue::NULL),
    };
    let result = db
        .prepare(sql::CAS_USER_BUCKET)
        .bind(&[
            next.tokens.into(),
            next.updated_at_ms.to_string().into(),
            js_int(user_id),
            prev_tokens,
            prev_updated_at,
        ])?
        .run()
        .await?;
    Ok(result.meta()?.and_then(|meta| meta.changes).unwrap_or(0) > 0)
}

/// Gathers the [`quota::PublishCounts`] for one prospective publish in a
/// single D1 batch - every statement is a point lookup or an aggregate
/// over an indexed column - plus whether the name has a `-`/`_` twin in
/// the scope (the deterministic reject the caller renders when the
/// package would be new; a preflight only - the persistence batch
/// repeats the guard transactionally).
async fn publish_counts(
    db: &D1Database,
    user_id: i64,
    scope: &str,
    name: &str,
    day_prefix: &str,
) -> worker::Result<(quota::PublishCounts, bool)> {
    let results = db
        .batch(vec![
            // Rejected versions are excluded: their bytes were refunded
            // when the verdict landed.
            db.prepare(sql::USER_STORED_BYTES)
                .bind(&[js_int(user_id)])?,
            // Both package quotas key on creation (`created_by`), so a
            // version published into someone else's package never counts
            // against the publisher's package quotas.
            db.prepare(sql::USER_PACKAGE_COUNTS)
                .bind(&[js_int(user_id), day_prefix.into()])?,
            db.prepare(sql::COUNT_PACKAGE_VERSIONS_SINCE).bind(&[
                scope.into(),
                name.into(),
                day_prefix.into(),
            ])?,
            db.prepare(sql::PACKAGE_EXISTS)
                .bind(&[scope.into(), name.into()])?,
            db.prepare(sql::TWIN_PACKAGE_EXISTS)
                .bind(&[scope.into(), name.into()])?,
        ])
        .await?;
    let user_usage: UserUsageRecord = first_row(&results, 0)?;
    let user_packages: PackageCountsRecord = first_row(&results, 1)?;
    let versions_today: CountRecord = first_row(&results, 2)?;
    let package_rows: CountRecord = first_row(&results, 3)?;
    let twin_rows: CountRecord = first_row(&results, 4)?;
    let counts = quota::PublishCounts {
        user_stored_bytes: non_negative(user_usage.stored_bytes),
        user_package_count: non_negative(user_packages.package_count),
        user_new_packages_today: non_negative(user_packages.new_today),
        package_versions_today: non_negative(versions_today.n),
        package_exists: package_rows.n > 0,
    };
    Ok((counts, twin_rows.n > 0))
}

/// The single row of one aggregate statement in a batch result.
fn first_row<T: serde::de::DeserializeOwned>(
    results: &[worker::D1Result],
    index: usize,
) -> worker::Result<T> {
    results
        .get(index)
        .ok_or_else(|| worker::Error::RustError(format!("missing batch result {index}")))?
        .results::<T>()?
        .into_iter()
        .next()
        .ok_or_else(|| worker::Error::RustError(format!("empty batch result {index}")))
}
