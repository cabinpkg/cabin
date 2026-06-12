//! Shared build-configuration preamble for the commands that resolve
//! a [`cabin_core::BuildConfiguration`] per package: `build`, `run`,
//! `test`, and `cabin explain build-config`.
//!
//! Each of those commands derives, from an already-resolved toolchain
//! and profile, the per-package build flags and the compiler-cache
//! wrapper, then folds them into a [`cabin_core::ToolchainSummary`].
//! [`resolve_build_prep`] is the single home for that fail-hard
//! sequence so a change to how those inputs are assembled lands in one
//! place. The caller keeps its own `resolve_build_configurations` call
//! (it needs the per-command package selection, which is computed at
//! the call site) and threads [`BuildPrep`] into it.
//!
//! Scope: this owns only the part *after* the caller has resolved (and,
//! for the building commands, detected / validated) the toolchain and
//! chosen the profile — those steps differ per command and stay at the
//! call sites. `cabin metadata` is intentionally not a caller: it uses
//! the fail-*soft* wrapper path (`resolve_compiler_wrapper`) and must
//! keep it.

use std::collections::{BTreeSet, HashMap};

use anyhow::Result;

use crate::cli::term_verbosity::Reporter;

/// Inputs to [`resolve_build_prep`]. The caller supplies the
/// already-resolved toolchain and profile; the helper owns the flag /
/// wrapper / summary derivation.
pub(crate) struct BuildConfigInputs<'a> {
    pub graph: &'a cabin_workspace::PackageGraph,
    pub host_platform: &'a cabin_core::TargetPlatform,
    pub toolchain: &'a cabin_core::ResolvedToolchain,
    /// Toolchain detection report. `Some` for the building commands
    /// (fail-hard detection already ran); `None` only when a
    /// fail-soft caller could not detect — compiler cfg conditions
    /// then evaluate as `unknown`.
    pub detection: Option<&'a cabin_core::ToolchainDetectionReport>,
    /// `--compiler-wrapper` / `--no-compiler-wrapper` override, already
    /// parsed from the command's toolchain args.
    pub cli_compiler_wrapper: Option<cabin_core::CompilerWrapperRequest>,
    pub manifest_compiler_wrapper: &'a cabin_core::CompilerWrapperManifestSettings,
    pub effective_config: &'a cabin_config::EffectiveConfig,
    pub profile: &'a cabin_core::ResolvedProfile,
    pub dev_for: &'a BTreeSet<String>,
    /// Resolved features for the selected closure. Gates each
    /// package's `[target.'cfg(feature = "...")'.profile]` flag
    /// layers; must be computed before this preamble so the build
    /// flags observe the selected feature set.
    pub feature_resolution: &'a cabin_feature::FeatureResolution,
    pub reporter: Reporter,
}

/// The resolved per-package flags, compiler-cache wrapper, and the
/// toolchain summary they fold into. The caller feeds
/// `toolchain_summary` + `build_flags` into
/// `resolve_build_configurations`, and threads `build_flags` +
/// `compiler_wrapper` into the planner.
pub(crate) struct BuildPrep {
    pub build_flags: HashMap<usize, cabin_core::ResolvedProfileFlags>,
    /// Standard-flag conflict candidates per package, detected on
    /// the pre-augmentation manifest flags. Threaded into
    /// `PlanRequest` so the planner can record violations for the
    /// compiles each candidate's scope actually covers.
    pub standard_flag_conflicts: HashMap<usize, Vec<cabin_core::StandardFlagConflict>>,
    pub compiler_wrapper: Option<cabin_core::ResolvedCompilerWrapper>,
    pub toolchain_summary: cabin_core::ToolchainSummary,
}

/// Resolve per-package build flags and the compiler-cache wrapper, and
/// fold them into a [`cabin_core::ToolchainSummary`].
///
/// Flag augmentation may emit reporter warnings; it runs at the same
/// point the inlined preamble ran (before the caller's package
/// selection / `resolve_build_configurations`), so the surfaced output
/// is unchanged. Wrapper resolution is silent on success and fatal on
/// failure (a misbehaving wrapper never silently bypasses caching).
#[allow(clippy::needless_pass_by_value)] // consumed: `cli_compiler_wrapper` is moved into the wrapper resolver
pub(crate) fn resolve_build_prep(inputs: BuildConfigInputs) -> Result<BuildPrep> {
    let (build_flags, standard_flag_conflicts) = crate::cli::resolve_per_package_build_flags(
        inputs.graph,
        inputs.profile.build.as_ref(),
        inputs.host_platform,
        inputs.feature_resolution,
        inputs.detection,
    );
    let build_flags = crate::cli::augment_build_flags(
        inputs.graph,
        inputs.host_platform,
        inputs.dev_for,
        build_flags,
        inputs.reporter,
    )?;
    let compiler_wrapper = crate::cli::resolve_compiler_wrapper_layered(
        inputs.cli_compiler_wrapper,
        inputs.manifest_compiler_wrapper,
        inputs.effective_config,
        inputs.host_platform,
    )?;
    let toolchain_summary = cabin_core::ToolchainSummary::from_resolved_parts(
        inputs.toolchain,
        compiler_wrapper.as_ref(),
    );
    Ok(BuildPrep {
        build_flags,
        standard_flag_conflicts,
        compiler_wrapper,
        toolchain_summary,
    })
}
