//! Resolve a [`cabin_core::ResolvedToolchain`] from layered
//! inputs.
//!
//! Precedence, applied per [`cabin_core::ToolKind`]:
//!
//! 1. CLI flag (`--cc`, `--cxx`, `--ar`).
//! 2. Environment variable (`CC`, `CXX`, `AR`). Empty values
//!    count as unset.
//! 3. Matching `[target.'cfg(...)'.toolchain]` block in the
//!    workspace root manifest.
//! 4. `[toolchain]` table in the workspace root manifest.
//! 5. Cabin's built-in default lookup (`c++` / `clang++` / `g++`
//!    for the C++ compiler; `cc` / `clang` / `gcc` for the C
//!    compiler; `ar` for the archiver).
//!
//! The first layer that yields a non-empty spec wins. Whichever
//! layer wins is recorded on the resulting [`cabin_core::ResolvedTool`]
//! via [`cabin_core::ToolSource`].

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use cabin_core::{
    ConditionalToolchainDecl, ResolvedTool, ResolvedToolchain, TargetPlatform, ToolKind,
    ToolSource, ToolSpec, ToolchainResolutionError, ToolchainSelection, ToolchainSettings,
};

use crate::path_search::{find_with_exe_suffix, search_path};

/// Deterministic environment lookup the resolver consults for
/// `CC` / `CXX` / `AR` / `PATH`. Production callers wrap
/// `std::env::var_os`; tests inject a hash-map-backed closure.
pub type EnvLookup<'a> = Box<dyn Fn(&str) -> Option<OsString> + 'a>;

/// Predicate the resolver uses to check whether a candidate path
/// points at an existing executable. Production callers wrap
/// `Path::is_file`; tests pass a `HashSet<PathBuf>`-backed closure.
pub type ExecutableProbe<'a> = Box<dyn Fn(&Path) -> bool + 'a>;

/// Toolchain inputs the resolver consumes. Production callers use
/// [`Inputs::from_process`]; tests inject a fake env via the more
/// granular constructor.
pub struct Inputs<'a> {
    pub selection: &'a ToolchainSelection,
    /// Optional config-supplied layer that slots between
    /// environment variables and the manifest. Typically built by
    /// `cabin` from the merged effective config; per-tool
    /// fields each carry their own config-source label so the
    /// resolved [`ResolvedTool`] can attribute the value
    /// correctly.
    pub config: Option<&'a ConfigToolchainLayer>,
    pub manifest: &'a ToolchainSettings,
    pub host_platform: &'a TargetPlatform,
    pub env: EnvLookup<'a>,
    pub probe: ExecutableProbe<'a>,
}

/// Per-tool config-derived layer for the precedence walker. Each
/// field is independent so a single config layer can mix sources
/// (e.g., user config sets `cxx`, workspace config sets `ar`).
#[derive(Debug, Clone, Default)]
pub struct ConfigToolchainLayer {
    pub cc: Option<ConfigToolEntry>,
    pub cxx: Option<ConfigToolEntry>,
    pub ar: Option<ConfigToolEntry>,
}

impl ConfigToolchainLayer {
    /// Whether the layer carries no fields at all. Useful so
    /// callers can avoid threading an entirely empty layer
    /// through.
    pub fn is_empty(&self) -> bool {
        self.cc.is_none() && self.cxx.is_none() && self.ar.is_none()
    }
}

/// One config-derived tool entry. `source` must be a config-flavor
/// variant of [`ToolSource`] (`UserConfig` / `WorkspaceConfig` /
/// `PackageConfig` / `ExplicitConfig`); the resolver propagates
/// it onto the resulting [`ResolvedTool`] so metadata reports the
/// exact file the value came from.
#[derive(Debug, Clone)]
pub struct ConfigToolEntry {
    pub spec: ToolSpec,
    pub source: ToolSource,
}

impl<'a> Inputs<'a> {
    /// Inputs that read environment variables from the running
    /// process and check executables on disk.
    pub fn from_process(
        selection: &'a ToolchainSelection,
        manifest: &'a ToolchainSettings,
        host_platform: &'a TargetPlatform,
    ) -> Self {
        Self {
            selection,
            config: None,
            manifest,
            host_platform,
            env: Box::new(|var| std::env::var_os(var)),
            probe: Box::new(Path::is_file),
        }
    }

    /// Builder-style setter for the optional config layer. Keeps
    /// `from_process` callers concise when no config is active.
    pub fn with_config(mut self, layer: &'a ConfigToolchainLayer) -> Self {
        self.config = Some(layer);
        self
    }
}

/// Resolve a [`ResolvedToolchain`] from the supplied inputs.
///
/// `cxx` and `ar` are always required; the resolver fails fast
/// when the user's system has neither an explicit selection nor
/// a default fallback for them. `cc` is best-effort: an explicit
/// selection (CLI flag, `CC` env var, `[toolchain]` table) must
/// resolve, and the documented fallback list is also tried so a
/// system C compiler is picked up without ceremony, but a
/// missing `cc` is *not* a hard error here. The planner surfaces
/// a precise "missing C compiler" diagnostic when (and only
/// when) a target carries `.c` sources and `cc` is `None`.
pub fn resolve_toolchain(
    inputs: &Inputs<'_>,
) -> Result<ResolvedToolchain, ToolchainResolutionError> {
    let cxx = resolve_kind(ToolKind::CxxCompiler, inputs, true)?
        .expect("required tool returned Some on success");
    let ar = resolve_kind(ToolKind::Archiver, inputs, true)?
        .expect("required tool returned Some on success");
    let cc = resolve_kind(ToolKind::CCompiler, inputs, false)?;
    Ok(ResolvedToolchain { cxx, ar, cc })
}

fn resolve_kind(
    kind: ToolKind,
    inputs: &Inputs<'_>,
    required: bool,
) -> Result<Option<ResolvedTool>, ToolchainResolutionError> {
    if let Some((spec, source)) = pick_explicit(kind, inputs) {
        let path = locate(&spec, &inputs.env, &inputs.probe).ok_or_else(|| {
            ToolchainResolutionError::ToolNotFound {
                kind,
                spec: spec.display(),
                selected_from: source,
            }
        })?;
        reject_unsupported_compiler(kind, &spec)?;
        return Ok(Some(ResolvedTool {
            kind,
            path,
            spec,
            source,
        }));
    }

    for candidate in default_fallbacks(kind) {
        let spec = ToolSpec::Name((*candidate).to_owned());
        if let Some(path) = locate(&spec, &inputs.env, &inputs.probe) {
            return Ok(Some(ResolvedTool {
                kind,
                path,
                spec,
                source: ToolSource::Default,
            }));
        }
    }
    if required {
        Err(ToolchainResolutionError::NoDefault { kind })
    } else {
        Ok(None)
    }
}

fn pick_explicit(kind: ToolKind, inputs: &Inputs<'_>) -> Option<(ToolSpec, ToolSource)> {
    let cli_slot = match kind {
        ToolKind::CCompiler => &inputs.selection.cc,
        ToolKind::CxxCompiler => &inputs.selection.cxx,
        ToolKind::Archiver => &inputs.selection.ar,
    };
    if let Some(spec) = &cli_slot.cli {
        return Some((spec.clone(), ToolSource::Cli));
    }

    if let Some(value) = (inputs.env)(env_var_for(kind))
        && !value.is_empty()
    {
        let spec = ToolSpec::parse(value.to_string_lossy().into_owned());
        return Some((spec, ToolSource::Env));
    }

    if let Some(layer) = inputs.config {
        let entry = match kind {
            ToolKind::CCompiler => &layer.cc,
            ToolKind::CxxCompiler => &layer.cxx,
            ToolKind::Archiver => &layer.ar,
        };
        if let Some(entry) = entry {
            return Some((entry.spec.clone(), entry.source));
        }
    }

    for cond in &inputs.manifest.conditional {
        if matches_condition(cond, inputs.host_platform)
            && let Some(spec) = cond.toolchain.get(kind)
        {
            return Some((spec.clone(), ToolSource::ManifestConditional));
        }
    }

    if let Some(spec) = inputs.manifest.general.get(kind) {
        return Some((spec.clone(), ToolSource::Manifest));
    }

    None
}

fn matches_condition(cond: &ConditionalToolchainDecl, platform: &TargetPlatform) -> bool {
    cond.condition.evaluate(platform)
}

fn env_var_for(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::CCompiler => "CC",
        ToolKind::CxxCompiler => "CXX",
        ToolKind::Archiver => "AR",
    }
}

fn default_fallbacks(kind: ToolKind) -> &'static [&'static str] {
    match kind {
        ToolKind::CCompiler => &["cc", "clang", "gcc"],
        ToolKind::CxxCompiler => &["c++", "clang++", "g++"],
        ToolKind::Archiver => &["ar"],
    }
}

fn reject_unsupported_compiler(
    kind: ToolKind,
    spec: &ToolSpec,
) -> Result<(), ToolchainResolutionError> {
    if !matches!(kind, ToolKind::CCompiler | ToolKind::CxxCompiler) {
        return Ok(());
    }
    let display = spec.display();
    let basename = Path::new(&display)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&display)
        .to_ascii_lowercase();
    let stem = basename.trim_end_matches(".exe");
    if matches!(stem, "cl" | "link" | "lib") {
        return Err(ToolchainResolutionError::UnsupportedCompiler {
            kind,
            spec: spec.display(),
        });
    }
    Ok(())
}

fn locate<F, P>(spec: &ToolSpec, env: &F, probe: &P) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<OsString> + ?Sized,
    P: Fn(&Path) -> bool + ?Sized,
{
    match spec {
        ToolSpec::Path(path) => {
            if probe(path) {
                Some(path.clone())
            } else {
                find_with_exe_suffix(path, probe)
            }
        }
        ToolSpec::Name(name) => {
            let path = Path::new(name);
            if path.is_absolute() || looks_like_relative_path(name) {
                if probe(path) {
                    return Some(path.to_path_buf());
                }
                return find_with_exe_suffix(path, probe);
            }
            search_path(name, env, probe)
        }
    }
}

fn looks_like_relative_path(name: &str) -> bool {
    name.contains('/') || (cfg!(windows) && name.contains('\\'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::{ToolchainDecl, ToolchainSelection};
    use std::collections::HashSet;

    fn host() -> TargetPlatform {
        let mut p = TargetPlatform::current();
        p.os = "linux".into();
        p
    }

    fn make_inputs<'a>(
        selection: &'a ToolchainSelection,
        manifest: &'a ToolchainSettings,
        platform: &'a TargetPlatform,
        env: std::collections::HashMap<&'static str, OsString>,
        existing: HashSet<PathBuf>,
    ) -> Inputs<'a> {
        Inputs {
            selection,
            config: None,
            manifest,
            host_platform: platform,
            env: Box::new(move |k| env.get(k).cloned()),
            probe: Box::new(move |p| existing.contains(p)),
        }
    }

    fn path_set(items: &[&str]) -> HashSet<PathBuf> {
        items.iter().map(PathBuf::from).collect()
    }

    fn fake_env(
        items: &[(&'static str, &str)],
    ) -> std::collections::HashMap<&'static str, OsString> {
        let mut env = std::collections::HashMap::new();
        for (k, v) in items {
            env.insert(*k, OsString::from(*v));
        }
        env
    }

    #[test]
    fn defaults_pick_first_existing_compiler() {
        let selection = ToolchainSelection::default();
        let manifest = ToolchainSettings::default();
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&["/usr/bin/clang++", "/usr/bin/ar"]);
        let inputs = make_inputs(&selection, &manifest, &host, env, existing);
        let resolved = resolve_toolchain(&inputs).unwrap();
        assert_eq!(resolved.cxx.path, PathBuf::from("/usr/bin/clang++"));
        assert_eq!(resolved.cxx.source, ToolSource::Default);
        assert_eq!(resolved.ar.source, ToolSource::Default);
        assert!(resolved.cc.is_none());
    }

    #[test]
    fn cli_overrides_env_and_manifest() {
        let mut manifest = ToolchainSettings::default();
        manifest.general.cxx = Some(ToolSpec::Name("g++".into()));
        let selection = ToolchainSelection::default()
            .with_cli(ToolKind::CxxCompiler, ToolSpec::Name("clang++".into()));
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin"), ("CXX", "/usr/bin/g++")]);
        let existing = path_set(&["/usr/bin/clang++", "/usr/bin/g++", "/usr/bin/ar"]);
        let inputs = make_inputs(&selection, &manifest, &host, env, existing);
        let r = resolve_toolchain(&inputs).unwrap();
        assert_eq!(r.cxx.path, PathBuf::from("/usr/bin/clang++"));
        assert_eq!(r.cxx.source, ToolSource::Cli);
    }

    #[test]
    fn env_overrides_manifest() {
        let mut manifest = ToolchainSettings::default();
        manifest.general.cxx = Some(ToolSpec::Name("g++".into()));
        let selection = ToolchainSelection::default();
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin"), ("CXX", "/usr/bin/clang++")]);
        let existing = path_set(&["/usr/bin/clang++", "/usr/bin/g++", "/usr/bin/ar"]);
        let inputs = make_inputs(&selection, &manifest, &host, env, existing);
        let r = resolve_toolchain(&inputs).unwrap();
        assert_eq!(r.cxx.path, PathBuf::from("/usr/bin/clang++"));
        assert_eq!(r.cxx.source, ToolSource::Env);
    }

    #[test]
    fn empty_env_is_treated_as_unset() {
        let mut manifest = ToolchainSettings::default();
        manifest.general.cxx = Some(ToolSpec::Name("g++".into()));
        let selection = ToolchainSelection::default();
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin"), ("CXX", "")]);
        let existing = path_set(&["/usr/bin/g++", "/usr/bin/ar"]);
        let inputs = make_inputs(&selection, &manifest, &host, env, existing);
        let r = resolve_toolchain(&inputs).unwrap();
        assert_eq!(r.cxx.source, ToolSource::Manifest);
        assert_eq!(r.cxx.path, PathBuf::from("/usr/bin/g++"));
    }

    #[test]
    fn matching_target_cfg_overrides_general_manifest() {
        let manifest = ToolchainSettings {
            general: ToolchainDecl {
                cxx: Some(ToolSpec::Name("g++".into())),
                ..Default::default()
            },
            conditional: vec![ConditionalToolchainDecl {
                condition: cabin_core::Condition::KeyValue {
                    key: cabin_core::ConditionKey::Os,
                    value: "linux".into(),
                },
                toolchain: ToolchainDecl {
                    cxx: Some(ToolSpec::Name("clang++".into())),
                    ..Default::default()
                },
            }],
        };
        let selection = ToolchainSelection::default();
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&["/usr/bin/clang++", "/usr/bin/g++", "/usr/bin/ar"]);
        let inputs = make_inputs(&selection, &manifest, &host, env, existing);
        let r = resolve_toolchain(&inputs).unwrap();
        assert_eq!(r.cxx.path, PathBuf::from("/usr/bin/clang++"));
        assert_eq!(r.cxx.source, ToolSource::ManifestConditional);
    }

    #[test]
    fn non_matching_target_cfg_is_skipped() {
        let manifest = ToolchainSettings {
            general: ToolchainDecl {
                cxx: Some(ToolSpec::Name("g++".into())),
                ..Default::default()
            },
            conditional: vec![ConditionalToolchainDecl {
                condition: cabin_core::Condition::KeyValue {
                    key: cabin_core::ConditionKey::Os,
                    value: "macos".into(),
                },
                toolchain: ToolchainDecl {
                    cxx: Some(ToolSpec::Name("clang++".into())),
                    ..Default::default()
                },
            }],
        };
        let selection = ToolchainSelection::default();
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&["/usr/bin/clang++", "/usr/bin/g++", "/usr/bin/ar"]);
        let inputs = make_inputs(&selection, &manifest, &host, env, existing);
        let r = resolve_toolchain(&inputs).unwrap();
        assert_eq!(r.cxx.path, PathBuf::from("/usr/bin/g++"));
        assert_eq!(r.cxx.source, ToolSource::Manifest);
    }

    #[test]
    fn missing_explicit_cxx_errors_clearly() {
        let selection = ToolchainSelection::default()
            .with_cli(ToolKind::CxxCompiler, ToolSpec::Name("clang++-99".into()));
        let manifest = ToolchainSettings::default();
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&["/usr/bin/g++", "/usr/bin/ar"]);
        let inputs = make_inputs(&selection, &manifest, &host, env, existing);
        let err = resolve_toolchain(&inputs).unwrap_err();
        match err {
            ToolchainResolutionError::ToolNotFound {
                kind,
                spec,
                selected_from,
            } => {
                assert_eq!(kind, ToolKind::CxxCompiler);
                assert_eq!(spec, "clang++-99");
                assert_eq!(selected_from, ToolSource::Cli);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn cl_exe_is_rejected_with_clear_error() {
        let selection = ToolchainSelection::default()
            .with_cli(ToolKind::CxxCompiler, ToolSpec::Name("cl.exe".into()));
        let manifest = ToolchainSettings::default();
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&["/usr/bin/cl.exe", "/usr/bin/ar"]);
        let inputs = make_inputs(&selection, &manifest, &host, env, existing);
        let err = resolve_toolchain(&inputs).unwrap_err();
        assert!(matches!(
            err,
            ToolchainResolutionError::UnsupportedCompiler {
                kind: ToolKind::CxxCompiler,
                ..
            }
        ));
    }

    #[test]
    fn no_compiler_anywhere_returns_no_default() {
        let selection = ToolchainSelection::default();
        let manifest = ToolchainSettings::default();
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&["/usr/bin/ar"]);
        let inputs = make_inputs(&selection, &manifest, &host, env, existing);
        let err = resolve_toolchain(&inputs).unwrap_err();
        assert!(matches!(
            err,
            ToolchainResolutionError::NoDefault {
                kind: ToolKind::CxxCompiler
            }
        ));
    }

    #[test]
    fn config_layer_wins_when_no_cli_or_env_value_is_set() {
        let selection = ToolchainSelection::default();
        let mut manifest = ToolchainSettings::default();
        manifest.general.cxx = Some(ToolSpec::Name("g++".into()));
        let host = host();
        let layer = ConfigToolchainLayer {
            cxx: Some(ConfigToolEntry {
                spec: ToolSpec::Name("clang++".into()),
                source: ToolSource::WorkspaceConfig,
            }),
            ..Default::default()
        };
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&["/usr/bin/clang++", "/usr/bin/g++", "/usr/bin/ar"]);
        let mut inputs = make_inputs(&selection, &manifest, &host, env, existing);
        inputs.config = Some(&layer);
        let resolved = resolve_toolchain(&inputs).unwrap();
        assert_eq!(resolved.cxx.path, PathBuf::from("/usr/bin/clang++"));
        assert_eq!(resolved.cxx.source, ToolSource::WorkspaceConfig);
    }

    #[test]
    fn env_overrides_config_layer() {
        let selection = ToolchainSelection::default();
        let manifest = ToolchainSettings::default();
        let host = host();
        let layer = ConfigToolchainLayer {
            cxx: Some(ConfigToolEntry {
                spec: ToolSpec::Name("g++".into()),
                source: ToolSource::UserConfig,
            }),
            ..Default::default()
        };
        let env = fake_env(&[("PATH", "/usr/bin"), ("CXX", "/usr/bin/clang++")]);
        let existing = path_set(&["/usr/bin/clang++", "/usr/bin/g++", "/usr/bin/ar"]);
        let mut inputs = make_inputs(&selection, &manifest, &host, env, existing);
        inputs.config = Some(&layer);
        let resolved = resolve_toolchain(&inputs).unwrap();
        assert_eq!(resolved.cxx.source, ToolSource::Env);
        assert_eq!(resolved.cxx.path, PathBuf::from("/usr/bin/clang++"));
    }

    #[test]
    fn config_layer_overrides_manifest() {
        let selection = ToolchainSelection::default();
        let mut manifest = ToolchainSettings::default();
        manifest.general.cxx = Some(ToolSpec::Name("g++".into()));
        let host = host();
        let layer = ConfigToolchainLayer {
            cxx: Some(ConfigToolEntry {
                spec: ToolSpec::Name("clang++".into()),
                source: ToolSource::PackageConfig,
            }),
            ..Default::default()
        };
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&["/usr/bin/clang++", "/usr/bin/g++", "/usr/bin/ar"]);
        let mut inputs = make_inputs(&selection, &manifest, &host, env, existing);
        inputs.config = Some(&layer);
        let resolved = resolve_toolchain(&inputs).unwrap();
        assert_eq!(resolved.cxx.source, ToolSource::PackageConfig);
        assert_eq!(resolved.cxx.path, PathBuf::from("/usr/bin/clang++"));
    }

    #[test]
    fn explicit_cc_is_resolved_when_requested() {
        let selection = ToolchainSelection::default()
            .with_cli(ToolKind::CCompiler, ToolSpec::Name("clang".into()));
        let manifest = ToolchainSettings::default();
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&["/usr/bin/clang", "/usr/bin/c++", "/usr/bin/ar"]);
        let inputs = make_inputs(&selection, &manifest, &host, env, existing);
        let r = resolve_toolchain(&inputs).unwrap();
        let cc = r.cc.expect("explicit C compiler resolved");
        assert_eq!(cc.path, PathBuf::from("/usr/bin/clang"));
        assert_eq!(cc.source, ToolSource::Cli);
    }
}
