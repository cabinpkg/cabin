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

use anyhow::{Context, Result, bail};
use cabin_publish::{RegistryPublishWorkflow, upstream};

use crate::plan::PortConversion;

/// Preflight inputs.
#[derive(Debug)]
pub struct PreflightRequest<'a> {
    /// Publication-ordered conversions from [`crate::plan`].
    pub conversions: &'a [PortConversion],
    /// Cabin cache root; upstream archives are cached under
    /// `<cache>/ports`, so an archive fetched by an earlier run is
    /// reused, not re-downloaded.
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
            match stage(conversion, &ports_cache, &sources_dir, registry_dir) {
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

/// Materialize one conversion through the shared staging step
/// (`cabin_publish::upstream`) and publish it into the file registry
/// at `registry_dir`, returning the staged package directory, the
/// publish report (lint warnings included), and how long each phase
/// took.  Cabin's own `cabin-ports/*` registry fixtures stage through
/// the same step, so they are byte-identical to what this stages.
fn stage(
    conversion: &PortConversion,
    ports_cache: &Path,
    sources_dir: &Path,
    registry_dir: &Path,
) -> Result<(PathBuf, cabin_publish::RegistryPublishReport, PortTimings)> {
    let package = upstream::UpstreamPackage {
        name: &conversion.scoped_name,
        version: &conversion.published_version,
        upstream: &conversion.upstream,
        committed_dir: &conversion.committed_dir,
        manifest: &conversion.manifest,
    };
    let started = Instant::now();
    let archive = upstream::resolve_archive(
        &package,
        ports_cache,
        upstream::ArchiveFetch::CacheOrDownload,
    )?;
    let fetched = started.elapsed();
    // The preflight clears `run/`, which holds `sources_dir`, so this
    // directory is fresh.
    let package_dir = sources_dir
        .join(conversion.scoped_name.base_name())
        .join(conversion.published_version.to_string());
    upstream::materialize(&package, &archive, &package_dir)?;
    let materialized = started.elapsed();
    let report = cabin_publish::publish_to_file_registry(RegistryPublishWorkflow {
        manifest_path: &package_dir.join("cabin.toml"),
        registry_dir,
        resolved_project: None,
        workspace_dep_requirements: cabin_core::WorkspaceDepRequirements::default(),
        // The preflight stages into a fresh registry, so every publish
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

/// Build the port from the preflight registry through a generated
/// probe package that depends on the exact published version, and
/// return how long the build and the probe runs took.  `--offline`
/// enforces that the file registry and the worker's run-local cache
/// satisfy the whole build.  Probes build concurrently, so each one's
/// output prints as a single block once it finishes.
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
        if !request.run_probes {
            return Ok((built, Duration::ZERO));
        }
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
    // requirements of the targets it links.  It takes the lowest
    // standard those accept (`plan::probe_standards`), or `c11` /
    // `c++17` when none is declared.  cl.exe has no `/std:` flag below
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
