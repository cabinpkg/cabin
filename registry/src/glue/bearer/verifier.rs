//! The verifier's surfaces (`verify` scope): the admin work lists, the
//! verdict endpoint's OIDC authentication and transaction, and the
//! rejected blob's reclaim. The lifecycle rules live in `crate::verify`.

use serde::Deserialize;
use worker::{Context, D1Database, Env, Request, Response, console_error, console_log};

use crate::auth::{self, AuthContext};
use crate::error;
use crate::glue::{
    CountRecord, MAX_MUTATION_BODY_BYTES, bounded_body, changed_rows, error_response,
    has_verify_scope, js_int, json_response, json_response_with_status, now_epoch_ms, now_iso8601,
};
use crate::{sql, trustpub, verify};

use super::verifier_pins;

#[derive(Deserialize)]
struct AdminVersionRecord {
    scope: String,
    name: String,
    version: String,
    revision: String,
    checksum: String,
    published_by: i64,
    published_at: String,
    metadata_json: String,
}

/// `GET /api/v1/admin/versions?status=<status>` (`verify` scope): the
/// verifier's work list. Each entry's `name` is the canonical
/// `<scope>/<name>` and carries the stored canonical metadata document
/// (parsed, so the response is one JSON value); the listing is
/// deterministic: ordered by scope, then name, then version.
pub(super) async fn admin_versions_response(
    req: &Request,
    db: &D1Database,
    auth: &AuthContext,
) -> worker::Result<Response> {
    if !has_verify_scope(auth) {
        return error_response(403, error::VERIFY_SCOPE_REQUIRED);
    }
    let url = req.url()?;
    let status = url
        .query_pairs()
        .find(|(key, _)| key == "status")
        .map(|(_, value)| value.into_owned());
    let Some(status) = status.as_deref().and_then(verify::Status::parse) else {
        return error_response(400, error::INVALID_STATUS_QUERY);
    };
    let records: Vec<AdminVersionRecord> = db
        .prepare(sql::REVISIONS_BY_VERIFICATION_STATUS)
        .bind(&[status.as_str().into()])?
        .all()
        .await?
        .results()?;
    let mut versions = Vec::with_capacity(records.len());
    for record in records {
        let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&record.metadata_json) else {
            console_error!(
                "stored metadata for {}/{}@{} is not valid JSON",
                record.scope,
                record.name,
                record.version
            );
            return error_response(500, error::INTERNAL);
        };
        versions.push(serde_json::json!({
            "name": format!("{}/{}", record.scope, record.name),
            "version": record.version,
            "revision": record.revision,
            "checksum": record.checksum,
            "published_by": record.published_by,
            "published_at": record.published_at,
            "metadata": metadata,
        }));
    }
    json_response(&serde_json::json!({ "versions": versions }).to_string())
}

#[derive(Deserialize)]
struct AdminPackageRecord {
    scope: String,
    name: String,
    vetted: i64,
}

/// `GET /api/v1/admin/packages` (`verify` scope): the corpus for the
/// verifier's name advisories (`docs/architecture.md`, "Name
/// fidelity"). Admin infrastructure like the versions listing: no
/// scope membership, and deliberately not budget-gated - the
/// verification pipeline must be able to drain the pending queue
/// whatever the service mode.
pub(super) async fn admin_packages_response(
    db: &D1Database,
    auth: &AuthContext,
) -> worker::Result<Response> {
    if !has_verify_scope(auth) {
        return error_response(403, error::VERIFY_SCOPE_REQUIRED);
    }
    let records: Vec<AdminPackageRecord> =
        db.prepare(sql::ADMIN_PACKAGES).all().await?.results()?;
    let packages: Vec<verify::CorpusPackage> = records
        .into_iter()
        .map(|record| verify::CorpusPackage {
            scope: record.scope,
            name: record.name,
            vetted: record.vetted != 0,
        })
        .collect();
    json_response(&verify::packages_json(&packages))
}

#[derive(Deserialize)]
struct VerdictTargetRecord {
    verification: String,
    checksum: String,
    published_at: String,
    archive_size: i64,
}

/// The listing binding: the row must still be the generation the
/// verifier listed, both its bytes and its publish event (a
/// byte-identical revival changes `published_at` but not the checksum,
/// which is why `parse_verdict` requires both for both verdicts).
/// Both mismatches are loud: a checksum mismatch means the verifier
/// judged different archive bytes than the row stores (the row was
/// found by the checksum's leading prefix, so the tails diverge) - the
/// "verifier saw a different artifact" alarm - and a `published_at`
/// mismatch means the verdict targets a superseded generation.
fn verdict_binding_matches(
    parsed: &verify::ParsedVerdict,
    target: &VerdictTargetRecord,
    scope: &str,
    name: &str,
    version: &str,
    revision: &str,
) -> bool {
    if parsed.checksum != target.checksum {
        console_error!(
            "verifier saw a different artifact for {scope}/{name}@{version}#{revision}: \
             verdict checksum {}, stored {}",
            parsed.checksum,
            target.checksum
        );
        return false;
    }
    if parsed.published_at != target.published_at {
        console_error!(
            "verdict for {scope}/{name}@{version}#{revision} names a superseded generation: \
             verdict published_at {}, stored {}",
            parsed.published_at,
            target.published_at
        );
        return false;
    }
    true
}

/// The verdict endpoint's authentication: the credential is a GitHub
/// Actions OIDC JWT presented as the bearer, minted by the one workflow
/// the `VERIFIER_*` vars pin ([`trustpub::VERIFIER_AUDIENCE`]). The
/// step order is deliberate, like the exchange's: the fully stateless
/// JWT verification first (JWKS via Cache/network, never D1), then the
/// pins (still no D1), then one transaction consuming the jti - its
/// zero-changed-rows answer is the replay refusal - with the lazy
/// expiry prune alongside. Consuming the jti before the business logic
/// is safe here, unlike the exchange's mint coupling: nothing hinges on
/// this request succeeding afterwards, and the workflow retries a
/// failed verdict with a freshly minted token. `false` means refused;
/// the caller answers the uniform 401 while the real reason is logged
/// here - no pin/signature/replay oracle.
pub(super) async fn verdict_authn(
    req: &Request,
    env: &Env,
    db: &D1Database,
) -> worker::Result<bool> {
    let refused = |reason: &str| -> worker::Result<bool> {
        console_log!("verdict refused: {reason}");
        Ok(false)
    };
    let Some(header) = req.headers().get("authorization")? else {
        return refused("no authorization header");
    };
    let Some(jwt) = auth::bearer_token(&header) else {
        return refused("the authorization header is not a bearer credential");
    };
    // Worker clocks are epoch MILLISECONDS; the verifier takes seconds.
    #[allow(clippy::cast_possible_truncation)]
    let now_secs = (now_epoch_ms() / 1000.0) as i64;
    let claims = match trustpub::verify(
        jwt,
        &trustpub::GithubJwks::from_env(env),
        trustpub::VERIFIER_AUDIENCE,
        now_secs,
    )
    .await
    {
        Ok(claims) => claims,
        Err(err) => return refused(&format!("jwt verification failed: {err:?}")),
    };
    let Some(pins) = verifier_pins(env) else {
        return refused("the VERIFIER_* pins are unset or unparsable");
    };
    if let Some(pin) = pins.refuses(&claims) {
        return refused(&format!("the claims fail the {pin} pin"));
    }
    let results = db
        .batch(vec![
            db.prepare(sql::CONSUME_OIDC_JTI)
                .bind(&[claims.jti.as_str().into(), js_int(claims.verifiable_until)])?,
            db.prepare(sql::PRUNE_EXPIRED_OIDC_JTIS)
                .bind(&[js_int(now_secs)])?,
        ])
        .await?;
    let consumed = results
        .first()
        .and_then(|result| result.meta().ok().flatten())
        .and_then(|meta| meta.changes)
        .unwrap_or(0);
    if consumed == 0 {
        return refused("replayed jti");
    }
    Ok(true)
}

/// `PATCH /api/v1/admin/versions/<scope>/<name>/<version>`
/// ([`verdict_authn`]): the verifier's verdict. Pending versions accept either verdict; a
/// repeat of the verdict a terminal version already carries is the
/// idempotent 200 (a repeat rejection also re-drives the blob
/// reclaim); the conflicting combinations are the 409 matrix in
/// [`verify::transition`]. The body's required `checksum` binds the
/// verdict to the bytes the verifier actually inspected (and so to
/// the revision those bytes name), and the
/// applying updates are themselves guarded on the row still being
/// pending with the bytes this request read - a verdict racing a
/// conflicting verdict or a replacement answers 409 instead of landing
/// on content it never saw. A rejection records the reason, refunds
/// the archive's bytes from the storage self-accounting when the row
/// was the blob's sole live reference (decided inside the same
/// transaction that flips the row, so a duplicate concurrent verdict
/// cannot refund twice), and then reclaims the blob itself.
/// Deliberately **not** gated by the budget breaker, unlike publish
/// and yank: a verdict stores no new bytes (a rejection frees them),
/// so blocking it would only stall the pending queue - verification
/// must be able to drain it whatever the service mode
/// (`docs/architecture.md`, "Billing model: the governor and the breaker").
/// The response reports the resulting state plus whether this request
/// changed it.
pub(super) async fn verdict_response(
    req: &mut Request,
    env: &Env,
    ctx: &Context,
    db: &D1Database,
    scope: &str,
    name: &str,
    version: &str,
) -> worker::Result<Response> {
    let Some(body) = bounded_body(req, MAX_MUTATION_BODY_BYTES).await? else {
        return error_response(400, error::INVALID_VERDICT_BODY);
    };
    let parsed = match verify::parse_verdict(&body) {
        Ok(parsed) => parsed,
        Err(detail) => return error_response(400, detail),
    };

    // The body's checksum names the revision the verdict targets (its
    // id is the digest's leading hex prefix); `parse_verdict` requires
    // it for both verdicts, so the lookup is never ambiguous when a
    // pending respin sits beside the version's other revisions.
    if !crate::checksum::is_canonical(&parsed.checksum) {
        return error_response(400, verify::INVALID_VERDICT_CHECKSUM);
    }
    let revision = crate::checksum::revision_id(&parsed.checksum).to_owned();
    let target: Option<VerdictTargetRecord> = db
        .prepare(sql::VERDICT_TARGET)
        .bind(&[
            scope.into(),
            name.into(),
            version.into(),
            revision.as_str().into(),
        ])?
        .first(None)
        .await?;
    let Some(target) = target else {
        return error_response(404, error::NOT_FOUND);
    };
    let Some(current) = verify::Status::parse(&target.verification) else {
        console_error!(
            "stored verification for {scope}/{name}@{version}#{revision} is invalid: {}",
            target.verification
        );
        return error_response(500, error::INTERNAL);
    };
    if !verdict_binding_matches(&parsed, &target, scope, name, version, &revision) {
        return error_response(409, error::VERDICT_TARGET_CHANGED);
    }

    let changed = match verify::transition(current, parsed.verdict) {
        verify::Transition::Conflict(detail) => {
            console_error!(
                "conflicting terminal verdict for {scope}/{name}@{version}#{revision}: \
                 the row is {}, refused: {detail}",
                current.as_str()
            );
            return error_response(409, detail);
        }
        verify::Transition::NoOp => {
            // A repeat rejection re-drives the blob reclaim: the first
            // rejection's row flip and refund commit in one D1
            // transaction, but the R2 delete runs after it and can fail
            // into the 500 the verifier is now retrying - reporting
            // success here without retrying the delete would orphan the
            // blob forever. Idempotent when the first attempt already
            // reclaimed it.
            if parsed.verdict == verify::Verdict::Rejected {
                delete_blob_if_unreferenced(env, db, &target.checksum).await?;
            }
            false
        }
        verify::Transition::Apply => {
            if !apply_verdict(env, db, scope, name, version, &revision, &parsed, &target).await? {
                // The row moved between this request's read and its
                // guarded update: a concurrent conflicting verdict or a
                // replacement won the race.
                console_error!(
                    "verdict for {scope}/{name}@{version}#{revision} lost the race: \
                     the row moved between the read and the guarded update"
                );
                return error_response(409, error::VERDICT_TARGET_CHANGED);
            }
            // The fast replication path: drain the just-enqueued
            // backup work off the response path. The queue row is
            // durable, so a lost kick only defers to the next breaker
            // cron pass.
            if parsed.verdict == verify::Verdict::Verified {
                let env = env.clone();
                ctx.wait_until(async move { crate::backup_glue::drain_backup_queue(&env).await });
            }
            true
        }
    };
    let resulting = match parsed.verdict {
        verify::Verdict::Verified => verify::Status::Verified,
        verify::Verdict::Rejected => verify::Status::Rejected,
    };
    json_response_with_status(
        200,
        &serde_json::json!({
            "ok": true,
            "name": format!("{scope}/{name}"),
            "version": version,
            "revision": revision,
            "verification": resulting.as_str(),
            "changed": changed,
        })
        .to_string(),
    )
}

/// Applies a verdict to a pending row under the transactional guards
/// (still pending, still the checksum and `published_at` this request
/// read); `false` means the row moved first and nothing was changed.
#[allow(clippy::too_many_arguments)] // the revision quad plus the verdict plumbing
async fn apply_verdict(
    env: &Env,
    db: &D1Database,
    scope: &str,
    name: &str,
    version: &str,
    revision: &str,
    parsed: &verify::ParsedVerdict,
    target: &VerdictTargetRecord,
) -> worker::Result<bool> {
    match parsed.verdict {
        verify::Verdict::Verified => {
            let now = now_iso8601();
            // One batch: the verified transition and its backup-queue
            // row commit together, so a crash right after can never
            // lose the replication work - the enqueue's guards repeat
            // the mark's, so the row appears exactly when the
            // transition applied (`sql::ENQUEUE_VERIFIED_BACKUP`).
            let results = db
                .batch(vec![
                    db.prepare(sql::MARK_REVISION_VERIFIED).bind(&[
                        now.as_str().into(),
                        scope.into(),
                        name.into(),
                        version.into(),
                        target.checksum.as_str().into(),
                        target.published_at.as_str().into(),
                        revision.into(),
                    ])?,
                    db.prepare(sql::ENQUEUE_VERIFIED_BACKUP).bind(&[
                        scope.into(),
                        name.into(),
                        version.into(),
                        target.checksum.as_str().into(),
                        target.published_at.as_str().into(),
                        now.as_str().into(),
                        revision.into(),
                    ])?,
                ])
                .await?;
            let mark = results
                .first()
                .ok_or_else(|| worker::Error::RustError("missing batch result 0".to_owned()))?;
            Ok(changed_rows(mark.meta()?) > 0)
        }
        verify::Verdict::Rejected => {
            let applied = apply_rejection(
                db,
                scope,
                name,
                version,
                revision,
                parsed.reason.as_deref().unwrap_or_default(),
                target,
            )
            .await?;
            if applied {
                delete_blob_if_unreferenced(env, db, &target.checksum).await?;
            }
            Ok(applied)
        }
    }
}

/// The rejection transaction: the storage refund is decided **before**
/// the row flips (statement order inside one atomic batch), so it fires
/// exactly when this row - still pending, still storing the bytes the
/// verdict was read against - is the checksum's sole live reference: a
/// concurrent duplicate rejection sees the row already rejected and
/// refunds nothing, a replacement that swapped the bytes disarms both
/// statements, and a shared blob (another live row with the same bytes)
/// is never refunded. The row-flip carries the same guards; `false`
/// means it lost such a race and nothing changed. `MAX(..., 0)` keeps
/// the counter integer-parseable even under drift; the breaker treats a
/// non-numeric value as unavailable and fails closed.
#[allow(clippy::too_many_arguments)] // the revision quad plus the verdict plumbing
async fn apply_rejection(
    db: &D1Database,
    scope: &str,
    name: &str,
    version: &str,
    revision: &str,
    reason: &str,
    target: &VerdictTargetRecord,
) -> worker::Result<bool> {
    let archive_size = js_int(target.archive_size);
    let results = db
        .batch(vec![
            db.prepare(sql::REFUND_STORED_BYTES_ON_REJECTION).bind(&[
                target.checksum.as_str().into(),
                scope.into(),
                name.into(),
                version.into(),
                archive_size,
                target.published_at.as_str().into(),
                revision.into(),
            ])?,
            db.prepare(sql::MARK_REVISION_REJECTED).bind(&[
                reason.into(),
                scope.into(),
                name.into(),
                version.into(),
                target.checksum.as_str().into(),
                target.published_at.as_str().into(),
                revision.into(),
            ])?,
        ])
        .await?;
    let row_flip = results
        .get(1)
        .ok_or_else(|| worker::Error::RustError("missing batch result 1".to_owned()))?;
    Ok(changed_rows(row_flip.meta()?) > 0)
}

/// Deletes `checksum`'s blob from the primary bucket when no live
/// (non-rejected) version row references it any more. Best-effort and
/// idempotent: a failed or crashed delete leaves an orphaned blob (the
/// same harmless, content-addressed garbage a crashed publish leaves -
/// see `docs/runbook.md`), and later reclaim paths retry it. Never
/// touches the meta counter - the caller accounts for the bytes when it
/// flips the row states - and never touches BACKUP, which is
/// append-only by design.
///
/// ponytail: the refcount read and the delete are not atomic with
/// publishes' R2 writes (two stores, no shared transaction). Publishers
/// close the practical window by re-checking their blob after their D1
/// batch commits and re-uploading if this delete beat them to it; a
/// delete that lands even later is loud (the artifact route's
/// missing-blob 500) and recoverable from the append-only BACKUP
/// replica. A reclaim queue with a grace period is the upgrade if
/// reclaim/publish races ever become a real operational pattern.
async fn delete_blob_if_unreferenced(
    env: &Env,
    db: &D1Database,
    checksum: &str,
) -> worker::Result<()> {
    let references: CountRecord = db
        .prepare(sql::COUNT_LIVE_BLOB_REFERENCES)
        .bind(&[checksum.into()])?
        .first(None)
        .await?
        .ok_or_else(|| worker::Error::RustError("empty COUNT(*) result".to_owned()))?;
    if references.n > 0 {
        return Ok(());
    }
    let key = format!("blobs/sha256/{}", crate::checksum::hex(checksum));
    // No live reference also means nothing needs a backup copy any
    // more: retire the queue row first (before the delete, so it can
    // never linger past the primary object), or a blob whose copy
    // never landed would keep the drain retrying against a deleted
    // primary object forever. The retire re-checks liveness inside
    // the statement - a verdict landing between this request's
    // refcount read and here enqueues transactionally, and its work
    // must not be lost to this stale reader.
    db.prepare(sql::RETIRE_DEAD_BACKUP_PENDING)
        .bind(&[key.as_str().into(), checksum.into()])?
        .run()
        .await?;
    // R2 deletes are not billable, so no consumption rides them. The
    // ledger entry deliberately stays committed: a successful delete is
    // NOT proof the key stays gone - a concurrent same-checksum publish
    // can recreate the content-addressed object at any moment, and a
    // release here could strand that publish's bytes outside the ledger
    // if it crashed before its own settle. Reconciliation reports the
    // entry as unreferenced, and releasing it is the operator's
    // explicit, evidence-backed action (`docs/runbook.md`, "The cost
    // governor"). The dump pool differs: its keys are cron-unique and
    // never concurrently recreated, so the dump jobs release their own.
    if let Err(err) = env.bucket("BLOBS")?.delete(&key).await {
        console_error!("reclaiming blob {key} failed (left as an orphan): {err}");
    }
    Ok(())
}
