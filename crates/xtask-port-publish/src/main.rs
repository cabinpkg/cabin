//! `xtask-port-publish` — repository tool that publishes the curated
//! foundation ports as `cabin-ports/<name>` registry packages.  See the library crate for the pipeline; this shim owns
//! argument parsing and default resolution.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Parser;
use clap::error::ErrorKind;
use xtask_port_publish::{Mode, Options, run};

const USAGE: &str = "\
usage: xtask-port-publish (--dry-run | --publish --index-url <URL>) [options]

Publishes the committed foundation ports as `cabin-ports/<name>` registry
packages, each from the `cabin.toml` committed in it, verbatim.  Both
modes run the complete local preflight
(materialize, package, publish into a temporary file registry, build
every port against it in publication order); --publish then uploads
every package through the registry API.

options:
  --index-url <URL>   registry index URL (required with --publish)
  --ports-dir <PATH>  ports directory (default: the repository's
                      ports)
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

/// The flags, with the mode pair validated after parsing: the two
/// refusals below are the tool's own wording, which its tests pin.
// `args_override_self`: a repeated flag keeps its last value, as the
// parser this replaces did, so a wrapper may supply a default and
// override it.
#[derive(Parser)]
#[command(args_override_self = true)]
struct Cli {
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    publish: bool,
    #[arg(long)]
    index_url: Option<String>,
    #[arg(long)]
    ports_dir: Option<PathBuf>,
    #[arg(long)]
    cache_dir: Option<PathBuf>,
    #[arg(long)]
    work_dir: Option<PathBuf>,
    #[arg(long)]
    cabin: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // `--help` answers on stdout; everything else is a refusal,
        // which exits 1 rather than clap's 2.
        Err(err) if err.kind() == ErrorKind::DisplayHelp => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            let _ = err.print();
            return ExitCode::FAILURE;
        }
    };
    match options(cli).and_then(|options| run_with_workdir_notice(&options)) {
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
    let mode = match (cli.dry_run, cli.publish) {
        (true, false) => {
            if cli.index_url.is_some() {
                bail!("--index-url only applies to --publish");
            }
            Mode::DryRun
        }
        (false, true) => Mode::Publish {
            index_url: cli
                .index_url
                .ok_or_else(|| anyhow::anyhow!("--publish requires --index-url"))?,
        },
        _ => bail!("pass exactly one of --dry-run or --publish\n\n{USAGE}"),
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
