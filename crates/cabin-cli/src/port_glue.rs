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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use semver::{Version, VersionReq};
use sha2::{Digest, Sha256};

use cabin_core::{DependencyKind, DependencySource, PortDepSource, TargetPlatform};
use cabin_index_http::HttpClient;
use cabin_manifest::load_manifest;
use cabin_port::{
    PortCache, PortEntry, PortFetchSource, PortOrigin, PortPlan, PortPrepareOptions, PreparedPort,
    load_port, prepare,
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

/// Discover every foundation-port dep reachable from `inputs.seeds`,
/// prepare each port once, and return the prepared records. The
/// returned slice is sorted by canonical port directory so
/// downstream metadata stays deterministic. Seeds are scoped to
/// the caller's resolved selection: walking only those manifests
/// (instead of every workspace member) keeps unrelated members'
/// ports out of the prep set, which matters on `--offline` /
/// uncached environments where a sibling's port would otherwise
/// block the command.
pub(crate) fn discover_and_prepare(inputs: PortPrepInputs<'_>) -> Result<Vec<PreparedPort>> {
    if inputs.seeds.is_empty() {
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
    )?;
    Ok(result.ports)
}

/// Project a [`PreparedPort`] into the
/// [`PortPackageSource`] view the workspace loader consumes.
pub(crate) fn workspace_source(prepared: &PreparedPort) -> PortPackageSource {
    PortPackageSource {
        name: prepared.name.clone(),
        version: prepared.version.clone(),
        manifest_path: prepared.source_dir.join("cabin.toml"),
        origin: prepared.origin.clone(),
    }
}

/// Convenience helper used by every command that loads a workspace:
/// resolve the caller's `selection` against a port-less skeleton,
/// prepare only the foundation ports reachable from that
/// selection's primary packages, and return the full workspace
/// graph with the prepared ports linked in.
///
/// Scoping to the selected closure is what protects
/// `cabin build --package <name>` from being blocked by an
/// unrelated sibling's port: only `<name>` and its transitive
/// path-dep closure are walked for port discovery, so
/// `--offline` and uncached HTTP-backed ports declared elsewhere
/// in the workspace cannot fail the command.
pub(crate) fn prepare_ports_and_load_initial_graph(
    manifest_path: &Path,
    cache_dir_override: Option<&Path>,
    offline: bool,
    frozen: bool,
    include_dev: bool,
    selection: &cabin_workspace::PackageSelection,
) -> Result<(Vec<PreparedPort>, PackageGraph)> {
    // Resolve the cache directory consulting the same precedence
    // chain the rest of the pipeline uses: CLI override ▶
    // `CABIN_CACHE_DIR` env ▶ `[paths] cache-dir` from the merged
    // config files ▶ the user-global XDG fallback. Without the
    // config layer, foundation ports would miss a cache the
    // artifact pipeline subsequently honours, defeating
    // `--frozen` reproducibility.
    let cfg = crate::config_glue::load_effective_config_for_manifest(manifest_path)?;
    let cache_dir = match crate::config_glue::resolve_cache_dir(cache_dir_override, &cfg) {
        Some((p, _)) => p,
        None => crate::cli::cache_dir_for(manifest_path, cache_dir_override)?,
    };
    let port_cache = PortCache::new(cache_dir.join("ports"));

    // Light-load a port-less skeleton so we can resolve the
    // caller's selection without first preparing ports — which
    // is precisely the chicken-and-egg this scoping avoids on
    // every other call to the workspace loader. Port deps are
    // simply absent from the skeleton graph; the walker rebuilds
    // them below.
    let skeleton = cabin_workspace::load_workspace_skip_ports(manifest_path)?;
    let resolved = cabin_workspace::resolve_package_selection(&skeleton, selection)?;
    let seeds: Vec<PathBuf> = resolved
        .packages
        .iter()
        .map(|&i| skeleton.packages[i].manifest_path.clone())
        .collect();

    let prepared = discover_and_prepare(PortPrepInputs {
        seeds: &seeds,
        cache: &port_cache,
        offline,
        frozen,
        include_dev,
    })?;
    let port_sources: Vec<PortPackageSource> = prepared.iter().map(workspace_source).collect();
    // Port discovery just walked only the selected primary
    // packages' closure, so siblings outside that closure may
    // declare port deps that aren't in `port_sources`. Tolerate
    // those missing entries so the graph loads anyway —
    // emitting an error here would resurrect the very
    // cross-member failure the scoped discovery exists to
    // avoid. Selected packages' port deps are always present
    // in `port_sources` because the walker resolved them.
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
    Ok((prepared, graph))
}

/// A discovered foundation-port dependency, keyed for dedup.
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd)]
enum PortKey {
    /// `{ port-path = "..." }` — keyed by canonical port directory.
    PortDir(PathBuf),
    /// `{ port = true }` — keyed by package name (the dep name).
    Builtin(String),
}

/// Walks the workspace's path-dep graph, recording every
/// `DependencySource::Port` it finds (deduped by key). The walker
/// stays network-free — it never follows version deps or downloads
/// anything. Both filesystem (`port-path`) deps and bundled
/// (`port = true`) deps are recorded.
#[derive(Debug)]
struct PortDiscovery<'a> {
    /// Discovered foundation-port keys reached so far.
    /// `BTreeSet` keeps iteration deterministic for downstream
    /// metadata.
    ports: BTreeSet<PortKey>,
    /// Every version requirement declared against a bundled port
    /// name, in declaration order. `build_plan_entries` resolves
    /// a single recipe against the first entry and then verifies
    /// it satisfies every subsequent entry — silently dropping
    /// later requirements would otherwise let mismatched
    /// consumers compile against a recipe that violates their
    /// declared constraint.
    builtin_reqs: BTreeMap<String, Vec<VersionReq>>,
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
                    DependencySource::Port(PortDepSource::Path(rel)) => {
                        // Best-effort: a missing or unreadable port
                        // directory is left for the workspace loader
                        // to surface as the typed
                        // `WorkspaceError::PortDirectoryMissing` /
                        // canonicalise diagnostic.
                        let port_dir = manifest_dir.join(rel);
                        if let Ok(canonical_port_dir) = std::fs::canonicalize(&port_dir) {
                            self.ports.insert(PortKey::PortDir(canonical_port_dir));
                        }
                    }
                    DependencySource::Port(PortDepSource::Builtin { name, version_req }) => {
                        let key = name.as_str().to_owned();
                        self.builtin_reqs
                            .entry(key.clone())
                            .or_default()
                            .push(version_req.clone());
                        self.ports.insert(PortKey::Builtin(key));
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
    for key in &discovery.ports {
        let (descriptor, origin) = match key {
            PortKey::PortDir(port_dir) => {
                let descriptor = load_port(port_dir.join("port.toml"))
                    .with_context(|| format!("loading port at {}", port_dir.display()))?;
                (descriptor, PortOrigin::PortDir(port_dir.clone()))
            }
            PortKey::Builtin(name) => {
                let reqs = discovery
                    .builtin_reqs
                    .get(name)
                    .expect("walk inserts builtin_reqs in lockstep with ports");
                let recipe = cabin_port::builtin::lookup(name, req).ok_or_else(|| {
                    // Task 4 promotes this to a typed `PortError::BuiltinVersionNotFound`.
                    anyhow!(
                        "no bundled foundation port `{name}` satisfies `{}`",
                        req
                    )
                })?;
                let descriptor =
                    cabin_port::parse_port_str(recipe.port_toml, std::path::Path::new("<builtin>"))
                        .with_context(|| format!("parsing bundled port `{name}`"))?;
                (descriptor, PortOrigin::Builtin(recipe.name))
            }
        };
        let source = resolve_fetch_source(&origin, &descriptor, cache, offline, &mut http_client)?;
        entries.push(PortEntry {
            descriptor,
            origin,
            source,
        });
    }
    entries.sort_by_key(|a| port_sort_key(&a.origin));
    Ok(entries)
}

/// Deterministic ordering for prepared ports: bundled ports
/// first (by name), then filesystem ports (by canonical dir).
fn port_sort_key(origin: &PortOrigin) -> (u8, std::ffi::OsString) {
    match origin {
        PortOrigin::Builtin(name) => (0, std::ffi::OsString::from(*name)),
        PortOrigin::PortDir(p) => (1, p.as_os_str().to_owned()),
    }
}

fn resolve_fetch_source(
    origin: &PortOrigin,
    descriptor: &cabin_port::PortDescriptor,
    cache: &PortCache,
    offline: bool,
    http_client: &mut Option<HttpClient>,
) -> Result<PortFetchSource> {
    let origin_label = match origin {
        PortOrigin::PortDir(p) => p.display().to_string(),
        PortOrigin::Builtin(name) => format!("<builtin:{name}>"),
    };
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
                    origin_label,
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
            // Foundation-port archive downloads commonly hit
            // GitHub-style 302 redirects out to a CDN origin.
            // The integrity of each port archive is established
            // by the SHA-256 pin in `port.toml`, so following
            // these redirects is safe — unlike sparse-HTTP-index
            // metadata fetches, where same-origin pinning is the
            // promise. The limit of 5 hops matches the redirect
            // budget every other standards-compliant client
            // honours.
            let client = http_client.get_or_insert_with(|| HttpClient::with_redirect_budget(5));
            let label = format!("{}-{}", descriptor.name.as_str(), descriptor.version);
            let bytes = client
                .download(url.as_str(), &label)
                .map_err(|err| anyhow!("failed to download {}: {err}", url))?;
            Ok(PortFetchSource::InMemoryArchive(bytes))
        }
        other => Err(anyhow!(
            "port at {} declares an unsupported archive URL scheme `{}`; foundation ports support `file://`, `http://`, and `https://`",
            origin_label,
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
