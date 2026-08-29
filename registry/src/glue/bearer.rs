//! The website origin's Bearer mutation and admin planes: publish, yank,
//! token exchange and minting, verdicts, and the publish write phase.

use serde::Deserialize;
use worker::{Context, D1Database, Env, Method, Request, Response, console_error, console_log};

use crate::auth::{self, AuthContext, Scope};
use crate::error;
use crate::governor::{self, Consume, Decision, OpPool, Reserve, StoragePool};
use crate::governor_client::{self, Gate};
use crate::publish;
use crate::routes::{ApiRoute, match_api_route, match_web_route};
use crate::web_glue;
use crate::{allowlist, quota, session_tokens, sql, trustpub, verify};

use super::cron::push_live_set_to_governor;
use super::{
    CountRecord, GENERATION_HEADER, MAX_MUTATION_BODY_BYTES, authenticate, bounded_body,
    bucket_from_columns, changed_rows, commit_object, consume_one, error_response,
    error_response_with_code, governor_refusal_response, has_verify_scope, js_int, json_response,
    json_response_with_status, non_negative, now_epoch_ms, now_iso8601, read_meta,
    registry_generation, unauthorized, web_origin, write_gate,
};

#[derive(Deserialize)]
struct OwnerRecord {
    user_id: i64,
}

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

/// The website origin: the OAuth plane (`/login`, `/callback`, and the
/// claim flow's `/claim/<scope>` and `/callback/claim`), the
/// session-only `/api/v1/user` subtree, and the Bearer mutation plane.
/// The read plane does not exist here - nothing outside those planes
/// matches a data route, so this origin never serves `/config.json`,
/// `/packages/*`, or `/artifacts/*`.
#[allow(clippy::too_many_lines)] // one dispatch ladder, one route per arm
pub(super) async fn handle_website(
    req: &mut Request,
    env: &Env,
    ctx: &Context,
    path: &str,
) -> worker::Result<(Response, Option<String>)> {
    if let Some(web_route) = match_web_route(path) {
        return Ok((web_glue::respond_web(req, env, web_route).await?, None));
    }
    // The whole subtree is session-only: a bearer token never reaches
    // it, and unknown paths under it answer as the session plane rather
    // than falling through to the bearer plane.
    if crate::routes::is_session_path(path) {
        let Some(session_route) = crate::routes::match_session_route(path) else {
            return Ok((error_response(404, error::NOT_FOUND)?, None));
        };
        let response = web_glue::respond_session(req, env, session_route).await?;
        return Ok((response, None));
    }
    // The public stats subtree: the one unauthenticated JSON plane on
    // this origin (`docs/architecture.md`, "Download counts").
    if crate::routes::is_stats_path(path) {
        return Ok((web_glue::respond_stats(req, env, path).await?, None));
    }

    // Everything else is the Bearer plane: deny by default, the uniform
    // 401 before any route matching or data lookup.
    let db = env.d1("DB")?;
    // The auth-exempt mint routes on the plane: each PUT's credential
    // travels in its body (the exchange's OIDC JWT, the session mint's
    // GitHub access token), so both dispatch before the token check.
    // The breaker's write gate runs inside each handler - on the
    // exchange's config arm only, and first of all for the session
    // mint.
    if req.method() == Method::Put
        && (path == crate::routes::TRUSTPUB_TOKENS_PATH
            || path == crate::routes::SESSION_TOKENS_PATH)
    {
        let (mut response, token_id) = if path == crate::routes::TRUSTPUB_TOKENS_PATH {
            trustpub_exchange_response(req, env, &db).await?
        } else {
            session_mint_response(req, env, &db).await?
        };
        if let Some(generation) = registry_generation(&db).await {
            response.headers_mut().set(GENERATION_HEADER, &generation)?;
        }
        return Ok((response, token_id));
    }
    // The other auth-exempt route: the verdict PATCH's credential is a
    // GitHub Actions OIDC JWT in the Authorization header, not a
    // registry token, so it too dispatches before the token check. No
    // token row exists to tie the request log to; the refusal reasons
    // are logged by the authn itself. The generation stamp mirrors the
    // exchange: uniform across the route's answers, refusals included.
    if req.method() == Method::Patch
        && let Some(ApiRoute::AdminVerdict {
            scope,
            name,
            version,
        }) = match_api_route(path)
    {
        let (scope, name, version) = (scope.to_owned(), name.to_owned(), version.to_owned());
        // The same pre-verification admission gate as the exchange,
        // ahead of `verdict_authn`'s JWT work; the refusal takes the
        // generation stamp below like the route's other answers.
        let mut response = if let Some(refusal) = oidc_admission(req, env).await? {
            refusal
        } else if verdict_authn(req, env, &db).await? {
            verdict_response(req, env, ctx, &db, &scope, &name, &version).await?
        } else {
            unauthorized(env)?
        };
        if let Some(generation) = registry_generation(&db).await {
            response.headers_mut().set(GENERATION_HEADER, &generation)?;
        }
        return Ok((response, None));
    }
    let Some(auth) = authenticate(req, &db, ctx).await? else {
        return Ok((unauthorized(env)?, None));
    };

    let mut response = match req.method() {
        // The admin listings and the governor snapshot are the only API
        // routes read with GET; anything else is an authenticated 404.
        Method::Get => match match_api_route(path) {
            Some(ApiRoute::AdminVersions) => admin_versions_response(req, &db, &auth).await?,
            Some(ApiRoute::AdminPackages) => admin_packages_response(&db, &auth).await?,
            Some(ApiRoute::AdminGovernor) => admin_governor_usage_response(env, &auth).await?,
            _ => error_response(404, error::NOT_FOUND)?,
        },
        Method::Put => match match_api_route(path) {
            Some(ApiRoute::Publish {
                scope,
                name,
                version,
            }) => {
                let (scope, name, version) =
                    (scope.to_owned(), name.to_owned(), version.to_owned());
                publish_response(req, env, &db, &auth, &scope, &name, &version).await?
            }
            Some(_) => error_response(405, error::METHOD_NOT_ALLOWED)?,
            None => error_response(404, error::NOT_FOUND)?,
        },
        Method::Post => match match_api_route(path) {
            Some(ApiRoute::AdminGovernor) => {
                admin_governor_mutation_response(req, env, &db, &auth).await?
            }
            Some(_) => error_response(405, error::METHOD_NOT_ALLOWED)?,
            None => error_response(404, error::NOT_FOUND)?,
        },
        Method::Delete => match match_api_route(path) {
            Some(route @ (ApiRoute::TrustPubTokens | ApiRoute::SessionTokens)) => {
                // Early return past the generation stamp below: the
                // kind guard's 401 must stay HEADER-identical to the
                // unauthenticated one (which never gets the stamp), or
                // the debug header becomes a token-validity oracle on
                // this route. The 204 goes unstamped with it.
                let statement = match route {
                    ApiRoute::SessionTokens => db.prepare(sql::DELETE_SESSION_TOKEN),
                    _ => db.prepare(sql::DELETE_TRUSTPUB_TOKEN),
                };
                let response = self_revoke_response(env, statement, &auth).await?;
                return Ok((response, Some(auth.token_id)));
            }
            Some(_) => error_response(405, error::METHOD_NOT_ALLOWED)?,
            None => error_response(404, error::NOT_FOUND)?,
        },
        Method::Patch => match match_api_route(path) {
            Some(ApiRoute::Yank {
                scope,
                name,
                version,
            }) => {
                let (scope, name, version) =
                    (scope.to_owned(), name.to_owned(), version.to_owned());
                yank_response(req, env, &db, &auth, &scope, &name, &version).await?
            }
            // AdminVerdict is unreachable here: every PATCH to it
            // dispatched before `authenticate` above.
            Some(_) => error_response(405, error::METHOD_NOT_ALLOWED)?,
            None => error_response(404, error::NOT_FOUND)?,
        },
        _ => error_response(405, error::METHOD_NOT_ALLOWED)?,
    };

    // The same generation stamp as the read plane (docs/runbook.md).
    if let Some(generation) = registry_generation(&db).await {
        response.headers_mut().set(GENERATION_HEADER, &generation)?;
    }
    Ok((response, Some(auth.token_id)))
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
async fn publish_response(
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
    if let Some(limited) = publish_rate_limit(env, db, auth, &quotas).await? {
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
    // request count. The CAS'd rate limit bounds that per token at the
    // bucket burst (an allowlisted user holding several tokens scales it
    // by their token count); the budget headroom and the breaker absorb
    // the transient. Move the checks into conditional inserts if that
    // ever stops holding.
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
async fn yank_response(
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

/// Pre-verification admission for the two public OIDC surfaces (the
/// exchange PUT and the verdict PATCH): one per-client-IP budget from
/// the `OIDC_LIMITER` ratelimit binding (`wrangler.jsonc`), spent
/// before the body, the JWT, or any JWKS access is touched, so
/// unauthenticated traffic cannot buy verification work - in
/// particular the unknown-kid JWKS refetch (`trustpub::verify`) -
/// beyond the budget. The refusal is one fixed 429 on both endpoints,
/// decided before any credential is read, so it carries no validity
/// signal; its `Retry-After` mirrors the binding's period. A missing
/// binding or a limiter error refuses too: the gate is load-bearing
/// for the JWKS refetch contract, so it fails closed (`cargo
/// check-deploy` requires the binding, so a healthy deploy never
/// takes that path).
async fn oidc_admission(req: &Request, env: &Env) -> worker::Result<Option<Response>> {
    // Cloudflare stamps `CF-Connecting-IP` on every edge request (see
    // `quota::artifact_read_fairness`); a request without it - only
    // reachable off the edge, e.g. `wrangler dev` - shares one bucket.
    let key = req.headers().get("cf-connecting-ip")?.unwrap_or_default();
    let allowed = match env.rate_limiter("OIDC_LIMITER") {
        Ok(limiter) => limiter
            .limit(key)
            .await
            .is_ok_and(|outcome| outcome.success),
        Err(_) => false,
    };
    if allowed {
        return Ok(None);
    }
    denial_response(env, &quota::OIDC_RATE_LIMITED, Some(60)).map(Some)
}

/// The trusted-publishing exchange body, exactly `{"jwt": "<token>"}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExchangeBody {
    jwt: String,
}

/// GitHub Actions OIDC JWTs run ~1.5-2.5 KB today. A dedicated cap
/// rather than [`MAX_MUTATION_BODY_BYTES`]: 4 KB leaves thin headroom
/// against GitHub growing its claim set, and a cap breach here is a
/// ports-publishing outage, not a client bug.
const MAX_EXCHANGE_BODY_BYTES: usize = 16 * 1024;

/// `PUT /api/v1/trusted_publishing/tokens`
/// (`docs/remote-registry.md`, "Trusted publishing"): exchanges a
/// verified GitHub Actions OIDC JWT for a short-lived multi-use
/// `trustpub` token, through one of two arms. [`oidc_admission`] runs
/// first of all - before the body is read - then the fully stateless
/// JWT verification (JWKS via Cache/network, never D1). Claims
/// matching the deployment-pinned verifier identity take the verifier
/// arm: a verify-scoped mint backed by the operator identity
/// `VERIFIER_BACKING_ACCOUNT_ID` names, deliberately in front of the
/// breaker's write gate - like the verdict route and the read plane's
/// verify exemption, the verification pipeline must be able to drain
/// the pending queue whatever the service mode. Everything else is the
/// config arm: the write gate (`503`, like every write - moved behind
/// the verification, so the gate now speaks only to callers presenting
/// a verifiable JWT, as it elsewhere speaks only to authenticated
/// ones), then config match and backing owner. Both arms end in
/// [`exchange_mint`]'s one transaction. Every refusal other than the
/// gate's answers the byte-identical uniform 401 - no
/// config/signature/replay oracle - with the real reason logged for
/// the operator. Returns the minted token row id so the request log
/// ties to the row.
async fn trustpub_exchange_response(
    req: &mut Request,
    env: &Env,
    db: &D1Database,
) -> worker::Result<(Response, Option<String>)> {
    if let Some(refusal) = oidc_admission(req, env).await? {
        return Ok((refusal, None));
    }
    let refused = |reason: String| -> worker::Result<(Response, Option<String>)> {
        console_log!("trustpub exchange refused: {reason}");
        Ok((unauthorized(env)?, None))
    };
    // A missing, oversized, or malformed body is an absent credential,
    // not a 400: the JWT in the body is the credential, and the refusal
    // stays the one uniform envelope.
    let Some(body) = bounded_body(req, MAX_EXCHANGE_BODY_BYTES).await? else {
        return refused("body over the cap".to_owned());
    };
    let Ok(ExchangeBody { jwt }) = serde_json::from_slice(&body) else {
        return refused("malformed body".to_owned());
    };

    // Worker clocks are epoch MILLISECONDS; the verifier takes seconds.
    #[allow(clippy::cast_possible_truncation)]
    let now_secs = (now_epoch_ms() / 1000.0) as i64;
    let claims = match trustpub::verify(
        &jwt,
        &trustpub::GithubJwks::from_env(env),
        trustpub::DEFAULT_AUDIENCE,
        now_secs,
    )
    .await
    {
        Ok(claims) => claims,
        Err(err) => return refused(format!("jwt verification failed: {err:?}")),
    };

    // The verifier arm. Missing or unparsable pins fail the arm closed
    // (never a default identity); the claims then fall through to the
    // config arm like anybody else's. The backing lookup runs before
    // the jti consume, like the config arm's owner lookup: the
    // identity is migration-seeded, so a missing row is a
    // var/seed mismatch, and the refusal must not burn the JWT so
    // the same run can retry once the mismatch is fixed.
    if let Some(pins) = verifier_pins(env)
        && pins.refuses(&claims).is_none()
    {
        let account = env
            .var("VERIFIER_BACKING_ACCOUNT_ID")
            .ok()
            .map(|value| value.to_string())
            .unwrap_or_default();
        if account.is_empty() {
            return refused("the VERIFIER_BACKING_ACCOUNT_ID var is unset".to_owned());
        }
        let backing: Option<OwnerRecord> = db
            .prepare(sql::VERIFIER_BACKING_USER)
            .bind(&[account.as_str().into()])?
            .first(None)
            .await?;
        let Some(backing) = backing else {
            return refused(format!(
                "the verifier backing account {account} has no identity row"
            ));
        };
        return exchange_mint(
            env,
            db,
            &claims,
            now_secs,
            backing.user_id,
            &pins.workflow_filename,
            ExchangeMint::Verify,
        )
        .await;
    }

    if let Some(blocked) = write_gate(env, db).await? {
        return Ok((blocked, None));
    }
    let configs: Vec<trustpub::TrustpubConfig> = db
        .prepare(sql::TRUSTPUB_CONFIGS_BY_REPOSITORY)
        .bind(&[
            js_int(claims.repository_owner_id),
            js_int(claims.repository_id),
        ])?
        .all()
        .await?
        .results()?;
    let config = match trustpub::select_config(&claims, &configs) {
        Ok(config) => config,
        Err(err) => return refused(err.to_string()),
    };

    // Before the jti consume on purpose: an unclaimed scope must not
    // burn the JWT, so claiming it and retrying the same run stays
    // possible while the token is still fresh.
    let owner: Option<OwnerRecord> = db
        .prepare(sql::TRUSTPUB_BACKING_OWNER)
        .bind(&[config.scope.as_str().into()])?
        .first(None)
        .await?;
    let Some(owner) = owner else {
        return refused(format!(
            "scope {} has no owner to back a token",
            config.scope
        ));
    };

    exchange_mint(
        env,
        db,
        &claims,
        now_secs,
        owner.user_id,
        &config.workflow_filename,
        ExchangeMint::Publish {
            scope: &config.scope,
            quota_class: &config.quota_class,
        },
    )
    .await
}

/// Which arm's token [`exchange_mint`] writes: the config arm's
/// scope-confined, quota-classed publish shape, or the verifier arm's
/// bare verify shape.
enum ExchangeMint<'a> {
    Publish {
        scope: &'a str,
        quota_class: &'a str,
    },
    Verify,
}

/// The exchange's terminal transaction, shared by both arms - the
/// spec's steps 6-8: the once-only jti consume, the mint (guarded
/// inside the SQL on the consume's `changes()`, so it must stay the
/// immediately following statement), then the lazy expiry prunes
/// (deliberately no cron). A replayed jti answers as zero consumed
/// rows without minting - the uniform 401; any batch failure rolls the
/// consume back with everything else, so a transient 500 never burns a
/// still-valid JWT.
async fn exchange_mint(
    env: &Env,
    db: &D1Database,
    claims: &trustpub::GithubClaims,
    now_secs: i64,
    user_id: i64,
    workflow_filename: &str,
    mint: ExchangeMint<'_>,
) -> worker::Result<(Response, Option<String>)> {
    let id = auth::hex(&web_glue::random_bytes::<16>()?);
    let token = auth::format_trustpub_token(&web_glue::random_bytes()?);
    let (created_at, expires_at) = token_window(trustpub::TOKEN_TTL_SECS);
    let name = format!("trusted-publishing: {workflow_filename}");
    let minted = match mint {
        ExchangeMint::Publish { scope, quota_class } => {
            db.prepare(sql::INSERT_TRUSTPUB_TOKEN).bind(&[
                id.as_str().into(),
                js_int(user_id),
                name.as_str().into(),
                auth::token_hash(&token).into(),
                created_at.as_str().into(),
                expires_at.as_str().into(),
                scope.into(),
                quota_class.into(),
            ])?
        }
        ExchangeMint::Verify => db.prepare(sql::INSERT_TRUSTPUB_VERIFY_TOKEN).bind(&[
            id.as_str().into(),
            js_int(user_id),
            name.as_str().into(),
            auth::token_hash(&token).into(),
            created_at.as_str().into(),
            expires_at.as_str().into(),
        ])?,
    };
    let results = db
        .batch(vec![
            db.prepare(sql::CONSUME_OIDC_JTI)
                .bind(&[claims.jti.as_str().into(), js_int(claims.verifiable_until)])?,
            minted,
            db.prepare(sql::PRUNE_EXPIRED_OIDC_JTIS)
                .bind(&[js_int(now_secs)])?,
            db.prepare(sql::PRUNE_EXPIRED_SHORT_LIVED_TOKENS)
                .bind(&[created_at.as_str().into()])?,
        ])
        .await?;
    let changed = |index: usize| -> usize {
        results
            .get(index)
            .and_then(|result| result.meta().ok().flatten())
            .and_then(|meta| meta.changes)
            .unwrap_or(0)
    };
    if changed(0) == 0 {
        console_log!("trustpub exchange refused: replayed jti");
        return Ok((unauthorized(env)?, None));
    }
    if changed(1) == 0 {
        // The guard misfired: the jti was consumed but no row was
        // stored, and the plaintext below would be a dead credential.
        console_error!("trustpub exchange consumed a jti without minting");
        return Ok((error_response(500, error::INTERNAL)?, None));
    }

    // The plaintext is rendered exactly once, here; D1 holds only the
    // SHA-256 hex.
    let body = serde_json::json!({ "token": token, "expires_at": expires_at }).to_string();
    Ok((json_response(&body)?, Some(id)))
}

/// A minted token's `(created_at, expires_at)` pair, exactly
/// `ttl_secs` apart from a single clock read.
fn token_window(ttl_secs: i64) -> (String, String) {
    let now_ms = now_epoch_ms();
    let iso = |ms: f64| {
        worker::js_sys::Date::new(&worker::wasm_bindgen::JsValue::from_f64(ms))
            .to_iso_string()
            .as_string()
            .unwrap_or_default()
    };
    #[allow(clippy::cast_precision_loss)] // both TTLs are exact in f64
    let ttl_ms = ttl_secs as f64 * 1000.0;
    (iso(now_ms), iso(now_ms + ttl_ms))
}

/// `DELETE /api/v1/trusted_publishing/tokens` and
/// `DELETE /api/v1/sessions/tokens`: the presented (already
/// authenticated) token revokes itself, iff it is the route's kind -
/// any other kind's id changes no rows and answers the same uniform
/// 401 as any invalid credential, so neither endpoint is a token-kind
/// oracle. Deletion also makes a repeat DELETE indistinguishable from
/// an unknown token (the 401 again), which is the documented
/// idempotent answer. Deliberately not behind the write gate, like the
/// verdict route: revocation removes a live credential, and blocking
/// it while over budget would keep that credential alive - the wrong
/// fail direction for a security operation.
async fn self_revoke_response(
    env: &Env,
    statement: worker::D1PreparedStatement,
    auth: &AuthContext,
) -> worker::Result<Response> {
    let deleted = statement
        .bind(&[auth.token_id.as_str().into()])?
        .run()
        .await?;
    if deleted.meta()?.and_then(|meta| meta.changes).unwrap_or(0) == 0 {
        return unauthorized(env);
    }
    Ok(Response::empty()?.with_status(204))
}

/// The production [`session_tokens::GithubUserProvider`]: one
/// check-token call through [`web_glue::github_check_token`] (which
/// carries the User-Agent GitHub requires and honors the
/// `GITHUB_API_BASE` override), so a 200 also proves the token was
/// issued by the registry's own OAuth app - another app's grant for
/// the same account is a refusal, not a login. Every failure shape is
/// a reason string for the log - never the access token itself - and
/// the caller's one uniform 401.
struct GithubUserApi<'a> {
    env: &'a Env,
}

impl session_tokens::GithubUserProvider for GithubUserApi<'_> {
    async fn user_id(&self, github_token: &str) -> Result<i64, String> {
        let body = match web_glue::github_check_token(self.env, github_token).await {
            Ok(Some(body)) => body,
            Ok(None) => return Err("github check-token refused the token".to_owned()),
            Err(err) => return Err(format!("github check-token fetch failed: {err}")),
        };
        session_tokens::parse_check_token_user_id(&body)
            .ok_or_else(|| "github check-token body did not parse".to_owned())
    }
}

/// `PUT /api/v1/sessions/tokens` (`docs/remote-registry.md`, "Login
/// sessions"): trades a GitHub access token
/// for a 12-hour `session` bearer token. The write gate answers first,
/// before the credential is even read: the mint is a write, and a
/// doomed request must not spend an outbound GitHub call - the
/// pre-auth 503 deliberately tells an anonymous caller the breaker
/// state, like the read plane's public over-budget answer. Then the
/// GitHub check-token proof, the allowlist, and the identity lookup - the
/// exact resolution the web OAuth login uses, minus its account
/// creation: `cabin login` is for accounts that exist, so an unknown
/// or unallowlisted id is a refusal, never a signup. Every refusal
/// past the gate answers the byte-identical uniform 401 with the real
/// reason logged. Returns the minted row id so the request log ties to
/// the row.
async fn session_mint_response(
    req: &mut Request,
    env: &Env,
    db: &D1Database,
) -> worker::Result<(Response, Option<String>)> {
    if let Some(blocked) = write_gate(env, db).await? {
        return Ok((blocked, None));
    }
    let refused = |reason: String| -> worker::Result<(Response, Option<String>)> {
        console_log!("session mint refused: {reason}");
        Ok((unauthorized(env)?, None))
    };
    // A missing, oversized, or malformed body is an absent credential,
    // not a 400, like the trustpub exchange's.
    let Some(body) = bounded_body(req, MAX_MUTATION_BODY_BYTES).await? else {
        return refused("body over the cap".to_owned());
    };
    let github_id = match session_tokens::resolve_github_id(&body, &GithubUserApi { env }).await {
        Ok(github_id) => github_id,
        Err(reason) => return refused(reason),
    };
    let allowed = allowlist::parse_allowed_ids(&env.var("ALLOWED_GITHUB_IDS")?.to_string());
    if !allowed.contains(&github_id) {
        return refused(format!("github id {github_id} is not allowlisted"));
    }
    let Some(user) = web_glue::user_record(db, github_id).await? else {
        return refused(format!("github id {github_id} has never signed in"));
    };

    let id = auth::hex(&web_glue::random_bytes::<16>()?);
    let token = auth::format_session_token(&web_glue::random_bytes()?);
    let (created_at, expires_at) = token_window(session_tokens::TOKEN_TTL_SECS);
    db.batch(vec![
        db.prepare(sql::INSERT_SESSION_TOKEN).bind(&[
            id.as_str().into(),
            js_int(user.user_id),
            auth::token_hash(&token).into(),
            created_at.as_str().into(),
            expires_at.as_str().into(),
        ])?,
        db.prepare(sql::PRUNE_EXPIRED_SHORT_LIVED_TOKENS)
            .bind(&[created_at.as_str().into()])?,
    ])
    .await?;

    // The plaintext is rendered exactly once, here; D1 holds only the
    // SHA-256 hex.
    let body = serde_json::json!({ "token": token, "expires_at": expires_at }).to_string();
    Ok((json_response(&body)?, Some(id)))
}

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
async fn admin_versions_response(
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
async fn admin_packages_response(db: &D1Database, auth: &AuthContext) -> worker::Result<Response> {
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

/// The pinned verifier identity from the four `VERIFIER_*` wrangler
/// vars; `None` - any var missing or unparsable - refuses every
/// verdict and disables the exchange's verifier arm (fail closed,
/// never a default).
fn verifier_pins(env: &Env) -> Option<trustpub::VerifierPins> {
    let var = |name: &str| env.var(name).ok().map(|value| value.to_string());
    trustpub::VerifierPins::parse(
        &var("VERIFIER_REPOSITORY_OWNER_ID")?,
        &var("VERIFIER_REPOSITORY_ID")?,
        &var("VERIFIER_WORKFLOW_FILENAME")?,
        &var("VERIFIER_GIT_REF")?,
    )
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
async fn verdict_authn(req: &Request, env: &Env, db: &D1Database) -> worker::Result<bool> {
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
async fn verdict_response(
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

/// `GET /api/v1/admin/governor` (`verify` scope): the governor
/// ledger's usage snapshot, for the operator (`docs/runbook.md`, "The
/// cost governor"). Admin infrastructure like the verifier listings:
/// no scope membership, not budget-gated - inspecting the ledger must
/// work in every service mode.
async fn admin_governor_usage_response(env: &Env, auth: &AuthContext) -> worker::Result<Response> {
    if !has_verify_scope(auth) {
        return error_response(403, error::VERIFY_SCOPE_REQUIRED);
    }
    match governor_client::usage(env).await {
        Some(snapshot) => json_response(
            &serde_json::to_string(&snapshot)
                .map_err(|err| worker::Error::RustError(err.to_string()))?,
        ),
        None => error_response(503, error::GOVERNOR_UNAVAILABLE),
    }
}

/// The admin governor mutation body: exactly one of an evidence-backed
/// release, the pre-launch ledger wipe, or an on-demand reconcile.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminGovernorBody {
    #[serde(default)]
    release: Option<governor::Release>,
    #[serde(default)]
    wipe: Option<bool>,
    #[serde(default)]
    reconcile: Option<bool>,
}

/// `POST /api/v1/admin/governor` (`verify` scope): the three explicit
/// operator actions on the ledger. `release` frees one object's entry
/// and must only follow evidence the object is gone - the endpoint
/// cannot check R2 for the operator, and a release for a live object
/// would make the ledger understate reality (`docs/runbook.md`, "The
/// cost governor"). `wipe` clears the primary storage rows and the
/// daily fairness windows (backup and dump rows survive - their
/// objects are never wiped - and the monthly op windows survive too:
/// they mirror already-metered R2 operations and cannot be rebuilt)
/// and is the registry wipe's companion, guarded on `meta.launched`
/// exactly like
/// `cargo registry-wipe`: only an affirmatively read `'false'` proceeds.
/// `reconcile` runs the cron pass's increase-only primary rebuild on
/// demand and answers with the report - the recovery path after a
/// ledger wipe or a Durable Object storage loss, when waiting up to
/// 15 minutes for the cron would leave admission running against an
/// empty ledger (`docs/runbook.md`, "Known ceilings").
async fn admin_governor_mutation_response(
    req: &mut Request,
    env: &Env,
    db: &D1Database,
    auth: &AuthContext,
) -> worker::Result<Response> {
    if !has_verify_scope(auth) {
        return error_response(403, error::VERIFY_SCOPE_REQUIRED);
    }
    let Some(body) = bounded_body(req, MAX_MUTATION_BODY_BYTES).await? else {
        return error_response(400, error::INVALID_GOVERNOR_BODY);
    };
    let parsed: AdminGovernorBody = match serde_json::from_slice(&body) {
        Ok(parsed) => parsed,
        Err(_) => return error_response(400, error::INVALID_GOVERNOR_BODY),
    };
    match (parsed.release, parsed.wipe, parsed.reconcile) {
        (Some(release), None, None) => {
            let decision = Decision {
                release: vec![release],
                ..Decision::default()
            };
            match governor_client::decide(env, &decision).await {
                Gate::Allowed => json_response(r#"{"ok":true}"#),
                Gate::Refused(_) => error_response(503, error::GOVERNOR_UNAVAILABLE),
            }
        }
        (None, Some(true), None) => {
            if read_meta(db, "launched").await?.as_deref() != Some("false") {
                return error_response(403, error::GOVERNOR_LEDGER_LAUNCHED);
            }
            if governor_client::wipe(env).await {
                json_response(r#"{"ok":true}"#)
            } else {
                error_response(503, error::GOVERNOR_UNAVAILABLE)
            }
        }
        (None, None, Some(true)) => match push_live_set_to_governor(env, db).await? {
            Some(report) => json_response(
                &serde_json::to_string(&report)
                    .map_err(|err| worker::Error::RustError(err.to_string()))?,
            ),
            None => error_response(503, error::GOVERNOR_UNAVAILABLE),
        },
        _ => error_response(400, error::INVALID_GOVERNOR_BODY),
    }
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
/// the row flips, mirroring [`apply_rejection`]: the counter regains
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
/// take was computed from, so concurrent requests on one token cannot
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
    // loses a race to a parallel publisher on the same token.
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
        if cas_bucket(db, &auth.token_id, bucket, outcome.bucket).await? {
            return Ok(None);
        }
        bucket = read_bucket(db, &auth.token_id).await?;
    }
    // Losing a burst's worth of races in a row means the token is being
    // spent concurrently right now; refusing the attempt is the limiter
    // working. The bucket refills within a minute, hence the short
    // Retry-After.
    denial_response(env, &quota::RATE_LIMITED, Some(1)).map(Some)
}

#[derive(Deserialize)]
struct BucketRecord {
    rl_tokens: Option<f64>,
    rl_updated_at: Option<String>,
}

/// The current bucket state straight from the token row.
async fn read_bucket(db: &D1Database, token_id: &str) -> worker::Result<Option<quota::Bucket>> {
    let record: Option<BucketRecord> = db
        .prepare(sql::TOKEN_BUCKET)
        .bind(&[token_id.into()])?
        .first(None)
        .await?;
    Ok(record
        .and_then(|record| bucket_from_columns(record.rl_tokens, record.rl_updated_at.as_deref())))
}

/// Persists a bucket take iff the row still holds `prev` (`IS` makes the
/// comparison NULL-safe for a token that has never published). `false`
/// means a concurrent request won the race. Round-trip exactness holds:
/// the stored text and REAL came from these same f64 values.
async fn cas_bucket(
    db: &D1Database,
    token_id: &str,
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
        .prepare(sql::CAS_TOKEN_BUCKET)
        .bind(&[
            next.tokens.into(),
            next.updated_at_ms.to_string().into(),
            token_id.into(),
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

/// Renders a quota or rate-limit [`quota::Denial`]; the quota family's
/// detail embeds the dashboard URL built from `WEB_ORIGIN`
/// ([`quota::detail_with_usage_url`]).
fn denial_response(
    env: &Env,
    denial: &quota::Denial,
    retry_after_secs: Option<u64>,
) -> worker::Result<Response> {
    let detail = quota::detail_with_usage_url(denial, &web_origin(env)?);
    error_response_with_code(denial.status, &detail, denial.code, retry_after_secs)
}
