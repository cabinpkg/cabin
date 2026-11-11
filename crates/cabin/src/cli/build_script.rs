//! Running a prepared workspace's build scripts before its build.
//!
//! A script's executable is a unit of the build context, and what the
//! script reports changes the plan of the units it prepares.  So the
//! scripts run in steps.  Each step plans the whole build with what the
//! scripts before it reported, has Ninja build only its own scripts'
//! executables and the programs those may run, and then runs its
//! scripts, or takes what one reported last time while that still holds
//! ([`cabin_build_script::load`]).  The build itself is the plan after
//! the last step.
//!
//! A step's Ninja file is the whole build's, not the build context's
//! alone: when Ninja compacts its log of header dependencies, it drops
//! those of every output the file it was given does not build.  The
//! sources a script yet to run generates are no part of a step's plan,
//! so after such a compaction they compile once more.

use anyhow::{Context as _, Result};
use camino::{Utf8Path, Utf8PathBuf};

use super::build_prep::{
    BuildScriptOutputs, PreparedWorkspace, plan_build_context, plan_target_context,
};
use super::ninja::{self, Locks};
use crate::cli::term_verbosity::Reporter;

/// Run `scripts`, the build scripts of `prepared` the build needs, and
/// return what they reported with the locks they ran under, which the
/// build goes on under; `None` when there is no script to run.
/// `selected` and `check` are the build's
/// ([`super::build_prep::build_prepared`]).
pub(crate) fn run(
    prepared: &PreparedWorkspace,
    scripts: &[cabin_build::BuildScript],
    selected: Option<&Vec<cabin_build::ManifestTargetSelector>>,
    check: bool,
    jobs: Option<cabin_core::BuildJobs>,
    reporter: Reporter,
) -> Result<(BuildScriptOutputs, Option<Locks>)> {
    let mut outputs = BuildScriptOutputs::default();
    let mut locks = None;
    let Some(context) = &prepared.build_context else {
        return Ok((outputs, locks));
    };
    let inherited: Vec<_> = std::env::vars_os().collect();
    // What a script is told of the context it prepares units of.
    let side = |script: &cabin_build::BuildScript| {
        if script.build_context {
            (
                &context.enabled_features,
                &context.detection_report,
                &context.platform,
            )
        } else {
            (
                &prepared.enabled_features,
                &prepared.detection_report,
                &prepared.platform,
            )
        }
    };
    let profile_dir = cabin_build::layout::profile_dir(&prepared.build_dir, &prepared.profile.name);
    let build_root = utf8(&prepared.build_root)?;

    // No executable links or runs a target-context unit, so the scripts
    // preparing those take one step together.
    // shortcut: each script preparing build-context units is a step of
    // its own, a plan and a Ninja run, since a later script may link
    // what it prepares; group them by depth when a build has many.
    let (build_machine, target) =
        scripts.split_at(scripts.partition_point(|script| script.build_context));
    let steps = build_machine
        .chunks(1)
        .chain([target])
        .filter(|step| !step.is_empty());
    for step in steps {
        let context_graph = plan_build_context(prepared, context, scripts, &outputs[1])?;
        // The plan builds each script and every executable one may run.
        let built = |package: usize, target: &str| {
            let package = prepared.graph.packages[package].package.name.as_str();
            context_graph.target_outputs[&(package.to_owned(), target.to_owned())].clone()
        };
        let mut runs = Vec::new();
        for script in step {
            let inputs = cabin_build::build_script_inputs(
                &prepared.graph,
                script.package,
                side(script).2,
                &context.enabled_features,
            )?;
            let tools: Vec<_> = inputs
                .tools
                .into_iter()
                .map(|(package, tool)| {
                    let path = built(package, &tool);
                    (tool, path)
                })
                .collect();
            runs.push((script, built(script.package, script.name()), tools));
        }

        let graph = match plan_target_context(prepared, selected.cloned(), check, &outputs[0]) {
            Ok(mut graph) => {
                graph.append(context_graph);
                graph
            }
            // A target whose every source a script generates plans only
            // once that script has run.
            Err(error)
                if matches!(
                    error.downcast_ref(),
                    Some(cabin_build::BuildError::EmptyTargetSources(_))
                ) =>
            {
                context_graph
            }
            Err(error) => return Err(error),
        };
        // Writing a Ninja file relies on a plan free of the standards
        // violations its planning recorded.
        cabin_build::validate_planned_standards(&graph)?;
        let request = prepared.ninja_request(&graph, jobs, reporter);
        if locks.is_none() {
            locks = Some(ninja::lock(&request)?);
        }
        let targets: Vec<&Utf8Path> = runs
            .iter()
            .flat_map(|(_, executable, tools)| {
                std::iter::once(executable).chain(tools.iter().map(|(_, tool)| tool))
            })
            .map(Utf8PathBuf::as_path)
            .collect();
        ninja::build_targets(&request, &targets)?;

        let empty = std::collections::BTreeSet::new();
        for (script, executable, tools) in &runs {
            let package = &prepared.graph.packages[script.package];
            let name = package.package.name.as_str();
            let (features, detection, platform) = side(script);
            let dir = cabin_build::layout::package_dir(
                &profile_dir,
                script.build_context,
                &package.package.name,
            )
            .join(cabin_build::layout::BUILD_SCRIPT_DIR);
            let run = cabin_build_script::Script {
                package: &package.package,
                features: features.get(&script.package).unwrap_or(&empty),
                root: utf8(&package.manifest_dir)?,
                build_dir: build_root,
                dir: utf8(&dir)?,
                executable,
                profile: prepared.profile.name.as_str(),
                platform,
                msvc: cabin_build::Dialect::from_compiler_kind(detection.lead().identity.kind)
                    == cabin_build::Dialect::Msvc,
                tools,
                inherited: &inherited,
            };
            let report = if let Some(report) = cabin_build_script::load(&run) {
                report
            } else {
                reporter.status("Running", format_args!("{} of {name}", script.name()));
                cabin_build_script::run(&run)
                    .with_context(|| format!("{} of package `{name}`", script.name()))?
            };
            outputs[usize::from(script.build_context)].extend(
                report
                    .into_iter()
                    .map(|(target, output)| ((script.package, target), output)),
            );
        }
    }
    Ok((outputs, locks))
}

fn utf8(path: &std::path::Path) -> Result<&Utf8Path> {
    Ok(path.try_into()?)
}
