//! Registry-fixture staging for integration tests that consume
//! `cabin-ports/*` packages.
//!
//! Fixtures are generated through the publisher's own pipeline
//! (`cabin_port_publish`): discovery via `plan::load_conversions`,
//! then materialization + file-registry publish via
//! `preflight::stage_conversion`.  Tests therefore
//! consume byte-identical packages to what `cabin-port-publish`
//! stages, instead of hand-written index metadata that could drift
//! from the real conversion rules.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use cabin_port::PortCache;
use cabin_port_publish::plan;
use cabin_port_publish::preflight::{ArchiveFetch, stage_conversion};

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
                "[package]\nname = \"{}\"\nversion = \"0.1.0\"\n{}\n\n[dependencies]\n{}\n\n[target.{}]\ntype = \"executable\"\nsources = [\"src/{}\"]\ndeps = [{}]\n",
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

/// The committed `ports/` tree staged into one
/// immutable file registry shared by every test in the calling
/// process.  Staging downloads the pinned real upstream
/// archives, so callers must be `#[ignore = "requires external
/// network"]` tests; sharing one registry keeps the ignored example
/// suite from re-downloading the whole set per test.
pub fn committed_ports_registry() -> &'static Path {
    static STAGED: OnceLock<StagedCommitted> = OnceLock::new();
    STAGED
        .get_or_init(|| {
            let root = assert_fs::TempDir::new().expect("committed-ports staging dir");
            let ports_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("ports");
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
    let conversions = plan::load_conversions(ports_dir).expect("load committed ports");
    let registry_dir = work_dir.join("registry");
    let sources_dir = work_dir.join("src");
    let port_cache = PortCache::new(cache_dir.join("ports"));
    for conversion in &conversions {
        stage_conversion(
            conversion,
            &port_cache,
            &sources_dir,
            &registry_dir,
            fetch,
            false,
        )
        .unwrap_or_else(|err| {
            panic!(
                "staging {} {} into the fixture registry: {err:#}",
                conversion.scoped_name.as_str(),
                conversion.published_version
            )
        });
    }
    registry_dir
}
