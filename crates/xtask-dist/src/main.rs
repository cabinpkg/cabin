//! Command-line shim for the dist packaging step; the step itself
//! lives in the library.

use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};
use xtask_dist::{check, package};

/// Release packaging steps for .github/workflows/dist.yml, run from a
/// workflow step through its Cargo alias.
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Stage the release binary, README.md and LICENSE, archive them,
    /// and print the archive's path (`cargo dist-package`).
    Package(PackageArgs),
    /// Unpack a packaged archive and run the binary inside it
    /// (`cargo dist-check`).
    Check(CheckArgs),
}

/// The archive `package` printed and the run's ref, as the workflow
/// step forwards them.
#[derive(clap::Parser)]
struct CheckArgs {
    /// The archive path `package` printed.
    archive: std::path::PathBuf,
    /// `$GITHUB_REF_NAME`, which a tag build requires to be the
    /// workspace version.
    #[arg(long)]
    ref_name: String,
    /// `$GITHUB_REF_TYPE`.
    #[arg(long, default_value = "")]
    ref_type: String,
}

/// The flags the workflow step passes, which this shim translates into
/// the library's own [`package::Arguments`].  Each may be empty: the
/// step forwards `$GITHUB_*` values, and a set-but-empty one is a
/// value.
#[derive(clap::Parser)]
struct PackageArgs {
    /// The target triple the release binary was built for.
    #[arg(long)]
    target: String,
    /// `$GITHUB_REF_NAME`, the version for a tag build.
    #[arg(long)]
    ref_name: String,
    /// `$GITHUB_REF_TYPE`; anything but `tag` versions the package
    /// `dev-<SHA[..12]>`.
    #[arg(long, default_value = "")]
    ref_type: String,
    /// `$GITHUB_SHA`.
    #[arg(long)]
    sha: String,
}

impl From<&PackageArgs> for package::Arguments {
    fn from(args: &PackageArgs) -> Self {
        Self {
            target: args.target.clone(),
            ref_name: args.ref_name.clone(),
            ref_type: args.ref_type.clone(),
            sha: args.sha.clone(),
        }
    }
}

fn main() -> ExitCode {
    match run(&Cli::parse().command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: &Command) -> Result<()> {
    match command {
        Command::Package(args) => package::run(&args.into()),
        Command::Check(args) => check::run(&args.archive, &args.ref_name, &args.ref_type),
    }
}
