//! The verifier's surfaces (`verify` scope): the admin work lists and
//! the verdict endpoint's OIDC authentication and transaction. The
//! lifecycle rules live in `crate::verify`.

use serde::Deserialize;
use worker::{D1Database, Env, Request, Response, console_error, console_log};

use crate::auth::{self, AuthContext};
use crate::error;
use crate::glue::publication::place_archives;
use crate::glue::{
    MAX_MUTATION_BODY_BYTES, bounded_body, changed_rows, error_response, has_verify_scope, js_int,
    json_response, json_response_with_status, now_epoch_ms, now_iso8601,
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
/// deterministic: ordered by scope, then name, then version, then
/// revision.
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
/// verifier's name advisories (`docs/remote-registry.md`, "The
/// verifier's checks"). Admin infrastructure like the versions listing:
/// no scope membership, and deliberately not budget-gated - the
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
/// is safe here, unlike the exchange's mint coupling, because nothing
/// hinges on this request succeeding afterwards: a request that fails
/// before its verdict commits leaves the row pending, and the next run
/// lists it again and `PATCH`es with a freshly minted token (one that
/// fails after the commit changed the row as intended). `false` means
/// refused; the caller answers
/// the uniform 401 while the real reason is logged here - no
/// pin/signature/replay oracle.
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
    // Worker clocks are epoch milliseconds; the verifier takes seconds.
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
    // The verifier acts for its backing account, so it needs the same
    // private-alpha admission as that account's tokens.
    let account = env
        .var("VERIFIER_BACKING_ACCOUNT_ID")
        .map(|value| value.to_string())
        .unwrap_or_default();
    if !crate::allowlist::admits(&crate::glue::allowed_ids(env)?, Some(&account)) {
        return refused("the verifier backing account is not admitted");
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
/// idempotent 200; the conflicting combinations are the 409 matrix in
/// [`verify::transition`]. The body's required `checksum` binds the
/// verdict to the bytes the verifier actually inspected (and so to
/// the revision those bytes name), and the
/// applying updates are themselves guarded on the row still being
/// pending under that same binding - a verdict racing a conflicting
/// verdict or a replacement answers 409 instead of landing on content
/// it never saw. A rejection records the reason and refunds the
/// archive's bytes to its publisher in the same transaction that flips
/// the row, so a duplicate concurrent verdict cannot refund twice. The
/// archive itself is left to the UPLOADS bucket's expiry.
/// Deliberately **not** gated by the budget breaker, unlike publish
/// and yank: a verdict admits no new bytes (placement copies ones
/// already charged, a rejection frees them), so blocking it would only
/// stall the pending queue - verification must be able to drain it
/// whatever the service mode.
/// The response reports the state the verdict left the row in plus
/// whether this request changed it. A verdict that verified a revision
/// runs an archive placement pass before it answers, so the revision is
/// normally served by then; the response is the verdict's either way.
pub(super) async fn verdict_response(
    req: &mut Request,
    env: &Env,
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
        verify::Transition::NoOp => false,
        verify::Transition::Apply => {
            if !apply_verdict(db, scope, name, version, &revision, &parsed).await? {
                // The row moved between this request's read and its
                // guarded update: a concurrent conflicting verdict, a
                // replacement, or the verification timeout won the race.
                console_error!(
                    "verdict for {scope}/{name}@{version}#{revision} lost the race: \
                     the row moved between the read and the guarded update"
                );
                return error_response(409, error::VERDICT_TARGET_CHANGED);
            }
            true
        }
    };
    let resulting = match parsed.verdict {
        verify::Verdict::Verified => verify::Status::Verified,
        verify::Verdict::Rejected => verify::Status::Rejected,
    };
    if changed && resulting == verify::Status::Verified {
        place_archives(env, db).await;
    }
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
/// (still pending, still the checksum and `published_at` the verifier
/// listed - the body's binding, which `verdict_binding_matches` found
/// equal to the row this request read, so D1 enforces the listing
/// binding as well as the read-to-update race); `false` means the row
/// moved first and nothing was changed. A rejection's refund follows
/// the row flip in one batch and applies only when the flip did
/// (`sql::REFUND_USER_STORED_BYTES`).
async fn apply_verdict(
    db: &D1Database,
    scope: &str,
    name: &str,
    version: &str,
    revision: &str,
    parsed: &verify::ParsedVerdict,
) -> worker::Result<bool> {
    let now = now_iso8601();
    let statements = match parsed.verdict {
        verify::Verdict::Verified => vec![db.prepare(sql::MARK_REVISION_VERIFIED).bind(&[
            now.as_str().into(),
            scope.into(),
            name.into(),
            version.into(),
            parsed.checksum.as_str().into(),
            parsed.published_at.as_str().into(),
            revision.into(),
        ])?],
        verify::Verdict::Rejected => vec![
            db.prepare(sql::MARK_REVISION_REJECTED).bind(&[
                parsed.reason.as_deref().unwrap_or_default().into(),
                scope.into(),
                name.into(),
                version.into(),
                parsed.checksum.as_str().into(),
                parsed.published_at.as_str().into(),
                revision.into(),
                now.as_str().into(),
            ])?,
            db.prepare(sql::REFUND_USER_STORED_BYTES).bind(&[
                scope.into(),
                name.into(),
                version.into(),
                revision.into(),
            ])?,
        ],
    };
    let results = db.batch(statements).await?;
    let row_flip = results
        .first()
        .ok_or_else(|| worker::Error::RustError("missing batch result 0".to_owned()))?;
    Ok(changed_rows(row_flip.meta()?) > 0)
}
