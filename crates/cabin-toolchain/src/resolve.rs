//! Resolve a [`cabin_core::ResolvedToolchain`] from layered
//! inputs.
//!
//! Precedence, applied per [`cabin_core::ToolKind`]:
//!
//! 1. CLI flag (`--cc`, `--cxx`, `--ar`).
//! 2. Environment variable (`CC`, `CXX`, `AR`).  Empty values
//!    count as unset.
//! 3. Config layer (`.cabin/config.toml` `[toolchain]`), when
//!    the caller supplies one.
//! 4. Matching `[target.'cfg(...)'.toolchain]` block in the
//!    workspace root manifest.
//! 5. `[toolchain]` table in the workspace root manifest.
//! 6. Cabin's built-in default lookup (`c++` / `clang++` / `g++`
//!    for the C++ compiler; `cc` / `clang` / `gcc` for the C
//!    compiler; `ar` for the archiver).  On Windows, `cl` replaces
//!    `c++` and `cc`, and the archiver list is `lib` / `llvm-lib` /
//!    `llvm-ar` / `ar`.
//!    When layers 1-5 chose a compiler for one language (the C++ one
//!    if both), the other slots first try the tool beside it, and a
//!    GCC-style choice skips the MSVC defaults (`cl`, `lib`,
//!    `llvm-lib`), so a defaulted slot does not mix in another
//!    toolchain.
//!
//! The first layer that yields a non-empty spec wins.  Whichever
//! layer wins is recorded on the resulting [`cabin_core::ResolvedTool`]
//! via [`cabin_core::ToolSource`].

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use camino::Utf8Path;

use cabin_core::{
    ConditionalToolchainDecl, ResolvedTool, ResolvedToolchain, TargetPlatform, ToolKind,
    ToolSource, ToolSpec, ToolchainResolutionError, ToolchainSelection, ToolchainSettings,
};

use crate::path_search::{find_with_exe_suffix, looks_like_relative_path, search_path};

/// Deterministic environment lookup the resolver consults for
/// `CC` / `CXX` / `AR` / `PATH`.  Production callers wrap
/// `std::env::var_os`; tests inject a hash-map-backed closure.
pub type EnvLookup<'a> = Box<dyn Fn(&str) -> Option<OsString> + 'a>;

/// Predicate the resolver uses to check whether a candidate path
/// points at an existing executable.  Production callers wrap
/// `Path::is_file`; tests pass a `HashSet<PathBuf>`-backed closure.
pub type ExecutableProbe<'a> = Box<dyn Fn(&Path) -> bool + 'a>;

/// Toolchain inputs the resolver consumes.  Production callers use
/// [`Inputs::from_process`]; tests build the struct directly with a
/// fake env and probe.
pub struct Inputs<'a> {
    pub selection: &'a ToolchainSelection,
    /// Optional config-supplied layer that slots between
    /// environment variables and the manifest.  Typically built by
    /// `cabin` from the merged effective config; per-tool
    /// fields each carry their own config-source label so the
    /// resolved [`ResolvedTool`] can attribute the value
    /// correctly.
    pub config: Option<&'a ConfigToolchainLayer>,
    pub manifest: &'a ToolchainSettings,
    pub platform: &'a TargetPlatform,
    pub env: EnvLookup<'a>,
    pub probe: ExecutableProbe<'a>,
}

/// Per-tool config-derived layer for the precedence walker.  Each
/// field is independent so a single config layer can mix sources
/// (e.g., user config sets `cxx`, workspace config sets `ar`).
#[derive(Debug, Clone, Default)]
pub struct ConfigToolchainLayer {
    pub cc: Option<ConfigToolEntry>,
    pub cxx: Option<ConfigToolEntry>,
    pub ar: Option<ConfigToolEntry>,
}

impl ConfigToolchainLayer {
    /// Whether the layer carries no fields at all.  Useful so
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
        platform: &'a TargetPlatform,
    ) -> Self {
        Self {
            selection,
            config: None,
            manifest,
            platform,
            env: Box::new(|var| std::env::var_os(var)),
            probe: Box::new(Path::is_file),
        }
    }

    /// Builder-style setter for the optional config layer.  Keeps
    /// `from_process` callers concise when no config is active.
    #[must_use]
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
/// missing `cc` is *not* a hard error here.  The planner surfaces
/// a precise "missing C compiler" diagnostic when (and only
/// when) a target carries `.c` sources and `cc` is `None`.
///
/// # Errors
/// Returns [`ToolchainResolutionError`]: `ToolNotFound` when an
/// explicitly selected tool cannot be located, `NoDefault` when a
/// required tool (`cxx` or `ar`) has neither an explicit selection
/// nor a resolvable default, `UnsupportedCompiler` when a
/// compiler slot names `link` or `lib` (the MSVC linker and archiver),
/// and `NonUtf8Path` when a `CC` / `CXX` / `AR` value or a located
/// tool path is not UTF-8.
pub fn resolve_toolchain(
    inputs: &Inputs<'_>,
) -> Result<ResolvedToolchain, ToolchainResolutionError> {
    let cxx = resolve_explicit(ToolKind::CxxCompiler, inputs)?;
    let cc = resolve_explicit(ToolKind::CCompiler, inputs)?;
    let lead = cxx.clone().or_else(|| cc.clone());
    let lead = lead.as_ref();
    let cxx = match cxx {
        Some(cxx) => cxx,
        None => resolve_default(ToolKind::CxxCompiler, lead, inputs)?.ok_or(
            ToolchainResolutionError::NoDefault {
                kind: ToolKind::CxxCompiler,
            },
        )?,
    };
    let ar = match resolve_explicit(ToolKind::Archiver, inputs)? {
        Some(ar) => ar,
        None => resolve_default(ToolKind::Archiver, lead, inputs)?.ok_or(
            ToolchainResolutionError::NoDefault {
                kind: ToolKind::Archiver,
            },
        )?,
    };
    let cc = match cc {
        Some(cc) => Some(cc),
        None => resolve_default(ToolKind::CCompiler, lead, inputs)?,
    };
    Ok(ResolvedToolchain { cxx, ar, cc })
}

/// Resolve `kind` from the first layer above the defaults that
/// chooses it, if any.
fn resolve_explicit(
    kind: ToolKind,
    inputs: &Inputs<'_>,
) -> Result<Option<ResolvedTool>, ToolchainResolutionError> {
    let Some((spec, source)) = pick_explicit(kind, inputs)? else {
        return Ok(None);
    };
    let path = locate(&spec, &inputs.env, &inputs.probe).ok_or_else(|| {
        ToolchainResolutionError::ToolNotFound {
            kind,
            spec: spec.display(),
            selected_from: source,
        }
    })?;
    reject_unsupported_compiler(kind, &spec)?;
    Ok(Some(ResolvedTool {
        kind,
        path: camino::Utf8PathBuf::from_path_buf(path)
            .map_err(|path| ToolchainResolutionError::NonUtf8Path { kind, path })?,
        spec,
        source,
    }))
}

/// Resolve `kind` from the defaults: the tool beside `lead`, the
/// compiler chosen for the other language, then the fallback list,
/// without the MSVC tools beside a GCC-style `lead`.
fn resolve_default(
    kind: ToolKind,
    lead: Option<&ResolvedTool>,
    inputs: &Inputs<'_>,
) -> Result<Option<ResolvedTool>, ToolchainResolutionError> {
    // Searched beside the lead's path, but recorded in the spelling the
    // lead was chosen in, so the fingerprint does not follow where the
    // toolchain is installed.
    let beside = lead.and_then(|lead| {
        let name = sibling(&lead.path, kind)?;
        let spec = match &lead.spec {
            ToolSpec::Path(path) => ToolSpec::Path(path.with_file_name(&name)),
            ToolSpec::Name(_) => ToolSpec::Name(name.clone()),
        };
        Some((ToolSpec::Path(lead.path.with_file_name(name)), spec))
    });
    let defaults = default_fallbacks(kind)
        .iter()
        .filter(|name| lead.is_none_or(|lead| msvc_style(lead.path.as_str()) || !msvc_style(name)))
        .map(|name| {
            let spec = ToolSpec::Name((*name).to_owned());
            (spec.clone(), spec)
        });
    for (search, spec) in beside.into_iter().chain(defaults) {
        if let Some(path) = locate(&search, &inputs.env, &inputs.probe) {
            return Ok(Some(ResolvedTool {
                kind,
                path: camino::Utf8PathBuf::from_path_buf(path)
                    .map_err(|path| ToolchainResolutionError::NonUtf8Path { kind, path })?,
                spec,
                source: ToolSource::Default,
            }));
        }
    }
    Ok(None)
}

fn pick_explicit(
    kind: ToolKind,
    inputs: &Inputs<'_>,
) -> Result<Option<(ToolSpec, ToolSource)>, ToolchainResolutionError> {
    let cli_slot = match kind {
        ToolKind::CCompiler => &inputs.selection.cc,
        ToolKind::CxxCompiler => &inputs.selection.cxx,
        ToolKind::Archiver => &inputs.selection.ar,
    };
    if let Some(spec) = &cli_slot.cli {
        return Ok(Some((spec.clone(), ToolSource::Cli)));
    }

    if let Some(value) = (inputs.env)(env_var_for(kind))
        && !value.is_empty()
    {
        // The env var names a tool (a bare command or a path) and
        // becomes Cabin-owned tool-resolution state, so reject a
        // non-UTF-8 value rather than lossily mangling it into a
        // spec that would resolve to the wrong tool.
        let raw = value
            .into_string()
            .map_err(|value| ToolchainResolutionError::NonUtf8Path {
                kind,
                path: PathBuf::from(value),
            })?;
        let spec = ToolSpec::parse(raw);
        return Ok(Some((spec, ToolSource::Env)));
    }

    if let Some(layer) = inputs.config {
        let entry = match kind {
            ToolKind::CCompiler => &layer.cc,
            ToolKind::CxxCompiler => &layer.cxx,
            ToolKind::Archiver => &layer.ar,
        };
        if let Some(entry) = entry {
            return Ok(Some((entry.spec.clone(), entry.source)));
        }
    }

    for cond in &inputs.manifest.conditional {
        if matches_condition(cond, inputs.platform)
            && let Some(spec) = cond.toolchain.get(kind)
        {
            return Ok(Some((spec.clone(), ToolSource::ManifestConditional)));
        }
    }

    if let Some(spec) = inputs.manifest.general.get(kind) {
        return Ok(Some((spec.clone(), ToolSource::Manifest)));
    }

    Ok(None)
}

fn matches_condition(cond: &ConditionalToolchainDecl, platform: &TargetPlatform) -> bool {
    // Toolchain `cfg(...)` selection is platform-only; feature and
    // compiler conditions are not supported on toolchain tables, so
    // the platform-only context is correct here.
    cond.condition
        .evaluate(&cabin_core::ConditionContext::platform_only(platform))
}

fn env_var_for(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::CCompiler => "CC",
        ToolKind::CxxCompiler => "CXX",
        ToolKind::Archiver => "AR",
    }
}

fn default_fallbacks(kind: ToolKind) -> &'static [&'static str] {
    // On Windows the default toolchain is MSVC: `cl` compiles both C and
    // C++, and `lib` is the static-library archiver.  The clang / GNU
    // names follow so an LLVM- or MinGW-based install still resolves.
    if cfg!(windows) {
        match kind {
            ToolKind::CCompiler => &["cl", "clang", "gcc"],
            ToolKind::CxxCompiler => &["cl", "clang++", "g++"],
            // MSVC `lib`, then LLVM's `llvm-lib`, then the GNU/BSD
            // `llvm-ar` / `ar`.  The two `/OUT:`-syntax archivers come
            // first because an MSVC-dialect build (`cl` or `clang-cl`)
            // needs one: an LLVM-only install commonly ships `clang-cl`
            // with `llvm-lib` but no Microsoft `lib.exe`, so without
            // `llvm-lib` here that build would fall through to `llvm-ar`
            // and be rejected as a mixed GNU/MSVC toolchain.  A GNU/Clang
            // build instead matches `llvm-ar` / `ar`, the `ar crs`-shaped
            // archivers it needs.
            ToolKind::Archiver => &["lib", "llvm-lib", "llvm-ar", "ar"],
        }
    } else {
        match kind {
            ToolKind::CCompiler => &["cc", "clang", "gcc"],
            ToolKind::CxxCompiler => &["c++", "clang++", "g++"],
            ToolKind::Archiver => &["ar"],
        }
    }
}

/// The C and C++ drivers of each GCC-style family.
const DRIVERS: [(&str, &str); 3] = [("gcc", "g++"), ("clang", "clang++"), ("cc", "c++")];

/// The file name of the `kind` tool beside `lead`, the compiler chosen
/// for the other language: the same target prefix and version suffix
/// with that family's other driver (`x86_64-w64-mingw32-g++` ->
/// `x86_64-w64-mingw32-gcc`, `clang++-18` -> `clang-18`), or its
/// archiver: `<prefix>-ar` for a target-prefixed driver, `llvm-ar`
/// beside Clang, `ar` beside GCC, and `lib` / `llvm-lib` beside `cl` /
/// `clang-cl`, which compile both languages.
fn sibling(lead: &Utf8Path, kind: ToolKind) -> Option<String> {
    let file = lead.file_name()?.to_ascii_lowercase();
    let (stem, exe) = file
        .strip_suffix(".exe")
        .map_or((file.as_str(), ""), |stem| (stem, ".exe"));
    let clang_cl_version = stem
        .strip_prefix("clang-cl")
        .filter(|rest| rest.is_empty() || rest.starts_with('-'));
    let name = match (stem, clang_cl_version, kind) {
        ("cl", _, ToolKind::Archiver) => format!("lib{exe}"),
        (_, Some(version), ToolKind::Archiver) => format!("llvm-lib{version}{exe}"),
        ("cl", ..) | (_, Some(_), _) => file.clone(),
        _ => {
            let mut parts: Vec<&str> = stem.split('-').collect();
            let at = parts
                .iter()
                .rposition(|part| DRIVERS.iter().any(|(c, cxx)| part == c || part == cxx))?;
            let (c, cxx) = DRIVERS
                .into_iter()
                .find(|(c, cxx)| parts[at] == *c || parts[at] == *cxx)?;
            // `lead` drives the other language; a name read otherwise
            // (`afl-clang-fast++`) has no sibling.
            match kind {
                ToolKind::CCompiler if parts[at] == cxx => parts[at] = c,
                ToolKind::CxxCompiler if parts[at] == c => parts[at] = cxx,
                ToolKind::CCompiler | ToolKind::CxxCompiler => return None,
                ToolKind::Archiver if at > 0 => {
                    parts.truncate(at);
                    parts.push("ar");
                }
                ToolKind::Archiver if c == "clang" => parts[at] = "llvm-ar",
                ToolKind::Archiver => parts = vec!["ar"],
            }
            format!("{}{exe}", parts.join("-"))
        }
    };
    Some(name)
}

/// Whether `name` (a tool's command or path) speaks the MSVC
/// command-line dialect.
fn msvc_style(name: &str) -> bool {
    let file = Utf8Path::new(name)
        .file_name()
        .unwrap_or(name)
        .to_ascii_lowercase();
    let stem = file.strip_suffix(".exe").unwrap_or(&file);
    // Distributions version the LLVM tools: `clang-cl-18`, `llvm-lib-18`.
    let unversioned = stem
        .rsplit_once('-')
        .filter(|(_, version)| version.bytes().all(|b| b.is_ascii_digit()))
        .map_or(stem, |(name, _)| name);
    matches!(unversioned, "cl" | "clang-cl" | "lib" | "llvm-lib")
}

fn reject_unsupported_compiler(
    kind: ToolKind,
    spec: &ToolSpec,
) -> Result<(), ToolchainResolutionError> {
    if !matches!(kind, ToolKind::CCompiler | ToolKind::CxxCompiler) {
        return Ok(());
    }
    let display = spec.display();
    let basename = Utf8Path::new(&display)
        .file_name()
        .unwrap_or(&display)
        .to_ascii_lowercase();
    let stem = basename.trim_end_matches(".exe");
    // `link` (the MSVC linker) and `lib` (the MSVC archiver) are never
    // compilers, so naming one for a C/C++ slot is a mistake caught here.
    if matches!(stem, "link" | "lib") {
        return Err(ToolchainResolutionError::UnsupportedCompiler {
            kind,
            spec: spec.display(),
        });
    }
    Ok(())
}

pub(crate) fn locate<F, P>(spec: &ToolSpec, env: &F, probe: &P) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<OsString> + ?Sized,
    P: Fn(&Path) -> bool + ?Sized,
{
    let found = match spec {
        ToolSpec::Path(path) => {
            if probe(path.as_std_path()) {
                Some(path.as_std_path().to_path_buf())
            } else {
                find_with_exe_suffix(path.as_std_path(), probe)
            }
        }
        ToolSpec::Name(name) => {
            let path = Path::new(name);
            if path.is_absolute() || looks_like_relative_path(name) {
                if probe(path) {
                    Some(path.to_path_buf())
                } else {
                    find_with_exe_suffix(path, probe)
                }
            } else {
                // On Windows without an activated MSVC environment, `cl` /
                // `lib` / `link` are not on `PATH`; fall back to discovering
                // them from the registry so a build works vcvars-free.
                search_path(name, env, probe).or_else(|| crate::msvc::msvc_tool_path(name))
            }
        }
    }?;
    // Ninja runs commands from the build directory, so a path found
    // relative to the current directory (or, on Windows, to its drive)
    // is anchored to it.  `absolute` keeps a symlink's name, which picks
    // a driver mode (`clang++`).
    if found.is_absolute() {
        Some(found)
    } else {
        std::path::absolute(found).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::{ToolchainDecl, ToolchainSelection};
    use camino::Utf8PathBuf;
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
            platform,
            env: Box::new(move |k| env.get(k).cloned()),
            probe: Box::new(move |p| std::path::absolute(p).is_ok_and(|p| existing.contains(&p))),
        }
    }

    /// `p` as resolution anchors it: unchanged on Unix, on the current
    /// drive on Windows.
    fn abs(p: &str) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(std::path::absolute(p).unwrap()).unwrap()
    }

    fn path_set(items: &[&str]) -> HashSet<PathBuf> {
        items
            .iter()
            .map(|p| std::path::absolute(p).unwrap())
            .collect()
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
        assert_eq!(resolved.cxx.path, abs("/usr/bin/clang++"));
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
        assert_eq!(r.cxx.path, abs("/usr/bin/clang++"));
        assert_eq!(r.cxx.source, ToolSource::Cli);
    }

    #[test]
    fn relative_path_is_anchored_to_the_current_directory() {
        let manifest = ToolchainSettings::default();
        let selection = ToolchainSelection::default()
            .with_cli(ToolKind::CxxCompiler, ToolSpec::parse("./bin/clang++"));
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&["./bin/clang++", "/usr/bin/ar"]);
        let inputs = make_inputs(&selection, &manifest, &host, env, existing);
        let r = resolve_toolchain(&inputs).unwrap();
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(r.cxx.path, cwd.join("bin").join("clang++"));
        assert_eq!(r.cxx.spec, ToolSpec::parse("./bin/clang++"));
    }

    // Ninja may run from a build directory on another drive.
    #[cfg(windows)]
    #[test]
    fn a_path_rooted_without_a_drive_gets_the_current_drive() {
        let manifest = ToolchainSettings::default();
        let selection = ToolchainSelection::default()
            .with_cli(ToolKind::CxxCompiler, ToolSpec::parse(r"\tools\cl.exe"));
        let host = host();
        let env = fake_env(&[("PATH", r"\tools")]);
        let existing = path_set(&[r"\tools\cl.exe", r"\tools\lib.exe"]);
        let inputs = make_inputs(&selection, &manifest, &host, env, existing);
        let r = resolve_toolchain(&inputs).unwrap();
        assert!(r.cxx.path.is_absolute(), "{}", r.cxx.path);
        assert_eq!(r.cxx.path, abs(r"\tools\cl.exe"));
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
        assert_eq!(r.cxx.path, abs("/usr/bin/clang++"));
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
        assert_eq!(r.cxx.path, abs("/usr/bin/g++"));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_env_value_is_rejected() {
        use std::os::unix::ffi::OsStringExt;
        // A `CXX` set to non-UTF-8 bytes is a tool-resolution input;
        // it must be rejected with a typed error, never lossily
        // converted into a spec that resolves to the wrong tool.
        let manifest = ToolchainSettings::default();
        let selection = ToolchainSelection::default();
        let host = host();
        let mut env: std::collections::HashMap<&'static str, OsString> =
            std::collections::HashMap::new();
        env.insert("PATH", OsString::from("/usr/bin"));
        env.insert("CXX", OsString::from_vec(vec![0xff]));
        let existing = path_set(&["/usr/bin/ar"]);
        let inputs = make_inputs(&selection, &manifest, &host, env, existing);
        let err = resolve_toolchain(&inputs).unwrap_err();
        assert!(matches!(
            err,
            ToolchainResolutionError::NonUtf8Path {
                kind: ToolKind::CxxCompiler,
                ..
            }
        ));
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
        assert_eq!(r.cxx.path, abs("/usr/bin/clang++"));
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
        assert_eq!(r.cxx.path, abs("/usr/bin/g++"));
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
    fn cl_exe_resolves_as_a_supported_compiler() {
        let selection = ToolchainSelection::default()
            .with_cli(ToolKind::CxxCompiler, ToolSpec::Name("cl.exe".into()));
        let manifest = ToolchainSettings::default();
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&["/usr/bin/cl.exe", "/usr/bin/ar"]);
        let inputs = make_inputs(&selection, &manifest, &host, env, existing);
        let resolved = resolve_toolchain(&inputs).unwrap();
        assert_eq!(resolved.cxx.path, abs("/usr/bin/cl.exe"));
    }

    #[test]
    fn lib_named_as_a_compiler_is_rejected() {
        let selection = ToolchainSelection::default()
            .with_cli(ToolKind::CxxCompiler, ToolSpec::Name("lib".into()));
        let manifest = ToolchainSettings::default();
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&["/usr/bin/lib", "/usr/bin/ar"]);
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
        assert_eq!(resolved.cxx.path, abs("/usr/bin/clang++"));
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
        assert_eq!(resolved.cxx.path, abs("/usr/bin/clang++"));
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
        assert_eq!(resolved.cxx.path, abs("/usr/bin/clang++"));
    }

    #[test]
    fn explicit_cc_is_resolved_when_requested() {
        let selection = ToolchainSelection::default()
            .with_cli(ToolKind::CCompiler, ToolSpec::Name("clang".into()));
        let manifest = ToolchainSettings::default();
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin")]);
        // `clang++` is in the default C++ fallback list on every host
        // (`c++ / clang++ / g++` on Unix, `cl / clang++ / g++` on
        // Windows), so the required `cxx` resolves regardless of the
        // platform's default order - the focus here is the explicit
        // `cc`.
        let existing = path_set(&[
            "/usr/bin/clang",
            "/usr/bin/clang++",
            "/usr/bin/c++",
            "/usr/bin/ar",
        ]);
        let inputs = make_inputs(&selection, &manifest, &host, env, existing);
        let r = resolve_toolchain(&inputs).unwrap();
        let cc = r.cc.expect("explicit C compiler resolved");
        assert_eq!(cc.path, abs("/usr/bin/clang"));
        assert_eq!(cc.source, ToolSource::Cli);
    }

    #[test]
    fn defaulted_slots_take_the_chosen_compilers_siblings() {
        let manifest = ToolchainSettings::default();
        let host = host();
        let selection = ToolchainSelection::default().with_cli(
            ToolKind::CxxCompiler,
            ToolSpec::Name("x86_64-w64-mingw32-g++".into()),
        );
        let existing = path_set(&[
            "/usr/bin/x86_64-w64-mingw32-g++",
            "/usr/bin/x86_64-w64-mingw32-gcc",
            "/usr/bin/x86_64-w64-mingw32-ar",
            "/usr/bin/cc",
            "/usr/bin/ar",
        ]);
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let r =
            resolve_toolchain(&make_inputs(&selection, &manifest, &host, env, existing)).unwrap();
        let cc = r.cc.unwrap();
        assert_eq!(cc.path, abs("/usr/bin/x86_64-w64-mingw32-gcc"));
        assert_eq!(cc.spec, ToolSpec::Name("x86_64-w64-mingw32-gcc".into()));
        assert_eq!(cc.source, ToolSource::Default);
        assert_eq!(r.ar.path, abs("/usr/bin/x86_64-w64-mingw32-ar"));

        // A chosen C compiler names the C++ compiler and archiver.
        let selection = ToolchainSelection::default().with_cli(
            ToolKind::CCompiler,
            ToolSpec::parse("/opt/llvm/bin/clang-18"),
        );
        let existing = path_set(&[
            "/opt/llvm/bin/clang-18",
            "/opt/llvm/bin/clang++-18",
            "/opt/llvm/bin/llvm-ar-18",
            "/usr/bin/c++",
            "/usr/bin/ar",
        ]);
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let r =
            resolve_toolchain(&make_inputs(&selection, &manifest, &host, env, existing)).unwrap();
        assert_eq!(r.cxx.path, abs("/opt/llvm/bin/clang++-18"));
        assert_eq!(r.cxx.spec, ToolSpec::parse("/opt/llvm/bin/clang++-18"));
        assert_eq!(r.ar.path, abs("/opt/llvm/bin/llvm-ar-18"));

        // Windows spells names in any case.
        let selection = ToolchainSelection::default()
            .with_cli(ToolKind::CxxCompiler, ToolSpec::parse("/vc/bin/CL.EXE"));
        let existing = path_set(&["/vc/bin/CL.EXE", "/vc/bin/lib.exe", "/usr/bin/ar"]);
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let r =
            resolve_toolchain(&make_inputs(&selection, &manifest, &host, env, existing)).unwrap();
        assert_eq!(r.ar.path, abs("/vc/bin/lib.exe"));

        let selection = ToolchainSelection::default()
            .with_cli(ToolKind::CxxCompiler, ToolSpec::parse("/llvm/clang-cl-18"));
        let existing = path_set(&["/llvm/clang-cl-18", "/llvm/llvm-lib-18", "/usr/bin/ar"]);
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let r =
            resolve_toolchain(&make_inputs(&selection, &manifest, &host, env, existing)).unwrap();
        assert_eq!(r.ar.path, abs("/llvm/llvm-lib-18"));
        assert_eq!(r.cc.unwrap().path, abs("/llvm/clang-cl-18"));

        // Not a C++ driver name, so no C sibling: not the lead itself.
        let selection = ToolchainSelection::default().with_cli(
            ToolKind::CxxCompiler,
            ToolSpec::Name("afl-clang-fast++".into()),
        );
        let existing = path_set(&["/usr/bin/afl-clang-fast++", "/usr/bin/gcc", "/usr/bin/ar"]);
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let r =
            resolve_toolchain(&make_inputs(&selection, &manifest, &host, env, existing)).unwrap();
        assert_eq!(r.cc.unwrap().path, abs("/usr/bin/gcc"));
    }

    /// Only Windows defaults to MSVC tools.
    #[cfg(windows)]
    #[test]
    fn a_gcc_style_choice_skips_the_msvc_defaults() {
        let selection = ToolchainSelection::default()
            .with_cli(ToolKind::CxxCompiler, ToolSpec::Name("clang++".into()));
        let manifest = ToolchainSettings::default();
        let host = host();
        let env = fake_env(&[("PATH", "/bin")]);
        let existing = path_set(&[
            "/bin/clang++.exe",
            "/bin/cl.exe",
            "/bin/gcc.exe",
            "/bin/lib.exe",
            "/bin/ar.exe",
        ]);
        let r =
            resolve_toolchain(&make_inputs(&selection, &manifest, &host, env, existing)).unwrap();
        assert_eq!(r.cc.unwrap().path, abs("/bin/gcc.exe"));
        assert_eq!(r.ar.path, abs("/bin/ar.exe"));
    }
}
