//! The governor Durable Object: the serialized authority behind
//! `crate::governor`'s accounting engine. One named singleton
//! ([`GOVERNOR_SINGLETON`]) backed by `SQLite` storage; the engine runs
//! its statements synchronously, so requests serialize on the object
//! and no decision interleaves with another.
//!
//! Protocol (internal, Worker-to-object only):
//! `POST /decide` with a [`governor::Decision`], `GET /usage`,
//! `POST /reconcile` with a [`governor::ReconcileRequest`]. Every
//! handled response is 200 JSON; anything else means the governor is
//! unavailable and callers fail closed (`crate::governor_client`).

use worker::{
    DurableObject, Env, Method, Request, Response, SqlStorage, SqlStorageValue, State,
    console_error, durable_object,
};

use crate::governor::{self, Store, Value};

/// The one instance's name; the client and the object agree on it.
pub const GOVERNOR_SINGLETON: &str = "governor";

/// [`governor::Store`] over the Durable Object's synchronous `SQLite`
/// API. Changed-row counts come from `SELECT changes()` right after
/// each statement - reliable `SQLite` semantics, unlike the cursor's
/// billing-oriented `rows_written`.
struct SqlStore(SqlStorage);

/// The adapter's one own statement, named so the SQL-consolidation
/// guard can pin the adapter to pass-through parameters and consts.
const CHANGED_ROWS: &str = "SELECT changes() AS n";

fn bindings(params: &[Value]) -> Vec<SqlStorageValue> {
    params
        .iter()
        .map(|value| match value {
            Value::Text(text) => SqlStorageValue::String(text.clone()),
            Value::Int(int) => SqlStorageValue::Integer(*int),
        })
        .collect()
}

#[derive(serde::Deserialize)]
struct ChangesRow {
    n: i64,
}

impl Store for SqlStore {
    fn exec(&mut self, sql: &str, params: &[Value]) -> Result<usize, String> {
        self.0
            .exec(sql, Some(bindings(params)))
            .map_err(|err| err.to_string())?;
        let changes = self
            .0
            .exec(CHANGED_ROWS, None)
            .map_err(|err| err.to_string())?
            .one::<ChangesRow>()
            .map_err(|err| err.to_string())?;
        Ok(usize::try_from(changes.n).unwrap_or(0))
    }

    fn rows(&mut self, sql: &str, params: &[Value]) -> Result<Vec<serde_json::Value>, String> {
        self.0
            .exec(sql, Some(bindings(params)))
            .map_err(|err| err.to_string())?
            .to_array::<serde_json::Value>()
            .map_err(|err| err.to_string())
    }
}

#[durable_object]
pub struct Governor {
    state: State,
    env: Env,
}

impl DurableObject for Governor {
    fn new(state: State, env: Env) -> Self {
        // Idempotent schema on every (re)initialization: a fresh or
        // reset object recreates its tables before serving anything.
        let sql = state.storage().sql();
        for statement in governor::SCHEMA {
            if let Err(err) = sql.exec(statement, None) {
                console_error!("governor schema statement failed: {err}");
            }
        }
        Self { state, env }
    }

    async fn fetch(&self, mut req: Request) -> worker::Result<Response> {
        let mut store = SqlStore(self.state.storage().sql());
        let limits = self.limits();
        let now = crate::glue::now_iso8601();
        let result = match (req.method(), req.path().as_str()) {
            (Method::Post, "/decide") => match req.json::<governor::Decision>().await {
                Ok(decision) => governor::decide(&mut store, &limits, &now, &decision)
                    .map(|outcome| serde_json::to_string(&outcome).unwrap_or_default()),
                Err(err) => return Response::error(format!("bad decision body: {err}"), 400),
            },
            (Method::Get, "/usage") => governor::usage(&mut store)
                .map(|snapshot| serde_json::to_string(&snapshot).unwrap_or_default()),
            (Method::Post, "/reconcile") => match req.json::<governor::ReconcileRequest>().await {
                Ok(request) => governor::reconcile(&mut store, &now, &request)
                    .map(|report| serde_json::to_string(&report).unwrap_or_default()),
                Err(err) => return Response::error(format!("bad reconcile body: {err}"), 400),
            },
            _ => return Response::error("unknown governor route", 404),
        };
        match result {
            Ok(body) => {
                let mut response = Response::ok(body)?;
                response
                    .headers_mut()
                    .set("content-type", "application/json")?;
                Ok(response)
            }
            Err(err) => {
                console_error!("governor storage failure: {err}");
                Response::error("governor storage failure", 500)
            }
        }
    }
}

impl Governor {
    fn limits(&self) -> governor::Limits {
        governor::Limits::from_lookup(
            |name| self.env.var(name).ok().map(|var| var.to_string()),
            |name| console_error!("{name} is set but not a number; failing closed to a zero limit"),
        )
    }
}
