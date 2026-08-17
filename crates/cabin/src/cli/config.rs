//! Glue between [`cabin_config::EffectiveConfig`] and the rest of
//! the CLI's command pipeline.
//!
//! Discovery, parsing, and merging live in `cabin-config`.  This
//! module owns the small amount of *orchestration* the CLI needs
//! to thread an [`EffectiveConfig`] into resolvers, paths, and
//! the metadata view - typed helpers in, typed values out, no TOML
//! awareness, no filesystem reads.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use camino::{Utf8Path, Utf8PathBuf};

use anyhow::{Context, Result, bail};
use cabin_config::{
    ConfigDiscoveryInputs, ConfigSource, EffectiveCompilerWrapper, EffectiveConfig,
    EffectivePathSetting, EffectiveRegistrySource, EffectiveTool, EffectiveToolchain,
    WorkspaceLayout, discover_config_files, merge_loaded_files,
};
use cabin_core::{
    CompilerWrapperSource, ConfigValueSource, ProfileName, ProfileSelection, ToolSource,
};
use cabin_toolchain::{ConfigToolEntry, ConfigToolchainLayer, ConfigWrapperLayer};
use cabin_workspace::PackageGraph;

/// Discover and merge config files for a command running against
/// `graph`.  Wraps the pure cabin-config API with the workspace
/// layout pulled out of the loaded graph.
pub(crate) fn load_effective_config(graph: &PackageGraph) -> Result<EffectiveConfig> {
    let workspace = WorkspaceLayout {
        root_dir: graph.root_dir.as_path(),
        is_workspace_root: graph.is_workspace_root,
    };
    let inputs = ConfigDiscoveryInputs::from_process(Some(workspace));
    let discovery = discover_config_files(&inputs).context("failed to load Cabin config")?;
    Ok(merge_loaded_files(discovery.loaded_files))
}

/// Discover and merge config files keyed off a manifest path
/// alone - no [`PackageGraph`] needed.  Used by stages that have
/// to consult the merged config *before* the workspace loader
/// can run.
///
/// Equivalence: when called against the same manifest as
/// `load_effective_config(&graph)`, both produce identical
/// effective values; `graph.root_dir` is `manifest_path.parent()`
/// and `graph.is_workspace_root` reflects the same `[workspace]`
/// table this helper parses out of the manifest.
pub(crate) fn load_effective_config_for_manifest(manifest_path: &Path) -> Result<EffectiveConfig> {
    // If the manifest is missing or unreadable, defer to the
    // workspace loader's typed diagnostic by silently producing
    // an empty effective config (user-level config files are
    // still ignored).  The caller will invariably try to load the
    // workspace immediately after and that path emits the
    // canonical `cabin::workspace::manifest_not_found` /
    // `cabin::manifest::unreadable` errors.
    let Ok(parsed) = cabin_manifest::load_manifest(manifest_path) else {
        return Ok(merge_loaded_files(Vec::new()));
    };
    let root_dir = manifest_path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "manifest path {} has no parent directory",
            manifest_path.display()
        )
    })?;
    let workspace = WorkspaceLayout {
        root_dir,
        is_workspace_root: parsed.workspace.is_some(),
    };
    let inputs = ConfigDiscoveryInputs::from_process(Some(workspace));
    let discovery = discover_config_files(&inputs).context("failed to load Cabin config")?;
    Ok(merge_loaded_files(discovery.loaded_files))
}

/// Build the typed config layer the toolchain resolver consumes.
/// Returns `None` when no config-file values apply.
pub(crate) fn toolchain_layer(config: &EffectiveConfig) -> Option<ConfigToolchainLayer> {
    let layer = ConfigToolchainLayer {
        cc: tool_entry(config.toolchain.cc.as_ref()),
        cxx: tool_entry(config.toolchain.cxx.as_ref()),
        ar: tool_entry(config.toolchain.ar.as_ref()),
    };
    if layer.is_empty() { None } else { Some(layer) }
}

/// Build the typed config layer the wrapper resolver consumes.
/// `None` when no wrapper choice was declared in any config file.
pub(crate) fn wrapper_layer(config: &EffectiveConfig) -> Option<ConfigWrapperLayer> {
    let EffectiveCompilerWrapper { request, source } = config.compiler_wrapper.as_ref()?;
    Some(ConfigWrapperLayer {
        request: request.clone(),
        source: wrapper_source_for(*source),
    })
}

fn tool_entry(value: Option<&EffectiveTool>) -> Option<ConfigToolEntry> {
    let entry = value?;
    Some(ConfigToolEntry {
        spec: entry.spec.clone(),
        source: tool_source_for(entry.source),
    })
}

fn tool_source_for(source: ConfigSource) -> ToolSource {
    match source {
        ConfigSource::User => ToolSource::UserConfig,
        ConfigSource::Workspace => ToolSource::WorkspaceConfig,
        ConfigSource::Package => ToolSource::PackageConfig,
        ConfigSource::Explicit => ToolSource::ExplicitConfig,
    }
}

fn wrapper_source_for(source: ConfigSource) -> CompilerWrapperSource {
    match source {
        ConfigSource::User => CompilerWrapperSource::UserConfig,
        ConfigSource::Workspace => CompilerWrapperSource::WorkspaceConfig,
        ConfigSource::Package => CompilerWrapperSource::PackageConfig,
        ConfigSource::Explicit => CompilerWrapperSource::ExplicitConfig,
    }
}

/// Map a [`ConfigSource`] onto the broader [`ConfigValueSource`]
/// used in metadata reporting.
pub(crate) fn config_value_source(source: ConfigSource) -> ConfigValueSource {
    match source {
        ConfigSource::User => ConfigValueSource::UserConfig,
        ConfigSource::Workspace => ConfigValueSource::WorkspaceConfig,
        ConfigSource::Package => ConfigValueSource::PackageConfig,
        ConfigSource::Explicit => ConfigValueSource::ExplicitConfig,
    }
}

/// Whether a config file's values count as the *user's* choice of
/// registry origin.  The user-level file and the one `CABIN_CONFIG`
/// points at are written by whoever runs Cabin; the workspace- and
/// package-level files travel inside the checkout, so a hostile
/// dependency or pull-request branch can write them.
pub(crate) const fn config_is_user_chosen(source: ConfigSource) -> bool {
    matches!(source, ConfigSource::User | ConfigSource::Explicit)
}

/// Whether the origin an index is finally reached through was chosen
/// by the user.
///
/// Deliberate ceiling: *any* `[source-replacement]` hop makes the
/// answer `false`, even one declared in user-level config, because
/// [`cabin_core::SourceReplacementResolution`] records the hops it
/// walked but not which file declared each of them.  The fallback is
/// `cabin login` for the mirror's origin, which stores an
/// origin-keyed token and is what `cabin login` already does for a
/// replaced source.
pub(crate) fn index_origin_user_chosen(
    source: &ResolvedIndexSource,
    resolution: &cabin_core::SourceReplacementResolution,
) -> bool {
    source.user_chosen && resolution.hops.is_empty()
}

/// Resolved index source that consumes CLI arguments first and
/// falls back to the merged config.
pub(crate) struct ResolvedIndexSource {
    pub kind: IndexSourceKind,
    /// Whether the *user* picked this source, rather than a
    /// checked-out project: a CLI flag, the user-level config file,
    /// or the one `CABIN_CONFIG` names.  A workspace- or
    /// package-level `.cabin/config.toml` ships inside the tree being
    /// built, so it is the project speaking, not the user - which is
    /// why it cannot make an origin eligible for the origin-key-less
    /// `CABIN_REGISTRY_TOKEN` override.  Same split
    /// [`effective_registry_index_url`](crate::cli::login::effective_registry_index_url)
    /// already applies to where a token is *stored*.
    pub user_chosen: bool,
}

pub(crate) enum IndexSourceKind {
    Path(Utf8PathBuf),
    Url(String),
}

/// Convert a resolved index source's kind into the core
/// [`cabin_core::SourceLocator`] the artifact pipeline consumes.
/// Centralizes the `Path` / `Url` mapping every command performs
/// before applying source-replacement, so a future third source
/// kind only needs one match arm updated.
pub(crate) fn index_source_kind_to_locator(kind: &IndexSourceKind) -> cabin_core::SourceLocator {
    match kind {
        IndexSourceKind::Path(p) => cabin_core::SourceLocator::IndexPath { path: p.clone() },
        IndexSourceKind::Url(u) => cabin_core::SourceLocator::IndexUrl { url: u.clone() },
    }
}

/// Apply the documented index-source precedence:
///
/// 1. `--index-path` ▶ CLI
/// 2. `--index-url` ▶ CLI
/// 3. config-supplied registry source (highest-priority file's
///    declared variant)
/// 4. unset (caller decides whether the absence is an error)
///
/// Passing both CLI flags is rejected at the call site (existing
/// behavior); this helper only reconciles a single CLI choice
/// against the config layer.
pub(crate) fn resolve_index_source(
    cli_index_path: Option<&Path>,
    cli_index_url: Option<&str>,
    config: &EffectiveConfig,
) -> Result<Option<ResolvedIndexSource>> {
    if cli_index_path.is_some() && cli_index_url.is_some() {
        bail!("use either --index-path or --index-url, not both");
    }
    if let Some(path) = cli_index_path {
        let path = Utf8Path::from_path(path).ok_or_else(|| {
            anyhow::anyhow!("`--index-path` is not valid UTF-8: {}", path.display())
        })?;
        return Ok(Some(ResolvedIndexSource {
            kind: IndexSourceKind::Path(path.to_path_buf()),
            user_chosen: true,
        }));
    }
    if let Some(url) = cli_index_url {
        if cabin_config::url_contains_credentials(url) {
            bail!(
                "`--index-url` must not contain credentials (userinfo): `{}`",
                cabin_config::redact_userinfo(url)
            );
        }
        return Ok(Some(ResolvedIndexSource {
            kind: IndexSourceKind::Url(url.to_owned()),
            user_chosen: true,
        }));
    }
    Ok(config.registry.source.as_ref().map(|src| match src {
        EffectiveRegistrySource::Path(value) => ResolvedIndexSource {
            kind: IndexSourceKind::Path(value.value.clone()),
            user_chosen: config_is_user_chosen(value.source),
        },
        EffectiveRegistrySource::Url(value) => ResolvedIndexSource {
            kind: IndexSourceKind::Url(value.value.clone()),
            user_chosen: config_is_user_chosen(value.source),
        },
    }))
}

/// The built-in fallback index source: Cabin's default hosted
/// registry.  Applied only at the points where a command needs an
/// index and neither the CLI nor the config supplies one, so
/// commands (and selections) without versioned dependencies never
/// observe it - their offline / frozen behavior is unchanged.
pub(crate) fn default_index_source() -> ResolvedIndexSource {
    ResolvedIndexSource {
        kind: IndexSourceKind::Url(cabin_core::registry::DEFAULT_INDEX_URL.to_owned()),
        user_chosen: false,
    }
}

/// Materialize the default hosted-registry index source for a
/// resolve / fetch pipeline, mirroring how a config-supplied URL
/// behaves: refuse under `--offline` before source replacement (a
/// configured URL is refused pre-replacement too), apply
/// `[source-replacement]`, and refuse a URL terminal under
/// `--frozen` (a replacement that rewrites the default origin to a
/// local path keeps frozen runs working, exactly like a config URL).
pub(crate) fn default_index_locator(
    offline: bool,
    frozen: bool,
    effective_config: &EffectiveConfig,
    no_patches: bool,
) -> Result<cabin_core::SourceLocator> {
    let default_url = cabin_core::registry::DEFAULT_INDEX_URL;
    if offline {
        bail!(
            "--offline forbids network access, but no index source is configured, so versioned dependencies would resolve through the default registry `{default_url}`; pass `--index-path <dir>` and re-run with a local index (e.g. a `cabin vendor` output)"
        );
    }
    let resolution = crate::cli::patch::apply_source_replacement(
        index_source_kind_to_locator(&default_index_source().kind),
        effective_config,
        no_patches,
    )?;
    if frozen
        && matches!(
            resolution.resolved,
            cabin_core::SourceLocator::IndexUrl { .. }
        )
    {
        bail!(
            "cannot resolve versioned dependencies with --frozen: no index source is configured, and the default registry `{default_url}` would require network fetches; pass `--index-path <dir>` (e.g. a `cabin vendor` output)"
        );
    }
    Ok(resolution.resolved)
}

/// Apply Cabin's CLI-vs-env precedence for the `--offline`
/// flag.  Returns `true` when the user passed `--offline` *or*
/// when [`cabin_env::CABIN_NET_OFFLINE`] is set to a truthy
/// value.  The CLI flag short-circuits the env lookup because
/// there is no negative form today; otherwise the env value must
/// use Cabin's documented boolean grammar.
pub(crate) fn effective_offline(cli: bool) -> Result<bool> {
    if cli {
        return Ok(true);
    }
    if let Some(raw) = std::env::var_os(cabin_env::CABIN_NET_OFFLINE) {
        let Some(s) = raw.to_str() else {
            bail!(
                "invalid {} value: expected valid UTF-8 boolean spelling",
                cabin_env::CABIN_NET_OFFLINE
            );
        };
        return cabin_env::parse_bool(s).map_err(|err| {
            anyhow::anyhow!(
                "invalid {} value {:?}: {err}",
                cabin_env::CABIN_NET_OFFLINE,
                s
            )
        });
    }
    Ok(false)
}

/// Reject any resolved-index-source that would require network
/// access when the caller passed `--offline`.  The check is the
/// single point where Cabin enforces the offline contract: an
/// HTTP index URL is the only network input the read path
/// recognizes today, so refusing one here is sufficient.
///
/// Returns `Ok(())` when offline is satisfied (no source, or a
/// path source); otherwise returns an actionable error that
/// names the URL and tells the user how to switch to a local
/// index or a vendor directory.
pub(crate) fn enforce_offline_index_source(
    offline: bool,
    resolved: Option<&ResolvedIndexSource>,
) -> Result<()> {
    if !offline {
        return Ok(());
    }
    if let Some(ResolvedIndexSource {
        kind: IndexSourceKind::Url(url),
        ..
    }) = resolved
    {
        bail!(
            "--offline forbids network access, but the resolved index source is the URL `{url}`; pass `--index-path <dir>` or remove `[registry] index-url` from the active config and re-run with a local index (e.g. a `cabin vendor` output)"
        );
    }
    Ok(())
}

/// Companion to [`enforce_offline_index_source`] that runs *after*
/// `apply_source_replacement`.  The pre-check only sees the source
/// the user requested; a `[source-replacement]` entry can still
/// rewrite an `index-path` into an `index-url` later in the
/// pipeline, and the artifact loader would happily open it.  This
/// check closes that gap.
///
/// Takes the typed [`cabin_core::SourceReplacementResolution`] so
/// it can give an accurate error: a non-empty `hops` list means
/// replacement fired, and the message can name the
/// `[source-replacement]` config the user needs to revisit.
pub(crate) fn enforce_offline_post_replacement(
    offline: bool,
    resolution: &cabin_core::SourceReplacementResolution,
) -> Result<()> {
    if !offline {
        return Ok(());
    }
    let cabin_core::SourceLocator::IndexUrl { url } = &resolution.resolved else {
        return Ok(());
    };
    if resolution.hops.is_empty() {
        bail!(
            "--offline forbids network access, but the resolved index source is the URL `{url}`; pass `--index-path <dir>` or remove `[registry] index-url` from the active config and re-run with a local index (e.g. a `cabin vendor` output)"
        );
    }
    bail!(
        "--offline forbids network access, but `[source-replacement]` redirected the index to the URL `{url}`; remove the offending source-replacement entry, pass `--no-patches`, or drop `--offline`"
    );
}

/// Post-`apply_source_replacement` variant of vendor's
/// local-index check.  The pre-replacement check at the call site
/// catches direct `[registry] index-url` cases; this one catches
/// the path → URL replacement case the same way
/// [`enforce_offline_post_replacement`] does for `--offline`.
pub(crate) fn enforce_vendor_local_index_post_replacement(
    resolution: &cabin_core::SourceReplacementResolution,
) -> Result<()> {
    let cabin_core::SourceLocator::IndexUrl { url } = &resolution.resolved else {
        return Ok(());
    };
    if resolution.hops.is_empty() {
        bail!(
            "`cabin vendor` requires a local `--index-path` source so per-package metadata can be copied verbatim into the vendor directory; the resolved index source is the URL `{url}`"
        );
    }
    bail!(
        "`cabin vendor` requires a local `--index-path` source, but `[source-replacement]` redirected the index to the URL `{url}`; remove the offending source-replacement entry or pass `--no-patches`"
    );
}

/// The inputs the artifact pipeline consumes once a command has a
/// concrete index source: the lock policy, the resolved artifact
/// cache directory, and the locator the index is reachable through
/// after source-replacement.
pub(crate) struct PipelineInputs {
    pub policy: crate::cli::LockPolicy,
    pub cache_dir: PathBuf,
    pub index_source: cabin_core::SourceLocator,
    /// See [`index_origin_user_chosen`].  Gates the
    /// `CABIN_REGISTRY_TOKEN` override at the HTTP index.
    pub index_user_chosen: bool,
}

/// Turn a resolved index source into [`PipelineInputs`] - the inner
/// band that `build` / `run` / `test` / `fetch` / `vendor` all run
/// once versioned dependencies force an index: derive the lock
/// policy, resolve the cache dir (preferring the config-resolved
/// value), convert the source to a [`cabin_core::SourceLocator`],
/// apply source-replacement, and enforce the post-replacement
/// offline rule (and, for `vendor`, the local-index rule).  A `None`
/// source falls back to the default hosted registry via
/// [`default_index_locator`].
///
/// Callers keep their own `resolve_index_source` /
/// `enforce_offline_index_source` / `resolve_cache_dir` preamble
/// (which runs unconditionally, before the has-versioned gate).
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
pub(crate) fn resolve_pipeline_inputs(
    index_source: Option<&ResolvedIndexSource>,
    effective_config: &EffectiveConfig,
    cache_dir_arg: Option<&Path>,
    resolved_cache_dir: Option<&(PathBuf, ConfigValueSource)>,
    offline: bool,
    locked: bool,
    frozen: bool,
    no_patches: bool,
    vendor_local_index: bool,
) -> Result<PipelineInputs> {
    let policy = crate::cli::LockPolicy::from_flags(locked, frozen);
    let cache_dir = match resolved_cache_dir {
        Some((path, _)) => path.clone(),
        None => crate::cli::cache_dir_for(cache_dir_arg)?,
    };
    let (index_source, index_user_chosen) = match index_source {
        Some(source) => {
            let initial_locator = index_source_kind_to_locator(&source.kind);
            let resolved_locator = crate::cli::patch::apply_source_replacement(
                initial_locator,
                effective_config,
                no_patches,
            )?;
            enforce_offline_post_replacement(offline, &resolved_locator)?;
            if vendor_local_index {
                enforce_vendor_local_index_post_replacement(&resolved_locator)?;
            }
            let user_chosen = index_origin_user_chosen(source, &resolved_locator);
            (resolved_locator.resolved, user_chosen)
        }
        // The default hosted registry passes the override gate on
        // origin equality, so it needs no provenance of its own - and
        // a replacement that rewrites it onto loopback must not.
        None => (
            default_index_locator(offline, frozen, effective_config, no_patches)?,
            false,
        ),
    };
    Ok(PipelineInputs {
        policy,
        cache_dir,
        index_source,
        index_user_chosen,
    })
}

/// Resolve the build directory the CLI should use for a build
/// invocation, consulting CLI flag → env var → config →
/// built-in default in that order.
///
/// `cli_value` is `Some(p)` only when the user passed
/// `--build-dir`; the clap default lives in the helper so an
/// explicit `--build-dir build` is still recognized as a CLI
/// choice and beats the env layer.  Precedence: `--build-dir`,
/// then [`cabin_env::CABIN_BUILD_DIR`], then `[paths] build-dir`,
/// then the built-in default (`build`).  The returned
/// [`ConfigValueSource`] lets metadata attribute the value.
pub(crate) fn resolve_build_dir_with_env(
    cli_value: Option<&Path>,
    config: &EffectiveConfig,
) -> (PathBuf, ConfigValueSource) {
    resolve_build_dir_layered(
        cli_value,
        std::env::var_os(cabin_env::CABIN_BUILD_DIR),
        config,
    )
}

fn resolve_build_dir_layered(
    cli_value: Option<&Path>,
    env_value: Option<OsString>,
    config: &EffectiveConfig,
) -> (PathBuf, ConfigValueSource) {
    if let Some(p) = cli_value {
        return (p.to_path_buf(), ConfigValueSource::Cli);
    }
    if let Some(value) = env_value.filter(|v| !v.is_empty()) {
        return (PathBuf::from(value), ConfigValueSource::Env);
    }
    if let Some(setting) = &config.paths.build_dir {
        return (
            setting.absolute().into_std_path_buf(),
            config_value_source(setting.source),
        );
    }
    (PathBuf::from("build"), ConfigValueSource::BuiltinDefault)
}

/// Resolve the build-jobs setting for a build invocation.
///
/// Precedence: CLI `--jobs` > [`cabin_env::CABIN_BUILD_JOBS`]
/// env var > `[build] jobs` config setting > backend default
/// (`None` - the Ninja runner omits `-j` and Ninja picks its
/// own default).
///
/// The env-var parser flows through the same typed
/// [`cabin_core::BuildJobs`] validator the CLI uses so the
/// error wording stays consistent across input sources.
pub(crate) fn resolve_build_jobs(
    cli_value: Option<cabin_core::BuildJobs>,
    config: &EffectiveConfig,
) -> Result<Option<cabin_core::BuildJobs>> {
    if let Some(jobs) = cli_value {
        return Ok(Some(jobs));
    }
    if let Some(raw) = std::env::var_os(cabin_env::CABIN_BUILD_JOBS) {
        // The job count feeds the typed validator below, so reject a
        // non-UTF-8 value explicitly rather than lossily mangling it
        // into a string that cannot parse anyway.
        let raw = raw.into_string().map_err(|_| {
            anyhow::anyhow!(
                "{env} is not valid UTF-8",
                env = cabin_env::CABIN_BUILD_JOBS
            )
        })?;
        if !raw.is_empty() {
            let jobs = raw.parse::<cabin_core::BuildJobs>().map_err(|err| {
                anyhow::anyhow!(
                    "invalid {env} value {raw:?}: {err}",
                    env = cabin_env::CABIN_BUILD_JOBS
                )
            })?;
            return Ok(Some(jobs));
        }
    }
    if let Some(setting) = &config.build.jobs {
        return Ok(Some(setting.value));
    }
    Ok(None)
}

/// Resolve the standard-aware version-preference mode for a
/// resolution.
///
/// Precedence: [`cabin_env::CABIN_RESOLVER_INCOMPATIBLE_STANDARDS`]
/// env var > `[resolver] incompatible-standards` config setting >
/// built-in default ([`cabin_core::IncompatibleStandards::Fallback`]).
/// The value vocabulary is Cargo's `resolver.incompatible-rust-versions`
/// verbatim.
pub(crate) fn resolve_incompatible_standards(
    config: &EffectiveConfig,
) -> Result<cabin_core::IncompatibleStandards> {
    resolve_incompatible_standards_sourced(config).map(|(value, _)| value)
}

/// Like [`resolve_incompatible_standards`], but also returns the layer
/// the effective value came from (env > config > builtin default).
/// `cabin metadata` renders this so its `resolver` block honors the
/// same precedence and `value_source` contract as every other config
/// value (`docs/config.md`), rather than reporting only the file layer.
pub(crate) fn resolve_incompatible_standards_sourced(
    config: &EffectiveConfig,
) -> Result<(cabin_core::IncompatibleStandards, ConfigValueSource)> {
    if let Some(raw) = std::env::var_os(cabin_env::CABIN_RESOLVER_INCOMPATIBLE_STANDARDS) {
        let raw = raw.into_string().map_err(|_| {
            anyhow::anyhow!(
                "{env} is not valid UTF-8",
                env = cabin_env::CABIN_RESOLVER_INCOMPATIBLE_STANDARDS
            )
        })?;
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            let value = cabin_core::IncompatibleStandards::parse(trimmed).map_err(|err| {
                anyhow::anyhow!(
                    "invalid {env} value {trimmed:?}: {err}",
                    env = cabin_env::CABIN_RESOLVER_INCOMPATIBLE_STANDARDS
                )
            })?;
            return Ok((value, ConfigValueSource::Env));
        }
    }
    if let Some(setting) = &config.resolver.incompatible_standards {
        return Ok((setting.value, config_value_source(setting.source)));
    }
    Ok((
        cabin_core::IncompatibleStandards::default(),
        ConfigValueSource::BuiltinDefault,
    ))
}

/// Resolve the cache directory for a build / fetch invocation.
///
/// Precedence: CLI `--cache-dir` > [`cabin_env::CABIN_CACHE_DIR`]
/// env var > `[paths] cache-dir` config setting > `None` (the
/// caller keeps its existing default behavior).  Mirrors the
/// sibling helpers [`resolve_build_dir_with_env`] and
/// [`resolve_build_jobs`].
pub(crate) fn resolve_cache_dir(
    cli_value: Option<&Path>,
    config: &EffectiveConfig,
) -> Option<(PathBuf, ConfigValueSource)> {
    resolve_cache_dir_layered(
        cli_value,
        std::env::var_os(cabin_env::CABIN_CACHE_DIR),
        config,
    )
}

fn resolve_cache_dir_layered(
    cli_value: Option<&Path>,
    env_value: Option<OsString>,
    config: &EffectiveConfig,
) -> Option<(PathBuf, ConfigValueSource)> {
    if let Some(p) = cli_value {
        return Some((p.to_path_buf(), ConfigValueSource::Cli));
    }
    if let Some(value) = env_value.filter(|v| !v.is_empty()) {
        return Some((PathBuf::from(value), ConfigValueSource::Env));
    }
    config.paths.cache_dir.as_ref().map(|setting| {
        (
            setting.absolute().into_std_path_buf(),
            config_value_source(setting.source),
        )
    })
}

/// Apply config-supplied profile defaults.  CLI flags (handled
/// upstream of this helper) win; otherwise the config-provided
/// profile name is parsed into a typed [`ProfileSelection`].
pub(crate) fn config_profile_selection(
    config: &EffectiveConfig,
) -> Result<Option<(ProfileSelection, ConfigValueSource)>> {
    let Some(profile) = config.build.profile.as_ref() else {
        return Ok(None);
    };
    let name = ProfileName::new(profile.name.clone())
        .with_context(|| format!("invalid `build.profile` in config: `{}`", profile.name))?;
    Ok(Some((
        ProfileSelection::from_name(name),
        config_value_source(profile.source),
    )))
}

/// JSON view of the loaded config files plus every effective
/// config-derived setting.  `None` is rendered as `null` in the
/// metadata view so the field is always present.
pub(crate) fn config_view_json(
    config: &EffectiveConfig,
    resolver_incompatible_standards: (cabin_core::IncompatibleStandards, ConfigValueSource),
) -> serde_json::Value {
    let loaded_files: Vec<serde_json::Value> = config
        .loaded_files
        .iter()
        .map(|file| {
            serde_json::json!({
                "source": file.source.as_key(),
                "path": file.path.as_str().to_owned(),
            })
        })
        .collect();

    let registry = match &config.registry.source {
        Some(EffectiveRegistrySource::Path(value)) => serde_json::json!({
            "kind": "path",
            "value": value.value.as_str().to_owned(),
            "value_source": config_value_source(value.source).as_key(),
        }),
        Some(EffectiveRegistrySource::Url(value)) => serde_json::json!({
            "kind": "url",
            "value": value.value,
            "value_source": config_value_source(value.source).as_key(),
        }),
        None => serde_json::Value::Null,
    };

    let paths = serde_json::json!({
        "cache_dir": path_setting_view(config.paths.cache_dir.as_ref()),
        "build_dir": path_setting_view(config.paths.build_dir.as_ref()),
    });

    let build = serde_json::json!({
        "profile": match &config.build.profile {
            Some(profile) => serde_json::json!({
                "name": profile.name,
                "value_source": config_value_source(profile.source).as_key(),
            }),
            None => serde_json::Value::Null,
        },
    });

    // Effective value + source (env > config > builtin default), not
    // the raw file layer: `docs/config.md` promises `cabin metadata`
    // reports every effective config value with its `value_source`, so
    // an env override or the built-in `fallback` default must be visible
    // here rather than showing `null` / a stale file value.
    let (resolver_value, resolver_source) = resolver_incompatible_standards;
    let resolver = serde_json::json!({
        "incompatible_standards": resolver_value.as_str(),
        "value_source": resolver_source.as_key(),
    });

    let toolchain = toolchain_view_json(&config.toolchain);

    let compiler_wrapper = match &config.compiler_wrapper {
        Some(wrapper) => serde_json::json!({
            "request": wrapper.request.as_key(),
            "value_source": config_value_source(wrapper.source).as_key(),
        }),
        None => serde_json::Value::Null,
    };

    serde_json::json!({
        "loaded_files": loaded_files,
        "registry": registry,
        "paths": paths,
        "build": build,
        "resolver": resolver,
        "toolchain": toolchain,
        "compiler_wrapper": compiler_wrapper,
    })
}

fn toolchain_view_json(toolchain: &EffectiveToolchain) -> serde_json::Value {
    serde_json::json!({
        "cc": tool_view(toolchain.cc.as_ref()),
        "cxx": tool_view(toolchain.cxx.as_ref()),
        "ar": tool_view(toolchain.ar.as_ref()),
    })
}

fn tool_view(value: Option<&EffectiveTool>) -> serde_json::Value {
    match value {
        Some(tool) => serde_json::json!({
            "spec": tool.spec.display(),
            "value_source": config_value_source(tool.source).as_key(),
        }),
        None => serde_json::Value::Null,
    }
}

fn path_setting_view(setting: Option<&EffectivePathSetting>) -> serde_json::Value {
    match setting {
        Some(s) => serde_json::json!({
            "value": s.value.as_str().to_owned(),
            "absolute": s.absolute().as_str().to_owned(),
            "value_source": config_value_source(s.source).as_key(),
        }),
        None => serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::{SourceLocator, SourceReplacementResolution};

    #[test]
    fn resolve_index_source_rejects_cli_url_with_credentials() {
        let cfg = cabin_config::EffectiveConfig::default();
        let Err(err) = resolve_index_source(None, Some("https://user:pw@bad.example.com/"), &cfg)
        else {
            panic!("expected credential rejection");
        };
        let message = err.to_string();
        assert!(
            !message.contains("user:pw"),
            "credentials must be redacted from error, got: {message}"
        );
        assert!(
            message.contains("credentials") || message.contains("userinfo"),
            "expected message to mention credentials, got: {message}"
        );
    }

    fn path_resolution(path: &str) -> SourceReplacementResolution {
        SourceReplacementResolution {
            resolved: SourceLocator::IndexPath {
                path: Utf8PathBuf::from(path),
            },
            hops: Vec::new(),
        }
    }

    fn url_resolution_with_hops(
        url: &str,
        hops: Vec<SourceLocator>,
    ) -> SourceReplacementResolution {
        SourceReplacementResolution {
            resolved: SourceLocator::IndexUrl {
                url: url.to_owned(),
            },
            hops,
        }
    }

    #[test]
    fn enforce_offline_post_replacement_allows_when_not_offline() {
        let resolution = url_resolution_with_hops(
            "https://example.com/idx",
            vec![SourceLocator::IndexPath {
                path: Utf8PathBuf::from("./mirror"),
            }],
        );
        enforce_offline_post_replacement(false, &resolution)
            .expect("non-offline must always succeed");
    }

    #[test]
    fn enforce_offline_post_replacement_allows_path_terminal() {
        let resolution = path_resolution("./mirror");
        enforce_offline_post_replacement(true, &resolution)
            .expect("offline + path terminal is the supported combination");
    }

    #[test]
    fn enforce_offline_post_replacement_blames_source_replacement_when_hops_present() {
        let resolution = url_resolution_with_hops(
            "https://example.com/idx",
            vec![SourceLocator::IndexPath {
                path: Utf8PathBuf::from("./mirror"),
            }],
        );
        let err = enforce_offline_post_replacement(true, &resolution)
            .expect_err("offline + url-after-replacement must bail");
        let message = err.to_string();
        assert!(
            message.contains("source-replacement"),
            "message must blame source-replacement, got: {message}"
        );
        assert!(
            message.contains("https://example.com/idx"),
            "message must name the offending URL, got: {message}"
        );
    }

    #[test]
    fn enforce_offline_post_replacement_falls_back_to_pre_check_wording_without_hops() {
        let resolution = url_resolution_with_hops("https://example.com/idx", Vec::new());
        let err = enforce_offline_post_replacement(true, &resolution)
            .expect_err("defensive: offline + url terminal still bails");
        let message = err.to_string();
        assert!(
            message.contains("--offline"),
            "message must reference --offline, got: {message}"
        );
        assert!(
            message.contains("https://example.com/idx"),
            "message must name the offending URL, got: {message}"
        );
    }

    #[test]
    fn enforce_vendor_local_index_post_replacement_allows_path_terminal() {
        let resolution = path_resolution("./mirror");
        enforce_vendor_local_index_post_replacement(&resolution)
            .expect("path terminal is acceptable for vendor");
    }

    fn cfg_with_cache_dir(value: &str, source: ConfigSource) -> EffectiveConfig {
        let mut cfg = EffectiveConfig::default();
        cfg.paths.cache_dir = Some(EffectivePathSetting {
            value: Utf8PathBuf::from(value),
            source,
            base: Utf8PathBuf::from("/base"),
        });
        cfg
    }

    fn cfg_with_build_dir(value: &str, source: ConfigSource) -> EffectiveConfig {
        let mut cfg = EffectiveConfig::default();
        cfg.paths.build_dir = Some(EffectivePathSetting {
            value: Utf8PathBuf::from(value),
            source,
            base: Utf8PathBuf::from("/base"),
        });
        cfg
    }

    #[test]
    fn resolve_build_dir_explicit_cli_wins_even_when_value_equals_default() {
        // Regression: an explicit `--build-dir build` (matching the
        // built-in default literal) must beat `CABIN_BUILD_DIR`.
        let cfg = EffectiveConfig::default();
        let cli = PathBuf::from("build");
        let (path, source) = resolve_build_dir_layered(
            Some(cli.as_path()),
            Some(OsString::from("/tmp/env-build")),
            &cfg,
        );
        assert_eq!(path, cli);
        assert_eq!(source, ConfigValueSource::Cli);
    }

    #[test]
    fn resolve_build_dir_env_beats_config() {
        let cfg = cfg_with_build_dir("config-build", ConfigSource::Workspace);
        let (path, source) =
            resolve_build_dir_layered(None, Some(OsString::from("/tmp/env-build")), &cfg);
        assert_eq!(path, PathBuf::from("/tmp/env-build"));
        assert_eq!(source, ConfigValueSource::Env);
    }

    #[test]
    fn resolve_build_dir_falls_back_to_config() {
        let cfg = cfg_with_build_dir("config-build", ConfigSource::Workspace);
        let (path, source) = resolve_build_dir_layered(None, None, &cfg);
        assert_eq!(path, PathBuf::from("/base").join("config-build"));
        assert_eq!(source, ConfigValueSource::WorkspaceConfig);
    }

    #[test]
    fn resolve_build_dir_builtin_default_when_nothing_set() {
        let cfg = EffectiveConfig::default();
        let (path, source) = resolve_build_dir_layered(None, None, &cfg);
        assert_eq!(path, PathBuf::from("build"));
        assert_eq!(source, ConfigValueSource::BuiltinDefault);
    }

    #[test]
    fn resolve_build_dir_empty_env_falls_through_to_config() {
        let cfg = cfg_with_build_dir("config-build", ConfigSource::Workspace);
        let (path, source) = resolve_build_dir_layered(None, Some(OsString::new()), &cfg);
        assert_eq!(path, PathBuf::from("/base").join("config-build"));
        assert_eq!(source, ConfigValueSource::WorkspaceConfig);
    }

    #[test]
    fn resolve_cache_dir_env_beats_config() {
        let cfg = cfg_with_cache_dir("config-cache", ConfigSource::Workspace);
        let (path, source) =
            resolve_cache_dir_layered(None, Some(OsString::from("/tmp/env-cache")), &cfg)
                .expect("env value should resolve");
        assert_eq!(path, PathBuf::from("/tmp/env-cache"));
        assert_eq!(source, ConfigValueSource::Env);
    }

    #[test]
    fn resolve_cache_dir_cli_beats_env() {
        let cfg = cfg_with_cache_dir("config-cache", ConfigSource::Workspace);
        let cli = PathBuf::from("/tmp/cli-cache");
        let (path, source) = resolve_cache_dir_layered(
            Some(cli.as_path()),
            Some(OsString::from("/tmp/env-cache")),
            &cfg,
        )
        .expect("cli value should resolve");
        assert_eq!(path, cli);
        assert_eq!(source, ConfigValueSource::Cli);
    }

    #[test]
    fn resolve_cache_dir_empty_env_falls_through_to_config() {
        let cfg = cfg_with_cache_dir("config-cache", ConfigSource::Workspace);
        let (path, source) = resolve_cache_dir_layered(None, Some(OsString::new()), &cfg)
            .expect("config value should resolve");
        assert_eq!(path, PathBuf::from("/base").join("config-cache"));
        assert_eq!(source, ConfigValueSource::WorkspaceConfig);
    }

    #[test]
    fn enforce_vendor_local_index_post_replacement_rejects_url_after_replacement() {
        let resolution = url_resolution_with_hops(
            "https://example.com/idx",
            vec![SourceLocator::IndexPath {
                path: Utf8PathBuf::from("./mirror"),
            }],
        );
        let err = enforce_vendor_local_index_post_replacement(&resolution)
            .expect_err("vendor must reject URL terminals");
        let message = err.to_string();
        assert!(
            message.contains("source-replacement"),
            "message must blame source-replacement, got: {message}"
        );
        assert!(
            message.contains("cabin vendor"),
            "message must reference `cabin vendor`, got: {message}"
        );
    }

    /// The trust split is exhaustive over `ConfigSource`, so a new
    /// variant - or one moved into the allowlist - has to be decided
    /// here rather than defaulting either way.  `Workspace` and
    /// `Package` are the tree-resident files: whoever supplies the
    /// checkout writes them.
    #[test]
    fn only_the_two_config_files_outside_the_checkout_count_as_user_chosen() {
        for (source, expected) in [
            (ConfigSource::User, true),
            (ConfigSource::Explicit, true),
            (ConfigSource::Workspace, false),
            (ConfigSource::Package, false),
        ] {
            assert_eq!(
                config_is_user_chosen(source),
                expected,
                "source: {}",
                source.as_key()
            );
        }
    }

    /// Any `[source-replacement]` hop disqualifies the origin, even
    /// under a user-chosen source: the resolution records which hops
    /// it walked, not which file declared each of them.
    #[test]
    fn a_replacement_hop_disqualifies_even_a_user_chosen_source() {
        let source = ResolvedIndexSource {
            kind: IndexSourceKind::Url("https://mirror.example.com".to_owned()),
            user_chosen: true,
        };
        let direct = url_resolution_with_hops("https://mirror.example.com", Vec::new());
        assert!(index_origin_user_chosen(&source, &direct));
        let replaced = url_resolution_with_hops(
            "http://127.0.0.1:8080",
            vec![SourceLocator::IndexUrl {
                url: "https://mirror.example.com".to_owned(),
            }],
        );
        assert!(!index_origin_user_chosen(&source, &replaced));
    }
}
