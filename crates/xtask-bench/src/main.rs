//! `cargo perf`: Cabin's end-to-end benchmark harness.  See
//! `docs/performance.md` for the workflows and
//! `docs/design/performance-observability.md` for the design.

mod ports;
mod report;
mod run;
mod scenario;
mod serve;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};

use crate::report::Verdict;
use crate::scenario::{Mutation, Scenario};

#[derive(Parser)]
#[command(name = "cargo perf", about = "Benchmark Cabin's commands end to end")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List the scenarios.
    List,
    /// Check and time scenarios, write results, and compare two
    /// binaries when given two.
    Run(RunArgs),
    /// Compare two results files (or saved baseline names).
    Compare {
        baseline: String,
        candidate: String,
        /// Directory for compare.md and compare.json [default:
        /// target/perf/runs/<unix time>].
        #[arg(long)]
        out: Option<PathBuf>,
        /// Exit with status 1 when any metric regressed or could not be
        /// compared.
        #[arg(long)]
        fail_on_regression: bool,
    },
    /// Prepare one scenario and run its command repeatedly, untimed,
    /// for a profiler to wrap.
    Exec {
        scenario: String,
        /// The Cabin binary to run.
        #[arg(long)]
        cabin: PathBuf,
        #[arg(long, default_value_t = 1)]
        repeat: usize,
    },
    /// Per-iteration state reset; hyperfine's `--prepare` runs it.
    #[command(hide = true)]
    Mutate { action: String, path: String },
}

#[derive(clap::Args)]
struct RunArgs {
    /// Only scenarios whose name contains one of these.
    #[arg(long)]
    filter: Vec<String>,
    /// Only the quick subset.
    #[arg(long)]
    smoke: bool,
    /// Include the foundation-port scenarios (downloads upstream
    /// archives on first use).
    #[arg(long)]
    ports: bool,
    /// A prebuilt binary to measure, as `LABEL=PATH`; repeatable.
    #[arg(long, value_name = "LABEL=PATH")]
    cabin: Vec<String>,
    /// Also build this git revision and measure it as `base`.
    #[arg(long, value_name = "REV")]
    base: Option<String>,
    /// Cargo profile for the binaries this command builds.
    #[arg(long, default_value = "release")]
    profile: String,
    /// Exact run count; by default hyperfine runs each scenario at
    /// least 10 times and for at least 3 seconds.
    #[arg(long)]
    runs: Option<u32>,
    /// Results directory [default: target/perf/runs/<unix time>].
    #[arg(long)]
    out: Option<PathBuf>,
    /// Also save each binary's results as a named baseline.
    #[arg(long, value_name = "NAME")]
    save_baseline: Option<String>,
    /// Exit with status 1 when the comparison finds a regression.
    #[arg(long)]
    fail_on_regression: bool,
}

fn main() -> ExitCode {
    match run_cli(Cli::parse()) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/xtask-bench sits two levels below the repository root")
        .to_path_buf()
}

fn default_out() -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    repo_root().join(format!("target/perf/runs/{now}"))
}

fn run_cli(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Command::List => {
            for s in scenario::catalog() {
                let tags = match (s.smoke, s.ports) {
                    (true, _) => "  [smoke]",
                    (_, true) => "  [ports]",
                    _ => "",
                };
                println!("{}{tags}", s.name);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Run(args) => run_suite(args),
        Command::Compare {
            baseline,
            candidate,
            out,
            fail_on_regression,
        } => {
            let baseline = report::read(&results_path(&baseline))?;
            let candidate = report::read(&results_path(&candidate))?;
            let out = out.unwrap_or_else(default_out);
            compare(&out, &baseline, &candidate, fail_on_regression)
        }
        Command::Exec {
            scenario,
            cabin,
            repeat,
        } => exec(&scenario, cabin, repeat),
        Command::Mutate { action, path } => {
            let mutation = match action.as_str() {
                "touch" => Mutation::Touch(path),
                "remove" => Mutation::Remove(path),
                other => bail!("unknown mutation {other}"),
            };
            mutation.apply(&std::env::current_dir()?)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn run_suite(args: RunArgs) -> Result<ExitCode> {
    let repo = repo_root();
    let scenarios: Vec<Scenario> = scenario::catalog()
        .into_iter()
        .filter(|s| !args.smoke || s.smoke)
        .filter(|s| args.ports || !s.ports)
        .filter(|s| {
            args.filter.is_empty() || args.filter.iter().any(|f| s.name.contains(f.as_str()))
        })
        .collect();
    ensure!(!scenarios.is_empty(), "no scenario matches");
    if let Some(name) = &args.save_baseline {
        run::ensure_plain("baseline name", name)?;
    }
    let specs = args
        .cabin
        .iter()
        .map(|spec| {
            spec.split_once('=')
                .with_context(|| format!("--cabin {spec}: expected LABEL=PATH"))
        })
        .collect::<Result<Vec<_>>>()?;
    check_labels(&specs, args.base.is_some())?;
    let out = args.out.unwrap_or_else(default_out);
    std::fs::create_dir_all(&out)?;
    // Scenario commands run from their own directories.  Not
    // `canonicalize`: `cmd.exe`, which runs `--prepare` on Windows,
    // refuses the `\\?\` form it returns there as a working directory.
    let out = std::path::absolute(&out)?;

    let mut binaries = Vec::new();
    if let Some(rev) = &args.base {
        binaries.push(run::build_revision(
            &repo,
            rev,
            &args.profile,
            "base",
            &out,
        )?);
    }
    if specs.is_empty() {
        binaries.push(run::build(
            &repo,
            &repo.join("target"),
            &args.profile,
            "candidate",
            &out,
        )?);
    }
    for (label, path) in specs {
        binaries.push(run::binary(label, std::path::absolute(path)?, &out)?);
    }

    let ctx = run::Harness::new(&repo, out.clone(), &scenarios)?;
    let hyperfine_args: Vec<String> = match (args.runs, args.smoke) {
        (Some(runs), _) => vec![
            "--warmup".into(),
            "1".into(),
            "--runs".into(),
            runs.to_string(),
        ],
        (None, true) => vec!["--warmup".into(), "1".into(), "--runs".into(), "5".into()],
        (None, false) => vec![
            "--warmup".into(),
            "2".into(),
            "--min-runs".into(),
            "10".into(),
        ],
    };
    let all = run::suite(&ctx, &scenarios, &binaries, &hyperfine_args)?;

    for results in &all {
        let path = out.join(format!("{}.json", results.label));
        std::fs::write(&path, serde_json::to_vec_pretty(results)?)?;
        if let Some(name) = &args.save_baseline {
            let saved = baseline_path(&format!("{name}-{}", results.label));
            std::fs::create_dir_all(saved.parent().unwrap())?;
            std::fs::copy(&path, &saved)?;
            eprintln!("perf: saved baseline {}", saved.display());
        }
    }
    report::write_report(&out, &all)?;
    eprintln!("perf: report {}", out.join("report.md").display());
    let invalid = all
        .iter()
        .flat_map(|r| &r.scenarios)
        .filter(|s| !s.valid)
        .count();
    let code = match all.as_slice() {
        [baseline, candidate] => compare(&out, baseline, candidate, args.fail_on_regression)?,
        _ => ExitCode::SUCCESS,
    };
    if invalid > 0 {
        eprintln!("perf: {invalid} scenario run(s) failed their check; see report.md");
        return Ok(ExitCode::FAILURE);
    }
    Ok(code)
}

/// Each label names a results file beside `compare.json`; checked
/// before anything is built.
fn check_labels(specs: &[(&str, &str)], base: bool) -> Result<()> {
    for &(label, _) in specs {
        run::ensure_plain("binary label", label)?;
    }
    let mut labels: Vec<&str> = specs.iter().map(|&(label, _)| label).collect();
    if base {
        labels.push("base");
    }
    if specs.is_empty() {
        labels.push("candidate");
    }
    ensure!(
        labels.iter().collect::<BTreeSet<_>>().len() == labels.len()
            && !labels.contains(&"compare"),
        "binary labels must be distinct and not `compare`: {}",
        labels.join(", ")
    );
    Ok(())
}

fn compare(
    out: &Path,
    baseline: &report::Results,
    candidate: &report::Results,
    fail_on_regression: bool,
) -> Result<ExitCode> {
    let rows = report::compare(baseline, candidate);
    std::fs::create_dir_all(out)?;
    report::write_compare(out, baseline, candidate, &rows)?;
    eprintln!("perf: comparison {}", out.join("compare.md").display());
    let failing = rows
        .iter()
        .filter(|r| matches!(r.verdict, Verdict::Regressed | Verdict::Invalid))
        .count();
    if failing > 0 {
        eprintln!("perf: {failing} regressed or invalid metric(s)");
        if fail_on_regression {
            return Ok(ExitCode::FAILURE);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn baseline_path(name: &str) -> PathBuf {
    repo_root().join(format!("target/perf/baselines/{name}.json"))
}

/// A results path, or the name of a baseline `--save-baseline` saved.
fn results_path(arg: &str) -> PathBuf {
    let path = PathBuf::from(arg);
    if path.exists() {
        path
    } else {
        baseline_path(arg)
    }
}

fn exec(name: &str, cabin: PathBuf, repeat: usize) -> Result<ExitCode> {
    let scenario = scenario::catalog()
        .into_iter()
        .find(|s| s.name == name)
        .with_context(|| format!("no scenario named {name}; see `cargo perf list`"))?;
    let repo = repo_root();
    let out = repo.join("target/perf/exec");
    let binary = run::binary("exec", std::path::absolute(cabin)?, &out)?;
    let ctx = run::Harness::new(&repo, out.clone(), std::slice::from_ref(&scenario))?;
    let prepared = run::prepare(&ctx, &scenario, &binary, &out.join(scenario.slug()))?;
    for _ in 0..repeat {
        if let Some(mutation) = &scenario.prepare {
            mutation.apply(&prepared.dir)?;
        }
        let status = run::cabin(&ctx, &binary, &prepared, &prepared.args).status()?;
        ensure!(
            status.success(),
            "`cabin {}` exited with {status}",
            prepared.args.join(" ")
        );
    }
    eprintln!(
        "perf: ran `cabin {}` {repeat} time(s) in {}",
        prepared.args.join(" "),
        prepared.dir.display()
    );
    Ok(ExitCode::SUCCESS)
}
