//! Resolve a [`cabin_core::ResolvedCompilerWrapper`] from layered
//! inputs.
//!
//! Precedence, applied in order:
//!
//! 1. CLI flag (`--compiler-wrapper <name>` / `--no-compiler-wrapper`).
//! 2. Environment variable (`CABIN_COMPILER_WRAPPER`). Empty values
//!    count as unset.
//! 3. Config `[build.cache]` layer.
//! 4. Matching `[target.'cfg(...)'.profile.cache]` overlay for the
//!    host platform.
//! 5. `[profile.cache]` table on the workspace root manifest.
//! 6. Default — no wrapper.
//!
//! The first layer that sets a [`cabin_core::CompilerWrapperRequest`]
//! wins. `Disabled` short-circuits the search and returns `None`;
//! `Use(_)` triggers a `PATH` lookup and an optional `--version`
//! probe.
//!
//! Like [`crate::resolve::resolve_toolchain`], the resolver takes
//! its environment and filesystem access through injectable
//! callbacks so the unit tests can drive every code path without
//! touching the host.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use cabin_core::{
    CompilerVersion, CompilerWrapperIdentity, CompilerWrapperKind, CompilerWrapperManifestSettings,
    CompilerWrapperRequest, CompilerWrapperSource, ResolvedCompilerWrapper, TargetPlatform,
};
use thiserror::Error;

use crate::detect::{RunError, ToolRunner};
use crate::resolve::{EnvLookup, ExecutableProbe};

/// Environment variable that selects a compiler-cache wrapper.
/// Mirrors the CLI flag and the manifest table — see
/// [`CompilerWrapperRequest::parse`] for accepted values.
pub(crate) const WRAPPER_ENV_VAR: &str = cabin_env::CABIN_COMPILER_WRAPPER;

/// Inputs the wrapper resolver consumes.
pub struct WrapperInputs<'a> {
    /// Highest-priority CLI selection (`--compiler-wrapper` /
    /// `--no-compiler-wrapper`). `None` means "no CLI override".
    pub cli: Option<CompilerWrapperRequest>,
    /// Optional config-derived layer that slots between the
    /// environment variable and the manifest. Built by
    /// `cabin` from the merged effective config; the embedded
    /// [`CompilerWrapperSource`] (one of the `*Config` variants)
    /// flows onto the resolved wrapper so metadata can attribute
    /// the value to the exact file.
    pub config: Option<ConfigWrapperLayer>,
    /// Manifest-derived declarations from the workspace root.
    pub manifest: &'a CompilerWrapperManifestSettings,
    /// Host platform used to evaluate
    /// `[target.'cfg(...)'.profile.cache]` predicates.
    pub host_platform: &'a TargetPlatform,
    /// Environment lookup. Production callers wrap
    /// `std::env::var_os`; tests inject a hash-map-backed closure.
    pub env: EnvLookup<'a>,
    /// Executable probe. Production callers wrap `Path::is_file`;
    /// tests inject a `HashSet<PathBuf>`-backed closure.
    pub probe: ExecutableProbe<'a>,
}

/// Per-layer config selection for the wrapper. The wrapper is a
/// single value per build invocation, so the layer just carries
/// one [`CompilerWrapperRequest`] plus the source label that
/// describes which config file it came from.
#[derive(Debug, Clone, Copy)]
pub struct ConfigWrapperLayer {
    pub request: CompilerWrapperRequest,
    pub source: CompilerWrapperSource,
}

impl<'a> WrapperInputs<'a> {
    /// Inputs that read environment variables from the running
    /// process and check executables on disk.
    pub fn from_process(
        cli: Option<CompilerWrapperRequest>,
        manifest: &'a CompilerWrapperManifestSettings,
        host_platform: &'a TargetPlatform,
    ) -> Self {
        Self {
            cli,
            config: None,
            manifest,
            host_platform,
            env: Box::new(|var| std::env::var_os(var)),
            probe: Box::new(Path::is_file),
        }
    }

    /// Builder-style setter for the optional config layer. Keeps
    /// `from_process` callers concise when no config is active.
    #[must_use]
    pub fn with_config(mut self, layer: ConfigWrapperLayer) -> Self {
        self.config = Some(layer);
        self
    }
}

/// Errors produced by [`resolve_compiler_wrapper`].
#[derive(Debug, Error)]
pub enum CompilerWrapperResolutionError {
    /// The user (or a manifest layer) asked for a specific wrapper
    /// but Cabin could not find a matching executable.
    #[error(
        "compiler-cache wrapper `{kind}` was requested by {source_label} but could not be found on PATH",
        source_label = source_label(*selected_from)
    )]
    NotFound {
        kind: CompilerWrapperKind,
        selected_from: CompilerWrapperSource,
    },
    /// `CABIN_COMPILER_WRAPPER` carried an invalid value. The
    /// inner error matches what
    /// [`CompilerWrapperRequest::parse`] returns.
    #[error("environment variable {var} is set but: {source}", var = WRAPPER_ENV_VAR)]
    EnvParse {
        #[source]
        source: cabin_core::CompilerWrapperParseError,
    },
    /// `CABIN_COMPILER_WRAPPER` is set to a non-UTF-8 value. The
    /// wrapper spec must be UTF-8 to parse, so the value is rejected
    /// rather than lossily mangled into an unintended request.
    #[error("environment variable {var} is set but is not valid UTF-8", var = WRAPPER_ENV_VAR)]
    EnvNotUtf8,
    /// Subprocess version probe failed. Treated as a hard error in
    /// the build path so missing wrappers do not silently slip
    /// through; `cabin metadata` is fail-soft and reports `null`.
    #[error(
        "failed to run wrapper `{kind}` at {path} for version detection: {source}",
        path = path.display()
    )]
    SubprocessFailed {
        kind: CompilerWrapperKind,
        path: PathBuf,
        #[source]
        source: RunError,
    },
    /// The wrapper was located on `PATH` but the resolved path is
    /// not valid UTF-8. Cabin assumes tool paths are UTF-8, so a
    /// wrapper under a non-UTF-8 directory is surfaced here rather
    /// than aborting the process.
    #[error("resolved wrapper `{kind}` path `{path}` is not valid UTF-8", path = path.display())]
    NonUtf8Path {
        kind: CompilerWrapperKind,
        path: PathBuf,
    },
}

fn source_label(source: CompilerWrapperSource) -> &'static str {
    match source {
        CompilerWrapperSource::Cli => "--compiler-wrapper",
        CompilerWrapperSource::Env => "the CABIN_COMPILER_WRAPPER environment variable",
        CompilerWrapperSource::UserConfig => "the user `[build.cache]` config table",
        CompilerWrapperSource::WorkspaceConfig => "the workspace `[build.cache]` config table",
        CompilerWrapperSource::PackageConfig => "the package `[build.cache]` config table",
        CompilerWrapperSource::ExplicitConfig => "the `CABIN_CONFIG` `[build.cache]` table",
        CompilerWrapperSource::ManifestConditional => "[target.'cfg(...)'.profile.cache]",
        CompilerWrapperSource::Manifest => "[profile.cache]",
    }
}

/// Resolve the compiler-cache wrapper to apply for this build.
///
/// `runner` is consulted only when the resolved wrapper is
/// `Use(_)`; production callers pass [`crate::ProcessRunner`] and
/// tests inject a fake. A `None` runner skips version detection
/// entirely (used by `cabin metadata`'s fail-soft path so a
/// misbehaving wrapper does not block inspection).
///
/// # Errors
/// Returns [`CompilerWrapperResolutionError`]: `EnvParse` when
/// `CABIN_COMPILER_WRAPPER` holds an invalid value, `NotFound` when
/// the requested wrapper cannot be located on `PATH`, and
/// `SubprocessFailed` when a non-`None` `runner` fails to probe the
/// wrapper's version.
pub fn resolve_compiler_wrapper(
    inputs: &WrapperInputs<'_>,
    runner: Option<&dyn ToolRunner>,
) -> Result<Option<ResolvedCompilerWrapper>, CompilerWrapperResolutionError> {
    let Some((request, source)) = pick_request(inputs)? else {
        return Ok(None);
    };
    let kind = match request {
        CompilerWrapperRequest::Disabled => return Ok(None),
        CompilerWrapperRequest::Use { wrapper } => wrapper,
    };
    let path = locate_wrapper(kind, &inputs.env, &inputs.probe).ok_or(
        CompilerWrapperResolutionError::NotFound {
            kind,
            selected_from: source,
        },
    )?;
    let identity = match runner {
        Some(runner) => Some(detect_identity(kind, &path, runner)?),
        None => None,
    };
    Ok(Some(ResolvedCompilerWrapper {
        kind,
        path: crate::path_search::into_utf8_tool_path(path)
            .map_err(|path| CompilerWrapperResolutionError::NonUtf8Path { kind, path })?,
        spec: kind.as_key().to_owned(),
        source,
        identity,
    }))
}

fn pick_request(
    inputs: &WrapperInputs<'_>,
) -> Result<Option<(CompilerWrapperRequest, CompilerWrapperSource)>, CompilerWrapperResolutionError>
{
    // 1. CLI flag.
    if let Some(req) = inputs.cli {
        return Ok(Some((req, CompilerWrapperSource::Cli)));
    }
    // 2. Env var.
    if let Some(value) = (inputs.env)(WRAPPER_ENV_VAR)
        && !value.is_empty()
    {
        // The wrapper spec becomes Cabin-owned tool-resolution state,
        // so reject a non-UTF-8 env value rather than lossily mangling
        // it before the parser sees it.
        let raw = value
            .into_string()
            .map_err(|_| CompilerWrapperResolutionError::EnvNotUtf8)?;
        let req = CompilerWrapperRequest::parse(&raw)
            .map_err(|source| CompilerWrapperResolutionError::EnvParse { source })?;
        return Ok(Some((req, CompilerWrapperSource::Env)));
    }
    // 3. Config layer (user / workspace / package / explicit).
    if let Some(layer) = inputs.config {
        return Ok(Some((layer.request, layer.source)));
    }
    // 4. Target-conditioned manifest overlay. Multiple matching
    // overlays settle in declaration order; the *last* match wins
    // so a more specific predicate listed later in the manifest
    // can override an earlier general one — same convention the
    // build-flag merger uses.
    let mut conditional_match: Option<CompilerWrapperRequest> = None;
    for entry in &inputs.manifest.conditional {
        // Compiler-wrapper `cfg(...)` selection is platform-only;
        // feature and compiler conditions are not accepted on these
        // tables, so the platform-only context is correct.
        if entry
            .condition
            .evaluate(&cabin_core::ConditionContext::platform_only(
                inputs.host_platform,
            ))
        {
            conditional_match = Some(entry.request);
        }
    }
    if let Some(req) = conditional_match {
        return Ok(Some((req, CompilerWrapperSource::ManifestConditional)));
    }
    // 5. General manifest table.
    if let Some(req) = inputs.manifest.general {
        return Ok(Some((req, CompilerWrapperSource::Manifest)));
    }
    // 6. Default — no wrapper.
    Ok(None)
}

fn locate_wrapper<F, P>(kind: CompilerWrapperKind, env: &F, probe: &P) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<OsString> + ?Sized,
    P: Fn(&Path) -> bool + ?Sized,
{
    crate::path_search::search_path(kind.default_command(), env, probe)
}

fn detect_identity(
    kind: CompilerWrapperKind,
    path: &Path,
    runner: &dyn ToolRunner,
) -> Result<CompilerWrapperIdentity, CompilerWrapperResolutionError> {
    let output = runner.run(path, &["--version"]).map_err(|source| {
        CompilerWrapperResolutionError::SubprocessFailed {
            kind,
            path: path.to_path_buf(),
            source,
        }
    })?;
    let combined = output.combined();
    if output.status != 0 {
        return Ok(CompilerWrapperIdentity::unknown_version(
            kind,
            crate::detect::first_non_empty_line(&combined),
        ));
    }
    let raw_line = crate::detect::first_non_empty_line(&combined);
    let version = parse_wrapper_version(&combined);
    Ok(CompilerWrapperIdentity {
        kind,
        version,
        raw_version_line: raw_line,
    })
}

/// Extract a numeric version substring from a wrapper's
/// `--version` output. ccache prints `ccache version 4.10.2` on
/// the first line; sccache prints `sccache 0.7.7`. The parser
/// looks for the first dot-separated number group following an
/// optional `version` keyword.
fn parse_wrapper_version(text: &str) -> Option<CompilerVersion> {
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?
        .to_owned();
    // Skip everything up to the first numeric token. Using
    // byte-wise scanning keeps the parser allocation-free until
    // the matched substring needs cloning.
    let bytes = first.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            // Trim trailing dots (`4.` → `4`).
            let mut end = i;
            while end > start && bytes[end - 1] == b'.' {
                end -= 1;
            }
            let candidate = &first[start..end];
            if let Some(parsed) = CompilerVersion::parse(candidate) {
                return Some(parsed);
            }
            // Not a parseable version — keep scanning.
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::ProcessRunner;
    use crate::detect::test_support::FakeRunner;
    use cabin_core::{
        CompilerWrapperKind, CompilerWrapperRequest, ConditionalCompilerWrapperDecl, TargetPlatform,
    };
    use camino::Utf8PathBuf;
    use std::collections::{HashMap, HashSet};

    fn host() -> TargetPlatform {
        let mut p = TargetPlatform::current();
        p.os = "linux".into();
        p
    }

    fn fake_env(items: &[(&'static str, &str)]) -> HashMap<&'static str, OsString> {
        let mut env = HashMap::new();
        for (k, v) in items {
            env.insert(*k, OsString::from(*v));
        }
        env
    }

    fn make_inputs<'a>(
        cli: Option<CompilerWrapperRequest>,
        manifest: &'a CompilerWrapperManifestSettings,
        platform: &'a TargetPlatform,
        env: HashMap<&'static str, OsString>,
        existing: HashSet<PathBuf>,
    ) -> WrapperInputs<'a> {
        WrapperInputs {
            cli,
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

    #[test]
    fn no_layer_yields_no_wrapper() {
        let manifest = CompilerWrapperManifestSettings::default();
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&[]);
        let inputs = make_inputs(None, &manifest, &host, env, existing);
        let resolved = resolve_compiler_wrapper(&inputs, None).unwrap();
        assert!(resolved.is_none());
    }

    #[test]
    fn cli_use_resolves_via_path_lookup() {
        let manifest = CompilerWrapperManifestSettings::default();
        let host = host();
        // A single PATH entry keeps the lookup portable: a `:`-joined
        // list is one entry on Windows (where `PATH` splits on `;`),
        // which would defeat the search. The wrapper still resolves by
        // scanning this directory off `PATH`.
        let env = fake_env(&[("PATH", "/usr/local/bin")]);
        let existing = path_set(&["/usr/local/bin/ccache"]);
        let inputs = make_inputs(
            Some(CompilerWrapperRequest::Use {
                wrapper: CompilerWrapperKind::Ccache,
            }),
            &manifest,
            &host,
            env,
            existing,
        );
        let resolved = resolve_compiler_wrapper(&inputs, None).unwrap().unwrap();
        assert_eq!(resolved.kind, CompilerWrapperKind::Ccache);
        assert_eq!(resolved.path, Utf8PathBuf::from("/usr/local/bin/ccache"));
        assert_eq!(resolved.source, CompilerWrapperSource::Cli);
        assert!(resolved.identity.is_none());
    }

    #[test]
    fn cli_disabled_short_circuits_even_when_manifest_selects_a_wrapper() {
        let manifest = CompilerWrapperManifestSettings {
            general: Some(CompilerWrapperRequest::Use {
                wrapper: CompilerWrapperKind::Ccache,
            }),
            ..Default::default()
        };
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&["/usr/bin/ccache"]);
        let inputs = make_inputs(
            Some(CompilerWrapperRequest::Disabled),
            &manifest,
            &host,
            env,
            existing,
        );
        let resolved = resolve_compiler_wrapper(&inputs, None).unwrap();
        assert!(resolved.is_none());
    }

    #[test]
    fn env_overrides_manifest() {
        let manifest = CompilerWrapperManifestSettings {
            general: Some(CompilerWrapperRequest::Use {
                wrapper: CompilerWrapperKind::Sccache,
            }),
            ..Default::default()
        };
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin"), ("CABIN_COMPILER_WRAPPER", "ccache")]);
        let existing = path_set(&["/usr/bin/ccache", "/usr/bin/sccache"]);
        let inputs = make_inputs(None, &manifest, &host, env, existing);
        let resolved = resolve_compiler_wrapper(&inputs, None).unwrap().unwrap();
        assert_eq!(resolved.kind, CompilerWrapperKind::Ccache);
        assert_eq!(resolved.source, CompilerWrapperSource::Env);
    }

    #[test]
    fn empty_env_is_treated_as_unset() {
        let manifest = CompilerWrapperManifestSettings {
            general: Some(CompilerWrapperRequest::Use {
                wrapper: CompilerWrapperKind::Ccache,
            }),
            ..Default::default()
        };
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin"), ("CABIN_COMPILER_WRAPPER", "")]);
        let existing = path_set(&["/usr/bin/ccache"]);
        let inputs = make_inputs(None, &manifest, &host, env, existing);
        let resolved = resolve_compiler_wrapper(&inputs, None).unwrap().unwrap();
        assert_eq!(resolved.source, CompilerWrapperSource::Manifest);
    }

    #[test]
    fn invalid_env_value_yields_clear_error() {
        let manifest = CompilerWrapperManifestSettings::default();
        let host = host();
        let env = fake_env(&[
            ("PATH", "/usr/bin"),
            ("CABIN_COMPILER_WRAPPER", "fastcache"),
        ]);
        let existing = path_set(&["/usr/bin/ccache"]);
        let inputs = make_inputs(None, &manifest, &host, env, existing);
        let err = resolve_compiler_wrapper(&inputs, None).unwrap_err();
        assert!(matches!(
            err,
            CompilerWrapperResolutionError::EnvParse { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_env_value_is_rejected() {
        use std::os::unix::ffi::OsStringExt;
        // A non-UTF-8 `CABIN_COMPILER_WRAPPER` is a wrapper-spec
        // input; reject it with a typed error rather than lossily
        // mangling it into an unintended request.
        let manifest = CompilerWrapperManifestSettings::default();
        let host = host();
        let mut env: HashMap<&'static str, OsString> = HashMap::new();
        env.insert("PATH", OsString::from("/usr/bin"));
        env.insert("CABIN_COMPILER_WRAPPER", OsString::from_vec(vec![0xff]));
        let existing = path_set(&["/usr/bin/ccache"]);
        let inputs = make_inputs(None, &manifest, &host, env, existing);
        let err = resolve_compiler_wrapper(&inputs, None).unwrap_err();
        assert!(matches!(err, CompilerWrapperResolutionError::EnvNotUtf8));
    }

    #[test]
    fn config_layer_wins_over_manifest_when_no_cli_or_env_value_is_set() {
        let manifest = CompilerWrapperManifestSettings {
            general: Some(CompilerWrapperRequest::Use {
                wrapper: CompilerWrapperKind::Sccache,
            }),
            ..Default::default()
        };
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&["/usr/bin/ccache", "/usr/bin/sccache"]);
        let mut inputs = make_inputs(None, &manifest, &host, env, existing);
        inputs.config = Some(ConfigWrapperLayer {
            request: CompilerWrapperRequest::Use {
                wrapper: CompilerWrapperKind::Ccache,
            },
            source: CompilerWrapperSource::WorkspaceConfig,
        });
        let resolved = resolve_compiler_wrapper(&inputs, None).unwrap().unwrap();
        assert_eq!(resolved.kind, CompilerWrapperKind::Ccache);
        assert_eq!(resolved.source, CompilerWrapperSource::WorkspaceConfig);
    }

    #[test]
    fn config_layer_can_disable_wrapper_even_when_manifest_selects_one() {
        let manifest = CompilerWrapperManifestSettings {
            general: Some(CompilerWrapperRequest::Use {
                wrapper: CompilerWrapperKind::Ccache,
            }),
            ..Default::default()
        };
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&["/usr/bin/ccache"]);
        let mut inputs = make_inputs(None, &manifest, &host, env, existing);
        inputs.config = Some(ConfigWrapperLayer {
            request: CompilerWrapperRequest::Disabled,
            source: CompilerWrapperSource::UserConfig,
        });
        let resolved = resolve_compiler_wrapper(&inputs, None).unwrap();
        assert!(resolved.is_none());
    }

    #[test]
    fn env_overrides_config_layer_for_wrapper() {
        let manifest = CompilerWrapperManifestSettings::default();
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin"), ("CABIN_COMPILER_WRAPPER", "ccache")]);
        let existing = path_set(&["/usr/bin/ccache", "/usr/bin/sccache"]);
        let mut inputs = make_inputs(None, &manifest, &host, env, existing);
        inputs.config = Some(ConfigWrapperLayer {
            request: CompilerWrapperRequest::Use {
                wrapper: CompilerWrapperKind::Sccache,
            },
            source: CompilerWrapperSource::PackageConfig,
        });
        let resolved = resolve_compiler_wrapper(&inputs, None).unwrap().unwrap();
        assert_eq!(resolved.kind, CompilerWrapperKind::Ccache);
        assert_eq!(resolved.source, CompilerWrapperSource::Env);
    }

    #[test]
    fn target_conditional_overlay_overrides_general_manifest() {
        let manifest = CompilerWrapperManifestSettings {
            general: Some(CompilerWrapperRequest::Use {
                wrapper: CompilerWrapperKind::Ccache,
            }),
            conditional: vec![ConditionalCompilerWrapperDecl {
                condition: cabin_core::Condition::KeyValue {
                    key: cabin_core::ConditionKey::Os,
                    value: "linux".into(),
                },
                request: CompilerWrapperRequest::Use {
                    wrapper: CompilerWrapperKind::Sccache,
                },
            }],
        };
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&["/usr/bin/ccache", "/usr/bin/sccache"]);
        let inputs = make_inputs(None, &manifest, &host, env, existing);
        let resolved = resolve_compiler_wrapper(&inputs, None).unwrap().unwrap();
        assert_eq!(resolved.source, CompilerWrapperSource::ManifestConditional);
        assert_eq!(resolved.kind, CompilerWrapperKind::Sccache);
    }

    #[test]
    fn missing_explicit_wrapper_yields_not_found() {
        let manifest = CompilerWrapperManifestSettings::default();
        let host = host();
        let env = fake_env(&[("PATH", "/usr/bin")]);
        let existing = path_set(&[]);
        let inputs = make_inputs(
            Some(CompilerWrapperRequest::Use {
                wrapper: CompilerWrapperKind::Ccache,
            }),
            &manifest,
            &host,
            env,
            existing,
        );
        let err = resolve_compiler_wrapper(&inputs, None).unwrap_err();
        match err {
            CompilerWrapperResolutionError::NotFound {
                kind,
                selected_from,
            } => {
                assert_eq!(kind, CompilerWrapperKind::Ccache);
                assert_eq!(selected_from, CompilerWrapperSource::Cli);
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn detect_identity_parses_ccache_version() {
        let runner = FakeRunner::new().with(
            "/usr/bin/ccache",
            &["--version"],
            "ccache version 4.10.2\n",
            "",
            0,
        );
        let identity = detect_identity(
            CompilerWrapperKind::Ccache,
            Path::new("/usr/bin/ccache"),
            &runner,
        )
        .unwrap();
        assert_eq!(identity.kind, CompilerWrapperKind::Ccache);
        let v = identity.version.unwrap();
        assert_eq!(v.major, 4);
        assert_eq!(v.minor, Some(10));
        assert_eq!(v.patch, Some(2));
    }

    #[test]
    fn detect_identity_parses_sccache_version() {
        let runner =
            FakeRunner::new().with("/usr/bin/sccache", &["--version"], "sccache 0.7.7\n", "", 0);
        let identity = detect_identity(
            CompilerWrapperKind::Sccache,
            Path::new("/usr/bin/sccache"),
            &runner,
        )
        .unwrap();
        let v = identity.version.unwrap();
        assert_eq!(v.major, 0);
        assert_eq!(v.minor, Some(7));
        assert_eq!(v.patch, Some(7));
    }

    #[test]
    fn detect_identity_keeps_unknown_version_when_status_is_nonzero() {
        let runner = FakeRunner::new().with("/usr/bin/ccache", &["--version"], "", "boom\n", 1);
        let identity = detect_identity(
            CompilerWrapperKind::Ccache,
            Path::new("/usr/bin/ccache"),
            &runner,
        )
        .unwrap();
        assert!(identity.version.is_none());
    }

    /// Exercise the production runner indirectly: `ProcessRunner` is
    /// the default for `from_process` and we assert it is `Send +
    /// Sync` so callers can plug it into `cabin metadata`'s
    /// fail-soft path. The actual subprocess invocation lives in
    /// `detect::tests`.
    #[test]
    fn process_runner_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ProcessRunner>();
    }
}
