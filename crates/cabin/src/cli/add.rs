//! `cabin add` - add a dependency to a `cabin.toml` manifest.
//!
//! v1 supports two dependency kinds: bundled foundation ports
//! (`--port <name>`) and local path dependencies (`--path <dir>`).
//! Bare registry names are rejected until Cabin has a hosted registry.
//! The manifest is edited format-preservingly via
//! [`cabin_manifest::edit`], and status output mirrors `cargo add`'s
//! visible lines (`Adding <name> v<ver> to dependencies`).  After a
//! successful add it also prints a note reminding the user that a
//! `[dependencies]` entry only declares the dep - each target's `deps`
//! list is what links it (Cabin-specific; unlike Cargo).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use semver::VersionReq;

use cabin_manifest::edit::{self, DepTable, NewDependency};

use crate::cli::term_verbosity::Reporter;

#[derive(Debug, Args)]
pub(crate) struct AddArgs {
    /// Dependency to add, as `<NAME>` or `<NAME>@<REQ>`.
    ///
    /// Required with `--port`.  With `--path`, the name is optional and
    /// defaults to the depended-on package's own name.
    #[arg(value_name = "DEP")]
    pub dep: Option<String>,

    /// Add a bundled foundation-port dependency (`port = true`).
    ///
    /// The version is resolved from the bundled recipe set: without an
    /// explicit `@<REQ>`, the newest bundled version is pinned with a
    /// caret requirement.
    #[arg(long, conflicts_with = "path")]
    pub port: bool,

    /// Add a local path dependency rooted at <PATH> (relative to the
    /// manifest being edited).
    #[arg(long, value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Add the entry to `[dev-dependencies]` instead of
    /// `[dependencies]`.
    #[arg(long)]
    pub dev: bool,

    /// Features to enable on the dependency.  Repeatable and/or
    /// comma-separated (`--features a,b`).
    #[arg(long, value_name = "FEATURES")]
    pub features: Vec<String>,

    /// Disable the dependency's default features
    /// (`default-features = false`).
    #[arg(long)]
    pub no_default_features: bool,

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

/// The version detail rendered in the `Adding …` status line.
enum AddStatus {
    /// A concrete version (foundation ports): `Adding zlib v1.3.1 …`.
    Version(String),
    /// A local path dependency: `Adding mylib (local) …`.
    Local,
}

pub(crate) fn add(args: &AddArgs, reporter: Reporter) -> Result<()> {
    let invocation = super::resolve_invocation_manifest(args.manifest_path.as_deref())?;
    let (manifest_path, _, _) =
        super::select_single_package_manifest(&invocation, &args.workspace_selection, "add")?
            .into_parts();
    let mut doc = super::manifest_edit::read_document(&manifest_path)?;
    if doc.get("package").is_none() {
        bail!(
            "{} is not a package manifest; `cabin add` needs a manifest with a [package] table",
            manifest_path.display()
        );
    }

    let table = if args.dev {
        DepTable::Dev
    } else {
        DepTable::Normal
    };
    let features = split_features(&args.features);

    let (dep, status) = if let Some(path) = &args.path {
        if args.dep.is_some() {
            bail!(
                "do not pass a dependency name with `--path`; the name is taken from the package \
                 at that path (Cabin does not support renamed dependencies)"
            );
        }
        build_path_dependency(path, &manifest_path, features, args.no_default_features)?
    } else if args.port {
        build_port_dependency(args, features)?
    } else {
        bail!(
            "registry dependencies are not supported yet; use `cabin add --port <name>` to add a \
             foundation port, or `cabin add --path <path>` for a local package"
        );
    };

    edit::upsert_dependency(&mut doc, table, &dep)?;
    super::manifest_edit::write_document(&manifest_path, &doc)?;

    let table_label = table.header();
    let name = &dep.name;
    match status {
        AddStatus::Version(version) => {
            reporter.status("Adding", format_args!("{name} v{version} to {table_label}"));
        }
        AddStatus::Local => {
            reporter.status("Adding", format_args!("{name} (local) to {table_label}"));
        }
    }
    // Declaring the dependency does not link it: in Cabin each target's
    // `deps` list is what pulls a dependency in.  Remind the
    // user of that follow-up step (mirrors the linker-error diagnostic
    // in `cabin-build`).
    reporter.note(format_args!(
        "`[{table_label}]` makes `{name}` available; add `{name}` to a target's `deps` to link \
         it, e.g. `deps = [\"{name}\"]`"
    ));
    Ok(())
}

/// Resolve a `--port` dependency: look the name up in the bundled
/// recipe set, pick the requirement to write, and report the concrete
/// version selected.
fn build_port_dependency(
    args: &AddArgs,
    features: Vec<String>,
) -> Result<(NewDependency, AddStatus)> {
    let spec = args
        .dep
        .as_deref()
        .context("`cabin add --port` requires a port name")?;
    let (name, req) = split_dep_spec(spec);

    let (version_to_write, resolved) = if let Some(req_str) = req {
        let req = parse_req(name, req_str)?;
        let port = cabin_port::builtin::lookup(name, &req).ok_or_else(|| {
            anyhow::anyhow!("no bundled foundation port `{name}` matches `{req_str}`")
        })?;
        (req_str.to_owned(), port.version.to_owned())
    } else {
        let port = cabin_port::builtin::lookup(name, &VersionReq::STAR)
            .ok_or_else(|| anyhow::anyhow!("no bundled foundation port named `{name}`"))?;
        (format!("^{}", port.version), port.version.to_owned())
    };

    let dep = NewDependency {
        name: name.to_owned(),
        version: Some(version_to_write),
        port: true,
        path: None,
        features,
        no_default_features: args.no_default_features,
    };
    Ok((dep, AddStatus::Version(resolved)))
}

/// Resolve a `--path` dependency.  The dependency key is always the
/// depended-on package's own name (Cabin requires the manifest key to
/// match the package name - there are no renamed/aliased deps), so the
/// target manifest is read to derive it, which also validates that the
/// path points at a real package.  The path is written as supplied; it
/// is interpreted relative to the edited manifest.
fn build_path_dependency(
    path: &Path,
    manifest_path: &Path,
    features: Vec<String>,
    no_default_features: bool,
) -> Result<(NewDependency, AddStatus)> {
    // The written value must be UTF-8 TOML; reject non-UTF-8 paths
    // rather than lossily mangling them into the manifest.
    let path_str = camino::Utf8Path::from_path(path)
        .with_context(|| format!("--path `{}` must be valid UTF-8", path.display()))?
        .as_str()
        .to_owned();

    let manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let dep_manifest = manifest_dir.join(path).join(super::MANIFEST_FILENAME);
    let parsed = cabin_manifest::load_manifest(&dep_manifest)
        .with_context(|| format!("failed to read the package at {}", dep_manifest.display()))?;
    let package = parsed.package.with_context(|| {
        format!(
            "{} has no [package] table; `--path` must point at a package",
            dep_manifest.display()
        )
    })?;

    let dep = NewDependency {
        name: package.name.as_str().to_owned(),
        version: None,
        port: false,
        path: Some(path_str),
        features,
        no_default_features,
    };
    Ok((dep, AddStatus::Local))
}

/// Split a `<name>@<req>` dependency spec; `<name>` alone yields no
/// requirement.
fn split_dep_spec(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once('@') {
        Some((name, req)) => (name, Some(req)),
        None => (spec, None),
    }
}

/// Parse a version requirement the same lenient way the manifest parser
/// does, so `cabin add foo@1.2` and a hand-written `foo = "1.2"` agree.
fn parse_req(name: &str, raw: &str) -> Result<VersionReq> {
    cabin_core::version_req::parse_lenient(raw)
        .map_err(|err| anyhow::anyhow!("invalid version requirement `{raw}` for `{name}`: {err}"))
}

/// Flatten repeated and comma/space-separated `--features` values into a
/// single list, dropping empty entries.
fn split_features(raw: &[String]) -> Vec<String> {
    raw.iter()
        .flat_map(|value| value.split([',', ' ']))
        .filter(|feature| !feature.is_empty())
        .map(str::to_owned)
        .collect()
}
