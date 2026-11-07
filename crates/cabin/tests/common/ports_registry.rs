//! Registry-fixture staging for integration tests that consume
//! `cabin-ports/*` packages.
//!
//! Fixtures are generated through the foundation-port publisher's
//! own staging step (`cabin_publish::upstream`): materialization
//! from the pinned archive, then the file-registry publish.  Tests
//! therefore consume byte-identical packages to what the publisher
//! stages, instead of hand-written index metadata that could drift
//! from the real conversion rules.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use cabin_publish::upstream::{ArchiveFetch, stage_committed_package};

/// Writer for a small executable package consuming `cabin-ports/*`
/// registry dependencies, shared by the registry-ports test modules.
pub struct RegistryConsumer<'a> {
    pub name: &'a str,
    /// Lines inside `[dependencies]`, e.g.
    /// `"cabin-ports/zlib" = "=1.3.1"`.
    pub dependencies: &'a str,
    /// Entries of the executable target's `deps` array.
    pub target_deps: &'a [&'a str],
    /// `c-standard = "c11"` or `cxx-standard = "c++17"`.
    pub standard: &'a str,
    /// `main.c` or `main.cc` (decides the compiled language).
    pub source_name: &'a str,
    pub source: &'a str,
}

impl RegistryConsumer<'_> {
    pub fn write(&self, root: &Path) -> PathBuf {
        let dir = root.join(self.name);
        fs::create_dir_all(dir.join("src")).expect("consumer src dir");
        fs::write(dir.join("src").join(self.source_name), self.source)
            .expect("write consumer source");
        let deps = self
            .target_deps
            .iter()
            .map(|dep| format!("\"{dep}\""))
            .collect::<Vec<_>>()
            .join(", ");
        fs::write(
            dir.join("cabin.toml"),
            format!(
                r#"[package]
name = "{}"
version = "0.1.0"
{}

[dependencies]
{}

[target.{}]
type = "executable"
sources = ["src/{}"]
deps = [{}]
"#,
                self.name, self.standard, self.dependencies, self.name, self.source_name, deps
            ),
        )
        .expect("write consumer manifest");
        dir.join("cabin.toml")
    }
}

/// Publish every port under `ports_dir` verbatim into
/// `<work_dir>/registry`,
/// returning the registry path for `--index-path`.  Cache-only on
/// purpose: every port's archive must already be seeded into
/// `cache_dir` (see
/// `FakePort::seed_archive_into_cache`), so a fixture
/// regression fails loudly instead of attempting the network.
pub fn stage_ports_registry(ports_dir: &Path, cache_dir: &Path, work_dir: &Path) -> PathBuf {
    stage(ports_dir, cache_dir, work_dir, ArchiveFetch::CacheOnly)
}

/// The pinned catalog's ports (`FOUNDATION_PORTS_DIR`, relative to the
/// repository root; `docs/foundation-ports.md`, "The pinned catalog")
/// staged into one immutable file registry shared by every test in the
/// calling process.  Staging downloads the pinned real upstream
/// archives, so callers must be `#[ignore = "requires external
/// network"]` tests; sharing one registry keeps the ignored example
/// suite from re-downloading the whole set per test.
pub fn pinned_ports_registry() -> &'static Path {
    static STAGED: OnceLock<StagedCommitted> = OnceLock::new();
    STAGED
        .get_or_init(|| {
            let root = assert_fs::TempDir::new().expect("pinned-ports staging dir");
            let ports_dir =
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .join(std::env::var_os("FOUNDATION_PORTS_DIR").expect(
                        "set FOUNDATION_PORTS_DIR to the pinned catalog's ports directory",
                    ));
            let registry = stage(
                &ports_dir,
                &root.path().join("cache"),
                root.path(),
                ArchiveFetch::CacheOrDownload,
            );
            StagedCommitted {
                _root: root,
                registry,
            }
        })
        .registry
        .as_path()
}

/// Keeps the staging `TempDir` alive for the whole test process so
/// the shared registry outlives every borrowing test.
struct StagedCommitted {
    _root: assert_fs::TempDir,
    registry: PathBuf,
}

fn stage(ports_dir: &Path, cache_dir: &Path, work_dir: &Path, fetch: ArchiveFetch) -> PathBuf {
    let registry_dir = work_dir.join("registry");
    let sources_dir = work_dir.join("src");
    let ports_cache = cache_dir.join("ports");
    let mut staged = 0;
    // `<name>/<version>/` directories; a file registry accepts
    // packages in any order, so no publication order is needed.
    for name_dir in sorted_dirs(ports_dir) {
        for package_dir in sorted_dirs(&name_dir) {
            stage_committed_package(
                &package_dir,
                &ports_cache,
                &sources_dir,
                &registry_dir,
                fetch,
            )
            .unwrap_or_else(|err| {
                panic!(
                    "staging {} into the fixture registry: {err}",
                    package_dir.display()
                )
            });
            staged += 1;
        }
    }
    assert!(staged > 0, "no ports found under {}", ports_dir.display());
    registry_dir
}

fn sorted_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("reading {}: {err}", dir.display()))
        .map(|entry| entry.expect("ports directory entry").path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}
