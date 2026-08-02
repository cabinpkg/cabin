use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Every fake port pins a never-fetched `https://` URL.  Publisher
/// conversion refuses a non-HTTPS pin, and the cache-first archive
/// lookup makes the unreachable host a no-op, so staging needs no
/// server and no network - see `seed_archive_into_cache`.
fn archive_url(archive_name: &str) -> String {
    format!("https://ports.invalid/{archive_name}")
}

/// Test fixture builder for local Cabin ports backed by loopback
/// archives.  It keeps test bodies focused on port topology while the
/// tarball, checksum, manifest, and HTTP plumbing stay in one place.
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
            patches: Vec::new(),
            manifest_body: None,
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
    sha256: String,
}

impl FakeArchive {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

impl FakePort {
    /// Seed the archive bytes into `cache_dir`'s content-addressed
    /// `ports` slot.  The manifest already pins the matching
    /// unreachable `https://` URL, so the cache-first lookup resolves
    /// without a server or the network.
    pub fn seed_archive_into_cache(&self, cache_dir: &Path) {
        let slot = cache_dir
            .join("ports")
            .join("archives")
            .join("sha256")
            .join(format!("{}.tar.gz", self.archive.sha256));
        fs::create_dir_all(slot.parent().expect("cache slot parent"))
            .expect("create fake port cache slot");
        fs::copy(&self.archive.path, &slot).expect("seed fake port archive into cache");
    }
}

pub struct FakePortBuilder {
    root: PathBuf,
    name: String,
    version: String,
    archive_prefix: Option<String>,
    files: Vec<(String, String)>,
    copies: Vec<(String, String)>,
    patches: Vec<(String, String)>,
    manifest_body: Option<String>,
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

    /// Declare a `[package.upstream].patches` entry and write the
    /// patch file under the port directory's `patches/` subdirectory.
    pub fn patch(mut self, file_name: &str, contents: &str) -> Self {
        self.patches
            .push((file_name.to_owned(), contents.to_owned()));
        self
    }

    pub fn manifest_body(mut self, manifest: &str) -> Self {
        self.manifest_body = Some(manifest.to_owned());
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
        let manifest_path = port_dir.join("cabin.toml");
        fs::write(
            &manifest_path,
            self.package_manifest(&sha256, &archive_name),
        )
        .expect("write fake port manifest");
        for (file_name, contents) in &self.patches {
            let patch_path = port_dir.join("patches").join(file_name);
            fs::create_dir_all(patch_path.parent().expect("patch parent"))
                .expect("create fake port patches dir");
            fs::write(&patch_path, contents).expect("write fake port patch");
        }
        FakePort {
            name: self.name,
            version: self.version,
            port_dir,
            archive: FakeArchive {
                name: archive_name,
                path: archive_path,
                sha256,
            },
        }
    }

    /// The committed manifest of a provenance-bearing package: the
    /// fixture's own `[package]` / `[target.*]` body with a
    /// `[package.upstream]` block stamped on top, exactly the shape a
    /// port directory carries in `crates/cabin-port/ports/`.
    fn package_manifest(&self, sha256: &str, archive_name: &str) -> String {
        let strip_prefix = self
            .archive_prefix
            .as_deref()
            .unwrap_or_else(|| panic!("fake port `{}` missing archive_prefix", self.name));
        let patches_key = if self.patches.is_empty() {
            String::new()
        } else {
            let entries: Vec<String> = self
                .patches
                .iter()
                .map(|(file_name, _)| format!("\"patches/{}\"", toml_escape(file_name)))
                .collect();
            format!("patches = [{}]\n", entries.join(", "))
        };
        let mut upstream = format!(
            "[package.upstream]\nurl = \"{}\"\nsha256 = \"{sha256}\"\nformat = \"tar.gz\"\nstrip-prefix = \"{strip_prefix}\"\n{patches_key}",
            archive_url(archive_name)
        );
        for (from, to) in &self.copies {
            write!(
                upstream,
                "\n[[package.upstream.copy]]\nfrom = \"{}\"\nto = \"{}\"\n",
                toml_escape(from),
                toml_escape(to)
            )
            .expect("append fake port copy section");
        }
        format!("{upstream}\n{}", self.render_manifest_body())
    }

    fn render_manifest_body(&self) -> String {
        self.manifest_body
            .as_ref()
            .unwrap_or_else(|| panic!("fake port `{}` missing manifest_body", self.name))
            .clone()
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

fn declared_sources(manifest: &str, target_name: &str) -> Vec<String> {
    let parsed = cabin_manifest::parse_manifest_str(manifest).expect("parse fake port manifest");
    let package = parsed.package.expect("fake port manifest package");
    let target = package
        .targets
        .iter()
        .find(|target| target.name.as_str() == target_name)
        .unwrap_or_else(|| panic!("fake port manifest missing target `{target_name}`"));
    target
        .sources
        .iter()
        .map(std::string::ToString::to_string)
        .collect()
}

fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
