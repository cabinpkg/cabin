//! Glue between [`cabin_config::EffectiveConfig`] and the rest of
//! the CLI's command pipeline.
//!
//! Discovery, parsing, and merging live in `cabin-config`. This
//! module owns the small amount of *orchestration* the CLI needs
//! to thread an [`EffectiveConfig`] into resolvers, paths, and
//! the metadata view — typed helpers in, typed values out, no TOML
//! awareness, no filesystem reads.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

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
/// `graph`. Wraps the pure cabin-config API with the workspace
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
/// alone — no [`PackageGraph`] needed. Used by stages that have
/// to consult the merged config *before* the workspace loader
/// can run (e.g. foundation-port preparation needs `[paths]
/// cache-dir` to point itself at the same archive cache the
/// later artifact pipeline uses).
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
    // still ignored). The caller will invariably try to load the
    // workspace immediately after and that path emits the
    // canonical `cabin::workspace::manifest_not_found` /
    // `cabin::manifest::unreadable` errors.
    let parsed = match cabin_manifest::load_manifest(manifest_path) {
        Ok(p) => p,
        Err(_) => return Ok(merge_loaded_files(Vec::new())),
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
        request: *request,
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

/// Resolved index source that consumes CLI arguments first and
/// falls back to the merged config.
pub(crate) struct ResolvedIndexSource {
    pub kind: IndexSourceKind,
}

pub(crate) enum IndexSourceKind {
    Path(PathBuf),
    Url(String),
}

/// Apply the documented index-source precedence:
///
/// 1. `--index-path`  ▶ CLI
/// 2. `--index-url`   ▶ CLI
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
        return Ok(Some(ResolvedIndexSource {
            kind: IndexSourceKind::Path(path.to_path_buf()),
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
        }));
    }
    Ok(config.registry.source.as_ref().map(|src| match src {
        EffectiveRegistrySource::Path(value) => ResolvedIndexSource {
            kind: IndexSourceKind::Path(value.value.clone()),
        },
        EffectiveRegistrySource::Url(value) => ResolvedIndexSource {
            kind: IndexSourceKind::Url(value.value.clone()),
        },
    }))
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
/// `apply_source_replacement`. The pre-check only sees the source
/// the user requested; a `[source-replacement]` entry can still
/// rewrite an `index-path` into an `index-url` later in the
/// pipeline, and the artifact loader would happily open it. This
/// check closes that gap.
///
/// Takes the typed [`cabin_core::SourceReplacementResolution`] so
/// it can give an accurate error: a non-empty `hops` list means
/// replacement actually fired, and the message can name the
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
/// local-index check. The pre-replacement check at the call site
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

/// Resolve the build directory the CLI should use for a build
/// invocation, consulting CLI flag → env var → config →
/// built-in default in that order.
///
/// `cli_value` is `Some(p)` only when the user actually passed
/// `--build-dir`; the clap default lives in the helper so an
/// explicit `--build-dir build` is still recognized as a CLI
/// choice and beats the env layer. Precedence: `--build-dir`,
/// then [`cabin_env::CABIN_BUILD_DIR`], then `[paths] build-dir`,
/// then the built-in default (`build`). The returned
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
        return (setting.absolute(), config_value_source(setting.source));
    }
    (PathBuf::from("build"), ConfigValueSource::BuiltinDefault)
}

/// Resolve the build-jobs setting for a build invocation.
///
/// Precedence: CLI `--jobs` > [`cabin_env::CABIN_BUILD_JOBS`]
/// env var > `[build] jobs` config setting > backend default
/// (`None` — the Ninja runner omits `-j` and Ninja picks its
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
        let raw = raw.to_string_lossy().into_owned();
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

/// Resolve the cache directory for a build / fetch invocation.
///
/// Precedence: CLI `--cache-dir` > [`cabin_env::CABIN_CACHE_DIR`]
/// env var > `[paths] cache-dir` config setting > `None` (the
/// caller keeps its existing default behavior). Mirrors the
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
    config
        .paths
        .cache_dir
        .as_ref()
        .map(|setting| (setting.absolute(), config_value_source(setting.source)))
}

/// Apply config-supplied profile defaults. CLI flags (handled
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
/// config-derived setting. `None` is rendered as `null` in the
/// metadata view so the field is always present.
pub(crate) fn config_view_json(config: &EffectiveConfig) -> serde_json::Value {
    let loaded_files: Vec<serde_json::Value> = config
        .loaded_files
        .iter()
        .map(|file| {
            serde_json::json!({
                "source": file.source.as_key(),
                "path": file.path.display().to_string(),
            })
        })
        .collect();

    let registry = match &config.registry.source {
        Some(EffectiveRegistrySource::Path(value)) => serde_json::json!({
            "kind": "path",
            "value": value.value.display().to_string(),
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
            "value": s.value.display().to_string(),
            "absolute": s.absolute().display().to_string(),
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
        let err = match resolve_index_source(None, Some("https://user:pw@bad.example.com/"), &cfg) {
            Ok(_) => panic!("expected credential rejection"),
            Err(e) => e,
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
                path: PathBuf::from(path),
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
                path: PathBuf::from("./mirror"),
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
                path: PathBuf::from("./mirror"),
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
            value: PathBuf::from(value),
            source,
            base: PathBuf::from("/base"),
        });
        cfg
    }

    fn cfg_with_build_dir(value: &str, source: ConfigSource) -> EffectiveConfig {
        let mut cfg = EffectiveConfig::default();
        cfg.paths.build_dir = Some(EffectivePathSetting {
            value: PathBuf::from(value),
            source,
            base: PathBuf::from("/base"),
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
                path: PathBuf::from("./mirror"),
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
}
