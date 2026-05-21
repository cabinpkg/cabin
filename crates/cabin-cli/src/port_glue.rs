//! CLI orchestration for foundation ports.
//!
//! The CLI is the only layer that performs network access.
//! `cabin-port::prepare` is HTTP-free: it accepts archive bytes
//! via [`PortFetchSource`] but does not download them itself.
//! This module bridges the gap by:
//!
//! 1. discovering every foundation-port dependency reachable from
//!    the root manifest;
//! 2. loading each `port.toml`;
//! 3. resolving the declared archive URL to a
//!    [`PortFetchSource`] — `file://` URLs become
//!    `LocalArchive(...)`, `http(s)://` URLs are downloaded via
//!    [`cabin_index_http::HttpClient`] and wrapped in
//!    `InMemoryArchive(...)`;
//! 4. calling [`cabin_port::prepare`] with one [`PortPlan`];
//! 5. translating the resulting [`cabin_port::PreparedPort`]s
//!    into [`PortPackageSource`] values the workspace loader
//!    understands.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use sha2::{Digest, Sha256};

use cabin_core::DependencySource;
use cabin_index_http::HttpClient;
use cabin_manifest::load_manifest;
use cabin_port::{
    PortCache, PortEntry, PortFetchSource, PortPlan, PortPrepareOptions, PreparedPort, load_port,
    prepare,
};
use cabin_workspace::{PackageGraph, PortPackageSource};

/// Inputs to [`discover_and_prepare`].
#[derive(Clone, Copy)]
pub(crate) struct PortPrepInputs<'a> {
    /// Manifest paths the walker uses as entry points — typically
    /// the resolved primary-package set for the current selection.
    /// Walking only these (instead of every workspace member) is
    /// what keeps an unrelated sibling's port from blocking
    /// `cabin build --package <name>` on a fresh checkout.
    pub seeds: &'a [PathBuf],
    /// Where prepared port archives + source trees live. The
    /// caller picks the directory (typically `<cache>/ports`).
    pub cache: &'a PortCache,
    /// `--offline` blocks any HTTP download. `file://` URLs
    /// still work because they read from disk.
    pub offline: bool,
    /// `--frozen` forbids populating the cache. If a prepared
    /// port is not already on disk, preparation fails with
    /// [`cabin_port::PortError::FrozenCacheMiss`].
    pub frozen: bool,
    /// Whether dependencies declared in `[dev-dependencies]`
    /// participate in port discovery. `cabin test` (and any
    /// future command that activates dev edges) sets this to
    /// `true`; ordinary commands like `cabin build` leave it
    /// `false` so they never fetch a port that only a sibling's
    /// dev-deps reference.
    pub include_dev: bool,
}

/// Discover every foundation-port dep reachable from the root
/// manifest, prepare each port once, and return one
/// [`PortPackageSource`] per port. The returned slice is sorted
/// by canonical port directory so downstream metadata stays
/// deterministic.
pub(crate) fn discover_and_prepare(
    inputs: PortPrepInputs<'_>,
) -> Result<Vec<PortPackageSource>> {
    // If the root manifest does not exist on disk yet, let the
    // workspace loader emit its own missing-manifest diagnostic
    // (it carries the typed error code + actionable text). Port
    // discovery is a best-effort scan that runs *before* the
    // workspace loader; surfacing a wrapped canonicalise error
    // here would shadow that diagnostic.
    if !inputs.root_manifest.is_file() {
        return Ok(Vec::new());
    }
    let host_platform = TargetPlatform::current();
    let mut discovery = PortDiscovery::new(inputs.include_dev, &host_platform);
    for seed in inputs.seeds {
        discovery
            .walk(seed)
            .with_context(|| format!("discovering ports from {}", seed.display()))?;
    }

    if discovery.ports.is_empty() {
        return Ok(Vec::new());
    }

    let entries = build_plan_entries(&discovery, inputs.cache, inputs.offline)?;
    let plan = PortPlan { entries };
    let result = prepare(
        &plan,
        inputs.cache,
        PortPrepareOptions {
            frozen: inputs.frozen,
        },
    )
    .map_err(|err| anyhow!(err.to_string()))?;
    Ok(result
        .ports
        .into_iter()
        .map(to_workspace_source)
        .collect())
}

fn to_workspace_source(prepared: PreparedPort) -> PortPackageSource {
    PortPackageSource {
        name: prepared.name,
        version: prepared.version,
        manifest_path: prepared.source_dir.join("cabin.toml"),
        port_dir: prepared.port_dir,
    }
}

/// Convenience helper used by every command that loads a workspace:
/// prepare every reachable foundation port and load the initial
/// workspace graph in one call. Returns the prepared port sources
/// alongside the graph so the caller can thread the ports
/// through any later [`cabin_workspace::load_workspace_with_options`]
/// call (e.g. once patches are resolved).
pub(crate) fn prepare_ports_and_load_initial_graph(
    manifest_path: &Path,
    cache_dir_override: Option<&Path>,
    offline: bool,
    frozen: bool,
) -> Result<(Vec<PortPackageSource>, PackageGraph)> {
    let cache_dir = crate::cli::cache_dir_for(manifest_path, cache_dir_override)?;
    let port_cache = PortCache::new(cache_dir.join("ports"));
    let port_sources = discover_and_prepare(PortPrepInputs {
        root_manifest: manifest_path,
        cache: &port_cache,
        offline,
        frozen,
        include_dev,
    })?;
    let graph = cabin_workspace::load_workspace_with_options(
        manifest_path,
        &cabin_workspace::WorkspaceLoadOptions {
            registry: &[],
            patches: &[],
            ports: &port_sources,
            strict_packages: &BTreeSet::new(),
            include_dev_for: &BTreeSet::new(),
            tolerate_missing_ports: true,
        },
    )?;
    Ok((port_sources, graph))
}

/// Walks the workspace's path-dep graph, recording every
/// `DependencySource::Port` it finds (deduped by canonical port
/// directory). The walker stays filesystem-only — it never
/// follows version deps or downloads anything.
#[derive(Debug, Default)]
struct PortDiscovery {
    /// Canonical absolute port directory paths reached so far.
    /// `BTreeSet` keeps iteration deterministic for downstream
    /// metadata.
    ports: BTreeSet<PathBuf>,
    /// Manifests we have already parsed so the recursive walk
    /// terminates on diamond-shaped path-dep graphs.
    visited: BTreeSet<PathBuf>,
    /// Whether `[dev-dependencies]` participate in discovery.
    /// See [`PortPrepInputs::include_dev`].
    include_dev: bool,
    /// Host platform used to evaluate `[target.'cfg(...)'.<kind>]`
    /// conditions. Cfg-gated deps targeting a non-matching
    /// platform are dropped so the loader's later
    /// `dep.matches_platform` filter is honoured up-front.
    host_platform: &'a TargetPlatform,
}

impl<'a> PortDiscovery<'a> {
    fn new(include_dev: bool, host_platform: &'a TargetPlatform) -> Self {
        Self {
            ports: BTreeSet::new(),
            builtin_reqs: BTreeMap::new(),
            visited: BTreeSet::new(),
            include_dev,
            host_platform,
        }
    }

    fn walk(&mut self, manifest_path: &Path) -> Result<()> {
        // Best-effort canonicalisation: if the file is missing
        // or the I/O fails, defer to the workspace loader so
        // its canonical diagnostic surfaces.
        let Ok(canonical) = std::fs::canonicalize(manifest_path) else {
            return Ok(());
        };
        if !self.visited.insert(canonical.clone()) {
            return Ok(());
        }

        let manifest_dir = canonical
            .parent()
            .ok_or_else(|| anyhow!("manifest path {} has no parent", canonical.display()))?
            .to_path_buf();
        // Surface parse errors here rather than swallowing them:
        // an unparseable manifest is a hard error the user must
        // see, and walking past it would still let port
        // discovery hand other members' ports to the prep
        // pipeline (network + cache side effects) for a
        // workspace the loader will subsequently reject.
        let parsed = load_manifest(&canonical)
            .with_context(|| format!("parsing manifest at {}", canonical.display()))?;

        // Seeds passed in by `discover_and_prepare` are already
        // member manifests (the resolved primary set), so a
        // walked manifest never carries a `[workspace]` table at
        // this point — workspace expansion belongs to the
        // skeleton load that produced the seeds. The remaining
        // recursion follows `DependencySource::Path` edges from
        // each `[package].dependencies` entry.

        if let Some(pkg) = &parsed.package {
            for dep in &pkg.dependencies {
                // Mirror the workspace loader's active-edge filter:
                // skip [dev-dependencies] unless the caller opted
                // into dev discovery (`cabin test`), and skip
                // [target.'cfg(...)'.<kind>] entries that do not
                // match the host platform. Both filters keep us
                // from prepping a port that no real graph edge
                // will ever reach.
                if !self.include_dev && dep.kind == DependencyKind::Dev {
                    continue;
                }
                if !dep.matches_platform(self.host_platform) {
                    continue;
                }
                match &dep.source {
                    DependencySource::Port(rel) => {
                        // Best-effort: a missing or unreadable port
                        // directory is left for the workspace loader
                        // to surface as the typed
                        // `WorkspaceError::PortDirectoryMissing` /
                        // canonicalise diagnostic.
                        let port_dir = manifest_dir.join(rel);
                        if let Ok(canonical_port_dir) = std::fs::canonicalize(&port_dir) {
                            self.ports.insert(canonical_port_dir);
                        }
                    }
                    DependencySource::Path(rel) => {
                        let nested = manifest_dir.join(rel).join("cabin.toml");
                        if nested.is_file() {
                            self.walk(&nested)?;
                        }
                    }
                    DependencySource::Version(_) | DependencySource::Workspace => {}
                }
            }
        }
        Ok(())
    }
}

fn build_plan_entries(
    discovery: &PortDiscovery,
    cache: &PortCache,
    offline: bool,
) -> Result<Vec<PortEntry>> {
    let mut entries: Vec<PortEntry> = Vec::with_capacity(discovery.ports.len());
    let mut http_client: Option<HttpClient> = None;
    for port_dir in &discovery.ports {
        let descriptor = load_port(port_dir.join("port.toml"))
            .with_context(|| format!("loading port at {}", port_dir.display()))?;
        let source =
            resolve_fetch_source(port_dir, &descriptor, cache, offline, &mut http_client)?;
        entries.push(PortEntry {
            descriptor,
            port_dir: port_dir.clone(),
            source,
        });
    }
    // Sort by canonical port directory so the prepared sources
    // emerge in deterministic order.
    entries.sort_by(|a, b| a.port_dir.cmp(&b.port_dir));
    Ok(entries)
}

fn resolve_fetch_source(
    port_dir: &Path,
    descriptor: &cabin_port::PortDescriptor,
    cache: &PortCache,
    offline: bool,
    http_client: &mut Option<HttpClient>,
) -> Result<PortFetchSource> {
    let cabin_port::PortSource::Archive { url, sha256, .. } = &descriptor.source;
    // Cache-first: if the archive cache already holds a file
    // whose bytes hash to the declared SHA-256, point cabin-port
    // at the cached path instead of re-downloading. cabin-port's
    // ensure_archive() short-circuits on a hash match so this
    // turns a repeat invocation into a pure-filesystem fast path.
    let expected_hex = sha256.to_hex();
    let cached_archive = cache.archive_path(&expected_hex);
    if archive_matches(&cached_archive, &expected_hex)? {
        return Ok(PortFetchSource::LocalArchive(cached_archive));
    }
    match url.scheme() {
        "file" => {
            let path = url.to_file_path().map_err(|()| {
                anyhow!(
                    "port at {} declares a file:// URL that does not map to a filesystem path: {}",
                    port_dir.display(),
                    url
                )
            })?;
            Ok(PortFetchSource::LocalArchive(path))
        }
        "http" | "https" => {
            if offline {
                return Err(anyhow!(
                    "cannot download port `{} {}` from {} because --offline was specified; rerun without --offline or vendor the archive locally",
                    descriptor.name.as_str(),
                    descriptor.version,
                    url
                ));
            }
            let client = http_client.get_or_insert_with(HttpClient::new);
            let label = format!("{}-{}", descriptor.name.as_str(), descriptor.version);
            let bytes = client
                .download(url.as_str(), &label)
                .map_err(|err| anyhow!("failed to download {}: {err}", url))?;
            Ok(PortFetchSource::InMemoryArchive(bytes))
        }
        other => Err(anyhow!(
            "port at {} declares an unsupported archive URL scheme `{}`; foundation ports support `file://`, `http://`, and `https://`",
            port_dir.display(),
            other
        )),
    }
}

/// Hash check on a cached archive: returns `Ok(true)` when the
/// file exists and its SHA-256 matches `expected_hex`. A missing
/// file is `Ok(false)` (clean cache miss); any other I/O error
/// surfaces as a typed anyhow error so a corrupt or unreadable
/// cache fails loudly instead of silently re-downloading.
fn archive_matches(path: &Path, expected_hex: &str) -> Result<bool> {
    use std::io::Read as _;
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => {
            return Err(anyhow!(
                "cached port archive at {} could not be opened: {err}",
                path.display()
            ));
        }
    };
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f
            .read(&mut buf)
            .with_context(|| format!("reading cached port archive at {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()) == expected_hex)
}
