//! Drives the suite: builds or accepts the binaries under test,
//! materializes fixtures, checks each scenario once, times it with
//! hyperfine, and returns the results.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail, ensure};
use serde::Deserialize;

use crate::report::{self, Build, Checkout, Host, Results, Samples, ScenarioResult};
use crate::scenario::{Expect, Scenario};
use crate::{ports, serve};
use xtask_bench::fixture::{self, FIXTURE_VERSION};

pub struct Binary {
    pub label: String,
    pub path: PathBuf,
    pub version: String,
    pub build: Build,
}

/// Shared inputs for every scenario of one invocation.
pub struct Harness {
    pub out: PathBuf,
    pub checkout: Checkout,
    pub self_exe: PathBuf,
    registry: Option<PathBuf>,
    ports: Option<PathBuf>,
    http: Option<serve::Server>,
    fixtures: PathBuf,
}

impl Harness {
    pub fn new(
        repo: &Path,
        checkout: Checkout,
        out: PathBuf,
        scenarios: &[Scenario],
    ) -> Result<Self> {
        fs::create_dir_all(&out)?;
        let perf = repo.join("target/perf");
        let fixtures = fixture::cache_root(&perf)?;
        for kind in scenarios.iter().map(|s| s.fixture).collect::<BTreeSet<_>>() {
            fixture::cached(&fixtures.join(kind.dir_name()), |dir| {
                crate::scenario::write_fixture(kind, dir)
            })?;
        }
        let registry = scenarios
            .iter()
            .any(|s| s.fixture.needs_registry())
            .then(|| {
                eprintln!(
                    "perf: generating the synthetic registry (cached until the harness changes)"
                );
                fixture::cached(&fixtures.join("registry"), |dir| {
                    fixture::write_registry(&dir.join("registry"), &dir.join("staging"))
                })
                .map(|dir| dir.join("registry"))
            })
            .transpose()?;
        let ports = scenarios
            .iter()
            .any(|s| s.ports)
            .then(|| {
                // Staged every run: the ports are read from `ports/`,
                // which can change without rebuilding the harness.
                eprintln!("perf: staging foundation ports (downloads upstream archives once)");
                // Under `work/`, which the harness owns, whatever `--out` is.
                let staging = out.join("work/ports");
                if staging.exists() {
                    fs::remove_dir_all(&staging)?;
                }
                ports::stage_registry(repo, &perf.join("ports-download-cache"), &staging)
            })
            .transpose()?;
        let http = match &registry {
            Some(registry) if scenarios.iter().any(Scenario::uses_http) => {
                Some(serve::serve(registry.clone())?)
            }
            _ => None,
        };
        Ok(Self {
            out,
            checkout,
            self_exe: std::env::current_exe()?,
            registry,
            ports,
            http,
            fixtures,
        })
    }
}

/// A scenario's private copy of its fixture, ready for the timed
/// command.
pub struct Prepared {
    pub dir: PathBuf,
    pub traces: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub fn prepare(
    ctx: &Harness,
    scenario: &Scenario,
    binary: &Binary,
    work: &Path,
) -> Result<Prepared> {
    if work.exists() {
        fs::remove_dir_all(work)?;
    }
    let dir = work.join("project");
    // Traces stay outside the package root, which `cabin package`
    // archives whole.
    let traces = work.join("traces");
    copy_dir(&ctx.fixtures.join(scenario.fixture.dir_name()), &dir)?;
    let traces_arg = traces.display().to_string();
    let substitute = |arg: &String| -> Result<String> {
        let path = |p: &Option<PathBuf>, what: &str| -> Result<String> {
            Ok(p.as_ref()
                .with_context(|| format!("{what} was not prepared"))?
                .display()
                .to_string())
        };
        Ok(match arg.as_str() {
            "{registry}" => path(&ctx.registry, "the synthetic registry")?,
            "{ports}" => path(&ctx.ports, "the ports registry")?,
            "{http}" => ctx.http.as_ref().context("no HTTP server")?.url.clone(),
            "{http-rtt}" => format!("{}/rtt", ctx.http.as_ref().context("no HTTP server")?.url),
            "{traces}" => traces_arg.clone(),
            _ => arg.clone(),
        })
    };
    let args = scenario
        .args
        .iter()
        .map(substitute)
        .collect::<Result<Vec<_>>>()?;
    let env = scenario
        .env
        .iter()
        .map(|(k, v)| Ok((k.clone(), substitute(v)?)))
        .collect::<Result<Vec<_>>>()?;
    let prepared = Prepared {
        dir,
        traces,
        args,
        env,
    };
    for setup in &scenario.setup {
        let setup = setup.iter().map(substitute).collect::<Result<Vec<_>>>()?;
        let output = cabin(ctx, binary, &prepared, &setup).output()?;
        ensure!(
            output.status.success(),
            "setup `cabin {}` failed:\n{}",
            setup.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(prepared)
}

/// The Cabin command line every run uses: a private config and cache
/// home, and no other inherited `CABIN_*` variable or Actions OIDC
/// request token, so the user's settings, build directory, and registry
/// credentials neither change the result nor leak into it, and no flags
/// from the shell's `CPPFLAGS` family, which would change every compile
/// line.
pub fn cabin(ctx: &Harness, binary: &Binary, prepared: &Prepared, args: &[String]) -> Command {
    let mut cmd = Command::new(&binary.path);
    cmd.args(args).current_dir(&prepared.dir);
    isolate(&mut cmd, &ctx.out);
    cmd.envs(prepared.env.iter().map(|(k, v)| (k, v)));
    cmd
}

fn isolate(cmd: &mut Command, out: &Path) {
    for (key, _) in std::env::vars_os() {
        if key
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("CABIN_")
        {
            cmd.env_remove(key);
        }
    }
    cmd.env(cabin_env::CABIN_CONFIG_HOME, out.join("home/config"))
        .env(cabin_env::CABIN_CACHE_HOME, out.join("home/cache"));
    for var in [
        cabin_env::ACTIONS_ID_TOKEN_REQUEST_TOKEN,
        cabin_env::build_flags::CPPFLAGS,
        cabin_env::build_flags::CFLAGS,
        cabin_env::build_flags::CXXFLAGS,
        cabin_env::build_flags::LDFLAGS,
    ] {
        cmd.env_remove(var);
    }
}

/// How many timed runs each binary gets per scenario, after `warmup`
/// untimed ones.
pub struct Schedule {
    pub warmup: u32,
    pub runs: Runs,
}

pub enum Runs {
    Exact(u32),
    /// At least this many, and enough to fill [`MIN_TIME`] at the
    /// pace of a run made as the timed runs are, as hyperfine's own
    /// default does.
    AtLeast(u32),
}

const MIN_TIME: Duration = Duration::from_secs(3);

/// The most blocks a binary's runs of one scenario are split into: the
/// rounds a comparison pairs.
const MAX_BLOCKS: u32 = 20;

impl Schedule {
    /// The run count of each block, in order.  `pace` is the slowest
    /// binary's [`paced`] run.
    fn blocks(&self, pace: Duration) -> Vec<u32> {
        let runs = match self.runs {
            Runs::Exact(runs) => runs,
            Runs::AtLeast(runs) => {
                let fill = MIN_TIME.as_micros().div_ceil(pace.as_micros().max(1_000));
                runs.max(u32::try_from(fill).unwrap_or(u32::MAX))
            }
        };
        let blocks = runs.min(MAX_BLOCKS);
        (0..blocks)
            .map(|block| runs / blocks + u32::from(block < runs % blocks))
            .collect()
    }
}

/// The order the binaries time their blocks in, in `round` of the
/// `scenario`th scenario.  It rotates every round (A B C, B C A,
/// C A B, ...), so each binary takes every place in turn and load that
/// comes and goes, or drifts, reaches every binary alike; each scenario
/// starts one place further on, so no binary always goes first.
fn turns(binaries: usize, scenario: usize, round: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..binaries).collect();
    order.rotate_left((scenario + round) % binaries);
    order
}

/// Prepare and check `scenario` for `binary`: the result so far, and
/// what timing it needs when the check passed.
fn checked(
    ctx: &Harness,
    scenario: &Scenario,
    binary: &Binary,
    schedule: &Schedule,
) -> (ScenarioResult, Option<(Prepared, Duration)>) {
    let work = ctx
        .out
        .join("work")
        .join(&binary.label)
        .join(scenario.slug());
    let mut result = ScenarioResult {
        name: scenario.name.clone(),
        valid: false,
        detail: String::new(),
        samples: Samples::default(),
        blocks: Vec::new(),
        phases: serde_json::Value::Null,
        process: serde_json::Value::Null,
    };
    let prepared = match prepare(ctx, scenario, binary, &work) {
        Ok(prepared) => prepared,
        Err(err) => {
            result.detail = format!("{err:#}");
            return (result, None);
        }
    };
    let checked = check(ctx, scenario, binary, &prepared).and_then(|timings| {
        // An exact run count needs no pace.
        let pace = match schedule.runs {
            Runs::Exact(_) => Duration::ZERO,
            Runs::AtLeast(_) => paced(ctx, scenario, binary, &prepared)?,
        };
        Ok((timings, pace))
    });
    match checked {
        Ok((timings, pace)) => {
            result.valid = true;
            result.phases = timings["phases"].clone();
            // Before format 2, Linux's own peak could be the harness's.
            if timings["format_version"].as_u64() >= Some(2) {
                result.process = timings["process"].clone();
            }
            (result, Some((prepared, pace)))
        }
        Err(err) => {
            result.detail = format!("{err:#}");
            (result, None)
        }
    }
}

/// Run the scenario once more as a timed run would, and return how
/// long it took: the pace the timed runs are sized from.  The check
/// run is no measure of it, since it always records a trace.
fn paced(
    ctx: &Harness,
    scenario: &Scenario,
    binary: &Binary,
    prepared: &Prepared,
) -> Result<Duration> {
    if let Some(mutation) = &scenario.prepare {
        mutation.apply(&prepared.dir)?;
    }
    let started = Instant::now();
    let output = cabin(ctx, binary, prepared, &prepared.args).output()?;
    let pace = started.elapsed();
    ensure_success(&output, &prepared.args)
        .context("running the scenario again after its check")?;
    Ok(pace)
}

/// Time one block of `runs` runs with hyperfine, exporting to `json`.
fn time_block(
    ctx: &Harness,
    scenario: &Scenario,
    binary: &Binary,
    prepared: &Prepared,
    json: &Path,
    warmup: u32,
    runs: u32,
) -> Result<Vec<Measurement>> {
    let mut cmd = Command::new("hyperfine");
    cmd.current_dir(&prepared.dir);
    isolate(&mut cmd, &ctx.out);
    cmd.envs(prepared.env.iter().map(|(k, v)| (k, v)))
        .args(["-N", "--style", "none", "--export-json"])
        .arg(json)
        .args(["--command-name", &scenario.name])
        .args(["--warmup", &warmup.to_string(), "--runs", &runs.to_string()]);
    if let Some(mutation) = &scenario.prepare {
        // hyperfine runs `--prepare` through the platform shell.
        let mut line = vec![ctx.self_exe.display().to_string(), "mutate".to_owned()];
        line.extend(mutation.args());
        cmd.args(["--prepare", &shell_line(&line)]);
    }
    let mut line = vec![binary.path.display().to_string()];
    line.extend(prepared.args.iter().cloned());
    // `-N` splits the command with POSIX word rules on every platform.
    cmd.arg(posix_line(&line));
    let status = cmd.status().context("running hyperfine")?;
    ensure!(status.success(), "hyperfine exited with {status}");
    read_hyperfine(json)
}

/// Run the scenario once, untimed and traced, and confirm it did what
/// the scenario claims to measure.  Returns the trace's timings
/// summary, or `Null` from a revision without trace capture.
pub fn check(
    ctx: &Harness,
    scenario: &Scenario,
    binary: &Binary,
    prepared: &Prepared,
) -> Result<serde_json::Value> {
    if let Some(mutation) = &scenario.prepare {
        mutation.apply(&prepared.dir)?;
    }
    let build_dir = prepared.dir.join("build/dev");
    let log = build_dir.join(".ninja_log");
    let before = fs::read(&log).unwrap_or_default();
    let check_traces = prepared.traces.join("check");
    let output = cabin(ctx, binary, prepared, &prepared.args)
        .env(cabin_env::CABIN_TRACE_DIR, &check_traces)
        .output()?;
    ensure_success(&output, &prepared.args)?;

    verify(&scenario.expect, &prepared.dir, || {
        let after = fs::read(&log).with_context(|| format!("reading {}", log.display()))?;
        ninja_actions(&before, &after)
    })?;

    // A revision older than trace capture writes no summary; it is
    // still checked and timed, only without phases.
    let timings = fs::read_dir(&check_traces)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.to_string_lossy().ends_with(".timings.json"));
    let Some(timings) = timings else {
        return Ok(serde_json::Value::Null);
    };
    Ok(serde_json::from_slice(&fs::read(timings)?)?)
}

/// Confirm the check run did what the scenario claims.  `actions`
/// reads the Ninja actions the run appended to the log.
fn verify(expect: &Expect, dir: &Path, actions: impl Fn() -> Result<Vec<String>>) -> Result<()> {
    let compiled = || -> Result<usize> {
        Ok(actions()?
            .iter()
            .filter(|output| {
                Path::new(output).extension().is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("o") || ext.eq_ignore_ascii_case("obj")
                })
            })
            .count())
    };
    let no_actions = || -> Result<()> {
        let ran = actions()?;
        ensure!(
            ran.is_empty(),
            "expected no Ninja actions, ran {}",
            ran.len()
        );
        Ok(())
    };
    match expect {
        Expect::Success => {}
        Expect::NoActions => no_actions()?,
        Expect::Compiles(expected) => {
            let objects = compiled()?;
            ensure!(
                objects == *expected,
                "expected {expected} compiled objects, Ninja compiled {objects}"
            );
        }
        Expect::Builds => ensure!(compiled()? > 0, "expected a build, Ninja compiled nothing"),
        Expect::Locks(expected) => {
            let lock =
                fs::read_to_string(dir.join("cabin.lock")).context("reading `cabin.lock`")?;
            let found = lock.lines().filter(|l| *l == "[[package]]").count();
            ensure!(
                found == *expected,
                "expected {expected} locked packages, found {found}"
            );
        }
        Expect::Entries(path, expected) => {
            let found = fs::read_dir(dir.join(path))
                .with_context(|| format!("reading `{path}`"))?
                .count();
            ensure!(
                found == *expected,
                "expected {expected} entries in `{path}`, found {found}"
            );
        }
        Expect::Collects { removed, kept } => {
            ensure!(
                !dir.join(removed).exists(),
                "expected `{removed}` to be removed"
            );
            ensure!(dir.join(kept).exists(), "expected `{kept}` to be kept");
        }
        Expect::TestCases(report, expected) => {
            no_actions()?;
            let xml = fs::read_to_string(dir.join(report))
                .with_context(|| format!("reading `{report}`"))?;
            let found = xml.matches("<testcase").count();
            ensure!(
                found == *expected,
                "expected {expected} test cases in `{report}`, found {found}"
            );
        }
    }
    Ok(())
}

/// The outputs of the actions Ninja appended to its log, `before` the
/// check run and `after` it.
fn ninja_actions(before: &[u8], after: &[u8]) -> Result<Vec<String>> {
    let appended = after
        .strip_prefix(before)
        .context("Ninja rewrote its log during the check run, so its actions are unknown")?;
    Ok(String::from_utf8_lossy(appended)
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split('\t').nth(3).map(str::to_owned))
        .collect())
}

fn ensure_success(output: &Output, args: &[String]) -> Result<()> {
    ensure!(
        output.status.success(),
        "`cabin {}` exited with {}:\n{}",
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[derive(Deserialize)]
struct HyperfineExport {
    schema_version: u32,
    results: Vec<HyperfineResult>,
}

#[derive(Deserialize)]
struct HyperfineResult {
    measurements: Vec<Measurement>,
}

#[derive(Deserialize)]
struct Measurement {
    time_wall_clock: Quantity,
    time_user: Quantity,
    time_system: Quantity,
    memory_peak_resident: Option<Quantity>,
    instructions: Option<Quantity>,
}

#[derive(Deserialize)]
struct Quantity {
    value: f64,
}

fn read_hyperfine(path: &Path) -> Result<Vec<Measurement>> {
    let export: HyperfineExport = serde_json::from_slice(&fs::read(path)?)
        .with_context(|| format!("parsing {}", path.display()))?;
    ensure!(
        export.schema_version == 2,
        "hyperfine export schema {} (this harness reads hyperfine 2's schema 2)",
        export.schema_version
    );
    Ok(export
        .results
        .into_iter()
        .next()
        .context("hyperfine exported no result")?
        .measurements)
}

fn samples(runs: &[Measurement]) -> Samples {
    let all = |f: fn(&Measurement) -> Option<f64>| -> Vec<f64> {
        let values: Vec<f64> = runs.iter().filter_map(f).collect();
        // A counter missing from any run is unusable for all of them.
        if values.len() == runs.len() {
            values
        } else {
            Vec::new()
        }
    };
    Samples {
        wall_s: all(|m| Some(m.time_wall_clock.value)),
        user_s: all(|m| Some(m.time_user.value)),
        system_s: all(|m| Some(m.time_system.value)),
        peak_rss_bytes: all(|m| m.memory_peak_resident.as_ref().map(|q| q.value)),
        // Zero instructions is a Linux counter that never got
        // scheduled, not a measurement.
        instructions: all(|m| {
            m.instructions
                .as_ref()
                .map(|q| q.value)
                .filter(|&count| count > 0.0)
        }),
    }
}

pub fn posix_line(words: &[String]) -> String {
    words
        .iter()
        .map(|w| format!("'{}'", w.replace('\'', r"'\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(unix)]
fn shell_line(words: &[String]) -> String {
    posix_line(words)
}

/// `cmd.exe` quoting; benchmark paths never contain `"`.  `cmd /C`
/// strips the first and last quote of a line holding more than two,
/// so the whole line gets one more pair.
#[cfg(windows)]
fn shell_line(words: &[String]) -> String {
    let line = words
        .iter()
        .map(|w| format!("\"{w}\""))
        .collect::<Vec<_>>()
        .join(" ");
    format!("\"{line}\"")
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Build Cabin from `source` with `profile` and copy the binary into
/// the results directory, so a later rebuild cannot change it mid-run.
pub fn build(
    source: &Path,
    checkout: Checkout,
    target_dir: &Path,
    profile: &str,
    label: &str,
    out: &Path,
) -> Result<Binary> {
    eprintln!(
        "perf: building `{label}` ({profile} profile) in {}",
        source.display()
    );
    // The build gets the compiler it is recorded with: as `RUSTC` it
    // also overrides a `build.rustc` in a Cargo config file.
    let rustc = std::env::var_os("RUSTC")
        .or_else(|| std::env::var_os("CARGO_BUILD_RUSTC"))
        .unwrap_or_else(|| "rustc".into());
    let provenance = Build {
        source: checkout,
        profile: Some(profile.to_owned()),
        rustc: rustc_version(&rustc, source),
    };
    let status = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["build", "--locked", "-p", "cabinpkg", "--profile", profile])
        .env("RUSTC", &rustc)
        .env("CARGO_TARGET_DIR", target_dir)
        .current_dir(source)
        .status()?;
    ensure!(status.success(), "building `{label}` failed");
    let dir = if profile == "dev" { "debug" } else { profile };
    let built = target_dir
        .join(dir)
        .join(format!("cabin{}", std::env::consts::EXE_SUFFIX));
    let path = out
        .join("bin")
        .join(label)
        .join(format!("cabin{}", std::env::consts::EXE_SUFFIX));
    fs::create_dir_all(path.parent().unwrap())?;
    fs::copy(&built, &path).with_context(|| format!("copying {}", built.display()))?;
    Ok(Binary {
        build: provenance,
        ..binary(label, path, out)?
    })
}

/// The checkout at `dir`, as far as git can tell.
pub fn checkout(dir: &Path) -> Checkout {
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    Checkout {
        commit: git(&["rev-parse", "HEAD"]),
        dirty: git(&["status", "--porcelain", "--untracked-files=normal"])
            .map(|status| !status.is_empty()),
    }
}

/// `rustc -V`, run as the build in `dir` runs it.
fn rustc_version(rustc: &std::ffi::OsStr, dir: &Path) -> Option<String> {
    let output = Command::new(rustc)
        .arg("-V")
        .current_dir(dir)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Build a git revision in a throwaway worktree.  Revisions share one
/// target directory, so consecutive base builds are incremental.
pub fn build_revision(
    repo: &Path,
    rev: &str,
    profile: &str,
    label: &str,
    out: &Path,
) -> Result<Binary> {
    let source = repo.join(format!("target/perf/src-{label}"));
    let git = |args: &[&str]| -> Result<()> {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()?;
        ensure!(status.success(), "git {} failed", args.join(" "));
        Ok(())
    };
    if source.exists() {
        git(&[
            "worktree",
            "remove",
            "--force",
            &source.display().to_string(),
        ])?;
    }
    git(&[
        "worktree",
        "add",
        "--detach",
        &source.display().to_string(),
        rev,
    ])?;
    let built = build(
        &source,
        checkout(&source),
        &repo.join("target/perf/target-revisions"),
        profile,
        label,
        out,
    );
    git(&[
        "worktree",
        "remove",
        "--force",
        &source.display().to_string(),
    ])?;
    built
}

/// A binary to measure, identified by its `--version`, which runs as
/// isolated as every scenario command.  Its build is unknown until
/// [`build`] fills it in.
pub fn binary(label: &str, path: PathBuf, out: &Path) -> Result<Binary> {
    ensure_plain("binary label", label)?;
    let mut cmd = Command::new(&path);
    isolate(&mut cmd, out);
    let output = cmd
        .arg("--version")
        .output()
        .with_context(|| format!("running {}", path.display()))?;
    ensure!(
        output.status.success(),
        "{} --version failed",
        path.display()
    );
    Ok(Binary {
        label: label.to_owned(),
        version: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        path,
        build: Build::UNKNOWN,
    })
}

/// Labels and baseline names become file and directory names, on
/// file systems that may ignore case.
pub fn ensure_plain(what: &str, name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'),
        "{what} `{name}`: use lowercase letters, digits, `-`, and `_`"
    );
    Ok(())
}

pub fn tool_version(tool: &str) -> Option<String> {
    let output = Command::new(tool).arg("--version").output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub fn suite(
    ctx: &Harness,
    scenarios: &[Scenario],
    binaries: &[Binary],
    schedule: &Schedule,
) -> Result<Vec<Results>> {
    let hyperfine = tool_version("hyperfine").context("hyperfine 2 is required on PATH")?;
    if !hyperfine.starts_with("hyperfine 2.") {
        bail!("{hyperfine} found; this harness reads hyperfine 2's export schema");
    }
    let host = || Host {
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        cpus: std::thread::available_parallelism().map_or(1, usize::from),
    };
    let mut all: Vec<Results> = binaries
        .iter()
        .map(|b| Results {
            format_version: report::RESULTS_FORMAT_VERSION,
            label: b.label.clone(),
            cabin_version: b.version.clone(),
            build: b.build.clone(),
            harness: ctx.checkout.clone(),
            fixture_version: FIXTURE_VERSION,
            host: host(),
            hyperfine: hyperfine.clone(),
            ninja: tool_version("ninja"),
            scenarios: Vec::new(),
        })
        .collect();
    for (index, scenario) in scenarios.iter().enumerate() {
        let mut results = Vec::new();
        let mut ready = Vec::new();
        for binary in binaries {
            let (result, prepared) = checked(ctx, scenario, binary, schedule);
            if !result.valid {
                eprintln!(
                    "perf: {} [{}] FAILED its check: {}",
                    scenario.name, binary.label, result.detail
                );
            }
            results.push(result);
            ready.push(prepared);
        }
        let pace = ready.iter().flatten().map(|(_, pace)| *pace).max();
        let blocks = pace.map(|pace| schedule.blocks(pace)).unwrap_or_default();
        if !blocks.is_empty() {
            eprintln!(
                "perf: {} ({} runs per binary in {} interleaved blocks)",
                scenario.name,
                blocks.iter().sum::<u32>(),
                blocks.len()
            );
        }
        let mut runs: Vec<Vec<Measurement>> = binaries.iter().map(|_| Vec::new()).collect();
        for (round, &size) in blocks.iter().enumerate() {
            for i in turns(binaries.len(), index, round) {
                let Some((prepared, _)) = &ready[i] else {
                    continue;
                };
                let json = prepared
                    .dir
                    .with_file_name(format!("hyperfine-{round}.json"));
                let warmup = if round == 0 { schedule.warmup } else { 0 };
                match time_block(ctx, scenario, &binaries[i], prepared, &json, warmup, size) {
                    Ok(block) => runs[i].extend(block),
                    Err(err) => {
                        eprintln!(
                            "perf: {} [{}] FAILED: {err:#}",
                            scenario.name, binaries[i].label
                        );
                        results[i].valid = false;
                        results[i].detail = format!("{err:#}");
                        ready[i] = None;
                    }
                }
            }
        }
        for ((result, runs), all) in results.into_iter().zip(runs).zip(&mut all) {
            let mut result = result;
            if result.valid {
                result.samples = samples(&runs);
                result.blocks.clone_from(&blocks);
            }
            all.scenarios.push(result);
        }
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_log_that_kept_its_earlier_entries_yields_actions() {
        let before = b"# ninja log v7\n0\t9\t1\t/b/old.o\taa\n0\t9\t1\t/b/old.o\taa\n";
        let appended = [&before[..], b"5\t20\t2\t/b/x.o\tbb\n"].concat();
        assert_eq!(ninja_actions(before, &appended).unwrap(), ["/b/x.o"]);
        assert!(ninja_actions(before, before).unwrap().is_empty());
        // Compacted, then grown past the old length: an offset into it
        // would start inside the new action and miss it.
        let compacted = b"# ninja log v7\n0\t9\t1\t/b/old.o\taa\n5\t20\t2\t/b/longer-name.o\tbb\n";
        assert!(ninja_actions(before, compacted).is_err());
    }

    #[test]
    fn binaries_alternate_block_by_block() {
        let order = |scenario| -> Vec<usize> {
            (0..4).flat_map(|round| turns(2, scenario, round)).collect()
        };
        assert_eq!(order(0), [0, 1, 1, 0, 0, 1, 1, 0]);
        assert_eq!(order(1), [1, 0, 0, 1, 1, 0, 0, 1]);
        // Reversed, the middle one of three would never run twice in a
        // row, while the others would.
        let three: Vec<usize> = (0..3).flat_map(|round| turns(3, 0, round)).collect();
        assert_eq!(three, [0, 1, 2, 1, 2, 0, 2, 0, 1]);
    }

    #[test]
    fn runs_split_into_at_most_twenty_blocks() {
        let exact = |runs| {
            Schedule {
                warmup: 1,
                runs: Runs::Exact(runs),
            }
            .blocks(Duration::from_secs(1))
        };
        assert_eq!(exact(5), [1; 5]);
        assert_eq!(exact(23), [[2; 3].as_slice(), &[1; 17]].concat());
        let at_least = |pace| {
            Schedule {
                warmup: 2,
                runs: Runs::AtLeast(10),
            }
            .blocks(Duration::from_millis(pace))
        };
        // A slow scenario gets the floor, a fast one 3 s at its pace.
        assert_eq!(at_least(2_000).iter().sum::<u32>(), 10);
        assert_eq!(at_least(30), [5; 20]);
    }

    #[test]
    fn a_zero_instruction_count_is_missing_not_measured() {
        let run = |user: f64, instructions: f64| {
            serde_json::json!({
                "time_wall_clock": {"value": 0.1},
                "time_user": {"value": user},
                "time_system": {"value": 0.0},
                "memory_peak_resident": {"value": 1e7},
                "instructions": {"value": instructions},
            })
        };
        let export = |runs: Vec<serde_json::Value>| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("hyperfine.json");
            let json =
                serde_json::json!({"schema_version": 2, "results": [{"measurements": runs}]});
            fs::write(&path, json.to_string()).unwrap();
            samples(&read_hyperfine(&path).unwrap())
        };

        let measured = export(vec![run(0.0, 5e7), run(0.03, 6e7)]);
        // Zero CPU time can be real: Windows charges it in clock ticks.
        assert_eq!(measured.user_s, [0.0, 0.03]);
        assert_eq!(measured.instructions, [5e7, 6e7]);

        let failed = export(vec![run(0.02, 0.0), run(0.03, 6e7)]);
        assert_eq!(failed.user_s, [0.02, 0.03]);
        assert!(failed.instructions.is_empty());
    }
}
