//! Command-line shim for the dist packaging steps; the steps
//! themselves live in the library.

use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};
use xtask_dist::checksums;
use xtask_dist::package;

/// Release packaging steps for .github/workflows/dist.yml, run from a
/// workflow step through their Cargo aliases.
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
    /// Write <archive>.sha256 and sha256.sum for every release archive
    /// in the working directory (`cargo dist-checksums`).
    Checksums,
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
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// `checksums` owns its status outright: its refusal is its own
/// sentence rather than this shim's `error:` rendering.
fn run(command: &Command) -> Result<ExitCode> {
    match command {
        Command::Package(args) => package::run(&args.into()).map(|()| ExitCode::SUCCESS),
        Command::Checksums => Ok(checksums::run()),
    }
}
