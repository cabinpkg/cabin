//! The website origin's Bearer mutation and admin planes: the route
//! dispatch and the helpers more than one plane needs. The planes
//! themselves: [`package`] (publish, yank, and the publish write
//! phase), [`tokens`] (the trusted-publishing exchange, the session
//! mint, self-revocation), [`verifier`] (the verifier's work lists and
//! verdict), and [`governor`] (the operator's ledger surface).

mod governor;
mod package;
mod tokens;
mod verifier;

use worker::{Context, Env, Method, Request, Response};

use crate::error;
use crate::routes::{ApiRoute, match_api_route, match_web_route};
use crate::web_glue;
use crate::{quota, sql, trustpub};

use super::{
    GENERATION_HEADER, authenticate, error_response, error_response_with_code, registry_generation,
    unauthorized, web_origin,
};

use governor::{admin_governor_mutation_response, admin_governor_usage_response};
use package::{publish_response, yank_response};
use tokens::{self_revoke_response, session_mint_response, trustpub_exchange_response};
use verifier::{admin_packages_response, admin_versions_response, verdict_authn, verdict_response};

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
    // Admission control and the breaker's write gate run inside each
    // handler: the gate on the exchange's config arm only, and right
    // after admission for the session mint.
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

/// Pre-credential admission for the public surfaces that spend work
/// before any credential is read - the two OIDC endpoints (the
/// exchange PUT and the verdict PATCH) and the session mint PUT: one
/// per-client-IP budget from the `OIDC_LIMITER` ratelimit binding
/// (`wrangler.jsonc`), spent before the body, the JWT, or any JWKS
/// access is touched, so unauthenticated traffic cannot buy
/// verification work - in particular the unknown-kid JWKS refetch
/// (`trustpub::verify`) - or the mint's outbound GitHub call beyond
/// the budget. The refusal is one fixed 429 on every endpoint,
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
