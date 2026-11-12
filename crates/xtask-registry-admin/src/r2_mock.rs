//! A local stand-in for the HTTP service the wipe drives: Cloudflare's
//! R2 REST object API (list and delete).  Requests reach it through the
//! real [`Buckets`] client, so key encoding and URL handling are
//! exercised as they run.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::r2::Buckets;

/// The account every [`Mock::buckets`] client names.
const ACCOUNT: &str = "acct";

/// What the mock holds and how it misbehaves.
#[derive(Default)]
pub(crate) struct State {
    /// `(bucket, key)`, so a listing comes back in key order as R2's does.
    pub(crate) objects: BTreeSet<(String, String)>,
    /// The most keys one listing page carries, whatever `per_page` asks;
    /// zero means no cap.
    pub(crate) page_cap: usize,
    /// An answer every listing gets instead of the bucket's contents.
    pub(crate) list_answer: Option<(u16, String)>,
    /// A key whose delete answers 403.
    pub(crate) undeletable: Option<String>,
    /// A key whose delete answers success and leaves it in place, as a
    /// writer putting it straight back would.
    pub(crate) sticky: Option<String>,
    /// Every request, as `METHOD url`.
    pub(crate) log: Vec<String>,
}

impl State {
    pub(crate) fn holding(objects: &[(&str, &str)]) -> Self {
        Self {
            objects: objects
                .iter()
                .map(|(bucket, key)| ((*bucket).to_owned(), (*key).to_owned()))
                .collect(),
            ..Self::default()
        }
    }
}

pub(crate) struct Mock {
    url: String,
    state: Arc<Mutex<State>>,
}

impl Mock {
    pub(crate) fn start(state: State) -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind the mock");
        let url = format!(
            "http://{}",
            server.server_addr().to_ip().expect("an IP listener")
        );
        let state = Arc::new(Mutex::new(state));
        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let (status, body) = answer(
                    &mut shared.lock().expect("the mock state"),
                    request.method(),
                    request.url(),
                );
                let _ = request
                    .respond(tiny_http::Response::from_string(body).with_status_code(status));
            }
        });
        Self { url, state }
    }

    pub(crate) fn buckets(&self) -> Buckets {
        Buckets::new(&self.url, ACCOUNT, "token".to_owned())
    }

    pub(crate) fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("the mock state")
    }

    /// What is left, as `(bucket, key)` pairs in key order.
    pub(crate) fn remaining(&self) -> Vec<(String, String)> {
        self.state().objects.iter().cloned().collect()
    }
}

fn answer(state: &mut State, method: &tiny_http::Method, url: &str) -> (u16, String) {
    state.log.push(format!("{method} {url}"));
    let Some(rest) = url.strip_prefix(&format!("/accounts/{ACCOUNT}/r2/buckets/")) else {
        return (404, "no such route".to_owned());
    };
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let Some((bucket, key)) = path.split_once("/objects") else {
        return (404, "no such route".to_owned());
    };
    let bucket = bucket.to_owned();
    match (method, key.strip_prefix('/')) {
        (tiny_http::Method::Delete, Some(key)) => {
            let key = decode(key);
            if state.undeletable.as_deref() == Some(key.as_str()) {
                return (
                    403,
                    r#"{"success":false,"errors":[{"code":10000}]}"#.to_owned(),
                );
            }
            // R2 answers success whether or not the key existed.
            if state.sticky.as_deref() != Some(key.as_str()) {
                state.objects.remove(&(bucket, key));
            }
            (200, r#"{"success":true,"result":{}}"#.to_owned())
        }
        (tiny_http::Method::Get, None) => {
            if let Some(answer) = state.list_answer.clone() {
                return answer;
            }
            let mut per_page: usize = query
                .strip_prefix("per_page=")
                .expect("a listing names only its page size")
                .parse()
                .expect("per_page");
            if state.page_cap != 0 {
                per_page = per_page.min(state.page_cap);
            }
            let matching: Vec<&String> = state
                .objects
                .iter()
                .filter(|(held, _)| *held == bucket)
                .map(|(_, key)| key)
                .collect();
            let page: Vec<serde_json::Value> = matching
                .iter()
                .take(per_page)
                .map(|key| serde_json::json!({ "key": key, "size": 1 }))
                .collect();
            let mut body = serde_json::json!({ "success": true, "errors": [], "result": page });
            // Like the API: only a truncated page carries `result_info`.
            if matching.len() > per_page {
                body["result_info"] = serde_json::json!({ "is_truncated": true, "cursor": "c" });
            }
            (200, body.to_string())
        }
        _ => (405, "unexpected method".to_owned()),
    }
}

/// Percent-decoding, as R2 reads a key out of the request path.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(byte) = text
                .get(index + 1..index + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            decoded.push(byte);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).expect("a UTF-8 key")
}
