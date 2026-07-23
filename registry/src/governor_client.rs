//! Worker-side client for the governor Durable Object
//! (`src/governor_do.rs`). Thin and fail-closed: any transport error,
//! non-200, or unparsable answer reads as "governor unavailable", and
//! callers must refuse to initiate new billable R2 work on it
//! (`docs/architecture.md`, "The cost governor").

use worker::{Env, Headers, Method, Request, RequestInit, Response, console_error};

use crate::governor::{
    Decision, Outcome, ReconcileReport, ReconcileRequest, Refusal, UsageSnapshot,
};
use crate::governor_do::GOVERNOR_SINGLETON;

/// The synthetic authority for stub requests; Durable Object fetches
/// route on the path only.
const BASE: &str = "https://governor.internal";

/// A gate answer for request paths: allowed, or a refusal to render.
/// `Refused(None)` is the fail-closed arm - the governor could not
/// answer - and must block any new billable R2 call.
pub(crate) enum Gate {
    Allowed,
    Refused(Option<Refusal>),
}

async fn call(env: &Env, path: &str, body: Option<String>) -> Result<Response, worker::Error> {
    let stub = env
        .durable_object("GOVERNOR")?
        .get_by_name(GOVERNOR_SINGLETON)?;
    let mut init = RequestInit::new();
    init.with_method(if body.is_some() {
        Method::Post
    } else {
        Method::Get
    });
    if let Some(body) = body {
        let headers = Headers::new();
        headers.set("content-type", "application/json")?;
        init.with_headers(headers).with_body(Some(body.into()));
    }
    stub.fetch_with_request(Request::new_with_init(&format!("{BASE}{path}"), &init)?)
        .await
}

async fn call_json<T: serde::de::DeserializeOwned>(
    env: &Env,
    path: &str,
    body: Option<String>,
) -> Option<T> {
    match call(env, path, body).await {
        Ok(mut response) if response.status_code() == 200 => match response.json::<T>().await {
            Ok(parsed) => Some(parsed),
            Err(err) => {
                console_error!("governor {path} answer did not parse: {err}");
                None
            }
        },
        Ok(response) => {
            console_error!("governor {path} answered {}", response.status_code());
            None
        }
        Err(err) => {
            console_error!("governor {path} is unreachable: {err}");
            None
        }
    }
}

/// One atomic decision. Serialization failures and transport failures
/// both land in the fail-closed `Refused(None)` arm.
pub(crate) async fn decide(env: &Env, decision: &Decision) -> Gate {
    let Ok(body) = serde_json::to_string(decision) else {
        return Gate::Refused(None);
    };
    match call_json::<Outcome>(env, "/decide", Some(body)).await {
        Some(Outcome { ok: true, .. }) => Gate::Allowed,
        Some(Outcome { refusal, .. }) => Gate::Refused(refusal),
        None => Gate::Refused(None),
    }
}

/// A best-effort settle (commits and releases record reality and never
/// refuse): a failure only logs, leaving conservative reserved state
/// for reconciliation to settle - it must never fail the response of a
/// write that already happened.
pub(crate) async fn settle(env: &Env, decision: &Decision) {
    if let Gate::Refused(_) = decide(env, decision).await {
        console_error!("governor settle failed; reconciliation will settle the reservation");
    }
}

pub(crate) async fn usage(env: &Env) -> Option<UsageSnapshot> {
    call_json(env, "/usage", None).await
}

/// The pre-launch ledger wipe; the admin route owns the launch guard.
/// `false` means the governor did not confirm.
pub(crate) async fn wipe(env: &Env) -> bool {
    #[derive(serde::Deserialize)]
    struct Confirmation {
        ok: bool,
    }
    call_json::<Confirmation>(env, "/wipe", Some(String::new()))
        .await
        .is_some_and(|answer| answer.ok)
}

pub(crate) async fn reconcile(env: &Env, request: &ReconcileRequest) -> Option<ReconcileReport> {
    let body = serde_json::to_string(request).ok()?;
    call_json(env, "/reconcile", Some(body)).await
}
