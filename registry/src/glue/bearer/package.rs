//! The package mutations: publish and yank. Publish's validation and
//! policy live in `crate::publish`, `crate::quota`, and `crate::upload`;
//! this is the runtime pipeline around them - the rate limit, the
//! disposition, the quota preflight, the atomic D1 admission, and the
//! store into UPLOADS that only an admitted upload reaches.

use serde::Deserialize;
use worker::{D1Database, Env, Request, Response, console_error};

use crate::auth::{AuthContext, Scope};
use crate::error;
use crate::glue::{
    CountRecord, MAX_MUTATION_BODY_BYTES, bounded_body, bucket_from_columns, changed_rows,
    consume_one, error_body_response, error_response, governor_refusal_response, iso8601_from_ms,
    js_int, json_response_with_status, non_negative, now_epoch_ms, now_iso8601, write_gate,
};
use crate::governor::OpPool;
use crate::governor_client::{self, Gate};
use crate::publish;
use crate::{quota, sql, upload, verify};

use super::denial_response;

#[derive(Deserialize)]
struct StoredRevisionRecord {
    revision: String,
    checksum: String,
    verification: String,
    upload_id: Option<String>,
    published_at: String,
    published_by: i64,
    stored: i64,
    resumable: i64,
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
/// the token's `publish` scope (`403`), the publish rate limit (`429`),
/// the scope-membership gate (the
/// uniform `403` - publishing under a scope creates the package row, so
/// membership alone decides, and a scope that does not exist answers
/// exactly like one the user is not a member of), the upload id
/// (`400`), and - for a version with no revisions yet - the upload
/// allowances that are already spent (`429`, before the body is read).
/// It is followed - for a new operation only, never for bytes already
/// recorded - by the archive-size cap (`413`), the `-`/`_` twin-name
/// reject for new packages (`400`), the per-user quota checks (`403`),
/// and the upload allowances (`429`): a preflight that shapes the
/// refusal, which the admission batch re-enforces in one transaction.
/// Only an admitted upload is written to UPLOADS, and only a stored one
/// becomes visible to the verifier. The `201` reports `pending`:
/// nothing becomes resolvable before the verifier says so.
// The route triple plus the request plumbing exceeds the argument lint,
// and the body is one linear sequence of gate checks in the documented
// order; splitting either would scatter that sequence across helpers.
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
    let upload_id = req.headers().get(upload::HEADER)?;
    let Some(upload_id) = upload::parse_id(upload_id.as_deref()) else {
        return error_response(400, upload::INVALID_UPLOAD_ID);
    };
    let (now, day_prefix) = clock()?;
    let preflight = Preflight {
        user_id: auth.user_id,
        shared_class: quota::shared_class(&auth.quota_class),
        scope,
        name,
        version,
        now: &now,
        day_prefix: &day_prefix,
        quotas: &quotas,
    };
    // A version with no revisions can only be a new operation, so an
    // allowance that is already spent refuses it without reading the
    // body. Any other request may be a retry of bytes already recorded,
    // which no allowance may refuse; it is decided once the bytes are
    // hashed.
    if let Some(refused) = preflight.spent_refusal(db).await? {
        return Ok(refused);
    }
    let Some(body) = bounded_body(req, publish::MAX_BODY_BYTES).await? else {
        return error_response(400, publish::BODY_TOO_LARGE);
    };
    let frame = match publish::decode_frame(&body) {
        Ok(frame) => frame,
        Err(detail) => return error_response(400, detail),
    };
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

    // The admission's own sample, taken with the body in hand: the
    // usage day, `published_at`, and the reservation's limit count from
    // here, not from when a slow upload began.
    let (now, day_prefix) = clock()?;
    let preflight = Preflight {
        now: &now,
        day_prefix: &day_prefix,
        ..preflight
    };
    let request = Submission {
        revision,
        checksum: &checksum,
        metadata_text,
        archive: frame.archive,
        upload_id,
        opt_in: new_revision,
    };
    // A request whose guarded write changed nothing lost a race - a
    // twin or byte-identical publish, a verdict, a released
    // reservation, or a publish that spent the quota headroom moved
    // first. It is decided once more against the winner's rows, exactly
    // as if it had arrived after them.
    for _ in 0..2 {
        if let Some(response) = attempt(env, db, &preflight, &request).await? {
            return Ok(response);
        }
    }
    // Still losing: the cause keeps moving (rejected again by a third
    // racer, or a stamp that does not advance the row's). The
    // conservative refusal covers the transient, and a retry resolves
    // it.
    error_response(409, error::VERSION_IMMUTABLE)
}

/// One clock sample and its UTC day prefix.
fn clock() -> worker::Result<(String, String)> {
    let now = now_iso8601();
    let day = quota::utc_day_prefix(&now)
        .ok_or_else(|| {
            worker::Error::RustError(format!("clock produced a non-ISO timestamp: {now}"))
        })?
        .to_owned();
    Ok((now, day))
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
/// under a foreign scope. Yank applies to verified versions only: a
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
/// (hash while the body streams in, no full buffer).
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

#[derive(Deserialize)]
struct UploadUsageRecord {
    user_bytes: i64,
    user_uploads: i64,
    shared_bytes: i64,
    shared_uploads: i64,
    pending: i64,
    version_has_revisions: i64,
    reset_at: String,
    retry_after_secs: i64,
}

impl UploadUsageRecord {
    fn usage(&self) -> quota::UploadUsage {
        quota::UploadUsage {
            user_bytes_today: non_negative(self.user_bytes),
            user_uploads_today: non_negative(self.user_uploads),
            shared_bytes_today: non_negative(self.shared_bytes),
            shared_uploads_today: non_negative(self.shared_uploads),
            pending: non_negative(self.pending),
        }
    }

    /// Renders an admission refusal: `429` with the denial's fields,
    /// and `Retry-After` only for an allowance that reopens on the
    /// clock.
    fn refusal(
        &self,
        denial: &quota::UploadDenial,
        archive_bytes: u64,
    ) -> worker::Result<Response> {
        let retry_after_secs = matches!(denial, quota::UploadDenial::Daily { .. })
            .then(|| non_negative(self.retry_after_secs));
        error_body_response(
            429,
            error::envelope_with_fields(
                &denial.detail(archive_bytes, &self.reset_at),
                denial.code(),
                denial.fields(&self.reset_at),
            ),
            retry_after_secs,
        )
    }
}

/// The validated bytes of one publish request.
struct Submission<'a> {
    /// The packaging-revision id: the checksum's leading hex prefix.
    revision: &'a str,
    checksum: &'a str,
    metadata_text: &'a str,
    archive: &'a [u8],
    upload_id: &'a str,
    /// The `new-revision` opt-in, re-enforced inside the admission.
    opt_in: bool,
}

impl Submission<'_> {
    fn archive_bytes(&self) -> u64 {
        self.archive.len() as u64
    }
}

/// How an operation takes its reservation (`crate::upload::Disposition`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Admit {
    New,
    Revive,
    Resume,
}

/// One pass of the publish write phase: decides the request against the
/// version's current rows and carries the decision out. `None` means a
/// guarded write changed nothing - the rows moved between the read and
/// the write - and the caller decides again.
///
/// The order is the contract (`docs/remote-registry.md`, "Publish"):
/// the D1 admission first, which reserves every allowance or none; then
/// the store, which only an admitted operation reaches; then the
/// confirmation, which is what makes the upload visible to the
/// verifier. A crash after the admission leaves a reservation the cron
/// settles from what UPLOADS holds; the daily allowances stay charged
/// either way.
async fn attempt(
    env: &Env,
    db: &D1Database,
    preflight: &Preflight<'_>,
    request: &Submission<'_>,
) -> worker::Result<Option<Response>> {
    let Preflight {
        scope,
        name,
        version,
        ..
    } = *preflight;
    let results = db
        .batch(vec![
            db.prepare(sql::EXISTING_REVISIONS).bind(&[
                scope.into(),
                name.into(),
                version.into(),
                preflight.now.into(),
                upload::RETRY_WINDOW.into(),
            ])?,
            db.prepare(sql::REVISION_BY_UPLOAD_ID)
                .bind(&[request.upload_id.into()])?,
        ])
        .await?;
    let existing: Vec<StoredRevisionRecord> = batch_rows(&results, 0)?;
    let bound: Vec<RevisionKeyRecord> = batch_rows(&results, 1)?;
    let recorded: Vec<upload::Recorded<'_>> = existing
        .iter()
        .map(|row| upload::Recorded {
            revision: &row.revision,
            checksum: &row.checksum,
            status: verify::Status::parse(&row.verification),
            upload_id: row.upload_id.as_deref(),
            stored: row.stored != 0,
            published_at: &row.published_at,
            published_by: row.published_by,
            resumable: row.resumable != 0,
        })
        .collect();
    let disposition = upload::dispose(
        &recorded,
        &upload::Request {
            revision: request.revision,
            checksum: request.checksum,
            upload_id: request.upload_id,
            user_id: preflight.user_id,
            now: preflight.now,
            new_revision: request.opt_in,
            upload_id_bound_elsewhere: bound.iter().any(|row| {
                (row.scope.as_str(), row.name.as_str(), row.version.as_str())
                    != (scope, name, version)
                    || row.revision != request.revision
            }),
        },
    );
    match disposition {
        upload::Disposition::NoOp(status) => no_op(request.revision, status),
        upload::Disposition::Conflict(detail) => error_response(409, detail).map(Some),
        upload::Disposition::Corrupt => {
            console_error!(
                "a stored revision of {scope}/{name}@{version} is outside the lifecycle"
            );
            error_response(500, error::INTERNAL).map(Some)
        }
        upload::Disposition::Converge {
            upload_id,
            published_at,
        } => {
            // The reservation is another request's, or this one's
            // earlier attempt: nothing new is admitted or charged.
            if let Some(refused) = govern_store(env, true).await? {
                return Ok(Some(refused));
            }
            let stored_at = store(env, upload_id, request.archive, true, preflight.now).await?;
            if confirm(
                db,
                preflight,
                request.revision,
                upload_id,
                published_at,
                &stored_at,
            )
            .await?
            {
                no_op(request.revision, verify::Status::Pending)
            } else {
                Ok(None)
            }
        }
        upload::Disposition::New => admit_and_store(env, db, preflight, request, Admit::New).await,
        upload::Disposition::Revive => {
            admit_and_store(env, db, preflight, request, Admit::Revive).await
        }
        upload::Disposition::Resume => {
            admit_and_store(env, db, preflight, request, Admit::Resume).await
        }
    }
}

/// The `200` of a publish that had nothing left to do.
fn no_op(revision: &str, status: verify::Status) -> worker::Result<Option<Response>> {
    json_response_with_status(
        200,
        &serde_json::json!({
            "ok": true,
            "no_op": true,
            "revision": revision,
            "verification": status.as_str(),
        })
        .to_string(),
    )
    .map(Some)
}

/// Takes the operation's reservation, stores its archive and confirms
/// it: the `201`, a refusal, or `None` when a guarded write changed
/// nothing ([`attempt`]).
async fn admit_and_store(
    env: &Env,
    db: &D1Database,
    preflight: &Preflight<'_>,
    request: &Submission<'_>,
    admit: Admit,
) -> worker::Result<Option<Response>> {
    let Preflight {
        scope,
        name,
        version,
        ..
    } = *preflight;
    if let Err(denial) = quota::check_archive_size(request.archive_bytes(), preflight.quotas) {
        return denial_response(env, &denial, None).map(Some);
    }
    if let Some(refused) = preflight
        .refusal(env, db, request.archive_bytes(), admit != Admit::Resume)
        .await?
    {
        return Ok(Some(refused));
    }
    // The R2 calls are governed before the admission: a refusal after
    // it would leave a charged reservation with nothing stored.
    let head_first = admit == Admit::Resume;
    if let Some(refused) = govern_store(env, head_first).await? {
        return Ok(Some(refused));
    }
    if !admit_upload(db, preflight, request, admit).await? {
        return Ok(None);
    }
    let stored_at = store(
        env,
        request.upload_id,
        request.archive,
        head_first,
        preflight.now,
    )
    .await?;
    if !confirm(
        db,
        preflight,
        request.revision,
        request.upload_id,
        preflight.now,
        &stored_at,
    )
    .await?
    {
        return Ok(None);
    }
    json_response_with_status(
        201,
        &serde_json::json!({
            "ok": true,
            "name": format!("{scope}/{name}"),
            "version": version,
            "checksum": request.checksum,
            "revision": request.revision,
            "verification": "pending",
        })
        .to_string(),
    )
    .map(Some)
}

#[derive(Deserialize)]
struct RevisionKeyRecord {
    scope: String,
    name: String,
    version: String,
    revision: String,
}

/// The admission batch of one operation (`sql::admission`): its gate
/// first, then the statements that apply only behind it. `true` when
/// the gate held and the reservation exists.
async fn admit_upload(
    db: &D1Database,
    preflight: &Preflight<'_>,
    request: &Submission<'_>,
    admit: Admit,
) -> worker::Result<bool> {
    let Preflight {
        user_id,
        shared_class,
        scope,
        name,
        version,
        now,
        ..
    } = *preflight;
    let size = js_int(i64::try_from(request.archive.len()).unwrap_or(i64::MAX));
    let gate = gate_binds(preflight, request, admit, &size);
    let charge_shared = || {
        db.prepare(sql::CHARGE_SHARED_DAILY_UPLOAD).bind(&[
            now.into(),
            shared_class.into(),
            size.clone(),
        ])
    };
    let count_stored_bytes = || {
        db.prepare(sql::COUNT_USER_STORED_BYTES)
            .bind(&[js_int(user_id), size.clone()])
    };
    // Where the batch reports the admission: the statement whose
    // changed row is the reservation.
    let (statements, admitted_at) = match admit {
        Admit::New => (
            vec![
                db.prepare(sql::ADMIT_NEW_UPLOAD).bind(&gate)?,
                charge_shared()?,
                db.prepare(sql::COUNT_ADMITTED_UPLOAD).bind(&[
                    js_int(user_id),
                    size.clone(),
                    scope.into(),
                    name.into(),
                ])?,
                db.prepare(sql::ENSURE_PACKAGE).bind(&[
                    scope.into(),
                    name.into(),
                    now.into(),
                    js_int(user_id),
                ])?,
                db.prepare(sql::ENSURE_VERSION).bind(&[
                    scope.into(),
                    name.into(),
                    version.into(),
                ])?,
                db.prepare(sql::INSERT_ADMITTED_REVISION).bind(&[
                    scope.into(),
                    name.into(),
                    version.into(),
                    request.revision.into(),
                    request.checksum.into(),
                    request.metadata_text.into(),
                    now.into(),
                    size.clone(),
                    js_int(user_id),
                    request.upload_id.into(),
                    shared_class.into(),
                ])?,
            ],
            5,
        ),
        Admit::Revive => (
            vec![
                db.prepare(sql::REVIVE_REJECTED_UPLOAD).bind(&gate)?,
                db.prepare(sql::CHARGE_USER_DAILY_UPLOAD).bind(&[
                    now.into(),
                    js_int(user_id),
                    size.clone(),
                ])?,
                charge_shared()?,
                count_stored_bytes()?,
            ],
            0,
        ),
        Admit::Resume => (
            vec![
                db.prepare(sql::RESUME_RELEASED_UPLOAD).bind(&gate)?,
                count_stored_bytes()?,
            ],
            0,
        ),
    };
    let results = db.batch(statements).await?;
    let admitted = results
        .get(admitted_at)
        .ok_or_else(|| worker::Error::RustError(format!("missing batch result {admitted_at}")))?;
    Ok(changed_rows(admitted.meta()?) > 0)
}

/// The bind list the three gates share, numbered as `sql::admission`
/// documents: each gate reads the positions it needs, and D1 wants
/// exactly as many values as the statement's highest position.
fn gate_binds(
    preflight: &Preflight<'_>,
    request: &Submission<'_>,
    admit: Admit,
    size: &worker::wasm_bindgen::JsValue,
) -> Vec<worker::wasm_bindgen::JsValue> {
    let quotas = preflight.quotas;
    let mut gate = vec![
        preflight.now.into(),
        js_int(preflight.user_id),
        size.clone(),
        preflight.scope.into(),
        preflight.name.into(),
        preflight.version.into(),
        request.revision.into(),
        request.checksum.into(),
        js_int(i64::from(request.opt_in)),
        request.upload_id.into(),
        preflight.shared_class.into(),
        js_limit(quotas.max_total_bytes_per_user),
        js_limit(quotas.max_versions_per_package_per_day),
        match admit {
            Admit::Resume => upload::RETRY_WINDOW.into(),
            Admit::New | Admit::Revive => js_limit(quotas.max_new_packages_per_day),
        },
        js_limit(quotas.max_packages_total),
        js_limit(quotas.max_upload_bytes_per_day),
        js_limit(quotas.max_uploads_per_day),
        js_limit(quotas.shared_upload_bytes_per_day),
        js_limit(quotas.shared_uploads_per_day),
        js_limit(quotas.max_pending_uploads),
    ];
    if admit == Admit::Revive {
        gate.push(request.metadata_text.into());
    }
    gate
}

/// Charges the governor for the R2 calls [`store`] is about to make:
/// the existence head when `head_first`, and the put. `Some` is the
/// refusal; nothing was stored or charged to the user.
async fn govern_store(env: &Env, head_first: bool) -> worker::Result<Option<Response>> {
    let pools = [
        head_first.then_some(OpPool::BPublish),
        Some(OpPool::APublish),
    ];
    for pool in pools.into_iter().flatten() {
        if let Gate::Refused(refusal) = governor_client::decide(env, &consume_one(pool)).await {
            return governor_refusal_response(refusal.as_ref(), true).map(Some);
        }
    }
    Ok(None)
}

/// Makes sure the operation's archive is in UPLOADS and returns when it
/// was stored. With `head_first` an object already there is kept as it
/// is: writing it again would restart the lifetime the bucket's expiry
/// counts. `now` is sampled before the put, so the recorded time is
/// never later than the object's own and the verification deadline it
/// starts never outlives the object.
async fn store(
    env: &Env,
    upload_id: &str,
    archive: &[u8],
    head_first: bool,
    now: &str,
) -> worker::Result<String> {
    let key = upload::object_key(upload_id);
    let bucket = env.bucket("UPLOADS")?;
    if head_first && let Some(object) = bucket.head(&key).await? {
        return Ok(iso8601_from_ms(object.uploaded().as_millis()));
    }
    bucket.put(&key, archive.to_vec()).execute().await?;
    Ok(now.to_owned())
}

/// Confirms one reservation generation stored ([`sql::CONFIRM_UPLOAD_STORED`]).
/// `false` means that generation no longer exists as a pending row.
async fn confirm(
    db: &D1Database,
    preflight: &Preflight<'_>,
    revision: &str,
    upload_id: &str,
    published_at: &str,
    stored_at: &str,
) -> worker::Result<bool> {
    let result = db
        .prepare(sql::CONFIRM_UPLOAD_STORED)
        .bind(&[
            stored_at.into(),
            preflight.scope.into(),
            preflight.name.into(),
            preflight.version.into(),
            revision.into(),
            upload_id.into(),
            published_at.into(),
        ])?
        .run()
        .await?;
    Ok(changed_rows(result.meta()?) > 0)
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
    Ok(changed_rows(result.meta()?) > 0)
}

/// The quota preflight of one prospective publish: what its refusals
/// read and check, and what the admission batch re-enforces inside its
/// gate.
#[derive(Clone, Copy)]
struct Preflight<'a> {
    user_id: i64,
    /// The shared allowance the token's class is charged to.
    shared_class: &'static str,
    scope: &'a str,
    name: &'a str,
    version: &'a str,
    /// The request's clock sample: the usage date, and the generation
    /// stamp of whatever this request admits.
    now: &'a str,
    day_prefix: &'a str,
    quotas: &'a quota::ClassQuotas,
}

impl Preflight<'_> {
    fn upload_usage(&self, db: &D1Database) -> worker::Result<worker::D1PreparedStatement> {
        db.prepare(sql::UPLOAD_USAGE).bind(&[
            self.now.into(),
            js_int(self.user_id),
            self.shared_class.into(),
            self.scope.into(),
            self.name.into(),
            self.version.into(),
        ])
    }

    /// The `429` for an upload allowance that is already spent, when
    /// the version has no revisions - the one case decidable before the
    /// body is read.
    async fn spent_refusal(&self, db: &D1Database) -> worker::Result<Option<Response>> {
        let record: UploadUsageRecord = self
            .upload_usage(db)?
            .first(None)
            .await?
            .ok_or_else(|| worker::Error::RustError("empty upload usage result".to_owned()))?;
        if record.version_has_revisions != 0 {
            return Ok(None);
        }
        let usage = record.usage();
        match quota::check_daily_uploads(0, &usage, self.quotas)
            .and_then(|()| quota::check_pending_uploads(&usage, self.quotas))
        {
            Ok(()) => Ok(None),
            Err(denial) => record.refusal(&denial, 0).map(Some),
        }
    }

    /// The twin `400`, the first quota `403`, or the first allowance
    /// `429` this publish draws from the current rows, if any. The
    /// deterministic `-`/`_` twin reject gates new packages only, and
    /// answers before the quotas: whether a name can exist does not
    /// depend on the publisher's quota state. `charged` is whether the
    /// operation is charged the daily allowances - every one but a
    /// resumed one, which already was.
    ///
    /// One D1 batch of point lookups and index ranges over one package
    /// or today's rows, never a scan of the account's or the scope's
    /// history (`docs/runbook.md`, "Quota tallies").
    async fn refusal(
        &self,
        env: &Env,
        db: &D1Database,
        archive_bytes: u64,
        charged: bool,
    ) -> worker::Result<Option<Response>> {
        let results = db
            .batch(vec![
                // Rejected revisions are excluded: their bytes were
                // refunded when they were rejected.
                db.prepare(sql::USER_STORED_BYTES)
                    .bind(&[js_int(self.user_id)])?,
                // Both package quotas key on creation (`created_by`), so
                // a version published into someone else's package never
                // counts against the publisher's package quotas.
                db.prepare(sql::USER_PACKAGE_COUNTS).bind(&[
                    js_int(self.user_id),
                    self.day_prefix.into(),
                    self.scope.into(),
                    self.name.into(),
                ])?,
                db.prepare(sql::COUNT_PACKAGE_VERSIONS_ON_DAY).bind(&[
                    self.scope.into(),
                    self.name.into(),
                    self.day_prefix.into(),
                ])?,
                db.prepare(sql::PACKAGE_EXISTS)
                    .bind(&[self.scope.into(), self.name.into()])?,
                db.prepare(sql::TWIN_PACKAGE_EXISTS)
                    .bind(&[self.scope.into(), self.name.into()])?,
                self.upload_usage(db)?,
            ])
            .await?;
        let user_usage: UserUsageRecord = first_row(&results, 0)?;
        let user_packages: PackageCountsRecord = first_row(&results, 1)?;
        let versions_today: CountRecord = first_row(&results, 2)?;
        let package_rows: CountRecord = first_row(&results, 3)?;
        let twin_rows: CountRecord = first_row(&results, 4)?;
        let upload_usage: UploadUsageRecord = first_row(&results, 5)?;
        let counts = quota::PublishCounts {
            user_stored_bytes: non_negative(user_usage.stored_bytes),
            user_package_count: non_negative(user_packages.package_count),
            user_new_packages_today: non_negative(user_packages.new_today),
            package_versions_today: non_negative(versions_today.n),
            package_exists: package_rows.n > 0,
        };
        if !counts.package_exists && twin_rows.n > 0 {
            return error_response(400, publish::NAME_TWIN_CONFLICT).map(Some);
        }
        if let Err(denial) = quota::check_publish(archive_bytes, &counts, self.quotas) {
            return denial_response(env, &denial, None).map(Some);
        }
        let usage = upload_usage.usage();
        let daily = if charged {
            quota::check_daily_uploads(archive_bytes, &usage, self.quotas)
        } else {
            Ok(())
        };
        match daily.and_then(|()| quota::check_pending_uploads(&usage, self.quotas)) {
            Ok(()) => Ok(None),
            Err(denial) => upload_usage.refusal(&denial, archive_bytes).map(Some),
        }
    }
}

/// A class limit as a D1 bind.
fn js_limit(limit: u64) -> worker::wasm_bindgen::JsValue {
    js_int(i64::try_from(limit).unwrap_or(i64::MAX))
}

/// The single row of one statement in a batch result.
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

/// Every row of one statement in a batch result.
fn batch_rows<T: serde::de::DeserializeOwned>(
    results: &[worker::D1Result],
    index: usize,
) -> worker::Result<Vec<T>> {
    results
        .get(index)
        .ok_or_else(|| worker::Error::RustError(format!("missing batch result {index}")))?
        .results::<T>()
}
