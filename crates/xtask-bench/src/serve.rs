//! A loopback static server for the HTTP index scenarios: it serves a
//! file registry the way a static host would, one request at a time.

use std::fs::File;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use anyhow::{Result, anyhow};

pub struct Server {
    listener: Arc<tiny_http::Server>,
    thread: Option<JoinHandle<()>>,
    pub url: String,
}

pub fn serve(root: PathBuf) -> Result<Server> {
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").map_err(|err| anyhow!(err))?);
    let port = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow!("loopback server has no IP address"))?
        .port();
    let worker = Arc::clone(&server);
    let thread = thread::spawn(move || {
        for request in worker.incoming_requests() {
            let file = file_for(&root, request.url())
                .and_then(|path| File::open(path).ok())
                .filter(|file| file.metadata().is_ok_and(|m| m.is_file()));
            let _ = match file {
                Some(file) => request.respond(tiny_http::Response::from_file(file)),
                None => request.respond(tiny_http::Response::empty(404)),
            };
        }
    });
    Ok(Server {
        listener: server,
        thread: Some(thread),
        url: format!("http://127.0.0.1:{port}"),
    })
}

/// The file a request URL names below `root`: plain names only, no
/// `..`, root, or drive prefix, whichever separator the platform reads.
fn file_for(root: &Path, url: &str) -> Option<PathBuf> {
    let path = url.split('?').next()?.trim_start_matches('/');
    let plain = !path.is_empty()
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
    plain.then(|| root.join(path))
}

impl Drop for Server {
    fn drop(&mut self) {
        self.listener.unblock();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_paths_below_the_root_are_served() {
        let root = Path::new("registry");
        assert_eq!(
            file_for(root, "/packages/bench/w000.json?x=1"),
            Some(root.join("packages/bench/w000.json"))
        );
        for hostile in [
            "/../secret",
            "/packages/../../secret",
            "/",
            "/./config.json",
        ] {
            assert_eq!(file_for(root, hostile), None, "{hostile}");
        }
        if cfg!(windows) {
            assert_eq!(file_for(root, "/..\\secret"), None);
            assert_eq!(file_for(root, "/C:/secret"), None);
        }
    }
}
