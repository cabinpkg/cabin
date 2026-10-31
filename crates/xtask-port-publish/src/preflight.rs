//! Local preflight: materialize every conversion, publish it into a
//! temporary file registry in publication order, and build every
//! port *from that registry*, the probes in parallel.
//!
//! The preflight is the gate in front of any remote mutation: a port
//! that cannot fetch, extract, package, publish, resolve, or compile
//! locally never reaches the registry API.
//!
//! Each port is exercised through a generated probe package that
//! depends on the just-published version, so the build consumes the
//! registry artifact - archive bytes, checksum, archived manifest,
//! source materialization - rather than the scratch source tree it
//! was packaged from.  The probe references the dependency the way
//! the published target set allows: the bare-package shorthand when
//! the package exposes exactly one library-like target, explicit
//! `package:target` selectors otherwise.  A probe consumer's source
//! is the port's committed consumer test (`tests/main.cc` or
//! `tests/main.c` in its version directory) when one exists, so the
//! build exercises the port's API, and an empty `main` otherwise.  A
//! dry-run then runs every probe, which must exit successfully;
//! `--publish` only builds them, keeping upstream code from running
//! beside the job's OIDC token.

use std::fmt;
use std::fs;
use std::io::{self, Write as _};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::{Mutex, MutexGuard, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use cabin_publish::RegistryPublishWorkflow;

use crate::plan::PortConversion;

/// Preflight inputs.
#[derive(Debug)]
pub struct PreflightRequest<'a> {
    /// Publication-ordered conversions from [`crate::plan`].
    pub conversions: &'a [PortConversion],
    /// Cabin cache root; upstream archives and materialized trees
    /// reuse the standard `<cache>/ports` layout, so an archive
    /// fetched by an earlier run is reused, not re-downloaded.
    pub cache_dir: &'a Path,
    /// Scratch root.  The preflight owns (and clears) exactly the
    /// `run/` directory beneath it, so pointing `--work-dir` at a
    /// directory with unrelated content is safe.
    pub work_dir: &'a Path,
    /// `cabin` binary used for the preflight builds.
    pub cabin: &'a Path,
    /// Whether to run each built probe as well.
    pub run_probes: bool,
}

/// Where the preflight left its outputs.
#[derive(Debug)]
pub struct PreflightReport {
    /// Temporary file registry holding every published package.
    pub registry_dir: PathBuf,
    /// Scratch package directory per port, in publication order
    /// (parallel to the request's `conversions`).
    pub package_dirs: Vec<PathBuf>,
}

/// Run the full local preflight.  This thread materializes each
/// conversion and publishes it into the temporary file registry in
/// publication order, handing every staged port to a pool of one probe
/// worker per available CPU, which builds (and, with `run_probes`,
/// runs) its probe against the registry.  Publication order stages a
/// version satisfying each of a port's dependencies before it, so its
/// probe can start the moment the port is staged.  A newer version
/// staged meanwhile may also satisfy one; the probe then resolves it,
/// as a consumer of the published registry would.
///
/// # Errors
/// Returns the failure of the earliest failing port in publication
/// order: archive fetch or checksum mismatch, preparation, staging or
/// publish-lint rejection, file-registry write, or a probe build (or,
/// with `run_probes`, run) exiting non-zero.  Staging and probes past
/// the earliest failure seen so far are skipped; those before it
/// always finish.
#[allow(clippy::too_many_lines)] // one stager feeding one probe pool through shared failure state
pub fn preflight(request: &PreflightRequest<'_>) -> Result<PreflightReport> {
    // The tool owns everything under `run/` and nothing else; a
    // reused work dir therefore cannot trip the file registry's
    // duplicate-version guard or leak a previous run's packages.
    let run_dir = request.work_dir.join("run");
    if run_dir.exists() {
        fs::remove_dir_all(&run_dir).with_context(|| format!("clearing {}", run_dir.display()))?;
    }
    ensure_consumer_tests_used(request.conversions)?;
    let registry_dir = run_dir.join("registry");
    let sources_dir = run_dir.join("src");
    let ports_cache = request.cache_dir.join("ports");
    let started = Instant::now();
    let workers = thread::available_parallelism().map_or(1, NonZeroUsize::get);
    println!(
        "preflight: staging into {} with {workers} probe workers",
        registry_dir.display()
    );

    // The earliest failing port in publication order, and its error.
    let failure: Mutex<Option<(usize, anyhow::Error)>> = Mutex::new(None);
    let failed_before = |index| {
        lock(&failure)
            .as_ref()
            .is_some_and(|(first, _)| *first < index)
    };
    let fail = |index, err| {
        let mut failure = lock(&failure);
        if failure.as_ref().is_none_or(|(first, _)| index < *first) {
            *failure = Some((index, err));
        }
    };
    let timings = Mutex::new(vec![PortTimings::default(); request.conversions.len()]);
    let (queue, inbox) = mpsc::channel::<usize>();
    let inbox = Mutex::new(inbox);

    let package_dirs = thread::scope(|scope| {
        // Owned by this closure, so leaving it - by any path - closes
        // the queue and lets the workers drain and exit.
        let queue = queue;
        let (failed_before, timings, fail, inbox) = (&failed_before, &timings, &fail, &inbox);
        let (run_dir, registry_dir) = (&run_dir, &registry_dir);
        for worker in 0..workers {
            // Concurrent Cabin processes must not share a source
            // cache: one extracting a package can remove the tree
            // another is still adopting.
            let cache_dir = run_dir.join("cache").join(worker.to_string());
            scope.spawn(move || {
                loop {
                    let next = lock(inbox).recv();
                    let Ok(index) = next else { break };
                    if failed_before(index) {
                        continue;
                    }
                    let conversion = &request.conversions[index];
                    match build_probe_against_registry(
                        request,
                        conversion,
                        run_dir,
                        registry_dir,
                        &cache_dir,
                    ) {
                        Ok((build, run)) => {
                            let timing = &mut lock(timings)[index];
                            timing.build = build;
                            timing.run = run;
                        }
                        Err(err) => fail(index, err),
                    }
                }
            });
        }

        let mut package_dirs = Vec::with_capacity(request.conversions.len());
        for (index, conversion) in request.conversions.iter().enumerate() {
            if failed_before(index) {
                break;
            }
            match stage(
                conversion,
                &ports_cache,
                &sources_dir,
                registry_dir,
                ArchiveFetch::CacheOrDownload,
            ) {
                Ok((package_dir, report, timing)) => {
                    for warning in &report.warnings {
                        eprintln!("warning: {warning}");
                    }
                    println!(
                        "preflight: staged {} {} ({timing})",
                        conversion.scoped_name.as_str(),
                        conversion.published_version
                    );
                    lock(timings)[index] = timing;
                    package_dirs.push(package_dir);
                    // Cannot fail: `inbox` outlives the scope.
                    let _ = queue.send(index);
                }
                Err(err) => {
                    fail(index, err);
                    break;
                }
            }
        }
        package_dirs
    });

    if let Some((_, err)) = failure.into_inner().unwrap_or_else(PoisonError::into_inner) {
        return Err(err);
    }
    report_timings(
        request.conversions,
        &timings.into_inner().unwrap_or_else(PoisonError::into_inner),
        started.elapsed(),
    );
    Ok(PreflightReport {
        registry_dir,
        package_dirs,
    })
}

/// A worker that panicked while holding a lock already fails the
/// whole preflight when the scope rejoins it, so a poisoned lock's
/// data is still fine to read.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Wall time each preflight phase took for one port.
#[derive(Debug, Clone, Copy, Default)]
struct PortTimings {
    /// Cached-archive verification, or the download on a miss.
    fetch: Duration,
    /// Extraction, `strip-prefix`, copy steps, and patches.
    materialize: Duration,
    /// Packaging and the file-registry write.
    publish: Duration,
    /// The probe's `cabin build`.
    build: Duration,
    /// The probe's `cabin run`s (dry-run only).
    run: Duration,
}

impl PortTimings {
    fn total(&self) -> Duration {
        self.fetch + self.materialize + self.publish + self.build + self.run
    }
}

impl fmt::Display for PortTimings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let phases = [
            ("fetch", self.fetch),
            ("materialize", self.materialize),
            ("publish", self.publish),
            ("build", self.build),
            ("run", self.run),
        ];
        let mut sep = "";
        for (name, duration) in phases {
            if !duration.is_zero() {
                write!(f, "{sep}{name} {:.2}s", duration.as_secs_f64())?;
                sep = ", ";
            }
        }
        Ok(())
    }
}

/// Print the wall time, each phase summed over every port, and the
/// slowest ports - where the next preflight speedup has to come from.
fn report_timings(conversions: &[PortConversion], timings: &[PortTimings], elapsed: Duration) {
    let summed = timings
        .iter()
        .fold(PortTimings::default(), |sum, timing| PortTimings {
            fetch: sum.fetch + timing.fetch,
            materialize: sum.materialize + timing.materialize,
            publish: sum.publish + timing.publish,
            build: sum.build + timing.build,
            run: sum.run + timing.run,
        });
    println!(
        "preflight: {} ports in {:.1}s; summed over ports: {summed}",
        conversions.len(),
        elapsed.as_secs_f64()
    );
    let mut slowest: Vec<(&PortConversion, &PortTimings)> =
        conversions.iter().zip(timings).collect();
    slowest.sort_by_key(|(_, timing)| std::cmp::Reverse(timing.total()));
    println!("preflight: slowest ports:");
    for (conversion, timing) in slowest.into_iter().take(10) {
        println!(
            "preflight:   {} {}: {timing}",
            conversion.scoped_name.as_str(),
            conversion.published_version
        );
    }
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
    /// upstream pin.
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
    ports_cache: &Path,
    sources_dir: &Path,
    registry_dir: &Path,
    fetch: ArchiveFetch,
) -> Result<(PathBuf, cabin_publish::RegistryPublishReport)> {
    stage(conversion, ports_cache, sources_dir, registry_dir, fetch)
        .map(|(package_dir, report, _)| (package_dir, report))
}

/// [`stage_conversion`], also returning how long each phase took.
fn stage(
    conversion: &PortConversion,
    ports_cache: &Path,
    sources_dir: &Path,
    registry_dir: &Path,
    fetch: ArchiveFetch,
) -> Result<(PathBuf, cabin_publish::RegistryPublishReport, PortTimings)> {
    let started = Instant::now();
    let (package_dir, fetched) = materialize(conversion, ports_cache, sources_dir, fetch)?;
    let materialized = started.elapsed();
    let report = cabin_publish::publish_to_file_registry(RegistryPublishWorkflow {
        manifest_path: &package_dir.join("cabin.toml"),
        registry_dir,
        resolved_project: None,
        workspace_dep_requirements: cabin_core::WorkspaceDepRequirements::default(),
        // Every caller stages into a fresh registry, so every publish
        // is a first revision.
        new_revision: false,
    })
    .with_context(|| {
        format!(
            "publishing {} {} into the file registry",
            conversion.scoped_name.as_str(),
            conversion.published_version
        )
    })?;
    let timing = PortTimings {
        fetch: fetched,
        materialize: materialized.saturating_sub(fetched),
        publish: started.elapsed().saturating_sub(materialized),
        ..PortTimings::default()
    };
    Ok((package_dir, report, timing))
}

/// Materialize one conversion into a staging directory and write the
/// manifest that publishes with it, returning the directory and the
/// time spent getting the archive.  The committed sources and the
/// shared port cache are never mutated.
fn materialize(
    conversion: &PortConversion,
    ports_cache: &Path,
    sources_dir: &Path,
    fetch: ArchiveFetch,
) -> Result<(PathBuf, Duration)> {
    let package_dir = sources_dir
        .join(conversion.scoped_name.base_name())
        .join(conversion.published_version.to_string());
    // Materialization extracts on top of this directory without ever
    // clearing it, so a directory left by a previous run could
    // smuggle stale files into the packaged archive.  The preflight
    // owns and clears its whole scratch root; any other caller must
    // supply a fresh one.
    if package_dir.exists() {
        bail!(
            "package directory {} already exists; stage into a fresh sources directory",
            package_dir.display()
        );
    }
    let fetched = materialize_package(conversion, ports_cache, &package_dir, fetch)?;

    // Written LAST, deliberately: the upstream archive extracts on
    // top of `package_dir`, so an upstream shipping its own root
    // `cabin.toml` would otherwise overwrite the committed manifest
    // and publish in its place.
    fs::write(package_dir.join("cabin.toml"), &conversion.manifest)
        .with_context(|| format!("writing the manifest into {}", package_dir.display()))?;
    Ok((package_dir, fetched))
}

/// Materialize a port's declared provenance through the shared
/// pipeline (`cabin_artifact::materialize_upstream`) - the same
/// implementation the registry verifier replays - then lay the
/// committed patch files on top at their declared paths.  Returns the
/// time spent getting the archive.
fn materialize_package(
    conversion: &PortConversion,
    ports_cache: &Path,
    package_dir: &Path,
    fetch: ArchiveFetch,
) -> Result<Duration> {
    let upstream = conversion.upstream.as_ref();
    let started = Instant::now();
    let archive = resolve_package_archive(conversion, ports_cache, fetch)?;
    let fetched = started.elapsed();
    fs::create_dir_all(package_dir)
        .with_context(|| format!("creating {}", package_dir.display()))?;

    // Patch bytes come from the committed package directory - the
    // same files the packaged archive will carry.
    let mut fetch_patch = |path: &camino::Utf8Path| {
        let committed = committed_patch_path(conversion, path).map_err(|err| {
            cabin_artifact::MaterializeError::Io {
                path: conversion.committed_dir.join(path.as_std_path()),
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
    Ok(fetched)
}

/// Resolve a declared patch path inside the committed package
/// directory, refusing symlinks on every component below it - the
/// bytes entering the published archive must be the committed
/// regular files themselves, never something a link points at.
fn committed_patch_path(conversion: &PortConversion, path: &camino::Utf8Path) -> Result<PathBuf> {
    let mut current = conversion.committed_dir.clone();
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

/// The pinned archive for a port, cache-first: a cached archive whose
/// bytes hash to the declared SHA-256 is reused, otherwise it
/// downloads into the shared content-addressed cache slot.
fn resolve_package_archive(
    conversion: &PortConversion,
    ports_cache: &Path,
    fetch: ArchiveFetch,
) -> Result<PathBuf> {
    let upstream = conversion.upstream.as_ref();
    let expected_hex = upstream.checksum().hex();
    let cached = cached_archive_path(ports_cache, expected_hex, upstream.format());
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

/// Content-addressed slot for a pinned upstream archive under the
/// ports cache root (`<cache>/ports`), keyed by the declared SHA-256
/// and the declared format's manifest spelling.  This subtree is the
/// publisher's own: `cabin-artifact` caches registry package archives
/// at `<cache>/archives` instead, and an ordinary build never fetches
/// an `[package.upstream]` archive at all.  Keeping the layout stable
/// is what lets repeat publisher runs - and the hermetic test
/// fixtures that pre-seed it - reuse a download instead of refetching.
fn cached_archive_path(
    ports_cache: &Path,
    hex: &str,
    format: cabin_core::UpstreamFormat,
) -> PathBuf {
    ports_cache
        .join("archives")
        .join("sha256")
        .join(format!("{hex}.{}", format.as_str()))
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

/// Build the port from the preflight registry through a generated
/// probe package that depends on the exact published version, and
/// return how long the build and the probe runs took.  The probe's
/// standards (`c11` / `c++17`) sit at or above every committed port's
/// interface floor.  `--offline` enforces that the file registry and
/// the worker's run-local cache satisfy the whole build.  Probes build
/// concurrently, so each one's output prints as a single block once it
/// finishes.
fn build_probe_against_registry(
    request: &PreflightRequest<'_>,
    conversion: &PortConversion,
    run_dir: &Path,
    registry_dir: &Path,
    cache_dir: &Path,
) -> Result<(Duration, Duration)> {
    let probe_dir = run_dir
        .join("probes")
        .join(conversion.scoped_name.base_name())
        .join(conversion.published_version.to_string());
    fs::create_dir_all(&probe_dir).with_context(|| format!("creating {}", probe_dir.display()))?;
    fs::write(probe_dir.join("cabin.toml"), probe_manifest(conversion))
        .with_context(|| format!("writing probe manifest into {}", probe_dir.display()))?;
    for consumer in probe_consumers(conversion) {
        fs::write(
            probe_dir.join(consumer.source),
            probe_source(conversion, consumer.source)?,
        )
        .with_context(|| format!("writing probe source into {}", probe_dir.display()))?;
    }

    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let mut cabin = |command: &[&str]| -> Result<ExitStatus> {
        let output = Command::new(request.cabin)
            .args(command)
            .arg("--manifest-path")
            .arg(probe_dir.join("cabin.toml"))
            .arg("--index-path")
            .arg(registry_dir)
            .arg("--cache-dir")
            .arg(cache_dir)
            .arg("--build-dir")
            .arg(probe_dir.join("build"))
            .arg("--offline")
            // One worker per CPU, each at Ninja's default of CPUs + 2,
            // would run that many heavy C++ compilations at once; two
            // per probe still splits the late, largest ports' builds.
            .args(["--jobs", "2"])
            .output()
            .with_context(|| format!("running {} {}", request.cabin.display(), command[0]))?;
        stdout.extend(output.stdout);
        stderr.extend(output.stderr);
        Ok(output.status)
    };
    let started = Instant::now();
    let probed = (|| {
        let status = cabin(&["build"])?;
        if !status.success() {
            bail!(
                "preflight build of {} {} against the generated registry failed ({status})",
                conversion.scoped_name.as_str(),
                conversion.published_version
            );
        }
        let built = started.elapsed();
        if request.run_probes {
            for consumer in probe_consumers(conversion) {
                let status = cabin(&["run", "--bin", consumer.target])?;
                if !status.success() {
                    bail!(
                        "preflight probe `{}` of {} {} failed ({status})",
                        consumer.target,
                        conversion.scoped_name.as_str(),
                        conversion.published_version
                    );
                }
            }
        }
        Ok((built, started.elapsed().saturating_sub(built)))
    })();

    // Both locks, stdout first everywhere, so no other probe's block
    // lands inside this one.  A failed write loses only log text.
    let (mut out, mut err) = (io::stdout().lock(), io::stderr().lock());
    let _ = out.write_all(&stdout);
    let _ = out.flush();
    let _ = err.write_all(&stderr);
    let _ = err.flush();
    if let Ok((build, run)) = probed {
        let timing = PortTimings {
            build,
            run,
            ..PortTimings::default()
        };
        let _ = writeln!(
            out,
            "preflight: built {} {} from the preflight registry ({timing})",
            conversion.scoped_name.as_str(),
            conversion.published_version
        );
    }
    probed
}

/// A probe consumer's source: the port's committed consumer test for
/// that language when one exists, an empty `main` otherwise.
fn probe_source(conversion: &PortConversion, source: &str) -> Result<Vec<u8>> {
    let test = conversion.committed_dir.join("tests").join(source);
    match fs::read(&test) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            Ok(b"int main() { return 0; }\n".to_vec())
        }
        read => read.with_context(|| format!("reading {}", test.display())),
    }
}

/// Every entry of a port's `tests/` directory must be a source one of
/// its probe consumers compiles: a misnamed or wrong-language test
/// would otherwise fall back to the empty `main` without a word.
fn ensure_consumer_tests_used(conversions: &[PortConversion]) -> Result<()> {
    for conversion in conversions {
        let tests_dir = conversion.committed_dir.join("tests");
        let entries = match fs::read_dir(&tests_dir) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            entries => entries.with_context(|| format!("reading {}", tests_dir.display()))?,
        };
        let sources: Vec<&str> = probe_consumers(conversion)
            .into_iter()
            .map(|consumer| consumer.source)
            .collect();
        for entry in entries {
            let entry = entry.with_context(|| format!("reading {}", tests_dir.display()))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Finder's `.DS_Store` and editor dotfiles are not tests.
            if name.starts_with('.') {
                continue;
            }
            if !sources.contains(&name.as_ref()) {
                bail!(
                    "{} is not a consumer test this port's probe compiles: expected \
                     tests/main.cc (tests/main.c only for a target declaring \
                     `interface-cxx-standard = \"none\"`)",
                    entry.path().display()
                );
            }
        }
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
    // (`plan::probe_standards`).  cl.exe has no `/std:` flag below
    // C11 / C++14 and Cabin refuses to stand another standard in, so
    // an MSVC host probes a lower declared floor at C11 / C++14.
    let msvc = cfg!(target_env = "msvc");
    let c = conversion
        .probe_standards
        .c
        .map_or(cabin_core::CStandard::C11, |c| {
            if msvc {
                c.max(cabin_core::CStandard::C11)
            } else {
                c
            }
        })
        .as_str();
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
            .map_or(cabin_core::CxxStandard::Cxx17, |cxx| {
                if msvc {
                    cxx.max(cabin_core::CxxStandard::Cxx14)
                } else {
                    cxx
                }
            })
            .as_str();
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

    fn conversion(target_keys: &[&str]) -> PortConversion {
        PortConversion {
            probe_standards: crate::plan::ProbeStandards::default(),
            committed_dir: PathBuf::from("ports/zlib/1.3.1"),
            upstream: Box::new(
                cabin_core::UpstreamProvenance::new(
                    "https://ports.invalid/zlib-1.3.1.tar.gz",
                    &format!("sha256:{}", "a".repeat(64)),
                    "tar.gz",
                    None,
                    Vec::new(),
                    Vec::new(),
                )
                .unwrap(),
            ),
            scoped_name: cabin_core::PackageName::new("cabin-ports/zlib").unwrap(),
            published_version: semver::Version::new(1, 3, 1),
            manifest: String::new(),
            dependencies: Vec::<PortDependencyEdge>::new(),
            declared_requirements: Vec::<crate::plan::DeclaredRequirement>::new(),
            library_like_target_keys: target_keys.iter().map(|k| (*k).to_owned()).collect(),
            sole_library_target: target_keys.len() == 1,
        }
    }

    fn probe_sources(conversion: &PortConversion) -> Vec<&'static str> {
        probe_consumers(conversion)
            .iter()
            .map(|consumer| consumer.source)
            .collect()
    }

    /// The committed consumer test replaces the empty `main` for its
    /// language only.
    #[test]
    fn a_committed_consumer_test_replaces_the_empty_main() {
        let dir = assert_fs::TempDir::new().unwrap();
        let mut conversion = conversion(&["z"]);
        conversion.committed_dir = dir.path().join("ports/zlib/1.3.1");
        fs::create_dir_all(conversion.committed_dir.join("tests")).unwrap();
        fs::write(
            conversion.committed_dir.join("tests/main.cc"),
            "// zlib test\n",
        )
        .unwrap();

        assert_eq!(
            probe_source(&conversion, "main.cc").unwrap(),
            b"// zlib test\n"
        );
        assert_eq!(
            probe_source(&conversion, "main.c").unwrap(),
            b"int main() { return 0; }\n"
        );
    }

    /// A test no probe compiles fails the preflight instead of being
    /// skipped - here a C test beside a port probed only from C++.
    #[test]
    fn an_unused_consumer_test_is_refused() {
        let dir = assert_fs::TempDir::new().unwrap();
        let mut conversion = conversion(&["z"]);
        conversion.committed_dir = dir.path().join("ports/zlib/1.3.1");
        let tests = conversion.committed_dir.join("tests");
        fs::create_dir_all(&tests).unwrap();
        fs::write(tests.join("main.cc"), "").unwrap();
        let conversions = [conversion];
        ensure_consumer_tests_used(&conversions).unwrap();

        fs::write(tests.join("main.c"), "").unwrap();
        let err = ensure_consumer_tests_used(&conversions).unwrap_err();
        let unused = tests.join("main.c").display().to_string();
        assert!(err.to_string().contains(&unused), "{err}");
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
        conversion.committed_dir = package_dir;
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
        let err = stage_conversion(
            &conversion(&["z"]),
            &dir.path().join("cache/ports"),
            &dir.path().join("src"),
            &dir.path().join("registry"),
            ArchiveFetch::CacheOnly,
        )
        .unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("no cached archive for cabin-ports/zlib 1.3.1")
                && message.contains("downloads are disabled"),
            "{message}"
        );
        assert!(
            !dir.path().join("registry").exists(),
            "a cache miss must not initialize the registry"
        );
    }

    /// Build a gzipped tarball from `(path, contents)` entries and
    /// return `(bytes, sha256_hex)`.
    fn tar_gz(entries: &[(&str, &str)]) -> (Vec<u8>, String) {
        use sha2::Digest as _;
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (rel, body) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            builder
                .append_data(&mut header, rel, body.as_bytes())
                .unwrap();
        }
        let bytes = builder.into_inner().unwrap().finish().unwrap();
        let hex = cabin_core::hash::hex_digest(&sha2::Sha256::digest(&bytes));
        (bytes, hex)
    }

    /// The committed manifest is written *after* materialization, so
    /// an upstream archive shipping its own root `cabin.toml` cannot
    /// displace what publishes.  Nothing else covers that ordering:
    /// no committed port's upstream ships one, so hoisting the write
    /// would move no published digest.
    #[test]
    fn an_upstream_root_manifest_never_displaces_the_committed_one() {
        const COMMITTED: &str = "[package]\nname = \"cabin-ports/zlib\"\nversion = \"1.3.1\"\n";
        let dir = assert_fs::TempDir::new().unwrap();
        let (bytes, hex) = tar_gz(&[
            (
                "zlib-1.3.1/cabin.toml",
                "[package]\nname = \"upstream-decoy\"\nversion = \"9.9.9\"\n",
            ),
            ("zlib-1.3.1/zlib.c", "int z(void) { return 0; }\n"),
        ]);
        let cache = dir.path().join("cache/ports");
        let slot = cache.join("archives/sha256").join(format!("{hex}.tar.gz"));
        fs::create_dir_all(slot.parent().unwrap()).unwrap();
        fs::write(&slot, &bytes).unwrap();

        let mut conversion = conversion(&["z"]);
        *conversion.upstream = cabin_core::UpstreamProvenance::new(
            "https://ports.invalid/zlib-1.3.1.tar.gz",
            &format!("sha256:{hex}"),
            "tar.gz",
            Some("zlib-1.3.1".to_owned()),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        conversion.manifest = COMMITTED.to_owned();

        let (package_dir, _) = materialize(
            &conversion,
            &cache,
            &dir.path().join("src"),
            ArchiveFetch::CacheOnly,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(package_dir.join("cabin.toml")).unwrap(),
            COMMITTED
        );
    }

    /// Materialization extracts without clearing, so staging into a
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
            &dir.path().join("cache/ports"),
            &sources_dir,
            &dir.path().join("registry"),
            ArchiveFetch::CacheOnly,
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
