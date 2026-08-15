//! Repository tool that publishes the curated foundation ports
//! (`ports/`) as ordinary registry packages under
//! the `cabin-ports` scope.  Every committed port is a package
//! directory, published verbatim.
//!
//! The tool has exactly two modes, both of which run the complete
//! local preflight (materialize every port, publish it into a
//! temporary file registry, and build every port against the
//! generated packages in publication order):
//!
//! - `--dry-run` stops after the preflight;
//! - `--publish` additionally uploads every package through the
//!   existing remote registry client, relying on the registry's
//!   byte-identical idempotency instead of skipping versions (the
//!   public index hides pending versions, so it cannot be used to
//!   decide what is already published).
//!
//! The committed tree is an input only: every port materializes into
//! a scratch directory, never in place.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

pub mod plan;
pub mod preflight;
pub mod remote;

/// What to do after the local preflight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Stop after the complete local preflight.
    DryRun,
    /// Preflight, then upload every package to the registry whose
    /// sparse index lives at this URL.
    Publish {
        /// Registry index URL (the publish API origin comes from its
        /// `config.json`).
        index_url: String,
    },
}

/// Resolved tool invocation.
#[derive(Debug)]
pub struct Options {
    pub mode: Mode,
    /// `ports/` directory to publish.
    pub ports_dir: PathBuf,
    /// Cabin cache root (upstream archives are reused from and
    /// cached into `<cache>/ports`).
    pub cache_dir: PathBuf,
    /// Scratch root for the staged packages, the temporary registry,
    /// and preflight build outputs.
    pub work_dir: PathBuf,
    /// `cabin` binary for the preflight builds.
    pub cabin: PathBuf,
}

/// Run the tool.
///
/// # Errors
/// Returns the first conversion, preflight, or upload failure.
pub fn run(options: &Options) -> Result<()> {
    if !options.ports_dir.is_dir() {
        bail!(
            "ports directory {} does not exist; pass --ports-dir",
            options.ports_dir.display()
        );
    }
    if !options.cabin.is_file() {
        bail!(
            "cabin binary not found at {}; build it first (`cargo build -p cabinpkg`) or pass \
             --cabin",
            options.cabin.display()
        );
    }
    std::fs::create_dir_all(&options.work_dir)
        .with_context(|| format!("creating {}", options.work_dir.display()))?;

    let conversions = plan::load_conversions(&options.ports_dir)?;
    println!(
        "loaded {} ports; publication order: {}",
        conversions.len(),
        conversions
            .iter()
            .map(|c| format!("{} {}", c.scoped_name.as_str(), c.published_version))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let report = preflight::preflight(&preflight::PreflightRequest {
        conversions: &conversions,
        cache_dir: &options.cache_dir,
        work_dir: &options.work_dir,
        cabin: &options.cabin,
    })?;

    match &options.mode {
        Mode::DryRun => {
            println!(
                "dry run complete: {} packages staged in {} and built against it",
                conversions.len(),
                report.registry_dir.display()
            );
        }
        Mode::Publish { index_url } => {
            let package_dirs: Vec<&std::path::Path> =
                report.package_dirs.iter().map(PathBuf::as_path).collect();
            remote::publish_all(&package_dirs, index_url, &options.cabin)?;
            println!("published {} packages", conversions.len());
        }
    }
    Ok(())
}
