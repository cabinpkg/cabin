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

use crate::plan::PortConversion;

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
        let package_dir = materialize(conversion, &port_cache, &sources_dir)?;
        let report = cabin_publish::publish_to_file_registry(RegistryPublishWorkflow {
            manifest_path: &package_dir.join("cabin.toml"),
            registry_dir: &registry_dir,
            resolved_project: None,
            workspace_dep_requirements: cabin_core::WorkspaceDepRequirements::default(),
        })
        .with_context(|| {
            format!(
                "publishing {} {} into the preflight registry",
                conversion.scoped_name.as_str(),
                conversion.published_version
            )
        })?;
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
) -> Result<PathBuf> {
    let source = resolve_fetch_source(conversion, port_cache)?;
    let plan = PortPlan {
        entries: vec![PortEntry {
            descriptor: conversion.descriptor.clone(),
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

    let package_dir = sources_dir
        .join(conversion.scoped_name.base_name())
        .join(conversion.published_version.to_string());
    copy_tree(&prepared.source_dir, &package_dir)?;
    fs::write(package_dir.join("cabin.toml"), &conversion.manifest)
        .with_context(|| format!("writing converted manifest into {}", package_dir.display()))?;
    Ok(package_dir)
}

/// Resolve where the pinned upstream archive's bytes come from,
/// mirroring the CLI's cache-first policy: a cached archive whose
/// bytes hash to the declared SHA-256 is reused; otherwise the
/// archive downloads with the same 5-hop redirect budget
/// foundation-port fetches use.  Conversion already validated every
/// recipe URL as credential-free HTTPS (the provenance rules), so no
/// other scheme reaches this point.
fn resolve_fetch_source(
    conversion: &PortConversion,
    port_cache: &PortCache,
) -> Result<PortFetchSource> {
    let source = &conversion.descriptor.source;
    let expected_hex = source.sha256.to_hex();
    let cached = port_cache.archive_path(&expected_hex, ArchiveKind::from_url(&source.url));
    if archive_matches(&cached, &expected_hex)? {
        return Ok(PortFetchSource::LocalArchive(cached));
    }
    let client = cabin_index_http::HttpClient::with_redirect_budget(5);
    let label = format!(
        "{}-{}",
        conversion.descriptor.name.as_str(),
        conversion.descriptor.version
    );
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
    fs::write(probe_dir.join("main.cc"), "int main() { return 0; }\n")
        .with_context(|| format!("writing probe source into {}", probe_dir.display()))?;

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

/// The probe package's manifest.  The exact requirement names the
/// published `major.minor.patch` (plus any pre-release tag);
/// requirement matching ignores build metadata, so a packaging
/// revision satisfies it too, and the temporary registry holds
/// exactly one version per conversion.  A sole library-like target
/// is referenced through the bare-package shorthand (exercising the
/// spelling consumers use); several are referenced through explicit
/// `package:target` selectors, since the shorthand is ambiguous
/// then; none leaves the dependency resolve-and-fetch-only.
fn probe_manifest(conversion: &PortConversion) -> String {
    let mut exact = conversion.published_version.clone();
    exact.build = semver::BuildMetadata::EMPTY;
    let scoped = conversion.scoped_name.as_str();
    let deps: Vec<String> = match conversion.library_like_target_keys.as_slice() {
        [] => Vec::new(),
        [_] => vec![format!("\"{scoped}\"")],
        keys => keys
            .iter()
            .map(|key| format!("\"{scoped}:{key}\""))
            .collect(),
    };
    format!(
        "[package]\nname = \"port-probe\"\nversion = \"0.0.0\"\nc-standard = \
         \"c11\"\ncxx-standard = \"c++17\"\n\n[dependencies]\n\"{scoped}\" = \
         \"={exact}\"\n\n[target.port-probe]\ntype = \"executable\"\nsources = \
         [\"main.cc\"]\ndeps = [{}]\n",
        deps.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{PortConversion, PortDependencyEdge};
    use cabin_port::{ArchiveSource, OverlayManifest, PortChecksum, PortDescriptor, PortMetadata};

    fn conversion(target_keys: &[&str]) -> PortConversion {
        PortConversion {
            recipe_dir: PathBuf::from("ports/zlib/1.3.1"),
            descriptor: PortDescriptor {
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
            },
            scoped_name: cabin_core::PackageName::new("cabin-ports/zlib").unwrap(),
            published_version: semver::Version::parse("1.3.1+cabin.1").unwrap(),
            manifest: String::new(),
            dependencies: Vec::<PortDependencyEdge>::new(),
            library_like_target_keys: target_keys.iter().map(|k| (*k).to_owned()).collect(),
        }
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
}
