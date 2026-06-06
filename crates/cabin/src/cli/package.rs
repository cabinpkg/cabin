use super::{
    Context, PackageArgs, PathBuf, PublishArgs, Reporter, ResolveFormat, Result, absolutise, bail,
    resolve_invocation_manifest, select_single_package_manifest,
};

pub(super) fn package(args: &PackageArgs, _reporter: Reporter) -> Result<()> {
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;
    let target =
        select_single_package_manifest(&manifest_path, &args.workspace_selection, "package")?;
    let output_dir = absolutise(&args.output_dir)
        .with_context(|| format!("failed to resolve {}", args.output_dir.display()))?;
    let artifact = cabin_package::package_with_project(
        cabin_package::PackageRequest {
            manifest_path: &target.manifest_path,
            output_dir: &output_dir,
        },
        target.resolved_project,
    )?;
    emit_package_output(&artifact, args.format)?;
    Ok(())
}

pub(super) fn publish(args: &PublishArgs, _reporter: Reporter) -> Result<()> {
    // `--output-dir` is for the staging-only `dist/` flow; combining
    // it with `--registry-dir` is meaningless and almost always
    // means the user picked the wrong flag, so refuse loudly.
    if args.output_dir.is_some() && args.registry_dir.is_some() {
        bail!("--output-dir is not compatible with --registry-dir; pick one");
    }

    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;
    let target =
        select_single_package_manifest(&manifest_path, &args.workspace_selection, "publish")?;

    match (args.registry_dir.as_deref(), args.dry_run) {
        (Some(registry_dir), true) => {
            let registry_dir = absolutise(registry_dir)
                .with_context(|| format!("failed to resolve {}", registry_dir.display()))?;
            let report = cabin_publish::dry_run_against_file_registry(
                cabin_publish::RegistryPublishWorkflow {
                    manifest_path: &target.manifest_path,
                    registry_dir: &registry_dir,
                    resolved_project: target.resolved_project.clone(),
                },
            )?;
            emit_registry_publish_output(&report, args.format)?;
        }
        (Some(registry_dir), false) => {
            let registry_dir = absolutise(registry_dir)
                .with_context(|| format!("failed to resolve {}", registry_dir.display()))?;
            let report =
                cabin_publish::publish_to_file_registry(cabin_publish::RegistryPublishWorkflow {
                    manifest_path: &target.manifest_path,
                    registry_dir: &registry_dir,
                    resolved_project: target.resolved_project.clone(),
                })?;
            emit_registry_publish_output(&report, args.format)?;
        }
        (None, true) => {
            let output_dir = args
                .output_dir
                .clone()
                .unwrap_or_else(|| PathBuf::from("dist"));
            let output_dir = absolutise(&output_dir)
                .with_context(|| format!("failed to resolve {}", output_dir.display()))?;
            let report = cabin_publish::dry_run(cabin_publish::DryRunRequest {
                manifest_path: &target.manifest_path,
                output_dir: &output_dir,
                resolved_project: target.resolved_project.clone(),
            })?;
            emit_dry_run_output(&report, args.format)?;
        }
        (None, false) => {
            return Err(cabin_publish::PublishError::DryRunRequired.into());
        }
    }
    Ok(())
}

pub(super) fn emit_package_output(
    artifact: &cabin_package::PackagedArtifact,
    format: ResolveFormat,
) -> Result<()> {
    match format {
        ResolveFormat::Human => {
            print_package_human(artifact);
            Ok(())
        }
        ResolveFormat::Json => print_package_json(artifact),
    }
}

pub(super) fn print_package_human(artifact: &cabin_package::PackagedArtifact) {
    println!("Packaged {} {}", artifact.name.as_str(), artifact.version);
    println!("  archive: {}", artifact.archive_path.display());
    println!("  metadata: {}", artifact.metadata_path.display());
    println!("  checksum: {}", artifact.checksum);
}

pub(super) fn print_package_json(artifact: &cabin_package::PackagedArtifact) -> Result<()> {
    let value = serde_json::json!({
        "name": artifact.name.as_str(),
        "version": artifact.version.to_string(),
        "archive_path": artifact.archive_path,
        "metadata_path": artifact.metadata_path,
        "checksum": artifact.checksum,
    });
    crate::print_pretty_json(&value, "failed to serialize package output as JSON")
}

pub(super) fn emit_dry_run_output(
    report: &cabin_publish::DryRunReport,
    format: ResolveFormat,
) -> Result<()> {
    match format {
        ResolveFormat::Human => {
            print_dry_run_human(report);
            Ok(())
        }
        ResolveFormat::Json => print_dry_run_json(report),
    }
}

pub(super) fn print_dry_run_human(report: &cabin_publish::DryRunReport) {
    println!(
        "Publish dry-run for {} {}",
        report.name.as_str(),
        report.version
    );
    println!();
    println!("Generated:");
    println!("  archive: {}", report.archive_path.display());
    println!("  metadata: {}", report.metadata_path.display());
    println!("  checksum: {}", report.checksum);
    println!();
    println!("This was a dry run. No registry was modified.");
}

pub(super) fn print_dry_run_json(report: &cabin_publish::DryRunReport) -> Result<()> {
    let value = serde_json::json!({
        "dry_run": true,
        "name": report.name.as_str(),
        "version": report.version.to_string(),
        "archive_path": report.archive_path,
        "metadata_path": report.metadata_path,
        "checksum": report.checksum,
        "registry_modified": report.registry_modified,
    });
    crate::print_pretty_json(&value, "failed to serialize publish dry-run output as JSON")
}

pub(super) fn emit_registry_publish_output(
    report: &cabin_publish::RegistryPublishReport,
    format: ResolveFormat,
) -> Result<()> {
    match format {
        ResolveFormat::Human => {
            print_registry_publish_human(report);
            Ok(())
        }
        ResolveFormat::Json => print_registry_publish_json(report),
    }
}

pub(super) fn print_registry_publish_human(report: &cabin_publish::RegistryPublishReport) {
    if report.dry_run {
        println!(
            "Publish dry-run for {} {} against file registry",
            report.name.as_str(),
            report.version
        );
    } else {
        println!(
            "Published {} {} to file registry",
            report.name.as_str(),
            report.version
        );
    }
    println!("  registry: {}", report.registry_dir.display());
    println!("  package index: {}", report.package_index_path.display());
    println!("  artifact: {}", report.artifact_path.display());
    println!("  checksum: {}", report.checksum);
    if report.dry_run {
        println!();
        if report.registry_initialized {
            println!("Registry would be initialized at this path.");
        }
        println!("This was a dry run. No registry was modified.");
    } else if report.registry_initialized {
        println!();
        println!("Registry was initialized at this path.");
    }
}

pub(super) fn print_registry_publish_json(
    report: &cabin_publish::RegistryPublishReport,
) -> Result<()> {
    let value = serde_json::json!({
        "published": !report.dry_run,
        "dry_run": report.dry_run,
        "name": report.name.as_str(),
        "version": report.version.to_string(),
        "registry_dir": report.registry_dir,
        "package_index_path": report.package_index_path,
        "artifact_path": report.artifact_path,
        "checksum": report.checksum,
        "source_path": report.source_path,
        "registry_modified": report.registry_modified,
        "registry_initialized": report.registry_initialized,
    });
    crate::print_pretty_json(&value, "failed to serialize publish output as JSON")
}
