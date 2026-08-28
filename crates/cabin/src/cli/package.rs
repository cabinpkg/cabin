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
    // The `--index-url` flag is remote-publish surface: presence
    // without the feature is an error even on the (entirely local)
    // dry-run path - silently ignoring it would let the same command
    // line mean "stage locally" today and "upload" once the feature
    // stabilizes.
    if args.index_url.is_some() && !features.is_enabled(ExperimentalFeature::RemoteRegistry) {
        bail!(cabin_core::registry::remote_registry_command_error(
            "cabin publish --index-url"
        ));
    }

    // Repeated `--manifest-path` names an explicit, ordered batch;
    // the workspace selection flags answer the different question
    // "which member of THIS workspace" and cannot combine with it.
    let selection = &args.workspace_selection;
    if args.manifest_path.len() > 1
        && (selection.workspace
            || selection.default_members
            || !selection.package.is_empty()
            || !selection.exclude.is_empty())
    {
        bail!(
            "repeated `--manifest-path` names an explicit batch of packages; the workspace \
             selection flags (`--workspace`, `--package`, `--default-members`, `--exclude`) are \
             not valid with it"
        );
    }
    // Resolve every named manifest up front, in the order given (the
    // flagless invocation keeps its current-directory resolution), so
    // a bad manifest anywhere in the batch fails before any work.
    let mut selections = Vec::new();
    if args.manifest_path.is_empty() {
        let manifest_path = resolve_invocation_manifest(None)?;
        selections.push(
            select_single_package_manifest(&manifest_path, &args.workspace_selection, "publish")?
                .into_parts(),
        );
    } else {
        for path in &args.manifest_path {
            let manifest_path = resolve_invocation_manifest(Some(path))?;
            selections.push(
                select_single_package_manifest(
                    &manifest_path,
                    &args.workspace_selection,
                    "publish",
                )?
                .into_parts(),
            );
        }
    }

    match (args.registry_dir.as_deref(), args.dry_run) {
        (Some(registry_dir), true) => {
            let registry_dir = absolutise(registry_dir)
                .with_context(|| format!("failed to resolve {}", registry_dir.display()))?;
            for (manifest_path, resolved_project, workspace_dep_requirements) in selections {
                let report = cabin_publish::dry_run_against_file_registry(
                    cabin_publish::RegistryPublishWorkflow {
                        manifest_path: &manifest_path,
                        registry_dir: &registry_dir,
                        resolved_project,
                        workspace_dep_requirements,
                        new_revision: args.new_revision,
                    },
                )
                .with_context(|| format!("dry-running {}", manifest_path.display()))?;
                emit_registry_publish_output(&report, args.format, reporter)?;
            }
        }
        (Some(registry_dir), false) => {
            let registry_dir = absolutise(registry_dir)
                .with_context(|| format!("failed to resolve {}", registry_dir.display()))?;
            for (manifest_path, resolved_project, workspace_dep_requirements) in selections {
                // A failure mid-batch leaves the earlier members
                // published; the context names the member it stopped
                // on (lint errors alone render only target names).
                let report = cabin_publish::publish_to_file_registry(
                    cabin_publish::RegistryPublishWorkflow {
                        manifest_path: &manifest_path,
                        registry_dir: &registry_dir,
                        resolved_project,
                        workspace_dep_requirements,
                        new_revision: args.new_revision,
                    },
                )
                .with_context(|| format!("publishing {}", manifest_path.display()))?;
                emit_registry_publish_output(&report, args.format, reporter)?;
            }
        }
        (None, true) => {
            let output_dir = args
                .output_dir
                .clone()
                .unwrap_or_else(|| PathBuf::from("dist"));
            let output_dir = absolutise(&output_dir)
                .with_context(|| format!("failed to resolve {}", output_dir.display()))?;
            // One shared `--output-dir` for the whole batch: distinct
            // names can flatten to one artifact stem (cabin-package's
            // filename rule), and the staging write fails CLOSED on a
            // byte mismatch - a stem-colliding same-version member
            // refuses, naming the file, rather than clobbering an
            // earlier member; such batches need separate invocations.
            for (manifest_path, resolved_project, workspace_dep_requirements) in selections {
                let report = cabin_publish::dry_run(cabin_publish::DryRunRequest {
                    manifest_path: &manifest_path,
                    output_dir: &output_dir,
                    resolved_project,
                    workspace_dep_requirements,
                })
                .with_context(|| format!("dry-running {}", manifest_path.display()))?;
                emit_dry_run_output(&report, args.format, reporter)?;
            }
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
            // anything else keeps the file-registry error path.  One
            // invocation publishes to one registry: every manifest
            // resolves its own effective config, and the batch must
            // agree ([`effective_batch_index_url`]).
            let Some(index) = effective_batch_index_url(
                args.index_url.as_deref(),
                features.is_enabled(ExperimentalFeature::RemoteRegistry),
                &selections,
            )?
            else {
                return Err(cabin_publish::PublishError::DryRunRequired.into());
            };
            if !features.is_enabled(ExperimentalFeature::RemoteRegistry) {
                bail!(cabin_core::registry::remote_registry_command_error(
                    "cabin publish --index-url"
                ));
            }
            publish_batch_to_remote_registry(
                &index,
                selections,
                args.new_revision,
                args.retry_rate_limits,
                args.format,
                reporter,
            )?;
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
) -> Result<Option<crate::cli::login::EffectiveRegistryIndex>> {
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
    let user_chosen = crate::cli::config::index_origin_user_chosen(&source, &resolution);
    match resolution.resolved {
        cabin_core::SourceLocator::IndexPath { .. } => Ok(None),
        cabin_core::SourceLocator::IndexUrl { url } => {
            Ok(Some(crate::cli::login::EffectiveRegistryIndex {
                url,
                user_chosen,
                from_cli: cli_index_url.is_some(),
            }))
        }
    }
}

/// The one registry a registry-less batch publishes to.  `--index-url`
/// is an explicit whole-batch override (config discovery is skipped,
/// so every manifest resolves identically).  Without it, each
/// manifest's OWN effective config - discovery plus
/// `[source-replacement]` - must name the same remote index URL: a
/// later member must never be published to a registry an earlier
/// member selected.  Agreement is byte equality of the resolved URLs -
/// nothing on this path canonicalizes spelling variants, and guessing
/// that two spellings mean one registry is how bytes land on the
/// wrong one - and the merged origin stays credential-eligible
/// (`user_chosen`) only when it would be for EVERY member alone.
/// Returns `None` - the local-staging error path - only when no
/// member resolves to a remote source; a batch mixing a remote member
/// with a local-or-absent one refuses, naming one member of each
/// kind.  Runs before staging, credential resolution, the OIDC
/// exchange, and every network read, so a refusal here has no remote
/// side effects.  Without `-Z remote-registry` both refusals answer
/// the standard experimental-feature error instead: config-supplied
/// HTTP indexes without the feature always fail with that diagnostic
/// (`docs/remote-registry.md`, "Publishing from the client"), and
/// which agreement refusal a batch would earn is feature-gated
/// detail.
fn effective_batch_index_url(
    cli_index_url: Option<&str>,
    remote_enabled: bool,
    selections: &[(
        PathBuf,
        Option<cabin_core::Package>,
        cabin_core::WorkspaceDepRequirements,
    )],
) -> Result<Option<crate::cli::login::EffectiveRegistryIndex>> {
    if cli_index_url.is_some() {
        return effective_publish_index_url(cli_index_url, &selections[0].0);
    }
    let mut remote: Option<(&Path, crate::cli::login::EffectiveRegistryIndex)> = None;
    let mut unresolved: Option<&Path> = None;
    for (manifest_path, _, _) in selections {
        match effective_publish_index_url(None, manifest_path)? {
            Some(index) => match &mut remote {
                None => remote = Some((manifest_path, index)),
                Some((first_path, first)) => {
                    if first.url != index.url {
                        if !remote_enabled {
                            bail!(cabin_core::registry::remote_registry_command_error(
                                "cabin publish --index-url"
                            ));
                        }
                        bail!(
                            "the batch does not agree on one registry: {} publishes to `{}` but \
                             {} publishes to `{}`; publish them in separate invocations, or pass \
                             an explicit `--index-url` for the whole batch",
                            first_path.display(),
                            first.url,
                            manifest_path.display(),
                            index.url
                        );
                    }
                    first.user_chosen &= index.user_chosen;
                }
            },
            None => unresolved = unresolved.or(Some(manifest_path)),
        }
    }
    match (remote, unresolved) {
        (Some(_), Some(_)) if !remote_enabled => bail!(
            cabin_core::registry::remote_registry_command_error("cabin publish --index-url")
        ),
        (Some((remote_path, _)), Some(unresolved_path)) => bail!(
            "the batch does not agree on one registry: {} publishes to a remote registry but {} \
             resolves no remote index (a local or absent registry source); publish them in \
             separate invocations",
            remote_path.display(),
            unresolved_path.display()
        ),
        (Some((_, index)), None) => Ok(Some(index)),
        (None, _) => Ok(None),
    }
}

/// What the remote publish flow did, for the CLI report.
struct RemotePublishReport {
    name: cabin_core::PackageName,
    version: semver::Version,
    /// Normalized index origin the publish targeted.
    registry: String,
    checksum: cabin_core::Checksum,
    /// `true` on a `201` (version created); `false` on the
    /// idempotent `200` no-op for byte-identical re-publishes.
    created: bool,
    /// The response's optional `"verification"` field: `"pending"` on
    /// a registry with the asynchronous verification lifecycle, `None`
    /// on one without it.
    verification: Option<String>,
    /// The response's optional `"revision"` field: the packaging
    /// revision the archive published under.
    revision: Option<String>,
    warnings: Vec<String>,
}

/// Publish a batch to a remote registry (`-Z remote-registry`): run
/// the exact staging pipeline the local file-registry publish runs -
/// same validation, same publish lints, same deterministic archive
/// and canonical per-version metadata document - for EVERY package
/// first, then upload the framed bytes to the API origin the
/// registry's `config.json` declares, in the order given.  Staging,
/// baselines, and publish lints all complete before the first upload,
/// so a validation failure anywhere in the batch publishes nothing -
/// and, on the trusted-publishing leg, the minted token is spent only
/// on the registry round-trips (the authenticated baseline reads and
/// the uploads), never on the staging work that precedes the
/// exchange.  An UPLOAD failure necessarily leaves the
/// earlier members live (the registry has no cross-package
/// transaction); each package's report is emitted the moment its
/// receipt arrives, so those members' checksums and revisions are
/// never swallowed by a later failure.  One credential serves the
/// whole batch: a trusted-publishing run exchanges exactly one OIDC
/// token however many packages it publishes.
///
/// The registry's `config.json` and the lint baselines ride the
/// authenticated sparse-HTTP read path; the uploads themselves go
/// through `cabin-registry-api` with the same credential, paced on
/// the registry's `429` answers when the batch has more than one
/// package (a serial batch can outrun the per-token publish bucket,
/// and every attempt charges it; a single publish keeps today's
/// fail-fast `429`).
fn publish_batch_to_remote_registry(
    registry: &crate::cli::login::EffectiveRegistryIndex,
    packages: Vec<(
        PathBuf,
        Option<cabin_core::Package>,
        cabin_core::WorkspaceDepRequirements,
    )>,
    new_revision: bool,
    retry_rate_limits: bool,
    format: ResolveFormat,
    reporter: Reporter,
) -> Result<()> {
    let index_url = registry.url.as_str();
    // Stage before touching the network so validation failures never
    // need a connection - the whole batch, before any upload.
    let mut all_staged = Vec::new();
    for (manifest_path, resolved_project, workspace_dep_requirements) in packages {
        let staged = cabin_package::stage_with_project(
            &manifest_path,
            resolved_project,
            None,
            &workspace_dep_requirements,
        )?;
        // Registry packages are always `<scope>/<name>`, and so are
        // the keys of their registry dependency maps: fail a bare
        // name here, before credentials, index reads, or the API
        // call.
        cabin_publish::require_scoped_name(&staged.name, &manifest_path)?;
        cabin_publish::require_scoped_dependency_names(&staged.metadata, &manifest_path)?;
        all_staged.push(staged);
    }

    // One credential resolution serves the reads and the API call
    // alike (`cli::trustpub` defines the precedence).  The
    // trusted-publishing exchange needs the discovered `api` origin,
    // so that leg opens the index tokenless first - fine on the
    // origins the exchange serves, whose config.json is public - and
    // reopens authenticated for the baseline reads below.  The
    // returned guard owns the minted token for exactly this function:
    // dropping it on any exit path best-effort revokes.
    let origin = cabin_credentials::normalize_origin(index_url)?;
    let no_api = || {
        format!(
            "registry `{origin}` does not declare an `api` URL in its config.json; publishing \
             needs one to locate the registry API origin"
        )
    };
    let (exchanged, token, index) = match crate::cli::trustpub::publish_credential(
        index_url,
        &origin,
        registry.user_chosen,
        registry.from_cli,
        reporter,
    )? {
        crate::cli::trustpub::PublishCredential::NeedsExchange => {
            let discovery =
                cabin_index_http::HttpIndex::open(index_url, cabin_index_http::HttpClient::new())?;
            let Some(api) = discovery.api() else {
                bail!(no_api());
            };
            let exchanged = crate::cli::trustpub::exchange(&origin, api)?;
            let token = exchanged.token().clone();
            let client = cabin_index_http::HttpClient::new().with_auth(
                cabin_index_http::RegistryAuth::for_index_url(index_url, token.clone())?,
            );
            (
                Some(exchanged),
                Some(token),
                cabin_index_http::HttpIndex::open(index_url, client)?,
            )
        }
        credential => {
            let token = match credential {
                crate::cli::trustpub::PublishCredential::Token(token) => Some(token),
                crate::cli::trustpub::PublishCredential::None { expired_at } => {
                    // No stored credential (or an expired session):
                    // an interactive terminal is offered the login
                    // flow inline - for a user-chosen registry only -
                    // and the publish proceeds with the fresh
                    // session; otherwise an expired session fails
                    // here with its cause, and a truly absent
                    // credential proceeds tokenless into the
                    // server's own `authentication required` answer.
                    crate::cli::login::offer_interactive_login(
                        index_url,
                        &origin,
                        expired_at.as_deref(),
                        registry.user_chosen,
                        reporter,
                    )?
                }
                crate::cli::trustpub::PublishCredential::NeedsExchange => {
                    unreachable!("handled by the arm above")
                }
            };
            let mut client = cabin_index_http::HttpClient::new();
            if let Some(token) = token.clone() {
                client = client.with_auth(cabin_index_http::RegistryAuth::for_index_url(
                    index_url, token,
                )?);
            }
            (
                None,
                token,
                cabin_index_http::HttpIndex::open(index_url, client)?,
            )
        }
    };
    // The mutation client targets the origin the credential belongs
    // to.  On the exchange leg that is the origin the token was
    // MINTED for - the first config.json read's `api` - never the
    // reopened index's answer: a config.json that changes between the
    // two reads must not route the minted token to a different
    // origin.
    let api = match &exchanged {
        Some(exchanged) => exchanged.api_url(),
        None => match index.api() {
            Some(api) => api,
            None => bail!(no_api()),
        },
    };

    // Baselines and publish lints for the WHOLE batch before the
    // first upload: a lint rejection (or a broken baseline read)
    // anywhere must publish nothing, and only network reads run here.
    let mut checked: Vec<(cabin_package::StagedPackage, String, Vec<String>)> = Vec::new();
    for staged in all_staged {
        // The PL3 baseline is the registry's own view of the already-
        // published versions; a package the registry does not know
        // yet simply has an empty baseline (first publish).
        let mut published: Vec<_> = match index.fetch_package(&staged.name) {
            Ok(entry) => entry
                .versions
                .into_iter()
                .map(|(version, meta)| (version, meta.standards))
                .collect(),
            Err(cabin_index_http::IndexHttpError::PackageNotFound { .. }) => Vec::new(),
            Err(err) => {
                return Err(anyhow::Error::from(err).context(format!(
                    "reading the published baseline for {}",
                    staged.name.as_str()
                )));
            }
        };
        // The ordered batch simulates sequential publishes: a later
        // version's baseline must also include the same-name versions
        // this invocation publishes before it, which the registry
        // cannot know yet.
        published.extend(
            checked
                .iter()
                .filter(|(earlier, _, _)| earlier.name == staged.name)
                .map(|(earlier, _, _)| {
                    (earlier.version.clone(), earlier.metadata.standards.clone())
                }),
        );
        // A lint rejection renders target names, not package names; in
        // a batch only this context says WHICH member to fix.
        // A lint rejection renders target names, not package names; in
        // a batch only this context says WHICH member to fix.
        let warnings = cabin_publish::staged_lint_warnings(&staged, &published)
            .with_context(|| format!("linting {} {}", staged.name.as_str(), staged.version))?;
        let metadata_json = cabin_package::metadata::render_canonical_json(&staged.metadata)?;
        checked.push((staged, metadata_json, warnings));
    }

    // Upload in the order given, reporting each package the moment
    // its receipt arrives: an upload failure later in the batch must
    // not swallow the checksums, revisions, and verification notices
    // of the packages already live on the registry.
    let pace = checked.len() > 1 || retry_rate_limits;
    let api_client = cabin_registry_api::RegistryApi::new(api, token)?;
    for (staged, metadata_json, warnings) in checked {
        let receipt = publish_with_rate_limit_pacing(
            &api_client,
            &staged,
            metadata_json.as_bytes(),
            new_revision,
            pace,
            reporter,
        )
        .with_context(|| format!("publishing {} {}", staged.name.as_str(), staged.version))?;
        let report = RemotePublishReport {
            name: staged.name,
            version: staged.version,
            registry: origin.clone(),
            checksum: staged.checksum,
            created: matches!(receipt.outcome, cabin_registry_api::PublishOutcome::Created),
            verification: receipt.verification,
            revision: receipt.revision,
            warnings,
        };
        emit_remote_publish_output(&report, format, reporter)?;
    }
    Ok(())
}

/// Attempts per package.  The default publish bucket refills one
/// token per minute, so a drained bucket needs at most one advertised
/// wait per token; a handful of attempts rides that out without
/// masking a persistently failing package.
const MAX_PUBLISH_ATTEMPTS: u32 = 5;

/// Fallback delay when the `429` carries no usable `Retry-After`,
/// matching the default class's one-token refill time.
const DEFAULT_RETRY_DELAY_SECS: u64 = 60;

/// Ceiling on any advertised delay, so a corrupt or hostile value can
/// never stall a batch for hours.
const MAX_RETRY_DELAY_SECS: u64 = 300;

/// Upload one staged package.  In a multi-package batch the
/// registry's `429` answers are waited out: a serial batch can outrun
/// the per-token publish bucket, and every attempt - byte-identical
/// no-ops included - charges it, so a rate-limited upload is retried
/// after the server-advertised delay instead of failing the batch.
/// The typed [`cabin_registry_api::RegistryApiError::RateLimited`] is
/// the signal - in-process, unspoofable by server-controlled detail
/// text.  `pace` is false for a bare single-package publish, which
/// keeps its historical fail-fast `429` - an unconditional wait would
/// turn a plain rate-limit refusal into twenty silent minutes of CI
/// time - and true for multi-package batches and
/// `--retry-rate-limits` (automation whose reruns hit the same
/// drained bucket).  Every other error fails fast.
fn publish_with_rate_limit_pacing(
    api_client: &cabin_registry_api::RegistryApi,
    staged: &cabin_package::StagedPackage,
    metadata_json: &[u8],
    new_revision: bool,
    pace: bool,
    reporter: Reporter,
) -> Result<cabin_registry_api::PublishReceipt> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match api_client.publish(
            staged.name.as_str(),
            &staged.version,
            metadata_json,
            &staged.archive_bytes,
            new_revision,
        ) {
            Err(cabin_registry_api::RegistryApiError::RateLimited { retry_after_secs })
                if pace && attempt < MAX_PUBLISH_ATTEMPTS =>
            {
                // One extra second over the advertised wait: the
                // server rounds its own estimate up, but the bucket
                // timestamps and this clock are not the same clock.
                let delay_secs = retry_after_secs
                    .unwrap_or(DEFAULT_RETRY_DELAY_SECS)
                    .min(MAX_RETRY_DELAY_SECS)
                    + 1;
                reporter.warning(format_args!(
                    "the registry rate limited {} {}; retrying in {delay_secs}s (attempt \
                     {attempt} of {MAX_PUBLISH_ATTEMPTS})",
                    staged.name.as_str(),
                    staged.version,
                ));
                std::thread::sleep(std::time::Duration::from_secs(delay_secs));
            }
            other => return Ok(other?),
        }
    }
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
    if let Some(revision) = &report.revision {
        println!("  revision: {revision}");
    }
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
        "revision": report.revision,
        "verification": report.verification,
        "warnings": report.warnings,
    });
    crate::print_json_line(&value, "failed to serialize publish output as JSON")
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
    crate::print_json_line(&value, "failed to serialize publish dry-run output as JSON")
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
    } else if report.no_op {
        // Mirror the remote flow's idempotent wording: the bytes are
        // already recorded under this revision, so the run succeeds
        // without rewriting anything.
        println!(
            "{} {} is already published to the file registry with identical bytes; nothing to do",
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
    println!("  revision: {}", report.revision);
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
        "revision": report.revision,
        "no_op": report.no_op,
        "source_path": report.source_path,
        "registry_modified": report.registry_modified,
        "registry_initialized": report.registry_initialized,
        "warnings": report.warnings,
    });
    crate::print_json_line(&value, "failed to serialize publish output as JSON")
}
