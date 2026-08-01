//! `cabin-port-publish` — repository tool that publishes the curated
//! foundation ports as `cabin-ports/<name>` registry packages.  See the library crate for the pipeline; this shim owns
//! argument parsing (hand-rolled: `clap` stays in the `cabin` crate)
//! and default resolution.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use cabin_port_publish::{Mode, Options, run};

const USAGE: &str = "\
usage: cabin-port-publish (--dry-run | --publish --index-url <URL>) [options]

Publishes the committed foundation ports as `cabin-ports/<name>` registry
packages: a recipe is converted, a migrated package directory is published
verbatim.  Both modes run the complete local preflight
(materialize, package, publish into a temporary file registry, build
every port against it in publication order); --publish then uploads
every package through the registry API.

options:
  --index-url <URL>   registry index URL (required with --publish)
  --ports-dir <PATH>  ports directory (default: the repository's
                      crates/cabin-port/ports)
  --cache-dir <PATH>  cabin cache root for upstream archives
                      (default: CABIN_CACHE_DIR, CABIN_CACHE_HOME, or
                      the platform cache directory + /cabin)
  --work-dir <PATH>   scratch directory; the tool owns (and clears)
                      exactly run/ beneath it (default: a per-run
                      directory under the system temp dir; kept on
                      failure)
  --cabin <PATH>      cabin binary for preflight builds (default: a
                      sibling of this executable)
  -h, --help          show this help
";

fn main() -> ExitCode {
    match parse_args().and_then(|options| match options {
        Parsed::Help => {
            print!("{USAGE}");
            Ok(())
        }
        Parsed::Run(options) => run_with_workdir_notice(&options),
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// Run, and on failure point at the kept scratch directory so the
/// staged packages and registry can be inspected.
fn run_with_workdir_notice(options: &Options) -> Result<()> {
    let result = run(options);
    if result.is_err() {
        eprintln!(
            "note: scratch directory kept at {}",
            options.work_dir.display()
        );
    }
    result
}

enum Parsed {
    Help,
    Run(Options),
}

fn parse_args() -> Result<Parsed> {
    let mut dry_run = false;
    let mut publish = false;
    let mut index_url: Option<String> = None;
    let mut ports_dir: Option<PathBuf> = None;
    let mut cache_dir: Option<PathBuf> = None;
    let mut work_dir: Option<PathBuf> = None;
    let mut cabin: Option<PathBuf> = None;

    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let Some(flag) = arg.to_str() else {
            bail!("arguments must be valid UTF-8: {}", arg.display());
        };
        let mut path_value = |name: &str| -> Result<PathBuf> {
            args.next()
                .map(PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("{name} requires a value"))
        };
        match flag {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--dry-run" => dry_run = true,
            "--publish" => publish = true,
            "--index-url" => {
                let value = args
                    .next()
                    .and_then(|v| v.to_str().map(str::to_owned))
                    .ok_or_else(|| anyhow::anyhow!("--index-url requires a value"))?;
                index_url = Some(value);
            }
            "--ports-dir" => ports_dir = Some(path_value("--ports-dir")?),
            "--cache-dir" => cache_dir = Some(path_value("--cache-dir")?),
            "--work-dir" => work_dir = Some(path_value("--work-dir")?),
            "--cabin" => cabin = Some(path_value("--cabin")?),
            other => bail!("unknown argument `{other}`\n\n{USAGE}"),
        }
    }

    let mode = match (dry_run, publish) {
        (true, false) => {
            if index_url.is_some() {
                bail!("--index-url only applies to --publish");
            }
            Mode::DryRun
        }
        (false, true) => Mode::Publish {
            index_url: index_url
                .ok_or_else(|| anyhow::anyhow!("--publish requires --index-url"))?,
        },
        _ => bail!("pass exactly one of --dry-run or --publish\n\n{USAGE}"),
    };

    Ok(Parsed::Run(Options {
        mode,
        ports_dir: match ports_dir {
            Some(dir) => dir,
            None => default_ports_dir(),
        },
        cache_dir: match cache_dir {
            Some(dir) => dir,
            None => default_cache_dir()?,
        },
        work_dir: match work_dir {
            Some(dir) => dir,
            None => std::env::temp_dir().join(format!("cabin-port-publish-{}", std::process::id())),
        },
        cabin: match cabin {
            Some(path) => path,
            None => default_cabin_path()?,
        },
    }))
}

/// The repository's ports directory, resolved from this crate's
/// compile-time location.  The tool is repository-owned (`publish =
/// false`), so the baked path is valid wherever the tool itself can
/// be built.
fn default_ports_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("cabin-port")
        .join("ports")
}

/// The standard cabin cache root: `CABIN_CACHE_DIR`, then
/// `CABIN_CACHE_HOME`, then the platform cache directory with the
/// `cabin` suffix — the same precedence the CLI resolves, so one
/// machine keeps a single upstream-archive cache across runs.
fn default_cache_dir() -> Result<PathBuf> {
    use etcetera::BaseStrategy;

    if let Some(dir) = std::env::var_os(cabin_env::CABIN_CACHE_DIR).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    if let Some(home) = std::env::var_os(cabin_env::CABIN_CACHE_HOME).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    let strategy = etcetera::choose_base_strategy()
        .context("no cache directory: set --cache-dir, CABIN_CACHE_DIR, or CABIN_CACHE_HOME")?;
    Ok(strategy.cache_dir().join("cabin"))
}

/// The `cabin` binary next to this executable (both land in the same
/// cargo target directory).
fn default_cabin_path() -> Result<PathBuf> {
    let current = std::env::current_exe().context("resolving the tool's own path")?;
    let dir = current
        .parent()
        .context("the tool's path has no parent directory")?;
    Ok(dir.join(format!("cabin{}", std::env::consts::EXE_SUFFIX)))
}
