//! Local preflight: materialize every conversion, publish it into a
//! temporary file registry, and build every port *from that
//! registry* in publication order.
//!
//! The preflight is the gate in front of any remote mutation: a
//! recipe that cannot fetch, extract, package, publish, resolve, or
//! compile locally never reaches the registry API.
//!
//! Each port is exercised through a generated probe package that
//! depends on the just-published version, so the build consumes the
//! registry artifact - archive bytes, checksum, archived manifest,
//! source materialization - rather than the scratch source tree it
//! was packaged from.  The probe's target references the dependency
//! through the bare-package shorthand, proving the published target
//! set supports it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use cabin_port::{
    ArchiveKind, PortCache, PortEntry, PortFetchSource, PortOrigin, PortPlan, PortPrepareOptions,
};
use cabin_publish::RegistryPublishWorkflow;

use crate::plan::{PortConversion, PortSource};

/// Preflight inputs.
#[derive(Debug)]
pub struct PreflightRequest<'a> {
    /// Publication-ordered conversions from [`crate::plan`].
    pub conversions: &'a [PortConversion],
    /// Cabin cache root; upstream archives and prepared trees reuse
    /// the standard `<cache>/ports` layout, so archives already
    /// fetched by ordinary builds are reused, not re-downloaded.
    pub cache_dir: &'a Path,
    /// Scratch root.  The preflight owns (and clears) exactly the
    /// `run/` directory beneath it, so pointing `--work-dir` at a
    /// directory with unrelated content is safe.
    pub work_dir: &'a Path,
    /// `cabin` binary used for the preflight builds.
    pub cabin: &'a Path,
}

/// Where the preflight left its outputs.
#[derive(Debug)]
pub struct PreflightReport {
    /// Temporary file registry holding every converted package.
    pub registry_dir: PathBuf,
    /// Scratch package directory per conversion, in publication
    /// order (parallel to the request's `conversions`).
    pub package_dirs: Vec<PathBuf>,
}

/// Run the full local preflight: for each conversion in publication
/// order, materialize it, publish it into the temporary file
/// registry, and immediately build its probe against the registry -
/// so a failure surfaces at the first port it affects.
///
/// # Errors
/// Returns the first failure: archive fetch or checksum mismatch,
/// preparation, staging or publish-lint rejection, file-registry
/// write, or a probe build exiting non-zero.
pub fn preflight(request: &PreflightRequest<'_>) -> Result<PreflightReport> {
    // The tool owns everything under `run/` and nothing else; a
    // reused work dir therefore cannot trip the file registry's
    // duplicate-version guard or leak a previous run's packages.
    let run_dir = request.work_dir.join("run");
    if run_dir.exists() {
        fs::remove_dir_all(&run_dir).with_context(|| format!("clearing {}", run_dir.display()))?;
    }
    let registry_dir = run_dir.join("registry");
    let sources_dir = run_dir.join("src");
    let port_cache = PortCache::new(request.cache_dir.join("ports"));

    let mut package_dirs = Vec::with_capacity(request.conversions.len());
    for conversion in request.conversions {
        let (package_dir, report) = stage_conversion(
            conversion,
            &port_cache,
            &sources_dir,
            &registry_dir,
            ArchiveFetch::CacheOrDownload,
            // The preflight registry is rebuilt from scratch every
            // run, so every publish is a first revision.
            false,
        )?;
        for warning in &report.warnings {
            eprintln!("warning: {warning}");
        }
        println!(
            "preflight: staged {} {} into {}",
            conversion.scoped_name.as_str(),
            conversion.published_version,
            report.registry_dir.display()
        );

        build_probe_against_registry(request, conversion, &run_dir, &registry_dir)?;
        println!(
            "preflight: built {} {} from the preflight registry",
            conversion.scoped_name.as_str(),
            conversion.published_version
        );
        package_dirs.push(package_dir);
    }

    Ok(PreflightReport {
        registry_dir,
        package_dirs,
    })
}

/// Where [`stage_conversion`] may get a conversion's pinned archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFetch {
    /// Reuse a cached archive, downloading on a miss - the
    /// publisher's mode.
    CacheOrDownload,
    /// Reuse a cached archive only, failing on a miss.  Hermetic
    /// test fixtures pre-seed the cache and rely on this so a
    /// checksum or cache-layout regression surfaces as an error
    /// instead of a network attempt against the (unreachable)
    /// recipe pin.
    CacheOnly,
}

/// Materialize one conversion and publish it into the file registry
/// at `registry_dir`, returning the staged package directory and the
/// publish report (lint warnings included - the caller decides how
/// to surface them).  This is the complete canonical-package
/// production step; Cabin's own integration tests reuse it so their
/// hermetic `cabin-ports/*` registry fixtures are generated by the
/// publisher's conversion pipeline instead of hand-written metadata.
///
/// # Errors
/// Returns the first failure: archive fetch or checksum mismatch (or
/// a cache miss under [`ArchiveFetch::CacheOnly`]), preparation, a
/// package directory left behind by a previous staging run, or
/// staging / publish-lint rejection / file-registry write.
pub fn stage_conversion(
    conversion: &PortConversion,
    port_cache: &PortCache,
    sources_dir: &Path,
    registry_dir: &Path,
    fetch: ArchiveFetch,
    new_revision: bool,
) -> Result<(PathBuf, cabin_publish::RegistryPublishReport)> {
    let package_dir = materialize(conversion, port_cache, sources_dir, fetch)?;
    let report = cabin_publish::publish_to_file_registry(RegistryPublishWorkflow {
        manifest_path: &package_dir.join("cabin.toml"),
        registry_dir,
        resolved_project: None,
        workspace_dep_requirements: cabin_core::WorkspaceDepRequirements::default(),
        new_revision,
    })
    .with_context(|| {
        format!(
            "publishing {} {} into the file registry",
            conversion.scoped_name.as_str(),
            conversion.published_version
        )
    })?;
    Ok((package_dir, report))
}

/// Materialize one conversion: prepare the recipe through the
/// standard port pipeline (checksum, safe extraction, strip-prefix,
/// `[[copy]]`, overlay identity cross-check), copy the prepared tree
/// into a scratch directory, and overwrite the manifest with the
/// converted text.  The committed overlay and the shared port cache
/// are never mutated.
fn materialize(
    conversion: &PortConversion,
    port_cache: &PortCache,
    sources_dir: &Path,
    fetch: ArchiveFetch,
) -> Result<PathBuf> {
    let package_dir = sources_dir
        .join(conversion.scoped_name.base_name())
        .join(conversion.published_version.to_string());
    // `copy_tree` overlays without clearing, so a directory left by a
    // previous run could smuggle stale files into the packaged
    // archive.  The preflight owns and clears its whole scratch root;
    // any other caller must supply a fresh one.
    if package_dir.exists() {
        bail!(
            "package directory {} already exists; stage into a fresh sources directory",
            package_dir.display()
        );
    }
    match &conversion.source {
        PortSource::Recipe(descriptor) => {
            let source = resolve_fetch_source(conversion, port_cache, fetch)?;
            let plan = PortPlan {
                entries: vec![PortEntry {
                    descriptor: (**descriptor).clone(),
                    origin: PortOrigin::PortDir(conversion.recipe_dir.clone()),
                    source,
                }],
            };
            let prepared = cabin_port::prepare(&plan, port_cache, PortPrepareOptions::default())
                .with_context(|| format!("preparing {}", conversion.recipe_dir.display()))?;
            let prepared = prepared
                .ports
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("prepare returned no ports"))?;
            copy_tree(&prepared.source_dir, &package_dir)?;
        }
        PortSource::Package { upstream } => {
            materialize_package(conversion, upstream, port_cache, &package_dir, fetch)?;
        }
    }

    // The committed manifest is what publishes: converted from the
    // overlay for a recipe, the package's own file otherwise.
    fs::write(package_dir.join("cabin.toml"), &conversion.manifest)
        .with_context(|| format!("writing the manifest into {}", package_dir.display()))?;
    Ok(package_dir)
}

/// Materialize a provenance-bearing package through the shared
/// pipeline (`cabin_artifact::materialize_upstream`) - the same
/// implementation the registry verifier replays - then lay the
/// committed patch files on top at their declared paths.
fn materialize_package(
    conversion: &PortConversion,
    upstream: &cabin_core::UpstreamProvenance,
    port_cache: &PortCache,
    package_dir: &Path,
    fetch: ArchiveFetch,
) -> Result<()> {
    let archive = resolve_package_archive(conversion, upstream, port_cache, fetch)?;
    fs::create_dir_all(package_dir)
        .with_context(|| format!("creating {}", package_dir.display()))?;

    // Patch bytes come from the committed package directory - the
    // same files the packaged archive will carry.
    let mut fetch_patch = |path: &camino::Utf8Path| {
        let committed = committed_patch_path(conversion, path).map_err(|err| {
            cabin_artifact::MaterializeError::Io {
                path: conversion.recipe_dir.join(path.as_std_path()),
                source: std::io::Error::other(format!("{err:#}")),
            }
        })?;
        match fs::read(&committed) {
            Ok(bytes) => Ok(cabin_artifact::PatchFetch::Found(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Ok(cabin_artifact::PatchFetch::Missing)
            }
            Err(source) => Err(cabin_artifact::MaterializeError::Io {
                path: committed,
                source,
            }),
        }
    };
    cabin_artifact::materialize_upstream(upstream, &archive, package_dir, &mut fetch_patch)
        .map_err(|err| {
            anyhow!(
                "materializing {} {}: {err}",
                conversion.scoped_name.as_str(),
                conversion.published_version
            )
        })?;

    // Place the declared patch files at their declared paths - the
    // published archive carries them, and the shared pipeline already
    // proved none shadows a produced file.
    for path in upstream.patches() {
        let committed = committed_patch_path(conversion, path)?;
        let placed = package_dir.join(path.as_std_path());
        if let Some(parent) = placed.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::copy(&committed, &placed).with_context(|| format!("placing {}", placed.display()))?;
    }
    Ok(())
}

/// Resolve a declared patch path inside the committed package
/// directory, refusing symlinks on every component below it - the
/// bytes entering the published archive must be the committed
/// regular files themselves, never something a link points at.
fn committed_patch_path(conversion: &PortConversion, path: &camino::Utf8Path) -> Result<PathBuf> {
    let mut current = conversion.recipe_dir.clone();
    for component in path.as_str().split('/') {
        current.push(component);
        if let Ok(metadata) = fs::symlink_metadata(&current)
            && metadata.file_type().is_symlink()
        {
            bail!(
                "declared patch path {} is (or crosses) a symlink; commit regular files",
                current.display()
            );
        }
    }
    Ok(current)
}

/// The pinned archive for a package, cache-first: a cached archive
/// whose bytes hash to the declared SHA-256 is reused, otherwise it
/// downloads into the same content-addressed slot recipes use.
fn resolve_package_archive(
    conversion: &PortConversion,
    upstream: &cabin_core::UpstreamProvenance,
    port_cache: &PortCache,
    fetch: ArchiveFetch,
) -> Result<PathBuf> {
    let expected_hex = upstream.sha256_hex();
    let kind = match upstream.format() {
        cabin_core::UpstreamFormat::TarGz => ArchiveKind::TarGz,
        cabin_core::UpstreamFormat::Zip => ArchiveKind::Zip,
    };
    let cached = port_cache.archive_path(expected_hex, kind);
    if archive_matches(&cached, expected_hex)? {
        return Ok(cached);
    }
    if fetch == ArchiveFetch::CacheOnly {
        bail!(
            "no cached archive for {} {} at {} and downloads are disabled; seed the port cache \
             before staging",
            conversion.scoped_name.as_str(),
            conversion.published_version,
            cached.display()
        );
    }
    let client = cabin_index_http::HttpClient::with_redirect_budget(5);
    let url = upstream.url().as_str();
    let label = format!(
        "{}-{}",
        conversion.scoped_name.base_name(),
        conversion.published_version
    );
    let bytes = client
        .download(url, &label)
        .map_err(|err| anyhow!("failed to download {url}: {err}"))?;
    println!("fetched {url} ({} bytes)", bytes.len());
    let actual = cabin_core::hash::hash_reader(bytes.as_slice())
        .with_context(|| format!("hashing the downloaded archive for {label}"))?;
    if actual != expected_hex {
        bail!("downloaded archive for {label} hashes to {actual}, expected {expected_hex}");
    }
    if let Some(parent) = cached.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    // Atomic sibling-rename write, so a concurrent reader never sees
    // a partial archive at the content-addressed slot.  A checksum-
    // invalid entry may still occupy the slot (that is why this is a
    // miss), and Windows' rename cannot replace an existing file -
    // clear it first so cache corruption can self-heal.
    if cached.exists() {
        fs::remove_file(&cached).with_context(|| format!("removing stale {}", cached.display()))?;
    }
    let staging = cached.with_extension("part");
    fs::write(&staging, &bytes).with_context(|| format!("writing {}", staging.display()))?;
    fs::rename(&staging, &cached).with_context(|| format!("moving into {}", cached.display()))?;
    Ok(cached)
}

/// Resolve where the pinned upstream archive's bytes come from,
/// mirroring the CLI's cache-first policy: a cached archive whose
/// bytes hash to the declared SHA-256 is reused; otherwise the
/// archive downloads with the same 5-hop redirect budget
/// foundation-port fetches use (a miss under
/// [`ArchiveFetch::CacheOnly`] is an error instead).  Conversion already validated every
/// recipe URL as credential-free HTTPS (the provenance rules), so no
/// other scheme reaches this point.
fn resolve_fetch_source(
    conversion: &PortConversion,
    port_cache: &PortCache,
    fetch: ArchiveFetch,
) -> Result<PortFetchSource> {
    let descriptor = conversion
        .source
        .descriptor()
        .ok_or_else(|| anyhow!("a package port has no recipe source to fetch"))?;
    let source = &descriptor.source;
    let expected_hex = source.sha256.to_hex();
    let cached = port_cache.archive_path(&expected_hex, ArchiveKind::from_url(&source.url));
    if archive_matches(&cached, &expected_hex)? {
        return Ok(PortFetchSource::LocalArchive(cached));
    }
    if fetch == ArchiveFetch::CacheOnly {
        bail!(
            "no cached archive for {} {} at {} and downloads are disabled; seed the port cache \
             before staging",
            descriptor.name.as_str(),
            descriptor.version,
            cached.display()
        );
    }
    let client = cabin_index_http::HttpClient::with_redirect_budget(5);
    let label = format!("{}-{}", descriptor.name.as_str(), descriptor.version);
    let bytes = client
        .download(source.url.as_str(), &label)
        .map_err(|err| anyhow!("failed to download {}: {err}", source.url))?;
    println!("fetched {} ({} bytes)", source.url, bytes.len());
    Ok(PortFetchSource::InMemoryArchive(bytes))
}

/// `Ok(true)` when a cached archive exists and hashes to
/// `expected_hex`; a missing file is a clean miss, any other read
/// failure surfaces.
fn archive_matches(path: &Path, expected_hex: &str) -> Result<bool> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => {
            return Err(anyhow!(
                "cached archive at {} could not be opened: {err}",
                path.display()
            ));
        }
    };
    let actual = cabin_core::hash::hash_reader(file)
        .with_context(|| format!("hashing cached archive at {}", path.display()))?;
    Ok(actual == expected_hex)
}

/// Recursively copy the prepared source tree.  Prepared trees hold
/// only regular files and directories (the extractors skip symlink
/// entries and reject other special entries), so a plain walk is
/// exhaustive.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    for entry in fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry.with_context(|| format!("reading {}", from.display()))?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let kind = entry
            .file_type()
            .with_context(|| format!("inspecting {}", source.display()))?;
        if kind.is_dir() {
            copy_tree(&source, &target)?;
        } else {
            fs::copy(&source, &target).with_context(|| format!("copying {}", source.display()))?;
        }
    }
    Ok(())
}

/// Build the port from the preflight registry through a generated
/// probe package that depends on the exact published version.  The
/// probe's standards (`c11` / `c++17`) sit at or above every
/// committed recipe's interface floor.  `--offline` enforces that
/// the file registry and the run-local cache satisfy the whole
/// build.
fn build_probe_against_registry(
    request: &PreflightRequest<'_>,
    conversion: &PortConversion,
    run_dir: &Path,
    registry_dir: &Path,
) -> Result<()> {
    let probe_dir = run_dir
        .join("probes")
        .join(conversion.scoped_name.base_name())
        .join(conversion.published_version.to_string());
    fs::create_dir_all(&probe_dir).with_context(|| format!("creating {}", probe_dir.display()))?;
    fs::write(probe_dir.join("cabin.toml"), probe_manifest(conversion))
        .with_context(|| format!("writing probe manifest into {}", probe_dir.display()))?;
    for source in probe_sources(conversion) {
        fs::write(probe_dir.join(source), "int main() { return 0; }\n")
            .with_context(|| format!("writing probe source into {}", probe_dir.display()))?;
    }

    let status = Command::new(request.cabin)
        .arg("build")
        .arg("--manifest-path")
        .arg(probe_dir.join("cabin.toml"))
        .arg("--index-path")
        .arg(registry_dir)
        .arg("--cache-dir")
        .arg(run_dir.join("cache"))
        .arg("--build-dir")
        .arg(probe_dir.join("build"))
        .arg("--offline")
        .status()
        .with_context(|| format!("running {} build", request.cabin.display()))?;
    if !status.success() {
        bail!(
            "preflight build of {} {} against the generated registry failed ({status})",
            conversion.scoped_name.as_str(),
            conversion.published_version
        );
    }
    Ok(())
}

/// One generated probe consumer: the target it is emitted as, the
/// language its source is written in, and the probed targets it
/// links.
struct ProbeConsumer<'a> {
    target: &'static str,
    source: &'static str,
    keys: Vec<&'a str>,
}

/// Split the probed targets across the consumers that can link them.
/// Targets forbidding C++ consumption go to a C consumer and the rest
/// to a C++ one, so a package mixing both shapes is still probed
/// whole.  A package with no library-like targets keeps the C++
/// consumer, leaving the dependency resolve-and-fetch-only.
fn probe_consumers(conversion: &PortConversion) -> Vec<ProbeConsumer<'_>> {
    let c_keys = &conversion.probe_standards.c_targets;
    let (c, cxx): (Vec<&str>, Vec<&str>) = conversion
        .library_like_target_keys
        .iter()
        .map(String::as_str)
        .partition(|key| c_keys.iter().any(|c| c == key));
    let mut consumers = Vec::new();
    if !cxx.is_empty() || c.is_empty() {
        consumers.push(ProbeConsumer {
            target: "port-probe",
            source: "main.cc",
            keys: cxx,
        });
    }
    if !c.is_empty() {
        consumers.push(ProbeConsumer {
            target: "port-probe-c",
            source: "main.c",
            keys: c,
        });
    }
    consumers
}

/// The probe sources to write beside the generated manifest.
fn probe_sources(conversion: &PortConversion) -> Vec<&'static str> {
    probe_consumers(conversion)
        .iter()
        .map(|consumer| consumer.source)
        .collect()
}

/// The probe package's manifest.  The exact requirement names the
/// published version.  A sole library-like target is referenced
/// through the bare-package shorthand (exercising the spelling
/// consumers use); several are referenced through explicit
/// `package:target` selectors, since the shorthand is ambiguous
/// then; none leaves the dependency resolve-and-fetch-only.
fn probe_manifest(conversion: &PortConversion) -> String {
    let exact = conversion.published_version.clone();
    let scoped = conversion.scoped_name.as_str();
    let sole_target = conversion.sole_library_target;
    // Each consumer's standards must satisfy the joined interface
    // requirements of the targets it links.  `c11` / `c++17` are the
    // floor; a package declaring a stricter interface raises them
    // (`plan::probe_standards`).
    let c = conversion
        .probe_standards
        .c
        .map_or("c11", cabin_core::CStandard::as_str);
    let consumers = probe_consumers(conversion);
    // A package with no C++-consumable target gets no `cxx-standard`:
    // declaring one the probe never compiles would be noise, and the
    // C++ edge is exactly what such a package forbids.
    let cxx = if consumers
        .iter()
        .any(|consumer| consumer.source == "main.cc")
    {
        let cxx = conversion
            .probe_standards
            .cxx
            .map_or("c++17", cabin_core::CxxStandard::as_str);
        format!("cxx-standard = \"{cxx}\"\n")
    } else {
        String::new()
    };
    let targets: Vec<String> = consumers
        .iter()
        .map(|consumer| {
            let deps: Vec<String> = consumer
                .keys
                .iter()
                .map(|key| {
                    if sole_target {
                        format!("\"{scoped}\"")
                    } else {
                        format!("\"{scoped}:{key}\"")
                    }
                })
                .collect();
            format!(
                "\n[target.{}]\ntype = \"executable\"\nsources = [\"{}\"]\ndeps = [{}]\n",
                consumer.target,
                consumer.source,
                deps.join(", ")
            )
        })
        .collect();
    format!(
        "[package]\nname = \"port-probe\"\nversion = \"0.0.0\"\nc-standard = \
         \"{c}\"\n{cxx}\n[dependencies]\n\"{scoped}\" = \"={exact}\"\n{}",
        targets.concat()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{PortConversion, PortDependencyEdge};
    use cabin_port::{ArchiveSource, OverlayManifest, PortChecksum, PortDescriptor, PortMetadata};

    fn conversion(target_keys: &[&str]) -> PortConversion {
        PortConversion {
            probe_standards: crate::plan::ProbeStandards::default(),
            recipe_dir: PathBuf::from("ports/zlib/1.3.1"),
            source: PortSource::Recipe(Box::new(PortDescriptor {
                name: cabin_core::PackageName::new("zlib").unwrap(),
                version: semver::Version::new(1, 3, 1),
                metadata: PortMetadata::default(),
                source: ArchiveSource {
                    url: url::Url::parse("https://example.com/zlib-1.3.1.tar.gz").unwrap(),
                    sha256: PortChecksum::parse_hex(&"a".repeat(64)).unwrap(),
                    strip_prefix: None,
                },
                overlay: OverlayManifest {
                    relative_path: camino::Utf8PathBuf::from("cabin.toml"),
                },
                copies: Vec::new(),
                patches: Vec::new(),
            })),
            scoped_name: cabin_core::PackageName::new("cabin-ports/zlib").unwrap(),
            published_version: semver::Version::new(1, 3, 1),
            manifest: String::new(),
            dependencies: Vec::<PortDependencyEdge>::new(),
            library_like_target_keys: target_keys.iter().map(|k| (*k).to_owned()).collect(),
            sole_library_target: target_keys.len() == 1,
        }
    }

    /// The committed-patch resolver is the only barrier between a
    /// symlinked patch file and the published archive - declared
    /// patch entries are exempt from the verifier's tree comparison,
    /// so a followed link would smuggle uncommitted host-local bytes
    /// into a verified package.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_committed_patch_is_refused() {
        let dir = assert_fs::TempDir::new().unwrap();
        let package_dir = dir.path().join("ports/zlib/1.3.1");
        fs::create_dir_all(package_dir.join("patches")).unwrap();
        fs::write(dir.path().join("outside.patch"), "not committed\n").unwrap();
        std::os::unix::fs::symlink(
            dir.path().join("outside.patch"),
            package_dir.join("patches/0001-fix.patch"),
        )
        .unwrap();

        let mut conversion = conversion(&["z"]);
        conversion.recipe_dir = package_dir;
        let err =
            committed_patch_path(&conversion, camino::Utf8Path::new("patches/0001-fix.patch"))
                .unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("symlink"), "{message}");
    }

    /// A cache miss under `CacheOnly` must fail before any network
    /// machinery runs: the fixture pins an RFC 2606 `.invalid` host,
    /// so an attempted download could only surface as a slow
    /// resolution error, never as this message.
    #[test]
    fn cache_only_staging_fails_on_a_cache_miss_without_downloading() {
        let dir = assert_fs::TempDir::new().unwrap();
        let mut conversion = conversion(&["z"]);
        if let PortSource::Recipe(descriptor) = &mut conversion.source {
            descriptor.source.url =
                url::Url::parse("https://ports.invalid/zlib-1.3.1.tar.gz").unwrap();
        }
        let err = stage_conversion(
            &conversion,
            &PortCache::new(dir.path().join("cache/ports")),
            &dir.path().join("src"),
            &dir.path().join("registry"),
            ArchiveFetch::CacheOnly,
            false,
        )
        .unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("no cached archive for zlib 1.3.1")
                && message.contains("downloads are disabled"),
            "{message}"
        );
        assert!(
            !dir.path().join("registry").exists(),
            "a cache miss must not initialize the registry"
        );
    }

    /// `copy_tree` overlays without clearing, so staging into a
    /// package directory left by a previous run must refuse instead
    /// of packaging stale files.
    #[test]
    fn staging_refuses_a_reused_package_directory() {
        let dir = assert_fs::TempDir::new().unwrap();
        let sources_dir = dir.path().join("src");
        let stale = sources_dir.join("zlib").join("1.3.1");
        fs::create_dir_all(&stale).unwrap();
        let err = stage_conversion(
            &conversion(&["z"]),
            &PortCache::new(dir.path().join("cache/ports")),
            &sources_dir,
            &dir.path().join("registry"),
            ArchiveFetch::CacheOnly,
            false,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("already exists"), "{err:#}");
    }

    #[test]
    fn probe_uses_the_bare_shorthand_for_a_sole_library_target() {
        let manifest = probe_manifest(&conversion(&["z"]));
        assert!(
            manifest.contains("\"cabin-ports/zlib\" = \"=1.3.1\""),
            "{manifest}"
        );
        assert!(
            manifest.contains("deps = [\"cabin-ports/zlib\"]"),
            "{manifest}"
        );
    }

    #[test]
    fn probe_qualifies_every_target_when_the_shorthand_is_ambiguous() {
        let manifest = probe_manifest(&conversion(&["z", "zextra"]));
        assert!(
            manifest.contains("deps = [\"cabin-ports/zlib:z\", \"cabin-ports/zlib:zextra\"]"),
            "{manifest}"
        );
    }

    #[test]
    fn probe_without_library_targets_still_depends_on_the_package() {
        let manifest = probe_manifest(&conversion(&[]));
        assert!(
            manifest.contains("\"cabin-ports/zlib\" = \"=1.3.1\""),
            "{manifest}"
        );
        assert!(manifest.contains("deps = []"), "{manifest}");
    }

    /// Cabin resolves a bare `deps` entry against every library-like
    /// target the dependency declares, so a package that publishes a
    /// second one behind a non-default feature makes the shorthand
    /// ambiguous even though the probe links only the enabled target.
    #[test]
    fn probe_qualifies_its_target_when_a_gated_sibling_publishes_too() {
        let mut conversion = conversion(&["z"]);
        conversion.sole_library_target = false;
        let manifest = probe_manifest(&conversion);
        assert!(
            manifest.contains("deps = [\"cabin-ports/zlib:z\"]"),
            "{manifest}"
        );
    }

    /// Every probed target that forbids C++ consumption is linked by
    /// the C probe and the rest by the C++ one, so a mixed package
    /// still compiles both halves.
    #[test]
    fn probe_splits_a_mixed_target_set_across_two_consumers() {
        let mut conversion = conversion(&["z", "zxx"]);
        conversion.probe_standards.c_targets = vec!["z".to_owned()];
        let manifest = probe_manifest(&conversion);
        assert!(
            manifest.contains(
                "[target.port-probe]\ntype = \"executable\"\nsources = \
                 [\"main.cc\"]\ndeps = [\"cabin-ports/zlib:zxx\"]\n"
            ),
            "{manifest}"
        );
        assert!(
            manifest.contains(
                "[target.port-probe-c]\ntype = \"executable\"\nsources = \
                 [\"main.c\"]\ndeps = [\"cabin-ports/zlib:z\"]\n"
            ),
            "{manifest}"
        );
        assert_eq!(probe_sources(&conversion), ["main.cc", "main.c"]);
    }

    /// A package with no C++-consumable target declares no
    /// `cxx-standard`: the probe never compiles C++.
    #[test]
    fn probe_for_a_c_only_package_declares_no_cxx_standard() {
        let mut conversion = conversion(&["z"]);
        conversion.probe_standards.c_targets = vec!["z".to_owned()];
        let manifest = probe_manifest(&conversion);
        assert!(!manifest.contains("cxx-standard"), "{manifest}");
        assert!(
            manifest.contains(
                "[target.port-probe-c]\ntype = \"executable\"\nsources = \
                 [\"main.c\"]\ndeps = [\"cabin-ports/zlib\"]\n"
            ),
            "{manifest}"
        );
        assert_eq!(probe_sources(&conversion), ["main.c"]);
    }
}
