use super::{BuildArgs, Reporter, Result, profile_descriptor};
use crate::cli::build_prep::{
    DevActivation, WorkspacePipelineArgs, plan_prepared, prepare_workspace,
};

/// Whether [`build`] produces real artifacts (`cabin build`) or only
/// syntax-checks the selected workspace sources (`cabin check`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BuildMode {
    Build,
    Check,
}

pub(super) fn build(
    args: &BuildArgs,
    reporter: Reporter,
    mode: BuildMode,
    color: cabin_core::ColorChoice,
    experimental_features: &cabin_core::ExperimentalFeatures,
) -> Result<()> {
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
            dev: DevActivation::Disabled,
        },
        reporter,
        experimental_features,
    )?;
    let plan_graph = plan_prepared(&prepared, None, matches!(mode, BuildMode::Check), color)?;

    // Profile-aware Ninja root: `build/<profile>/build.ninja`
    // and `build/<profile>/compile_commands.json`.  Keeps dev /
    // release / custom builds from overwriting each other and
    // matches the per-package output tree the planner emits.
    // Build-specific verbose context (the shared "wrote …" /
    // "invoking …" lines are emitted by `invoke_ninja_and_report`).
    reporter.verbose(format_args!(
        "cabin: profile = {}",
        prepared.profile.name.as_str()
    ));
    reporter.verbose(format_args!(
        "cabin: build dir = {}",
        prepared.build_dir.display()
    ));
    reporter.verbose(format_args!(
        "cabin: c++ compiler = {}",
        prepared.toolchain.cxx.path
    ));
    if let Some(cc) = &prepared.toolchain.cc {
        reporter.very_verbose(format_args!("cabin: c compiler = {}", cc.path));
    }
    reporter.very_verbose(format_args!(
        "cabin: archiver = {}",
        prepared.toolchain.ar.path
    ));

    let jobs = crate::cli::config::resolve_build_jobs(args.jobs, &prepared.effective_config)?;
    let elapsed =
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
            jobs,
            reporter,
        })?;

    // Cargo-style `Finished` summary: profile name, the resolved
    // optimization / debuginfo descriptor, and the wall-clock
    // duration the Ninja invocation took.
    reporter.status(
        "Finished",
        format_args!(
            "`{}` profile [{}] target(s) in {:.2}s",
            prepared.profile.name.as_str(),
            profile_descriptor(&prepared.profile),
            elapsed.as_secs_f64(),
        ),
    );

    Ok(())
}
