//! `cabin remove` - remove a dependency from a `cabin.toml` manifest.
//!
//! Deletes a `[dependencies]` (or, with `--dev`, `[dev-dependencies]`)
//! entry by name, leaving the rest of the manifest - comments,
//! ordering, unrelated tables - untouched.  If the table becomes empty
//! it is removed too, matching `cargo remove`.

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Args;

use cabin_manifest::edit::{self, DepTable};

use crate::cli::term_verbosity::Reporter;

#[derive(Debug, Args)]
pub(crate) struct RemoveArgs {
    /// Dependency (package name) to remove.
    #[arg(value_name = "DEP")]
    pub dep: String,

    /// Remove from `[dev-dependencies]` instead of `[dependencies]`.
    #[arg(long)]
    pub dev: bool,

    /// Path to the cabin.toml manifest.  Defaults to the manifest
    /// discovered from the current directory.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Workspace package-selection flags.  Inside a workspace, pass a
    /// single `--package <name>` to choose which member's manifest to
    /// edit.
    #[command(flatten)]
    pub workspace_selection: super::WorkspaceSelectionArgs,
}

pub(crate) fn remove(args: &RemoveArgs, reporter: Reporter) -> Result<()> {
    let invocation = super::resolve_invocation_manifest(args.manifest_path.as_deref())?;
    let (manifest_path, _, _) =
        super::select_single_package_manifest(&invocation, &args.workspace_selection, "remove")?
            .into_parts();
    let mut doc = super::manifest_edit::read_document(&manifest_path)?;

    let table = if args.dev {
        DepTable::Dev
    } else {
        DepTable::Normal
    };
    let table_label = table.header();

    if !edit::remove_dependency(&mut doc, table, &args.dep) {
        bail!(
            "the dependency `{}` could not be found in `{table_label}`",
            args.dep
        );
    }

    super::manifest_edit::write_document(&manifest_path, &doc)?;

    reporter.status("Removing", format_args!("{} from {table_label}", args.dep));
    Ok(())
}
