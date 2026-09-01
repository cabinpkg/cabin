//! The operator's governor ledger surface: the usage snapshot and the
//! three explicit ledger actions (`docs/runbook.md`, "The cost
//! governor").

use serde::Deserialize;
use worker::{D1Database, Env, Request, Response};

use crate::auth::AuthContext;
use crate::error;
use crate::glue::cron::push_live_set_to_governor;
use crate::glue::{
    MAX_MUTATION_BODY_BYTES, bounded_body, error_response, has_verify_scope, json_response,
    read_meta,
};
use crate::governor::{self, Decision};
use crate::governor_client::{self, Gate};

/// `GET /api/v1/admin/governor` (`verify` scope): the governor
/// ledger's usage snapshot, for the operator (`docs/runbook.md`, "The
/// cost governor"). Admin infrastructure like the verifier listings:
/// no scope membership, not budget-gated - inspecting the ledger must
/// work in every service mode.
pub(super) async fn admin_governor_usage_response(
    env: &Env,
    auth: &AuthContext,
) -> worker::Result<Response> {
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
pub(super) async fn admin_governor_mutation_response(
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
