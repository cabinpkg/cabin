//! The operator's governor surface: the usage snapshot
//! (`docs/runbook.md`, "The cost governor").

use worker::{Env, Response};

use crate::auth::AuthContext;
use crate::error;
use crate::glue::{error_response, has_verify_scope, json_response};
use crate::governor_client;

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
