//! `xtask-port-publish` — repository tool that publishes the curated
//! foundation ports as `cabin-ports/<name>` registry packages.  See the library crate for the pipeline; this shim owns
//! argument parsing and default resolution.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{ArgGroup, Parser};
use xtask_port_publish::{Mode, Options, run};

/// Publishes the committed foundation ports as `cabin-ports/<name>`
/// registry packages, each from the `cabin.toml` committed in it,
/// verbatim.  Both modes run the complete local preflight; --publish
/// then uploads every package through the registry API.
#[derive(Parser)]
#[command(group(ArgGroup::new("mode").required(true)))]
struct Cli {
    #[arg(long, group = "mode")]
    dry_run: bool,
    #[arg(long, group = "mode", requires = "index_url")]
    publish: bool,
    /// Registry index URL (required with --publish).
    // `conflicts_with`, not `requires = "publish"`: a `SetTrue` flag
    // always carries its defaulted `false`, which satisfies `requires`
    // even when the flag was never passed. The pairing still holds:
    // the URL forbids --dry-run, the required group then forces
    // --publish, and --publish requires the URL back.
    #[arg(long, conflicts_with = "dry_run")]
    index_url: Option<String>,
    /// Ports directory (default: the repository's ports).
    #[arg(long)]
    ports_dir: Option<PathBuf>,
    /// Cabin cache root for upstream archives (default:
    /// `CABIN_CACHE_DIR`, `CABIN_CACHE_HOME`, or the platform cache
    /// directory + /cabin).
    #[arg(long)]
    cache_dir: Option<PathBuf>,
    /// Scratch directory; the tool owns (and clears) exactly run/
    /// beneath it (default: a per-run directory under the system temp
    /// dir; kept on failure).
    #[arg(long)]
    work_dir: Option<PathBuf>,
    /// Cabin binary for preflight builds (default: a sibling of this
    /// executable).
    #[arg(long)]
    cabin: Option<PathBuf>,
}

fn main() -> ExitCode {
    match options(Cli::parse()).and_then(|options| run_with_workdir_notice(&options)) {
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

fn options(cli: Cli) -> Result<Options> {
    // Clap enforces the pairing: `--index-url` is present exactly when
    // `--publish` is.
    let mode = match cli.index_url {
        Some(index_url) => Mode::Publish { index_url },
        None => Mode::DryRun,
    };

    Ok(Options {
        mode,
        ports_dir: match cli.ports_dir {
            Some(dir) => dir,
            None => default_ports_dir(),
        },
        cache_dir: match cli.cache_dir {
            Some(dir) => dir,
            None => default_cache_dir()?,
        },
        work_dir: match cli.work_dir {
            Some(dir) => dir,
            None => std::env::temp_dir().join(format!("xtask-port-publish-{}", std::process::id())),
        },
        cabin: match cli.cabin {
            Some(path) => path,
            None => default_cabin_path()?,
        },
    })
}

/// The repository's ports directory, resolved from this crate's
/// compile-time location.  The tool is repository-owned (`publish =
/// false`), so the baked path is valid wherever the tool itself can
/// be built.
fn default_ports_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
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
