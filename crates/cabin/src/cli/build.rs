use super::{BuildArgs, Reporter, Result, profile_descriptor};
use crate::cli::build_prep::{
    DevActivation, WorkspacePipelineArgs, build_prepared, prepare_workspace,
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
            build_context: !matches!(mode, BuildMode::Check),
            install: false,
        },
        reporter,
    )?;
    // `build_prepared` emits the shared "wrote …" / "invoking …"
    // verbose lines.
    reporter.verbose(format_args!(
        "cabin: profile = {}",
        prepared.profile.name.as_str()
    ));
    reporter.verbose(format_args!(
        "cabin: build dir = {}",
        prepared.build_dir.display()
    ));
    if let Some(cxx) = &prepared.toolchain.cxx {
        reporter.verbose(format_args!("cabin: c++ compiler = {}", cxx.path));
    }
    if let Some(cc) = &prepared.toolchain.cc {
        reporter.very_verbose(format_args!("cabin: c compiler = {}", cc.path));
    }
    reporter.very_verbose(format_args!(
        "cabin: archiver = {}",
        prepared.toolchain.ar.path
    ));

    let jobs = crate::cli::config::resolve_build_jobs(args.jobs, &prepared.effective_config)?;
    let (_, built) = build_prepared(
        &prepared,
        None,
        matches!(mode, BuildMode::Check),
        color,
        jobs,
        reporter,
    )?;

    reporter.status(
        "Finished",
        format_args!(
            "`{}` profile [{}] target(s) in {:.2}s",
            prepared.profile.name.as_str(),
            profile_descriptor(&prepared.profile),
            built.elapsed.as_secs_f64(),
        ),
    );

    Ok(())
}
