//! Parse a `.cabin/config.toml` file into a typed
//! [`ParsedConfig`] value.
//!
//! Validation runs at parse time so the rest of the workspace
//! never sees raw TOML or a half-typed shape.  Every parse failure
//! travels through [`crate::ConfigParseError`] with stable wording
//! so integration tests can match substrings.

use std::collections::BTreeMap;

use camino::Utf8PathBuf;

use cabin_core::{
    ColorChoice, CompilerWrapperRequest, IncompatibleStandards, PackageName, PatchSource,
    SourceLocator, ToolSpec, Verbosity,
};

use crate::error::ConfigParseError;
use crate::raw::{
    RawBuild, RawConfig, RawConfigPatch, RawConfigSourceReplacement, RawPaths, RawRegistry,
    RawResolver, RawTerm, RawToolchain,
};

/// Validated, typed contents of one config file.  The raw
/// `RawConfig` is intentionally not exposed; this struct is what
/// the merger and metadata view consume.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedConfig {
    pub registry: Option<ParsedRegistry>,
    pub paths: ParsedPaths,
    pub build: ParsedBuild,
    pub resolver: ParsedResolver,
    pub toolchain: ParsedToolchain,
    pub term: ParsedTerm,
    pub patches: BTreeMap<PackageName, PatchSource>,
    pub source_replacements: BTreeMap<SourceLocator, ParsedSourceReplacement>,
}

/// Validated `[resolver]` table.  `incompatible_standards` is `None`
/// when the file left the key unset, so merging keeps a lower-priority
/// file's value.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedResolver {
    pub incompatible_standards: Option<IncompatibleStandards>,
}

/// Validated `[term]` table.  Each field is `Option` so an absent
/// key is distinguishable from a deliberate `false` / explicit
/// value during merging.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedTerm {
    pub color: Option<ColorChoice>,
    /// Resolved `term.verbose` / `term.quiet` pair, mapped into a
    /// single typed [`Verbosity`].  `None` when neither key was
    /// set in this file.
    pub verbosity: Option<Verbosity>,
}

/// One typed `[source-replacement]` entry.  The original (table
/// key) is held by the surrounding map; this struct carries
/// the replacement target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSourceReplacement {
    pub replacement: SourceLocator,
}

/// `[registry]` table after validation.  A single config file may
/// declare *either* `index-path` or `index-url`, never both - the
/// validation layer rejects the combination so two sources cannot
/// silently coexist at the same precedence level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedRegistry {
    Path(Utf8PathBuf),
    Url(String),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedPaths {
    pub cache_dir: Option<Utf8PathBuf>,
    pub build_dir: Option<Utf8PathBuf>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedBuild {
    pub profile: Option<String>,
    pub compiler_wrapper: Option<CompilerWrapperRequest>,
    pub jobs: Option<cabin_core::BuildJobs>,
    /// Temporary migration switch for the experimental
    /// `-Z standard-compat` check: `false` demotes violated
    /// dependency edges from errors back to warnings.
    pub standard_compat_errors: Option<bool>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedToolchain {
    pub cc: Option<ToolSpec>,
    pub cxx: Option<ToolSpec>,
    pub ar: Option<ToolSpec>,
}

/// Parse a config file's contents.  The caller normally pairs the
/// result with a path + [`crate::ConfigSource`] to build a
/// [`crate::LoadedConfigFile`].
pub fn parse_config_str(input: &str) -> Result<ParsedConfig, ConfigParseError> {
    let raw: RawConfig =
        toml::from_str(input).map_err(|err| ConfigParseError::Toml(err.to_string()))?;
    parsed_from_raw(raw)
}

fn parsed_from_raw(raw: RawConfig) -> Result<ParsedConfig, ConfigParseError> {
    if let Some(target) = raw.target {
        // Pick a representative inner table name so the message
        // can quote one offending entry.  Fallback wording covers
        // the unusual `[target]` (without a sub-table) case.
        let inner = target.keys().next().map_or("<...>", String::as_str);
        return Err(ConfigParseError::TargetConditionedNotSupported {
            table: inner.to_owned(),
        });
    }
    if let Some(extra_key) = raw.extra.keys().next() {
        if let Some(reason) = unsupported_auth_table_key(extra_key) {
            return Err(reason);
        }
        return Err(ConfigParseError::UnknownTopLevelTable {
            table: extra_key.clone(),
        });
    }

    let registry = match raw.registry {
        Some(r) => parsed_registry_from_raw(r)?,
        None => None,
    };
    let paths = match raw.paths {
        Some(p) => parsed_paths_from_raw(p)?,
        None => ParsedPaths::default(),
    };
    let build = match raw.build {
        Some(b) => parsed_build_from_raw(b)?,
        None => ParsedBuild::default(),
    };
    let resolver = match raw.resolver {
        Some(r) => parsed_resolver_from_raw(r)?,
        None => ParsedResolver::default(),
    };
    let toolchain = match raw.toolchain {
        Some(t) => parsed_toolchain_from_raw(t)?,
        None => ParsedToolchain::default(),
    };
    let term = match raw.term {
        Some(t) => parsed_term_from_raw(t)?,
        None => ParsedTerm::default(),
    };
    let patches = match raw.patch {
        Some(rows) => parsed_patches_from_raw(rows)?,
        None => BTreeMap::new(),
    };
    let source_replacements = match raw.source_replacement {
        Some(rows) => parsed_source_replacements_from_raw(rows)?,
        None => BTreeMap::new(),
    };
    Ok(ParsedConfig {
        registry,
        paths,
        build,
        resolver,
        toolchain,
        term,
        patches,
        source_replacements,
    })
}

fn parsed_resolver_from_raw(raw: RawResolver) -> Result<ParsedResolver, ConfigParseError> {
    let incompatible_standards = match raw.incompatible_standards {
        Some(value) => Some(
            IncompatibleStandards::parse(value.trim())
                .map_err(ConfigParseError::InvalidIncompatibleStandards)?,
        ),
        None => None,
    };
    Ok(ParsedResolver {
        incompatible_standards,
    })
}

fn parsed_registry_from_raw(raw: RawRegistry) -> Result<Option<ParsedRegistry>, ConfigParseError> {
    if raw.index_path.is_some() && raw.index_url.is_some() {
        return Err(ConfigParseError::RegistryConflict);
    }
    if let Some(path) = raw.index_path {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(ConfigParseError::EmptyIndexPath);
        }
        return Ok(Some(ParsedRegistry::Path(Utf8PathBuf::from(trimmed))));
    }
    if let Some(url) = raw.index_url {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            return Err(ConfigParseError::EmptyIndexUrl);
        }
        if url_contains_credentials(trimmed) {
            return Err(ConfigParseError::IndexUrlContainsCredentials {
                url: redact_userinfo(trimmed),
            });
        }
        return Ok(Some(ParsedRegistry::Url(trimmed.to_owned())));
    }
    Ok(None)
}

fn parsed_paths_from_raw(raw: RawPaths) -> Result<ParsedPaths, ConfigParseError> {
    let cache_dir = match raw.cache_dir {
        Some(p) => Some(non_empty_path(p, "cache-dir")?),
        None => None,
    };
    let build_dir = match raw.build_dir {
        Some(p) => Some(non_empty_path(p, "build-dir")?),
        None => None,
    };
    Ok(ParsedPaths {
        cache_dir,
        build_dir,
    })
}

fn non_empty_path(p: Utf8PathBuf, key: &'static str) -> Result<Utf8PathBuf, ConfigParseError> {
    if p.as_str().is_empty() {
        return Err(ConfigParseError::EmptyPath { key });
    }
    Ok(p)
}

fn parsed_build_from_raw(raw: RawBuild) -> Result<ParsedBuild, ConfigParseError> {
    let profile = match raw.profile {
        Some(name) => {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                return Err(ConfigParseError::EmptyProfile);
            }
            Some(trimmed.to_owned())
        }
        None => None,
    };
    let compiler_wrapper = parsed_compiler_wrapper_from_raw(raw.compiler_wrapper)?;
    let jobs = match raw.jobs {
        Some(value) => Some(parsed_build_jobs(value)?),
        None => None,
    };
    Ok(ParsedBuild {
        profile,
        compiler_wrapper,
        jobs,
        standard_compat_errors: raw.standard_compat_errors,
    })
}

/// Validate a raw `build.jobs` integer and lift it into the
/// typed [`cabin_core::BuildJobs`] model.  The integer is
/// rejected when it is `0`, negative, or outside the supported
/// `u32` range - every other layer (CLI, env) flows through
/// the same final type so consumers downstream see one
/// validated shape.
fn parsed_build_jobs(value: i64) -> Result<cabin_core::BuildJobs, ConfigParseError> {
    if value <= 0 {
        return Err(ConfigParseError::InvalidBuildJobs {
            value: value.to_string(),
        });
    }
    let positive = u32::try_from(value).map_err(|_| ConfigParseError::InvalidBuildJobs {
        value: value.to_string(),
    })?;
    cabin_core::BuildJobs::new(positive).map_err(|_| ConfigParseError::InvalidBuildJobs {
        value: value.to_string(),
    })
}

fn parsed_compiler_wrapper_from_raw(
    raw: Option<String>,
) -> Result<Option<CompilerWrapperRequest>, ConfigParseError> {
    let Some(value) = raw else {
        return Ok(None);
    };
    CompilerWrapperRequest::parse(&value)
        .map(Some)
        .map_err(ConfigParseError::InvalidCompilerWrapper)
}

fn parsed_term_from_raw(raw: RawTerm) -> Result<ParsedTerm, ConfigParseError> {
    let color = match raw.color {
        Some(value) => Some(
            ColorChoice::from_config_value(value.trim())
                .map_err(ConfigParseError::InvalidTermColor)?,
        ),
        None => None,
    };
    let verbosity = Verbosity::from_config_pair(raw.verbose, raw.quiet)
        .map_err(|_| ConfigParseError::InvalidTermVerbosityCombination)?;
    Ok(ParsedTerm { color, verbosity })
}

fn parsed_toolchain_from_raw(raw: RawToolchain) -> Result<ParsedToolchain, ConfigParseError> {
    let cc = match raw.cc {
        Some(s) => Some(parse_tool_spec(&s, "cc")?),
        None => None,
    };
    let cxx = match raw.cxx {
        Some(s) => Some(parse_tool_spec(&s, "cxx")?),
        None => None,
    };
    let ar = match raw.ar {
        Some(s) => Some(parse_tool_spec(&s, "ar")?),
        None => None,
    };
    Ok(ParsedToolchain { cc, cxx, ar })
}

fn parse_tool_spec(raw: &str, key: &'static str) -> Result<ToolSpec, ConfigParseError> {
    ToolSpec::parse_non_empty(raw).ok_or(ConfigParseError::EmptyToolSpec { key })
}

fn parsed_patches_from_raw(
    rows: BTreeMap<String, RawConfigPatch>,
) -> Result<BTreeMap<PackageName, PatchSource>, ConfigParseError> {
    let mut out = BTreeMap::new();
    for (raw_name, row) in rows {
        let package = PackageName::new(raw_name)
            .map_err(|err| ConfigParseError::InvalidPatchPackageName(err.to_string()))?;
        let RawConfigPatch { path } = row;
        let source = PatchSource::from_path_field(package.as_str(), path).map_err(|source| {
            ConfigParseError::InvalidPatch {
                package: package.as_str().to_owned(),
                source,
            }
        })?;
        out.insert(package, source);
    }
    Ok(out)
}

fn parsed_source_replacements_from_raw(
    rows: BTreeMap<String, RawConfigSourceReplacement>,
) -> Result<BTreeMap<SourceLocator, ParsedSourceReplacement>, ConfigParseError> {
    let mut out = BTreeMap::new();
    for (raw_original, row) in rows {
        reject_credentials_in_url(&raw_original, &raw_original)?;
        let original = locator_from_string(&raw_original);
        let RawConfigSourceReplacement {
            index_path,
            index_url,
        } = row;
        let replacement = match (index_path, index_url) {
            (Some(_), Some(_)) => {
                return Err(ConfigParseError::InvalidSourceReplacement {
                    original: raw_original,
                    source: cabin_core::SourceReplacementError::AmbiguousReplacement {
                        original: original.display(),
                    },
                });
            }
            (Some(path), None) => {
                let trimmed = path.trim();
                if trimmed.is_empty() {
                    return Err(ConfigParseError::InvalidSourceReplacement {
                        original: raw_original,
                        source: cabin_core::SourceReplacementError::MissingReplacement {
                            original: original.display(),
                        },
                    });
                }
                SourceLocator::IndexPath {
                    path: Utf8PathBuf::from(trimmed),
                }
            }
            (None, Some(url)) => {
                let trimmed = url.trim();
                if trimmed.is_empty() {
                    return Err(ConfigParseError::InvalidSourceReplacement {
                        original: raw_original,
                        source: cabin_core::SourceReplacementError::MissingReplacement {
                            original: original.display(),
                        },
                    });
                }
                reject_credentials_in_url(trimmed, &raw_original)?;
                SourceLocator::IndexUrl {
                    url: trimmed.to_owned(),
                }
            }
            (None, None) => {
                return Err(ConfigParseError::InvalidSourceReplacement {
                    original: raw_original,
                    source: cabin_core::SourceReplacementError::MissingReplacement {
                        original: original.display(),
                    },
                });
            }
        };
        out.insert(original, ParsedSourceReplacement { replacement });
    }
    Ok(out)
}

/// Distinguish a URL-shaped key from a path-shaped key.  URLs are
/// identified by the presence of the `://` separator, which
/// covers every supported scheme today (`http`, `https`).
fn locator_from_string(raw: &str) -> SourceLocator {
    if raw.contains("://") {
        SourceLocator::IndexUrl {
            url: raw.to_owned(),
        }
    } else {
        SourceLocator::IndexPath {
            path: Utf8PathBuf::from(raw),
        }
    }
}

/// Replace the `userinfo` component of a URL with `***` so error
/// messages and lockfile output never echo `user:password`.  Inputs
/// that are not URL-shaped, or that have no userinfo, are returned
/// unchanged.  The replacement keeps scheme, host, port, path, query,
/// and fragment intact so the user can still identify the offending
/// URL by host.
pub fn redact_userinfo(raw: &str) -> String {
    let Some((scheme, rest)) = raw.split_once("://") else {
        return raw.to_owned();
    };
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(auth_end);
    match authority.rsplit_once('@') {
        Some((_userinfo, hostport)) => format!("{scheme}://***@{hostport}{tail}"),
        None => raw.to_owned(),
    }
}

/// Return `true` when `raw` carries `userinfo` in its authority
/// (e.g. `https://user:pass@example.com/...`).  The check is a
/// cheap structural lookahead so callers can pair it with a
/// context-specific error variant.
pub fn url_contains_credentials(raw: &str) -> bool {
    if let Some(rest) = raw.split_once("://").map(|(_, rest)| rest) {
        // The authority ends at the first `/`, `?`, or `#`, same as
        // `redact_userinfo`; an `@` in a query or fragment is not
        // userinfo.
        let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        return rest[..auth_end].contains('@');
    }
    false
}

/// Reject URLs that carry `userinfo`.  Cabin's source-replacement
/// model does not handle credentials; quietly accepting them
/// would risk leaking secrets into log output, the lockfile, or
/// the metadata view.  `entry` is the `[source-replacement]` table
/// key so the error identifies the offending row by its key - the
/// same identity every other error in
/// `parsed_source_replacements_from_raw` uses - even when the
/// credentials sit in the replacement URL value.  Both are
/// redacted so `user:password` never reaches stderr or logs.
fn reject_credentials_in_url(raw: &str, entry: &str) -> Result<(), ConfigParseError> {
    if url_contains_credentials(raw) {
        return Err(ConfigParseError::InvalidSourceReplacement {
            original: redact_userinfo(entry),
            source: cabin_core::SourceReplacementError::CredentialsInUrl {
                url: redact_userinfo(raw),
            },
        });
    }
    Ok(())
}

/// Map a known unsupported top-level table to its dedicated error
/// variant.  Auth / credential / token tables get a stable rejection
/// message so a typo cannot smuggle a secret into a published
/// archive.
fn unsupported_auth_table_key(key: &str) -> Option<ConfigParseError> {
    match key {
        "auth" => Some(ConfigParseError::UnsupportedAuthKey { key: "auth" }),
        "credentials" => Some(ConfigParseError::UnsupportedAuthKey { key: "credentials" }),
        "tokens" => Some(ConfigParseError::UnsupportedAuthKey { key: "tokens" }),
        "token" => Some(ConfigParseError::UnsupportedAuthKey { key: "token" }),
        "registries" => Some(ConfigParseError::UnsupportedAuthKey { key: "registries" }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8Path;

    #[test]
    fn empty_config_parses_to_default() {
        let parsed = parse_config_str("").unwrap();
        assert_eq!(parsed, ParsedConfig::default());
    }

    #[test]
    fn registry_index_path_is_captured() {
        let parsed = parse_config_str(
            r#"
            [registry]
            index-path = "registry"
            "#,
        )
        .unwrap();
        assert_eq!(
            parsed.registry,
            Some(ParsedRegistry::Path(Utf8PathBuf::from("registry")))
        );
    }

    #[test]
    fn registry_index_url_is_captured() {
        let parsed = parse_config_str(
            r#"
            [registry]
            index-url = "https://example.com/index"
            "#,
        )
        .unwrap();
        assert_eq!(
            parsed.registry,
            Some(ParsedRegistry::Url("https://example.com/index".into()))
        );
    }

    #[test]
    fn registry_index_url_with_credentials_is_rejected() {
        let err = parse_config_str(
            r#"
            [registry]
            index-url = "https://user:pass@example.com/index"
            "#,
        )
        .unwrap_err();
        match err {
            ConfigParseError::IndexUrlContainsCredentials { url } => {
                assert!(
                    !url.contains("user:pass"),
                    "credentials must be redacted from error, got {url:?}"
                );
                assert!(
                    url.contains("***"),
                    "redacted URL should mark removed userinfo with '***', got {url:?}"
                );
                assert!(
                    url.contains("example.com"),
                    "redacted URL should still name the host, got {url:?}"
                );
            }
            other => panic!("expected IndexUrlContainsCredentials, got {other:?}"),
        }
    }

    #[test]
    fn redact_userinfo_replaces_userinfo_with_marker() {
        assert_eq!(
            redact_userinfo("https://user:pass@example.com/index"),
            "https://***@example.com/index"
        );
        assert_eq!(
            redact_userinfo("http://only-user@host:8080/p?q#f"),
            "http://***@host:8080/p?q#f"
        );
        // No userinfo → unchanged.
        assert_eq!(
            redact_userinfo("https://example.com/index"),
            "https://example.com/index"
        );
        // Garbage with no scheme → unchanged.
        assert_eq!(redact_userinfo("not a url"), "not a url");
    }

    #[test]
    fn url_contains_credentials_checks_only_the_authority() {
        assert!(url_contains_credentials("https://user:pass@example.com"));
        assert!(url_contains_credentials("https://user@example.com?q=1"));
        assert!(!url_contains_credentials("https://example.com/index"));
        // An `@` in the query or fragment is not userinfo, even
        // when the URL has no path component.
        assert!(!url_contains_credentials(
            "https://example.com?token=user@domain.com"
        ));
        assert!(!url_contains_credentials("https://example.com#a@b"));
        assert!(!url_contains_credentials(
            "https://example.com/path?token=a@b"
        ));
    }

    #[test]
    fn registry_index_path_and_url_in_same_file_is_rejected() {
        let err = parse_config_str(
            r#"
            [registry]
            index-path = "registry"
            index-url = "https://example.com/index"
            "#,
        )
        .unwrap_err();
        assert_eq!(err, ConfigParseError::RegistryConflict);
    }

    #[test]
    fn paths_capture_cache_and_build_dir() {
        let parsed = parse_config_str(
            r#"
            [paths]
            cache-dir = ".cabin/cache"
            build-dir = "build"
            "#,
        )
        .unwrap();
        assert_eq!(
            parsed.paths.cache_dir.as_deref(),
            Some(Utf8Path::new(".cabin/cache"))
        );
        assert_eq!(
            parsed.paths.build_dir.as_deref(),
            Some(Utf8Path::new("build"))
        );
    }

    #[test]
    fn paths_with_spaces_round_trip_as_utf8() {
        // Directory paths containing spaces are valid UTF-8 config
        // values and must reach `ParsedPaths` as `Utf8PathBuf`
        // unchanged.
        let parsed = parse_config_str(
            r#"
            [paths]
            cache-dir = "my cache dir"
            build-dir = "out dir/target build"
            "#,
        )
        .unwrap();
        assert_eq!(
            parsed.paths.cache_dir.as_deref(),
            Some(Utf8Path::new("my cache dir"))
        );
        assert_eq!(
            parsed.paths.build_dir.as_deref(),
            Some(Utf8Path::new("out dir/target build"))
        );
    }

    #[test]
    fn paths_reject_empty_string() {
        let err = parse_config_str(
            r#"
            [paths]
            cache-dir = ""
            "#,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigParseError::EmptyPath { key: "cache-dir" }
        ));
    }

    #[test]
    fn build_profile_is_captured() {
        let parsed = parse_config_str(
            r#"
            [build]
            profile = "release"
            "#,
        )
        .unwrap();
        assert_eq!(parsed.build.profile.as_deref(), Some("release"));
    }

    #[test]
    fn build_profile_rejects_empty_value() {
        let err = parse_config_str(
            r#"
            [build]
            profile = ""
            "#,
        )
        .unwrap_err();
        assert_eq!(err, ConfigParseError::EmptyProfile);
    }

    #[test]
    fn build_compiler_wrapper_round_trips() {
        let parsed = parse_config_str(
            r#"
            [build]
            compiler-wrapper = "ccache"
            "#,
        )
        .unwrap();
        assert_eq!(
            parsed.build.compiler_wrapper,
            Some(CompilerWrapperRequest::Use {
                wrapper: cabin_core::ToolSpec::Name("ccache".into()),
            })
        );
    }

    #[test]
    fn build_compiler_wrapper_accepts_any_executable() {
        let parsed = parse_config_str(
            r#"
            [build]
            compiler-wrapper = "fastcache"
            "#,
        )
        .unwrap();
        assert_eq!(
            parsed.build.compiler_wrapper,
            Some(CompilerWrapperRequest::Use {
                wrapper: cabin_core::ToolSpec::Name("fastcache".into()),
            })
        );
    }

    #[test]
    fn toolchain_fields_become_typed_specs() {
        let parsed = parse_config_str(
            r#"
            [toolchain]
            cc = "clang"
            cxx = "/opt/llvm/bin/clang++"
            ar = "llvm-ar"
            "#,
        )
        .unwrap();
        assert_eq!(parsed.toolchain.cc, Some(ToolSpec::Name("clang".into())));
        assert_eq!(
            parsed.toolchain.cxx,
            Some(ToolSpec::Path(Utf8PathBuf::from("/opt/llvm/bin/clang++")))
        );
        assert_eq!(parsed.toolchain.ar, Some(ToolSpec::Name("llvm-ar".into())));
    }

    #[test]
    fn toolchain_rejects_empty_spec() {
        let err = parse_config_str(
            r#"
            [toolchain]
            cxx = "   "
            "#,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigParseError::EmptyToolSpec { key: "cxx" }
        ));
    }

    #[test]
    fn unknown_top_level_table_is_named_in_error() {
        let err = parse_config_str(
            r#"
            [networking]
            mode = "offline"
            "#,
        )
        .unwrap_err();
        match err {
            ConfigParseError::UnknownTopLevelTable { table } => assert_eq!(table, "networking"),
            other => panic!("expected UnknownTopLevelTable, got {other:?}"),
        }
    }

    #[test]
    fn unknown_field_inside_known_table_is_rejected() {
        let err = parse_config_str(
            r#"
            [registry]
            index-path = "registry"
            mirror = "https://mirror.example.com"
            "#,
        )
        .unwrap_err();
        // Mirror is not a known field on `[registry]`; the
        // serde-level rejection surfaces as a Toml error.
        assert!(matches!(err, ConfigParseError::Toml(_)));
    }

    #[test]
    fn target_conditioned_table_is_rejected_clearly() {
        let err = parse_config_str(
            r#"
            [target.'cfg(os = "linux")'.toolchain]
            cxx = "clang++"
            "#,
        )
        .unwrap_err();
        match err {
            ConfigParseError::TargetConditionedNotSupported { table } => {
                assert_eq!(table, "cfg(os = \"linux\")");
            }
            other => panic!("expected TargetConditionedNotSupported, got {other:?}"),
        }
    }

    #[test]
    fn patch_table_parses_path_entry() {
        let parsed = parse_config_str(
            r#"
            [patch]
            fmt = { path = "../fmt" }
            "#,
        )
        .unwrap();
        assert_eq!(parsed.patches.len(), 1);
        let key = PackageName::new("fmt").unwrap();
        match parsed.patches.get(&key) {
            Some(PatchSource::Path { path }) => assert_eq!(path, &Utf8PathBuf::from("../fmt")),
            other => panic!("expected Path patch, got {other:?}"),
        }
    }

    #[test]
    fn patch_table_rejects_unknown_field() {
        let err = parse_config_str(
            r#"
            [patch]
            fmt = { branch = "main" }
            "#,
        )
        .unwrap_err();
        // `deny_unknown_fields` on the row catches `branch`.
        assert!(matches!(err, ConfigParseError::Toml(_)));
    }

    #[test]
    fn source_replacement_parses_index_path_target() {
        let parsed = parse_config_str(
            r#"
            [source-replacement]
            "https://example.com/index" = { index-path = "../mirror" }
            "#,
        )
        .unwrap();
        assert_eq!(parsed.source_replacements.len(), 1);
        let original = SourceLocator::IndexUrl {
            url: "https://example.com/index".into(),
        };
        let entry = parsed
            .source_replacements
            .get(&original)
            .expect("entry present");
        assert!(matches!(
            entry.replacement,
            SourceLocator::IndexPath { ref path } if path == &Utf8PathBuf::from("../mirror")
        ));
    }

    #[test]
    fn source_replacement_rejects_both_path_and_url() {
        let err = parse_config_str(
            r#"
            [source-replacement]
            "https://example.com/index" = { index-path = "../mirror", index-url = "https://other.example.com/index" }
            "#,
        )
        .unwrap_err();
        match err {
            ConfigParseError::InvalidSourceReplacement { source, .. } => assert!(matches!(
                source,
                cabin_core::SourceReplacementError::AmbiguousReplacement { .. }
            )),
            other => panic!("expected InvalidSourceReplacement, got {other:?}"),
        }
    }

    #[test]
    fn source_replacement_rejects_credentials_in_original() {
        let err = parse_config_str(
            r#"
            [source-replacement]
            "https://user:pw@example.com/index" = { index-path = "../mirror" }
            "#,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("must not contain credentials"),
            "expected credential rejection, got: {message}"
        );
        assert!(
            !message.contains("user:pw"),
            "credentials must be redacted from error, got: {message}"
        );
    }

    #[test]
    fn source_replacement_rejects_credentials_in_replacement_url() {
        let err = parse_config_str(
            r#"
            [source-replacement]
            "https://example.com/index" = { index-url = "https://user:pw@mirror.example.com/index" }
            "#,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("must not contain credentials"),
            "expected credential rejection, got: {message}"
        );
        assert!(
            !message.contains("user:pw"),
            "credentials must be redacted from error, got: {message}"
        );
        // The entry is identified by its table key, like every
        // other source-replacement error, so the user knows which
        // row to fix.
        assert!(
            message.contains("https://example.com/index"),
            "error must name the offending entry by key, got: {message}"
        );
    }

    #[test]
    fn source_replacement_rejects_unknown_field() {
        // Generic coverage that `deny_unknown_fields` on a
        // `[source-replacement]` row rejects unknown keys.
        let err = parse_config_str(
            r#"
            [source-replacement]
            "https://example.com/index" = { not-a-real-key = "x" }
            "#,
        )
        .unwrap_err();
        assert!(matches!(err, ConfigParseError::Toml(_)));
    }

    #[test]
    fn source_replacement_rejects_missing_target() {
        let err = parse_config_str(
            r#"
            [source-replacement]
            "https://example.com/index" = { }
            "#,
        )
        .unwrap_err();
        match err {
            ConfigParseError::InvalidSourceReplacement { source, .. } => assert!(matches!(
                source,
                cabin_core::SourceReplacementError::MissingReplacement { .. }
            )),
            other => panic!("expected InvalidSourceReplacement, got {other:?}"),
        }
    }

    #[test]
    fn term_color_auto_is_captured() {
        let parsed = parse_config_str(
            r#"
            [term]
            color = "auto"
            "#,
        )
        .unwrap();
        assert_eq!(parsed.term.color, Some(ColorChoice::Auto));
    }

    #[test]
    fn term_color_always_and_never_round_trip() {
        for (raw, expected) in [
            ("always", ColorChoice::Always),
            ("never", ColorChoice::Never),
        ] {
            let body = format!(
                r#"
                [term]
                color = "{raw}"
                "#
            );
            let parsed = parse_config_str(&body).unwrap();
            assert_eq!(parsed.term.color, Some(expected));
        }
    }

    #[test]
    fn term_color_unknown_value_is_rejected() {
        let err = parse_config_str(
            r#"
            [term]
            color = "sometimes"
            "#,
        )
        .unwrap_err();
        match err {
            ConfigParseError::InvalidTermColor(inner) => {
                assert_eq!(inner.value, "sometimes");
            }
            other => panic!("expected InvalidTermColor, got {other:?}"),
        }
    }

    #[test]
    fn term_color_unknown_field_is_rejected_by_serde() {
        let err = parse_config_str(
            r"
            [term]
            unicode = true
            ",
        )
        .unwrap_err();
        // `deny_unknown_fields` on `[term]` rejects every key
        // except the documented set (`color`, `verbose`,
        // `quiet`).  The serde-level rejection surfaces as a TOML
        // error.
        assert!(matches!(err, ConfigParseError::Toml(_)));
    }

    #[test]
    fn term_verbose_true_yields_verbose_level() {
        let parsed = parse_config_str("[term]\nverbose = true\n").unwrap();
        assert_eq!(parsed.term.verbosity, Some(cabin_core::Verbosity::Verbose));
    }

    #[test]
    fn term_quiet_true_yields_quiet_level() {
        let parsed = parse_config_str("[term]\nquiet = true\n").unwrap();
        assert_eq!(parsed.term.verbosity, Some(cabin_core::Verbosity::Quiet));
    }

    #[test]
    fn term_verbose_and_quiet_both_true_is_rejected() {
        let err = parse_config_str("[term]\nverbose = true\nquiet = true\n").unwrap_err();
        assert!(matches!(
            err,
            ConfigParseError::InvalidTermVerbosityCombination
        ));
    }

    #[test]
    fn term_verbose_false_falls_back_to_unset() {
        let parsed = parse_config_str("[term]\nverbose = false\nquiet = false\n").unwrap();
        assert_eq!(parsed.term.verbosity, None);
    }

    #[test]
    fn auth_token_credential_keys_are_rejected_with_dedicated_error() {
        for key in ["auth", "credentials", "tokens", "token", "registries"] {
            let body = format!("[{key}]\nfoo = \"bar\"\n");
            let err = parse_config_str(&body).unwrap_err();
            match err {
                ConfigParseError::UnsupportedAuthKey { key: rejected } => {
                    assert_eq!(rejected, key, "expected {key} in error, got {rejected}");
                }
                other => panic!("expected UnsupportedAuthKey for `{key}`, got {other:?}"),
            }
        }
    }

    #[test]
    fn build_jobs_positive_integer_parses() {
        let parsed = parse_config_str("[build]\njobs = 4\n").unwrap();
        let jobs = parsed.build.jobs.expect("jobs parsed");
        assert_eq!(jobs.get(), 4);
    }

    #[test]
    fn build_jobs_zero_is_rejected() {
        let err = parse_config_str("[build]\njobs = 0\n").unwrap_err();
        match err {
            ConfigParseError::InvalidBuildJobs { value } => assert_eq!(value, "0"),
            other => panic!("expected InvalidBuildJobs, got {other:?}"),
        }
    }

    #[test]
    fn build_jobs_negative_is_rejected() {
        let err = parse_config_str("[build]\njobs = -2\n").unwrap_err();
        match err {
            ConfigParseError::InvalidBuildJobs { value } => assert_eq!(value, "-2"),
            other => panic!("expected InvalidBuildJobs, got {other:?}"),
        }
    }

    #[test]
    fn build_jobs_non_integer_is_rejected_by_serde() {
        let err = parse_config_str("[build]\njobs = \"many\"\n").unwrap_err();
        assert!(matches!(err, ConfigParseError::Toml(_)));
    }

    #[test]
    fn build_jobs_missing_yields_none() {
        let parsed = parse_config_str("[build]\nprofile = \"dev\"\n").unwrap();
        assert!(parsed.build.jobs.is_none());
    }

    #[test]
    fn build_standard_compat_errors_parses_both_values() {
        let parsed = parse_config_str("[build]\nstandard-compat-errors = false\n").unwrap();
        assert_eq!(parsed.build.standard_compat_errors, Some(false));
        let parsed = parse_config_str("[build]\nstandard-compat-errors = true\n").unwrap();
        assert_eq!(parsed.build.standard_compat_errors, Some(true));
    }

    #[test]
    fn build_standard_compat_errors_missing_yields_none() {
        let parsed = parse_config_str("[build]\nprofile = \"dev\"\n").unwrap();
        assert!(parsed.build.standard_compat_errors.is_none());
    }

    #[test]
    fn build_standard_compat_errors_non_bool_is_rejected_by_serde() {
        let err = parse_config_str("[build]\nstandard-compat-errors = \"warn\"\n").unwrap_err();
        assert!(matches!(err, ConfigParseError::Toml(_)));
    }

    #[test]
    fn resolver_incompatible_standards_parses_both_values() {
        for (raw, expected) in [
            ("allow", IncompatibleStandards::Allow),
            ("fallback", IncompatibleStandards::Fallback),
        ] {
            let body = format!("[resolver]\nincompatible-standards = \"{raw}\"\n");
            let parsed = parse_config_str(&body).unwrap();
            assert_eq!(parsed.resolver.incompatible_standards, Some(expected));
        }
    }

    #[test]
    fn resolver_incompatible_standards_missing_yields_none() {
        let parsed = parse_config_str("[build]\nprofile = \"dev\"\n").unwrap();
        assert!(parsed.resolver.incompatible_standards.is_none());
    }

    #[test]
    fn resolver_incompatible_standards_unknown_value_is_rejected() {
        let err = parse_config_str("[resolver]\nincompatible-standards = \"warn\"\n").unwrap_err();
        match err {
            ConfigParseError::InvalidIncompatibleStandards(inner) => {
                assert_eq!(inner.value, "warn");
            }
            other => panic!("expected InvalidIncompatibleStandards, got {other:?}"),
        }
    }

    #[test]
    fn resolver_unknown_field_is_rejected_by_serde() {
        let err = parse_config_str("[resolver]\nprefer = \"newest\"\n").unwrap_err();
        assert!(matches!(err, ConfigParseError::Toml(_)));
    }
}
