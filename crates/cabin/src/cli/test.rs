//! Glue layer for `cabin test`.
//!
//! `cabin test` builds the selected `test` targets through
//! the same pipeline as `cabin build` (workspace load → artifact
//! pipeline → planner → Ninja → invoke ninja), then hands the
//! resulting [`cabin_build::BuildGraph`] to
//! [`cabin_test::run_tests`] which spawns each test executable
//! and reports a deterministic summary.
//!
//! This module owns only the orchestration.  Test planning and
//! test execution live in the dedicated `cabin-test` crate;
//! workspace loading, dependency resolution, build planning, and
//! Ninja generation live in their respective crates.  The CLI
//! layer threads typed values between them.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Args;

use cabin_build::{ManifestTargetSelector, select_targets_of_kind};
use cabin_core::TargetKind;

use crate::cli::build_prep::{
    DevActivation, WorkspacePipelineArgs, plan_prepared, prepare_workspace,
};
use crate::cli::{
    ToolchainSelectionArgs, WorkspaceSelectionArgs, build_workspace_selection,
    resolve_invocation_manifest,
};
use crate::plural;

/// `cabin test` arguments.  Subset of `BuildArgs` plus a few
/// test-specific knobs.  Mutually exclusive flags are enforced by
/// `clap`.
#[derive(Debug, Args)]
pub(crate) struct TestArgs {
    /// Path to the cabin.toml manifest.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Directory for build outputs (build.ninja, object files,
    /// linked test executables).  Defaults to `build`.
    #[arg(long, value_name = "PATH")]
    pub build_dir: Option<PathBuf>,

    /// Build with optimizations.
    ///
    /// Compatibility alias for `--profile release`; cannot be
    /// used together with `--profile`.
    #[arg(short = 'r', long, conflicts_with = "profile")]
    pub release: bool,

    /// Build profile (`dev`, `release`, or any custom profile
    /// declared in `[profile.<name>]`).  Defaults to `dev` -
    /// the same default as `cabin build` so test runs match the
    /// developer's working profile.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Path to a directory containing the local JSON package
    /// index.  Required when the test build closure has any
    /// versioned dependency and `--index-url` is not given.
    #[arg(long, value_name = "PATH")]
    pub index_path: Option<PathBuf>,

    /// Sparse HTTP index URL.
    #[arg(long, value_name = "URL")]
    pub index_url: Option<String>,

    /// Override the default artifact cache directory.
    #[arg(long, value_name = "PATH")]
    pub cache_dir: Option<PathBuf>,

    /// Require an existing, current `cabin.lock`.
    #[arg(long, conflicts_with = "frozen")]
    pub locked: bool,

    /// Like `--locked`, but also rejects state-writing side
    /// effects.
    #[arg(long)]
    pub frozen: bool,

    /// Forbid network access.  Combine with `cabin vendor` to run
    /// `cabin test` against a self-contained local index.
    #[arg(long)]
    pub offline: bool,

    /// Enable named features for the selected packages.
    #[arg(long, value_name = "FEATURES")]
    pub features: Vec<String>,

    /// Enable every feature declared by selected packages.
    #[arg(long)]
    pub all_features: bool,

    /// Disable each selected package's default features.
    #[arg(long)]
    pub no_default_features: bool,

    /// Workspace package-selection flags.
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgs,

    /// Toolchain-selection flags.
    #[command(flatten)]
    pub toolchain: ToolchainSelectionArgs,

    /// Disable every active patch and source-replacement entry
    /// for this invocation.
    #[arg(long)]
    pub no_patches: bool,

    /// Run only the named `test` target; may be repeated.
    ///
    /// Each name must match a `test` target declared by a
    /// selected package; a name that does not is an error.
    /// Every match across the selected packages runs, so
    /// workspace members may share a test name.
    #[arg(long = "test", value_name = "NAME")]
    pub test: Vec<String>,

    /// Exit successfully when the selected packages declare no
    /// `test` targets.  By default, an empty selection errors
    /// so CI does not silently pass when tests have not been
    /// declared yet.
    #[arg(long)]
    pub allow_no_tests: bool,
}

/// Run `cabin test`: build the selected `test` targets,
/// invoke each linked executable in deterministic order, and
/// print a summary.  Exits non-zero on any test failure.
pub(crate) fn test(
    args: &TestArgs,
    reporter: crate::cli::term_verbosity::Reporter,
    color: cabin_core::ColorChoice,
    experimental_features: &cabin_core::ExperimentalFeatures,
) -> Result<()> {
    // `--allow-no-tests` succeeds without building anything, so an
    // empty test selection must not activate dev deps at all - not
    // even the dev-aware port discovery in the shared pipeline,
    // which would fail on a missing dev path dep or download dev
    // ports for a run that builds nothing.  Targets are
    // manifest-level, so a ports-free, dev-blind skeleton
    // enumerates them exactly like the final strict graph will.
    // `--test <NAME>` is excluded: an unknown name must keep
    // erroring even under `--allow-no-tests`, and that validation
    // lives on the fully loaded pipeline path.
    if args.allow_no_tests && args.test.is_empty() {
        // The env-driven offline fallback stays validated on this
        // fast path too: a malformed CABIN_NET_OFFLINE must fail
        // the run even when there is nothing to build.
        crate::cli::config::effective_offline(args.offline)?;
        let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;
        let workspace_selection = build_workspace_selection(&args.workspace_selection);
        let skeleton = cabin_workspace::load_workspace_skip_ports(&manifest_path)?;
        let skeleton_selection =
            cabin_workspace::resolve_package_selection(&skeleton, &workspace_selection)?;
        if select_targets_of_kind(
            &skeleton,
            Some(&skeleton_selection.packages),
            TargetKind::Test,
        )
        .is_empty()
        {
            println!("cabin test: no test targets found");
            return Ok(());
        }
    }

    let prepared = prepare_workspace(
        &WorkspacePipelineArgs {
            manifest_path: args.manifest_path.as_deref(),
            offline: args.offline,
            cache_dir: args.cache_dir.as_deref(),
            build_dir: args.build_dir.as_deref(),
            locked: args.locked,
            frozen: args.frozen,
            no_patches: args.no_patches,
            features: &args.features,
            all_features: args.all_features,
            no_default_features: args.no_default_features,
            index_path: args.index_path.as_deref(),
            index_url: args.index_url.as_deref(),
            profile: args.profile.as_deref(),
            release: args.release,
            workspace_selection: &args.workspace_selection,
            toolchain: &args.toolchain,
            dev: DevActivation::SelectedPrimaries,
        },
        reporter,
        experimental_features,
    )?;

    // Build every test target in the selected packages, narrowed
    // to the requested names when `--test` is given (`--target`
    // stays reserved for a platform/toolchain target).  The
    // deselected count feeds the summary's `filtered out` field.
    let all_test_selectors: Vec<ManifestTargetSelector> = select_targets_of_kind(
        &prepared.graph,
        Some(&prepared.resolved_selection.packages),
        TargetKind::Test,
    );
    let total_test_targets = all_test_selectors.len();
    let test_selectors: Vec<ManifestTargetSelector> = if args.test.is_empty() {
        // Enumeration skips feature-gated tests (they count as
        // filtered out below).  A test named via `--test` is an
        // explicit request instead: it stays selected here and the
        // planner hard-errors with the missing features.
        all_test_selectors
            .iter()
            .filter(|sel| {
                cabin_build::selector_required_features_met(
                    sel,
                    &prepared.graph,
                    &prepared.enabled_features,
                )
            })
            .cloned()
            .collect()
    } else {
        select_named_test_targets(
            &prepared.graph,
            &prepared.resolved_selection.packages,
            &all_test_selectors,
            &args.test,
        )?
    };
    let filtered_out = total_test_targets - test_selectors.len();

    if test_selectors.is_empty() {
        // Distinguish "no tests declared" from "every declared test
        // is feature-gated": the latter must name the gate, not
        // suggest declaring a test target.
        if filtered_out > 0 {
            let gated: Vec<String> = all_test_selectors
                .iter()
                .map(|sel| {
                    format!(
                        "{}:{}",
                        sel.package.as_deref().unwrap_or_default(),
                        sel.name
                    )
                })
                .collect();
            if args.allow_no_tests {
                println!(
                    "cabin test: no runnable test targets ({filtered_out} filtered out by required-features)"
                );
                return Ok(());
            }
            bail!(
                "every test target in the selected packages requires features that are not enabled ({}); enable them with `--features <name>`, or run `cabin test --test <name>` to see a target's missing features",
                gated.join(", ")
            );
        }
        if args.allow_no_tests {
            println!("cabin test: no test targets found");
            return Ok(());
        }
        bail!(
            "no test targets found in the selected packages; declare a `test` target or pass `--allow-no-tests`"
        );
    }

    let plan_graph = plan_prepared(&prepared, Some(test_selectors), false, color)?;

    // `cabin test` builds with Ninja's default parallelism (no
    // `-j`) and prints no `Finished` banner - the test summary is
    // its completion signal - so the returned build duration is
    // unused here.
    crate::cli::ninja::invoke_ninja_and_report(&crate::cli::ninja::NinjaInvocationRequest {
        build_dir: &prepared.build_dir,
        profile: &prepared.profile,
        plan_graph: &plan_graph,
        graph: &prepared.graph,
        toolchain: &prepared.toolchain,
        cxx_kind: prepared.detection_report.cxx.identity.kind,
        feature_resolution: &prepared.feature_resolution,
        dev_for: &prepared.dev_for,
        ninja: &prepared.ninja,
        jobs: None,
        reporter,
    })?;

    // Build → run hand-off.  The plan builder reads `test`
    // targets out of the graph and aligns them with the
    // `default_outputs` the planner emitted, so empty
    // `default_outputs` produce a clear error rather than a
    // silent no-op.
    let mut test_plan = cabin_test::plan_tests(
        &prepared.graph,
        &plan_graph,
        Some(&prepared.resolved_selection.packages),
    );
    populate_test_env_overlay(
        &mut test_plan,
        &prepared.graph,
        &prepared.profile,
        &prepared.build_dir,
    )?;
    if test_plan.is_empty() {
        if args.allow_no_tests {
            println!("cabin test: no test targets found");
            return Ok(());
        }
        bail!("no test targets were produced by the build graph; pass `--allow-no-tests` to skip");
    }

    // Status lines mirror `cargo test`: a blank line, the
    // `running N tests` header, one `test <pkg>:<target> ... ok`
    // line per executable as it finishes (emitted live by the
    // streaming sink), and a blank-line-separated epilogue with
    // the optional `failures:` recap plus the summary line.
    let mut sink = cabin_test::StreamingSink {
        stdout: std::io::stdout().lock(),
        stderr: std::io::stderr().lock(),
    };
    println!(
        "\nrunning {} test{}",
        test_plan.len(),
        plural(test_plan.len())
    );
    let summary = cabin_test::run_tests(&test_plan, &mut sink)?;
    let _ = writeln!(
        sink.stdout,
        "{}",
        cabin_test::render_epilogue(&summary, filtered_out)
    );
    if !summary.all_passed() {
        bail!(
            "test failures: {} of {} test executables failed",
            summary.failed(),
            summary.total()
        );
    }
    Ok(())
}

/// Resolve the `--test <NAME>` selection: narrow `all` (the full
/// `test`-target enumeration for the selected packages) to the
/// requested names.  Every requested name must match a `test`
/// target declared by a selected package; every match across
/// those packages is kept, so two workspace members may run a
/// same-named test in one invocation.  Diagnostics mirror
/// `cabin run --bin`: an unknown name and a name that only
/// matches another target kind get distinct messages.
fn select_named_test_targets(
    graph: &cabin_workspace::PackageGraph,
    selected_packages: &[usize],
    all: &[ManifestTargetSelector],
    names: &[String],
) -> Result<Vec<ManifestTargetSelector>> {
    // A `BTreeSet` dedupes repeated `--test` names and keeps the
    // validation order deterministic.
    let requested: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    for &name in &requested {
        if all.iter().any(|sel| sel.name == name) {
            continue;
        }
        // Walk every selected package before deciding which
        // diagnostic to emit: the kind-mismatch message must only
        // fire when the name exists somewhere with another kind.
        let other_kind = selected_packages
            .iter()
            .flat_map(|&idx| graph.packages[idx].package.targets.iter())
            .find(|t| t.name.as_str() == name)
            .map(|t| t.kind);
        if let Some(kind) = other_kind {
            bail!(
                "--test `{name}` matched a target of kind `{}`; expected `test`",
                kind.as_str()
            );
        }
        bail!("--test `{name}` was not found in the selected packages");
    }
    Ok(all
        .iter()
        .filter(|sel| requested.contains(sel.name.as_str()))
        .cloned()
        .collect())
}

/// Walk every executable in `plan` and attach the typed
/// `CABIN_*` package-execution overlay produced by
/// [`cabin_env::package_env`].  The overlay is layered on top of
/// the inherited environment at runtime; PATH and friends remain
/// intact so test executables can still find shared system
/// tools.  The only fallible step is mapping each executable back
/// to its workspace package.
fn populate_test_env_overlay(
    plan: &mut cabin_test::TestPlan,
    graph: &cabin_workspace::PackageGraph,
    profile: &cabin_core::ResolvedProfile,
    build_dir: &std::path::Path,
) -> Result<()> {
    let mut failure = None;
    plan.for_each_executable_mut(|exe| {
        if failure.is_some() {
            return;
        }
        let Some(idx) = graph.index_of(exe.package.as_str()) else {
            failure = Some(anyhow::anyhow!(
                "failed to build test env for `{}:{}`: package is not present in the workspace graph",
                exe.package,
                exe.target
            ));
            return;
        };
        let pkg = &graph.packages[idx];
        exe.env = cabin_env::package_env(&cabin_env::PackageEnvInputs {
            manifest_dir: pkg.manifest_dir.as_path(),
            manifest_path: pkg.manifest_path.as_path(),
            package_name: pkg.package.name.as_str(),
            package_version: &pkg.package.version.to_string(),
            profile: profile.name.as_str(),
            build_dir,
        });
    });
    if let Some(err) = failure {
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use cabin_build::BuildGraph;
    use cabin_core::{
        Package, PackageName, ProfileDefinition, ProfileName, ProfileSelection, Target, TargetKind,
        TargetName, resolve_profile,
    };
    use cabin_workspace::{PackageGraph, PackageKind, WorkspacePackage};
    use camino::Utf8PathBuf;

    fn dev_profile() -> cabin_core::ResolvedProfile {
        resolve_profile(
            &ProfileSelection::default_dev(),
            &BTreeMap::<ProfileName, ProfileDefinition>::new(),
        )
        .expect("built-in dev profile resolves")
    }

    fn test_graph() -> PackageGraph {
        let target = Target {
            name: TargetName::new("demo_test").unwrap(),
            kind: TargetKind::Test,
            sources: Vec::new(),
            include_dirs: Vec::new(),
            defines: Vec::new(),
            deps: Vec::new(),
            required_features: Vec::new(),
            language: Default::default(),
        };
        let package = Package::new(
            PackageName::new("demo").unwrap(),
            semver::Version::parse("0.1.0").unwrap(),
            vec![target],
            Vec::new(),
        )
        .unwrap();
        PackageGraph {
            root_manifest_path: PathBuf::from("demo/cabin.toml"),
            root_dir: PathBuf::from("demo"),
            is_workspace_root: false,
            root_package: Some(0),
            root_settings: Default::default(),
            primary_packages: vec![0],
            default_members: vec![0],
            excluded_members: Vec::new(),
            packages: vec![WorkspacePackage {
                package,
                manifest_path: PathBuf::from("demo/cabin.toml"),
                manifest_dir: PathBuf::from("demo"),
                deps: Vec::new(),
                kind: PackageKind::Local,
                is_port: false,
            }],
        }
    }

    /// Two-package graph for `--test` selection tests: `alpha`
    /// declares a library plus two tests (one name shared with
    /// `beta`); `beta` declares the shared test plus an
    /// executable.
    fn named_selection_graph() -> PackageGraph {
        let target = |name: &str, kind: TargetKind| Target {
            name: TargetName::new(name).unwrap(),
            kind,
            sources: Vec::new(),
            include_dirs: Vec::new(),
            defines: Vec::new(),
            deps: Vec::new(),
            required_features: Vec::new(),
            language: Default::default(),
        };
        let alpha = Package::new(
            PackageName::new("alpha").unwrap(),
            semver::Version::parse("0.1.0").unwrap(),
            vec![
                target("alpha", TargetKind::Library),
                target("alpha_test", TargetKind::Test),
                target("shared_test", TargetKind::Test),
            ],
            Vec::new(),
        )
        .unwrap();
        let beta = Package::new(
            PackageName::new("beta").unwrap(),
            semver::Version::parse("0.1.0").unwrap(),
            vec![
                target("shared_test", TargetKind::Test),
                target("beta_tool", TargetKind::Executable),
            ],
            Vec::new(),
        )
        .unwrap();
        let member = |package: Package, dir: &str| WorkspacePackage {
            package,
            manifest_path: PathBuf::from(format!("{dir}/cabin.toml")),
            manifest_dir: PathBuf::from(dir),
            deps: Vec::new(),
            kind: PackageKind::Local,
            is_port: false,
        };
        PackageGraph {
            root_manifest_path: PathBuf::from("ws/cabin.toml"),
            root_dir: PathBuf::from("ws"),
            is_workspace_root: true,
            root_package: None,
            root_settings: Default::default(),
            primary_packages: vec![0, 1],
            default_members: vec![0, 1],
            excluded_members: Vec::new(),
            packages: vec![member(alpha, "ws/alpha"), member(beta, "ws/beta")],
        }
    }

    /// Enumerate the selection's `test` targets the way the
    /// command does, then apply the `--test` name filter.
    fn named_selection(
        graph: &PackageGraph,
        selected: &[usize],
        names: &[String],
    ) -> Result<Vec<ManifestTargetSelector>> {
        let all = select_targets_of_kind(graph, Some(selected), TargetKind::Test);
        select_named_test_targets(graph, selected, &all, names)
    }

    #[test]
    fn named_selection_keeps_every_match_across_packages() {
        let graph = named_selection_graph();
        let selectors = named_selection(&graph, &[0, 1], &["shared_test".to_owned()]).unwrap();
        let got: Vec<(Option<&str>, &str)> = selectors
            .iter()
            .map(|s| (s.package.as_deref(), s.name.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (Some("alpha"), "shared_test"),
                (Some("beta"), "shared_test")
            ]
        );
    }

    #[test]
    fn named_selection_filters_to_requested_names_and_dedupes() {
        let graph = named_selection_graph();
        let selectors = named_selection(
            &graph,
            &[0, 1],
            &["alpha_test".to_owned(), "alpha_test".to_owned()],
        )
        .unwrap();
        assert_eq!(selectors.len(), 1);
        assert_eq!(selectors[0].package.as_deref(), Some("alpha"));
        assert_eq!(selectors[0].name, "alpha_test");
    }

    #[test]
    fn named_selection_unknown_name_errors() {
        let graph = named_selection_graph();
        let err = named_selection(&graph, &[0, 1], &["missing_test".to_owned()])
            .expect_err("unknown name must error");
        assert_eq!(
            err.to_string(),
            "--test `missing_test` was not found in the selected packages"
        );
    }

    #[test]
    fn named_selection_kind_mismatch_errors() {
        let graph = named_selection_graph();
        let err = named_selection(&graph, &[0, 1], &["beta_tool".to_owned()])
            .expect_err("non-test target kind must error");
        assert_eq!(
            err.to_string(),
            "--test `beta_tool` matched a target of kind `executable`; expected `test`"
        );
    }

    #[test]
    fn named_selection_respects_package_selection() {
        let graph = named_selection_graph();
        // `alpha_test` exists in the graph but not in the selected
        // package set; the lookup must not see it.
        let err = named_selection(&graph, &[1], &["alpha_test".to_owned()])
            .expect_err("name outside the selected packages must error");
        assert_eq!(
            err.to_string(),
            "--test `alpha_test` was not found in the selected packages"
        );
    }

    #[test]
    fn populate_test_env_overlay_errors_when_package_missing_from_graph() {
        let graph = test_graph();
        let build_graph = BuildGraph {
            actions: Vec::new(),
            dialect: cabin_build::Dialect::GnuLike,
            default_outputs: vec![Utf8PathBuf::from("build/dev/packages/demo/demo_test")],
            compile_commands: Vec::new(),
            standard_violations: Vec::new(),
            standard_compat_violations: Vec::new(),
            planned_packages: BTreeSet::default(),
        };
        let mut plan = cabin_test::plan_tests(&graph, &build_graph, Some(&[0]));
        assert_eq!(plan.len(), 1);
        // Detach the executable from any workspace package so the
        // only remaining fallible step (graph lookup) trips.
        plan.for_each_executable_mut(|exe| exe.package.clear());

        let err = populate_test_env_overlay(&mut plan, &graph, &dev_profile(), Path::new("build"))
            .expect_err("an executable with no owning package must be surfaced");

        assert!(
            err.to_string().contains("failed to build test env"),
            "unexpected error: {err}"
        );
    }
}
