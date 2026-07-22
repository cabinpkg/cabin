use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

const ARCHIVE_URL_PLACEHOLDER: &str = "http://127.0.0.1:1/__fake-port-archive.tar.gz";

/// Test fixture builder for local Cabin ports backed by loopback
/// archives.  It keeps test bodies focused on port topology while the
/// tarball, checksum, `port.toml`, and HTTP plumbing stay in one place.
pub struct FakePortRepo {
    root: PathBuf,
}

impl FakePortRepo {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    pub fn port(&self, name: &str, version: &str) -> FakePortBuilder {
        FakePortBuilder {
            root: self.root.clone(),
            name: name.to_owned(),
            version: version.to_owned(),
            archive_prefix: None,
            files: Vec::new(),
            copies: Vec::new(),
            overlay_manifest: None,
            path_deps: Vec::new(),
        }
    }
}

pub struct FakePort {
    pub name: String,
    pub version: String,
    pub port_dir: PathBuf,
    pub archive: FakeArchive,
}

#[derive(Clone)]
pub struct FakeArchive {
    name: String,
    path: PathBuf,
    port_toml: PathBuf,
}

impl FakeArchive {
    pub fn name(&self) -> &str {
        &self.name
    }
}

pub struct FakePortBuilder {
    root: PathBuf,
    name: String,
    version: String,
    archive_prefix: Option<String>,
    files: Vec<(String, String)>,
    copies: Vec<(String, String)>,
    overlay_manifest: Option<String>,
    path_deps: Vec<(String, PathBuf)>,
}

impl FakePortBuilder {
    pub fn archive_prefix(mut self, prefix: &str) -> Self {
        self.archive_prefix = Some(prefix.to_owned());
        self
    }

    pub fn file(mut self, path: &str, contents: &str) -> Self {
        self.files.push((path.to_owned(), contents.to_owned()));
        self
    }

    pub fn stub_declared_sources_except(
        mut self,
        manifest: &str,
        target: &str,
        real_sources: &[&str],
    ) -> Self {
        let real_sources = real_sources.iter().copied().collect::<BTreeSet<_>>();
        self.files.extend(
            declared_sources(manifest, target)
                .into_iter()
                .filter(|source| !real_sources.contains(source.as_str()))
                .map(|source| (source, String::new())),
        );
        self
    }

    pub fn copy(mut self, from: &str, to: &str) -> Self {
        self.copies.push((from.to_owned(), to.to_owned()));
        self
    }

    pub fn depends_on_builtin_or_path_port(mut self, name: &str, port: &FakePort) -> Self {
        self.path_deps
            .push((name.to_owned(), port.port_dir.clone()));
        self
    }

    pub fn overlay_manifest(mut self, manifest: &str) -> Self {
        self.overlay_manifest = Some(manifest.to_owned());
        self
    }

    pub fn build(self) -> FakePort {
        let archive_prefix = self
            .archive_prefix
            .clone()
            .unwrap_or_else(|| format!("{}-{}", self.name, self.version));
        let archive_name = format!("{archive_prefix}.tar.gz");
        let archive_dir = self.root.join("downloads");
        let archive_path = archive_dir.join(&archive_name);
        let sha256 = write_archive(&archive_path, &archive_prefix, &self.files);
        let port_dir = self.root.join("ports").join(&self.name).join(&self.version);
        fs::create_dir_all(&port_dir).expect("fake port dir");
        let port_toml = port_dir.join("port.toml");
        fs::write(&port_toml, self.port_toml(&sha256)).expect("write fake port.toml");
        fs::write(port_dir.join("cabin.toml"), self.render_overlay_manifest())
            .expect("write fake port overlay");
        FakePort {
            name: self.name,
            version: self.version,
            port_dir,
            archive: FakeArchive {
                name: archive_name,
                path: archive_path,
                port_toml,
            },
        }
    }

    fn port_toml(&self, sha256: &str) -> String {
        let strip_prefix = self
            .archive_prefix
            .as_deref()
            .unwrap_or_else(|| panic!("fake port `{}` missing archive_prefix", self.name));
        let mut toml = format!(
            "[port]\nname = \"{}\"\nversion = \"{}\"\n\n[source]\ntype = \"archive\"\nurl = \"{}\"\nsha256 = \"{}\"\nstrip_prefix = \"{}\"\n\n[overlay]\nmanifest = \"cabin.toml\"\n",
            self.name, self.version, ARCHIVE_URL_PLACEHOLDER, sha256, strip_prefix
        );
        for (from, to) in &self.copies {
            write!(
                toml,
                "\n[[copy]]\nfrom = \"{}\"\nto = \"{}\"\n",
                toml_escape(from),
                toml_escape(to)
            )
            .expect("append fake port copy section");
        }
        toml
    }

    fn render_overlay_manifest(&self) -> String {
        let mut manifest = self
            .overlay_manifest
            .as_ref()
            .unwrap_or_else(|| panic!("fake port `{}` missing overlay_manifest", self.name))
            .clone();
        for (name, port_dir) in &self.path_deps {
            let token = path_dep_token(name);
            let replacement = toml_escape(&port_dir.to_string_lossy());
            if manifest.contains(&token) {
                manifest = manifest.replace(&token, &replacement);
            } else {
                manifest = rewrite_builtin_port_dep_to_path(&manifest, name, &replacement);
            }
        }
        manifest
    }
}

pub struct FakeArchiveServer {
    archives: Vec<FakeArchive>,
    running: Option<RunningArchiveServer>,
}

impl FakeArchiveServer {
    pub fn new() -> Self {
        Self {
            archives: Vec::new(),
            running: None,
        }
    }

    pub fn serve(mut self, archive: &FakeArchive) -> Self {
        assert!(
            self.running.is_none(),
            "fake archive server cannot register archives after start"
        );
        self.archives.push(archive.clone());
        self
    }

    pub fn start(mut self) -> Self {
        assert!(
            self.running.is_none(),
            "fake archive server cannot start twice"
        );
        let server =
            Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"));
        let addr = server.server_addr().to_ip().expect("loopback addr");
        let base_url = format!("http://{addr}");
        let archives = self
            .archives
            .drain(..)
            .map(|archive| {
                let url = format!("{base_url}/{}", archive.name);
                patch_port_toml_url(&archive.port_toml, &url);
                (
                    archive.name,
                    fs::read(&archive.path).expect("read fake archive"),
                )
            })
            .collect::<Vec<_>>();
        let request_counts = Arc::new(Mutex::new(BTreeMap::new()));
        let counts_for_thread = Arc::clone(&request_counts);
        let server_for_thread = Arc::clone(&server);
        let archives = Arc::new(archives);
        let archives_for_thread = Arc::clone(&archives);
        let thread = std::thread::spawn(move || {
            while let Ok(req) = server_for_thread.recv() {
                let requested = req.url().trim_start_matches('/').to_owned();
                if let Some((name, body)) = archives_for_thread
                    .iter()
                    .find(|(name, _)| name.as_str() == requested)
                {
                    *counts_for_thread
                        .lock()
                        .expect("request counts lock")
                        .entry(name.clone())
                        .or_default() += 1;
                    let _ = req.respond(tiny_http::Response::from_data(body.clone()));
                } else {
                    let _ = req.respond(tiny_http::Response::empty(404));
                }
            }
        });
        self.running = Some(RunningArchiveServer {
            server,
            thread: Some(thread),
            request_counts,
        });
        self
    }

    pub fn requests_for(&self, archive_name: &str) -> usize {
        self.running()
            .request_counts
            .lock()
            .expect("request counts lock")
            .get(archive_name)
            .copied()
            .unwrap_or(0)
    }

    pub fn total_requests(&self) -> usize {
        self.running()
            .request_counts
            .lock()
            .expect("request counts lock")
            .values()
            .sum()
    }

    fn running(&self) -> &RunningArchiveServer {
        self.running
            .as_ref()
            .expect("fake archive server must be started before inspection")
    }
}

struct RunningArchiveServer {
    server: Arc<tiny_http::Server>,
    thread: Option<JoinHandle<()>>,
    request_counts: Arc<Mutex<BTreeMap<String, usize>>>,
}

impl Drop for RunningArchiveServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

fn write_archive(path: &Path, prefix: &str, files: &[(String, String)]) -> String {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("archive parent dir");
    }
    let file = fs::File::create(path).expect("create fake archive");
    let enc = GzEncoder::new(file, Compression::default());
    let mut builder = tar::Builder::new(enc);
    for (rel, body) in files {
        let bytes = body.as_bytes();
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                format!("{prefix}/{rel}"),
                &mut std::io::Cursor::new(bytes),
            )
            .expect("append fake archive entry");
    }
    let enc = builder.into_inner().expect("finalize fake tar");
    enc.finish()
        .expect("finalize fake gzip")
        .flush()
        .expect("flush fake gzip");
    let bytes = fs::read(path).expect("hash fake archive");
    let mut h = Sha256::new();
    h.update(&bytes);
    cabin_core::hash::hex_digest(&h.finalize())
}

fn patch_port_toml_url(path: &Path, url: &str) {
    let body = fs::read_to_string(path).expect("read fake port.toml");
    fs::write(
        path,
        body.replace(ARCHIVE_URL_PLACEHOLDER, &toml_escape(url)),
    )
    .expect("patch fake port URL");
}

fn path_dep_token(name: &str) -> String {
    let upper = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("__FAKE_PORT_{upper}_PATH__")
}

fn rewrite_builtin_port_dep_to_path(manifest: &str, name: &str, replacement: &str) -> String {
    let mut rewritten = String::with_capacity(manifest.len());
    let mut replaced = false;
    for line in manifest.lines() {
        let trimmed = line.trim_start();
        let indent_len = line.len() - trimmed.len();
        if !replaced
            && trimmed.starts_with(&format!("{name} = {{"))
            && trimmed.contains("port = true")
        {
            rewritten.push_str(&line[..indent_len]);
            writeln!(rewritten, "{name} = {{ port-path = \"{replacement}\" }}")
                .expect("rewrite fake port dependency");
            replaced = true;
        } else {
            rewritten.push_str(line);
            rewritten.push('\n');
        }
    }
    assert!(
        replaced,
        "fake port overlay did not contain builtin `{name}` dependency to rewrite"
    );
    rewritten
}

fn declared_sources(manifest: &str, target_name: &str) -> Vec<String> {
    let parsed = cabin_manifest::parse_manifest_str(manifest).expect("parse fake port overlay");
    let package = parsed.package.expect("fake port overlay package");
    let target = package
        .targets
        .iter()
        .find(|target| target.name.as_str() == target_name)
        .unwrap_or_else(|| panic!("fake port overlay missing target `{target_name}`"));
    target
        .sources
        .iter()
        .map(std::string::ToString::to_string)
        .collect()
}

fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
