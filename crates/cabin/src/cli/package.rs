use super::{
    Context, PackageArgs, Path, PathBuf, PublishArgs, Reporter, ResolveFormat, Result, absolutise,
    bail, resolve_invocation_manifest, select_single_package_manifest,
};

use cabin_core::{ExperimentalFeature, ExperimentalFeatures};

pub(super) fn package(args: &PackageArgs, _reporter: Reporter) -> Result<()> {
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;
    let (manifest_path, resolved_project, workspace_dep_requirements) =
        select_single_package_manifest(&manifest_path, &args.workspace_selection, "package")?
            .into_parts();
    let output_dir = absolutise(&args.output_dir)
        .with_context(|| format!("failed to resolve {}", args.output_dir.display()))?;
    let artifact = cabin_package::package_with_project(
        cabin_package::PackageRequest {
            manifest_path: &manifest_path,
            output_dir: &output_dir,
        },
        resolved_project,
        &workspace_dep_requirements,
    )?;
    emit_package_output(&artifact, args.format)?;
    Ok(())
}

pub(super) fn publish(
    args: &PublishArgs,
    reporter: Reporter,
    features: &ExperimentalFeatures,
) -> Result<()> {
    // `--output-dir` is for the staging-only `dist/` flow; combining
    // it with `--registry-dir` is meaningless and almost always
    // means the user picked the wrong flag, so refuse loudly.
    if args.output_dir.is_some() && args.registry_dir.is_some() {
        bail!("--output-dir is not compatible with --registry-dir; pick one");
    }
    // The `--index-url` flag is remote-registry surface: presence
    // without the feature is an error even on the (entirely local)
    // dry-run path, matching how the registry `config.json` fields
    // gate on presence rather than being silently ignored.
    if args.index_url.is_some() && !features.is_enabled(ExperimentalFeature::RemoteRegistry) {
        bail!(cabin_core::registry::remote_registry_field_error(
            "cabin publish --index-url"
        ));
    }

    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;
    let (manifest_path, resolved_project, workspace_dep_requirements) =
        select_single_package_manifest(&manifest_path, &args.workspace_selection, "publish")?
            .into_parts();

    match (args.registry_dir.as_deref(), args.dry_run) {
        (Some(registry_dir), true) => {
            let registry_dir = absolutise(registry_dir)
                .with_context(|| format!("failed to resolve {}", registry_dir.display()))?;
            let report = cabin_publish::dry_run_against_file_registry(
                cabin_publish::RegistryPublishWorkflow {
                    manifest_path: &manifest_path,
                    registry_dir: &registry_dir,
                    resolved_project,
                    workspace_dep_requirements,
                },
            )?;
            emit_registry_publish_output(&report, args.format, reporter)?;
        }
        (Some(registry_dir), false) => {
            let registry_dir = absolutise(registry_dir)
                .with_context(|| format!("failed to resolve {}", registry_dir.display()))?;
            let report =
                cabin_publish::publish_to_file_registry(cabin_publish::RegistryPublishWorkflow {
                    manifest_path: &manifest_path,
                    registry_dir: &registry_dir,
                    resolved_project,
                    workspace_dep_requirements,
                })?;
            emit_registry_publish_output(&report, args.format, reporter)?;
        }
        (None, true) => {
            let output_dir = args
                .output_dir
                .clone()
                .unwrap_or_else(|| PathBuf::from("dist"));
            let output_dir = absolutise(&output_dir)
                .with_context(|| format!("failed to resolve {}", output_dir.display()))?;
            let report = cabin_publish::dry_run(cabin_publish::DryRunRequest {
                manifest_path: &manifest_path,
                output_dir: &output_dir,
                resolved_project,
                workspace_dep_requirements,
            })?;
            emit_dry_run_output(&report, args.format, reporter)?;
        }
        (None, false) => {
            // `--output-dir` belongs to the dry-run staging flow.  A
            // non-dry-run invocation must not silently ignore it -
            // with a config-supplied `index-url` that would turn an
            // intended local staging run into a real remote publish.
            if args.output_dir.is_some() {
                return Err(cabin_publish::PublishError::DryRunRequired.into());
            }
            // Publishing without a local registry targets the
            // effective HTTP index source, when one is configured;
            // anything else keeps the file-registry error path.
            let Some(index_url) =
                effective_publish_index_url(args.index_url.as_deref(), &manifest_path)?
            else {
                return Err(cabin_publish::PublishError::DryRunRequired.into());
            };
            if !features.is_enabled(ExperimentalFeature::RemoteRegistry) {
                bail!(cabin_core::registry::remote_registry_field_error(
                    "cabin publish --index-url"
                ));
            }
            let report = publish_to_remote_registry(
                &index_url,
                &manifest_path,
                resolved_project,
                &workspace_dep_requirements,
                reporter,
                features,
            )?;
            emit_remote_publish_output(&report, args.format, reporter)?;
        }
    }
    Ok(())
}

/// Resolve the index source a registry-less `cabin publish` targets:
/// the `--index-url` flag (which skips config discovery entirely,
/// like `cabin login`), else the config-supplied registry source,
/// with `[source-replacement]` applied so the publish goes to the
/// origin a later fetch would actually contact.  Returns `None` when
/// the effective source is absent or a local path.
fn effective_publish_index_url(
    cli_index_url: Option<&str>,
    manifest_path: &Path,
) -> Result<Option<String>> {
    let config = if cli_index_url.is_some() {
        cabin_config::EffectiveConfig::default()
    } else {
        crate::cli::config::load_effective_config_for_manifest(manifest_path)?
    };
    let Some(source) = crate::cli::config::resolve_index_source(None, cli_index_url, &config)?
    else {
        return Ok(None);
    };
    let locator = crate::cli::config::index_source_kind_to_locator(&source.kind);
    let resolution = crate::cli::patch::apply_source_replacement(locator, &config, false)?;
    match resolution.resolved {
        cabin_core::SourceLocator::IndexPath { .. } => Ok(None),
        cabin_core::SourceLocator::IndexUrl { url } => Ok(Some(url)),
    }
}

/// What the remote publish flow did, for the CLI report.
struct RemotePublishReport {
    name: cabin_core::PackageName,
    version: semver::Version,
    /// Normalized index origin the publish targeted.
    registry: String,
    checksum: String,
    /// `true` on a `201` (version created); `false` on the
    /// idempotent `200` no-op for byte-identical re-publishes.
    created: bool,
    /// The response's optional `"verification"` field: `"pending"` on
    /// a registry with the asynchronous verification lifecycle, `None`
    /// on one without it.
    verification: Option<String>,
    warnings: Vec<String>,
}

/// Publish to a remote registry (`-Z remote-registry`): run the exact
/// staging pipeline the local file-registry publish runs - same
/// validation, same publish lints, same deterministic archive and
/// canonical per-version metadata document - then upload the framed
/// bytes to the API origin the registry's `config.json` declares.
///
/// The registry's `config.json` and the lint baseline ride the
/// authenticated sparse-HTTP read path; the upload itself goes
/// through `cabin-registry-api` with the same credential.
fn publish_to_remote_registry(
    index_url: &str,
    manifest_path: &Path,
    resolved_project: Option<cabin_core::Package>,
    workspace_dep_requirements: &cabin_core::WorkspaceDepRequirements,
    reporter: Reporter,
    features: &ExperimentalFeatures,
) -> Result<RemotePublishReport> {
    // Stage before touching the network so validation failures never
    // need a connection.
    let staged = cabin_package::stage_with_project(
        manifest_path,
        resolved_project,
        None,
        workspace_dep_requirements,
    )?;
    // Registry packages are always `<scope>/<name>`: fail a bare name
    // here, before credentials, index reads, or the API call.
    cabin_publish::require_scoped_name(&staged.name, manifest_path)?;

    // One credential lookup serves the reads and the API call alike.
    let origin = cabin_credentials::normalize_origin(index_url)?;
    let lookup = cabin_credentials::lookup_token(&origin)?;
    if let Some(warning) = lookup.permissions_warning {
        reporter.warning(format_args!("{warning}"));
    }
    let token = lookup.token;
    let mut client = cabin_index_http::HttpClient::new();
    if let Some(token) = token.clone() {
        client = client.with_auth(cabin_index_http::RegistryAuth::for_index_url(
            index_url, token,
        )?);
    }
    let index = cabin_index_http::HttpIndex::open_with_features(index_url, client, features)?;
    let Some(api) = index.api() else {
        bail!(
            "registry `{origin}` does not declare an `api` URL in its config.json; publishing \
             needs one to locate the registry API origin"
        );
    };

    // The PL3 baseline is the registry's own view of the already-
    // published versions; a package the registry does not know yet
    // simply has an empty baseline (first publish).
    let published = match index.fetch_package(&staged.name) {
        Ok(entry) => entry
            .versions
            .into_iter()
            .map(|(version, meta)| (version, meta.standards))
            .collect(),
        Err(cabin_index_http::IndexHttpError::PackageNotFound { .. }) => Vec::new(),
        Err(err) => return Err(err.into()),
    };
    let warnings = cabin_publish::staged_lint_warnings(&staged, &published)?;

    let metadata_json = cabin_package::metadata::render_canonical_json(&staged.metadata)?;
    let api_client = cabin_registry_api::RegistryApi::new(api, token)?;
    let receipt = api_client.publish(
        staged.name.as_str(),
        &staged.version,
        metadata_json.as_bytes(),
        &staged.archive_bytes,
    )?;
    Ok(RemotePublishReport {
        name: staged.name,
        version: staged.version,
        registry: origin,
        checksum: staged.checksum,
        created: matches!(receipt.outcome, cabin_registry_api::PublishOutcome::Created),
        verification: receipt.verification,
        warnings,
    })
}

fn emit_remote_publish_output(
    report: &RemotePublishReport,
    format: ResolveFormat,
    reporter: Reporter,
) -> Result<()> {
    match format {
        ResolveFormat::Human => {
            print_remote_publish_human(report);
            print_lint_warnings(reporter, &report.warnings);
            Ok(())
        }
        ResolveFormat::Json => print_remote_publish_json(report),
    }
}

fn print_remote_publish_human(report: &RemotePublishReport) {
    if report.created {
        println!(
            "Published {} {} to {}",
            report.name.as_str(),
            report.version,
            report.registry
        );
    } else {
        // Mirror the local flows' "re-running with identical input
        // succeeds" semantics: the bytes are already there, so the
        // run reports the no-op and exits successfully.
        println!(
            "{} {} is already published to {} with identical bytes; nothing to do",
            report.name.as_str(),
            report.version,
            report.registry
        );
    }
    println!("  checksum: {}", report.checksum);
    // A registry with the asynchronous verification lifecycle accepts
    // the upload as pending; say when it becomes resolvable.
    if report.verification.as_deref() == Some("pending") {
        println!(
            "  verification: pending (the version was accepted and becomes resolvable \
             after verification, typically within a few minutes)"
        );
    }
}

fn print_remote_publish_json(report: &RemotePublishReport) -> Result<()> {
    let value = serde_json::json!({
        "published": true,
        "no_op": !report.created,
        "name": report.name.as_str(),
        "version": report.version.to_string(),
        "registry": report.registry,
        "checksum": report.checksum,
        "verification": report.verification,
        "warnings": report.warnings,
    });
    crate::print_pretty_json(&value, "failed to serialize publish output as JSON")
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
    reporter: Reporter,
) -> Result<()> {
    match format {
        ResolveFormat::Human => {
            print_dry_run_human(report);
            print_lint_warnings(reporter, &report.warnings);
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
    if report.standards_check_skipped {
        println!("Patch-release requirement check (PL3) skipped: no registry to compare against.");
    }
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
        "warnings": report.warnings,
        "standards_check_skipped": report.standards_check_skipped,
    });
    crate::print_pretty_json(&value, "failed to serialize publish dry-run output as JSON")
}

pub(super) fn emit_registry_publish_output(
    report: &cabin_publish::RegistryPublishReport,
    format: ResolveFormat,
    reporter: Reporter,
) -> Result<()> {
    match format {
        ResolveFormat::Human => {
            print_registry_publish_human(report);
            print_lint_warnings(reporter, &report.warnings);
            Ok(())
        }
        ResolveFormat::Json => print_registry_publish_json(report),
    }
}

/// Print non-rejecting standard-compatibility lint warnings (PL2, PL3)
/// through the reporter's stderr warning channel, one per line, so
/// human-mode stdout stays the report and CI logs still capture the
/// advice.
fn print_lint_warnings(reporter: Reporter, warnings: &[String]) {
    for message in warnings {
        reporter.warning(format_args!("{message}"));
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
        "warnings": report.warnings,
    });
    crate::print_pretty_json(&value, "failed to serialize publish output as JSON")
}
