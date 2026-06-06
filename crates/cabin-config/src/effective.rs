//! Merge a stack of [`crate::LoadedConfigFile`]s into a typed
//! [`EffectiveConfig`].
//!
//! Caller orders files lowest-priority first; the merger walks the
//! list once and lets later (higher-priority) files override
//! earlier ones field-by-field. Every effective value records
//! which file it ultimately came from so `cabin metadata` can
//! report provenance.

use std::collections::BTreeMap;

use camino::Utf8PathBuf;

use cabin_core::{
    ColorChoice, CompilerWrapperRequest, PackageName, PatchSource, SourceReplacementEntry,
    SourceReplacementSettings, ToolSpec, Verbosity,
};

use crate::parse::{ParsedConfig, ParsedRegistry};
use crate::source::{ConfigSource, LoadedConfigFile, SourcedValue};

/// Fully merged config consumed by the rest of the workspace.
///
/// Every leaf field is `Option<SourcedValue<...>>`: `None` means
/// no config file declared a value at all, in which case higher
/// layers (CLI, env, manifest, built-in defaults) take over via
/// the existing per-feature precedence chain.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EffectiveConfig {
    pub registry: EffectiveRegistry,
    pub paths: EffectivePaths,
    pub build: EffectiveBuild,
    pub toolchain: EffectiveToolchain,
    pub compiler_wrapper: Option<EffectiveCompilerWrapper>,
    pub term: EffectiveTerm,
    /// Active config-derived patches keyed by package name.
    /// Higher-priority files override lower files on overlap.
    pub patches: BTreeMap<PackageName, EffectivePatch>,
    /// Active config-derived source replacements keyed by the
    /// original [`cabin_core::SourceLocator`].
    pub source_replacements: SourceReplacementSettings,
    pub loaded_files: Vec<LoadedConfigFile>,
}

/// `[term]` view, after merging.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EffectiveTerm {
    pub color: Option<EffectiveColor>,
    pub verbosity: Option<EffectiveVerbosity>,
}

/// One resolved `term.color` value plus the file it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveColor {
    pub choice: ColorChoice,
    pub source: ConfigSource,
}

/// One resolved `term.verbose` / `term.quiet` pair plus the file
/// it came from.  The boolean pair is folded into a single
/// [`Verbosity`] at parse time; the merger only attaches the
/// source attribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveVerbosity {
    pub level: Verbosity,
    pub source: ConfigSource,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EffectiveRegistry {
    pub source: Option<EffectiveRegistrySource>,
}

/// Resolved registry source. Mirrors the crate-internal
/// `ParsedRegistry` but adds source-attribution so consumers can
/// tell which file the value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectiveRegistrySource {
    Path(SourcedValue<Utf8PathBuf>),
    Url(SourcedValue<String>),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EffectivePaths {
    pub cache_dir: Option<EffectivePathSetting>,
    pub build_dir: Option<EffectivePathSetting>,
}

/// One path setting from a config file. `value` is left as the
/// path the user wrote — relative paths are resolved against the
/// `base` directory at the consumption site (see
/// [`EffectivePathSetting::absolute`]) so the merge stage stays
/// pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectivePathSetting {
    pub value: Utf8PathBuf,
    pub source: ConfigSource,
    /// Directory the config file lived in. Relative `value`s
    /// resolve against this directory.
    pub base: Utf8PathBuf,
}

impl EffectivePathSetting {
    /// Concrete absolute (or root-relative) path. Relative
    /// `value`s join with `base`; absolute paths pass through.
    pub fn absolute(&self) -> Utf8PathBuf {
        resolve_relative(&self.base, &self.value)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EffectiveBuild {
    pub profile: Option<EffectiveProfile>,
    pub jobs: Option<EffectiveBuildJobs>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveProfile {
    pub name: String,
    pub source: ConfigSource,
}

/// Resolved `[build] jobs` value plus the file it came from.
/// The typed [`cabin_core::BuildJobs`] inner value rules out a
/// zero / negative count at the merge boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveBuildJobs {
    pub value: cabin_core::BuildJobs,
    pub source: ConfigSource,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EffectiveToolchain {
    pub cc: Option<EffectiveTool>,
    pub cxx: Option<EffectiveTool>,
    pub ar: Option<EffectiveTool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveTool {
    pub spec: ToolSpec,
    pub source: ConfigSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveCompilerWrapper {
    pub request: CompilerWrapperRequest,
    pub source: ConfigSource,
}

/// One config-derived patch entry, ready for the orchestration
/// layer to consume. Carries the source value as the user wrote
/// it plus the directory of the config file that declared it so
/// callers can absolutise relative paths against the right base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectivePatch {
    pub spec: PatchSource,
    pub source: ConfigSource,
    /// Absolute path of the config file that declared this
    /// patch. Used to resolve relative `path = "..."` values.
    pub declared_in: Utf8PathBuf,
}

/// Merge the supplied loaded files in order. Caller is
/// responsible for ordering (lowest priority first).
pub fn merge_loaded_files(loaded: Vec<LoadedConfigFile>) -> EffectiveConfig {
    let mut effective = EffectiveConfig::default();
    for file in &loaded {
        apply_file(&mut effective, file);
    }
    effective.loaded_files = loaded;
    effective
}

fn apply_file(effective: &mut EffectiveConfig, file: &LoadedConfigFile) {
    let base = file
        .path
        .parent()
        .map(Utf8Path::to_path_buf)
        .unwrap_or_default();
    apply_parsed(effective, file.source, &base, &file.parsed, file);
}

fn apply_parsed(
    effective: &mut EffectiveConfig,
    source: ConfigSource,
    base: &Utf8Path,
    parsed: &ParsedConfig,
    file: &LoadedConfigFile,
) {
    if let Some(reg) = &parsed.registry {
        effective.registry.source = Some(match reg {
            ParsedRegistry::Path(path) => EffectiveRegistrySource::Path(SourcedValue::new(
                resolve_relative(base, path),
                source,
            )),
            ParsedRegistry::Url(url) => {
                EffectiveRegistrySource::Url(SourcedValue::new(url.clone(), source))
            }
        });
    }
    if let Some(cache) = &parsed.paths.cache_dir {
        effective.paths.cache_dir = Some(EffectivePathSetting {
            value: cache.clone(),
            source,
            base: base.to_path_buf(),
        });
    }
    if let Some(build) = &parsed.paths.build_dir {
        effective.paths.build_dir = Some(EffectivePathSetting {
            value: build.clone(),
            source,
            base: base.to_path_buf(),
        });
    }
    if let Some(profile) = &parsed.build.profile {
        effective.build.profile = Some(EffectiveProfile {
            name: profile.clone(),
            source,
        });
    }
    if let Some(jobs) = parsed.build.jobs {
        effective.build.jobs = Some(EffectiveBuildJobs {
            value: jobs,
            source,
        });
    }
    if let Some(wrapper) = &parsed.build.compiler_wrapper {
        effective.compiler_wrapper = Some(EffectiveCompilerWrapper {
            request: *wrapper,
            source,
        });
    }
    if let Some(spec) = &parsed.toolchain.cc {
        effective.toolchain.cc = Some(EffectiveTool {
            spec: spec.clone(),
            source,
        });
    }
    if let Some(spec) = &parsed.toolchain.cxx {
        effective.toolchain.cxx = Some(EffectiveTool {
            spec: spec.clone(),
            source,
        });
    }
    if let Some(spec) = &parsed.toolchain.ar {
        effective.toolchain.ar = Some(EffectiveTool {
            spec: spec.clone(),
            source,
        });
    }
    if let Some(choice) = parsed.term.color {
        effective.term.color = Some(EffectiveColor { choice, source });
    }
    if let Some(level) = parsed.term.verbosity {
        effective.term.verbosity = Some(EffectiveVerbosity { level, source });
    }
    for (package, spec) in &parsed.patches {
        effective.patches.insert(
            package.clone(),
            EffectivePatch {
                spec: spec.clone(),
                source,
                declared_in: file.path.clone(),
            },
        );
    }
    for (original, parsed_replacement) in &parsed.source_replacements {
        effective.source_replacements.entries.insert(
            original.clone(),
            SourceReplacementEntry {
                original: original.clone(),
                replacement: parsed_replacement.replacement.clone(),
                provenance: source_to_value(source),
            },
        );
    }
}

fn source_to_value(source: ConfigSource) -> cabin_core::ConfigValueSource {
    match source {
        ConfigSource::User => cabin_core::ConfigValueSource::UserConfig,
        ConfigSource::Workspace => cabin_core::ConfigValueSource::WorkspaceConfig,
        ConfigSource::Package => cabin_core::ConfigValueSource::PackageConfig,
        ConfigSource::Explicit => cabin_core::ConfigValueSource::ExplicitConfig,
    }
}

fn resolve_relative(base: &Utf8Path, value: &Utf8Path) -> Utf8PathBuf {
    if value.is_absolute() {
        value.to_path_buf()
    } else {
        base.join(value)
    }
}

use camino::Utf8Path;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{ParsedBuild, ParsedConfig, ParsedPaths, ParsedToolchain};
    use cabin_core::CompilerWrapperKind;

    fn loaded(source: ConfigSource, path: &str, parsed: ParsedConfig) -> LoadedConfigFile {
        LoadedConfigFile {
            source,
            path: Utf8PathBuf::from(path),
            parsed,
        }
    }

    #[test]
    fn empty_input_yields_empty_effective_config() {
        let effective = merge_loaded_files(Vec::new());
        assert_eq!(effective, EffectiveConfig::default());
    }

    #[test]
    fn workspace_overrides_user_for_overlapping_keys() {
        let user = ParsedConfig {
            build: ParsedBuild {
                profile: Some("dev".into()),
                ..Default::default()
            },
            paths: ParsedPaths {
                cache_dir: Some(Utf8PathBuf::from("user-cache")),
                ..Default::default()
            },
            ..Default::default()
        };
        let workspace = ParsedConfig {
            build: ParsedBuild {
                profile: Some("release".into()),
                ..Default::default()
            },
            paths: ParsedPaths {
                build_dir: Some(Utf8PathBuf::from("ws-build")),
                ..Default::default()
            },
            ..Default::default()
        };
        let effective = merge_loaded_files(vec![
            loaded(ConfigSource::User, "/u/.config/cabin/config.toml", user),
            loaded(ConfigSource::Workspace, "/ws/.cabin/config.toml", workspace),
        ]);
        // Profile overlap: workspace wins.
        assert_eq!(
            effective.build.profile,
            Some(EffectiveProfile {
                name: "release".into(),
                source: ConfigSource::Workspace,
            })
        );
        // Non-overlap: user contributes cache-dir, workspace
        // contributes build-dir.
        let cache = effective.paths.cache_dir.expect("user cache-dir kept");
        assert_eq!(cache.source, ConfigSource::User);
        assert_eq!(cache.value, Utf8PathBuf::from("user-cache"));
        let build_dir = effective.paths.build_dir.expect("workspace build-dir kept");
        assert_eq!(build_dir.source, ConfigSource::Workspace);
    }

    #[test]
    fn relative_index_path_resolves_against_config_directory() {
        let parsed = ParsedConfig {
            registry: Some(ParsedRegistry::Path(Utf8PathBuf::from("registry"))),
            ..Default::default()
        };
        let effective = merge_loaded_files(vec![loaded(
            ConfigSource::Workspace,
            "/abs/ws/.cabin/config.toml",
            parsed,
        )]);
        match effective.registry.source {
            Some(EffectiveRegistrySource::Path(SourcedValue { value, source })) => {
                assert_eq!(value, Utf8PathBuf::from("/abs/ws/.cabin/registry"));
                assert_eq!(source, ConfigSource::Workspace);
            }
            other => panic!("expected resolved Path, got {other:?}"),
        }
    }

    #[test]
    fn absolute_index_path_passes_through_unmodified() {
        let parsed = ParsedConfig {
            registry: Some(ParsedRegistry::Path(Utf8PathBuf::from("/abs/registry"))),
            ..Default::default()
        };
        let effective = merge_loaded_files(vec![loaded(
            ConfigSource::User,
            "/u/.config/cabin/config.toml",
            parsed,
        )]);
        match effective.registry.source {
            Some(EffectiveRegistrySource::Path(SourcedValue { value, .. })) => {
                assert_eq!(value, Utf8PathBuf::from("/abs/registry"));
            }
            other => panic!("expected absolute Path, got {other:?}"),
        }
    }

    #[test]
    fn workspace_url_overrides_user_path_completely() {
        let user = ParsedConfig {
            registry: Some(ParsedRegistry::Path(Utf8PathBuf::from("user-index"))),
            ..Default::default()
        };
        let workspace = ParsedConfig {
            registry: Some(ParsedRegistry::Url("https://w.example.com/index".into())),
            ..Default::default()
        };
        let effective = merge_loaded_files(vec![
            loaded(ConfigSource::User, "/u/.config/cabin/config.toml", user),
            loaded(ConfigSource::Workspace, "/ws/.cabin/config.toml", workspace),
        ]);
        match effective.registry.source {
            Some(EffectiveRegistrySource::Url(SourcedValue { value, source })) => {
                assert_eq!(value, "https://w.example.com/index");
                assert_eq!(source, ConfigSource::Workspace);
            }
            other => panic!("expected Url override from workspace, got {other:?}"),
        }
    }

    #[test]
    fn toolchain_fields_track_per_field_source() {
        let user = ParsedConfig {
            toolchain: ParsedToolchain {
                cxx: Some(ToolSpec::Name("clang++".into())),
                ar: Some(ToolSpec::Name("ar".into())),
                ..Default::default()
            },
            ..Default::default()
        };
        let workspace = ParsedConfig {
            toolchain: ParsedToolchain {
                ar: Some(ToolSpec::Name("llvm-ar".into())),
                ..Default::default()
            },
            ..Default::default()
        };
        let effective = merge_loaded_files(vec![
            loaded(ConfigSource::User, "/u/.config/cabin/config.toml", user),
            loaded(ConfigSource::Workspace, "/ws/.cabin/config.toml", workspace),
        ]);
        // CXX comes from user (workspace did not declare it).
        let cxx = effective.toolchain.cxx.expect("user cxx kept");
        assert_eq!(cxx.source, ConfigSource::User);
        assert_eq!(cxx.spec, ToolSpec::Name("clang++".into()));
        // AR comes from workspace (override).
        let ar = effective.toolchain.ar.expect("workspace ar wins");
        assert_eq!(ar.source, ConfigSource::Workspace);
        assert_eq!(ar.spec, ToolSpec::Name("llvm-ar".into()));
    }

    #[test]
    fn compiler_wrapper_override_keeps_winning_source() {
        let user = ParsedConfig {
            build: ParsedBuild {
                compiler_wrapper: Some(CompilerWrapperRequest::Use {
                    wrapper: CompilerWrapperKind::Ccache,
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let workspace = ParsedConfig {
            build: ParsedBuild {
                compiler_wrapper: Some(CompilerWrapperRequest::Disabled),
                ..Default::default()
            },
            ..Default::default()
        };
        let effective = merge_loaded_files(vec![
            loaded(ConfigSource::User, "/u/.config/cabin/config.toml", user),
            loaded(ConfigSource::Workspace, "/ws/.cabin/config.toml", workspace),
        ]);
        let wrapper = effective.compiler_wrapper.expect("workspace wins");
        assert_eq!(wrapper.request, CompilerWrapperRequest::Disabled);
        assert_eq!(wrapper.source, ConfigSource::Workspace);
    }

    #[test]
    fn term_color_workspace_overrides_user() {
        use crate::parse::ParsedTerm;
        let user = ParsedConfig {
            term: ParsedTerm {
                color: Some(ColorChoice::Auto),
                verbosity: None,
            },
            ..Default::default()
        };
        let workspace = ParsedConfig {
            term: ParsedTerm {
                color: Some(ColorChoice::Always),
                verbosity: None,
            },
            ..Default::default()
        };
        let effective = merge_loaded_files(vec![
            loaded(ConfigSource::User, "/u/.config/cabin/config.toml", user),
            loaded(ConfigSource::Workspace, "/ws/.cabin/config.toml", workspace),
        ]);
        let term = effective.term.color.expect("merged color present");
        assert_eq!(term.choice, ColorChoice::Always);
        assert_eq!(term.source, ConfigSource::Workspace);
    }

    #[test]
    fn term_color_user_kept_when_workspace_silent() {
        use crate::parse::ParsedTerm;
        let user = ParsedConfig {
            term: ParsedTerm {
                color: Some(ColorChoice::Never),
                verbosity: None,
            },
            ..Default::default()
        };
        let workspace = ParsedConfig::default();
        let effective = merge_loaded_files(vec![
            loaded(ConfigSource::User, "/u/.config/cabin/config.toml", user),
            loaded(ConfigSource::Workspace, "/ws/.cabin/config.toml", workspace),
        ]);
        let term = effective.term.color.expect("user color survives");
        assert_eq!(term.choice, ColorChoice::Never);
        assert_eq!(term.source, ConfigSource::User);
    }

    #[test]
    fn effective_path_setting_resolves_relative_to_base() {
        let setting = EffectivePathSetting {
            value: Utf8PathBuf::from("artifacts"),
            source: ConfigSource::Workspace,
            base: Utf8PathBuf::from("/abs/ws/.cabin"),
        };
        assert_eq!(
            setting.absolute(),
            Utf8PathBuf::from("/abs/ws/.cabin/artifacts")
        );
    }

    #[test]
    fn effective_path_setting_passes_absolute_through() {
        let setting = EffectivePathSetting {
            value: Utf8PathBuf::from("/abs/cache"),
            source: ConfigSource::User,
            base: Utf8PathBuf::from("/u/.config/cabin"),
        };
        assert_eq!(setting.absolute(), Utf8PathBuf::from("/abs/cache"));
    }
}
