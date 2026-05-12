use std::io;
use std::path::PathBuf;

use cabin_core::TargetKind;
use cabin_core::ValidationError;
use miette::Diagnostic;
use thiserror::Error;

/// Errors produced while loading or interpreting a `cabin.toml`.
#[derive(Debug, Error, Diagnostic)]
pub enum ManifestError {
    #[error("failed to read manifest from {path}: {source}", path = path.display())]
    #[diagnostic(code(cabin::manifest::unreadable))]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to parse cabin.toml: {0}")]
    #[diagnostic(code(cabin::manifest::parse_error))]
    Toml(#[from] toml::de::Error),

    /// Variant emitted when the parser knows the on-disk source
    /// path and original text. Wraps a [`ManifestParseError`]
    /// that owns the source / span metadata the diagnostic
    /// renderer needs to draw an `annotate-snippets`-style
    /// caret. Falls back to the same `parse_error` code as
    /// [`ManifestError::Toml`]. The inner error is boxed because
    /// it carries the full source text plus span metadata.
    #[error(transparent)]
    #[diagnostic(transparent)]
    TomlAt(Box<ManifestParseError>),

    #[error("manifest contains neither a [package] nor a [workspace] table")]
    EmptyManifest,

    #[error("invalid version {value:?} in [package]: {source}")]
    Version {
        value: String,
        #[source]
        source: semver::Error,
    },

    #[error(
        "unknown target type {value:?} for target {target:?} (expected one of: {})",
        supported_target_types()
    )]
    UnknownTargetType { target: String, value: String },

    #[error("dependency {name:?} requires either a `path` or a `version` field")]
    DependencyMissingSource { name: String },

    #[error("dependency {name:?} cannot specify both `path` and `version`; pick one")]
    DependencyHasPathAndVersion { name: String },

    #[error(
        "dependency {name:?} sets `workspace = true`; remove `path` / `version` from the same table — pick one source"
    )]
    WorkspaceDependencyHasOtherSource { name: String },

    #[error(
        "dependency {name:?} sets `workspace = false`; either remove the field or pick a `path` or `version` source"
    )]
    WorkspaceDependencyExplicitlyDisabled { name: String },

    #[error("default workspace member {member:?} is not listed under workspace.members")]
    WorkspaceDefaultMemberMissing { member: String },

    #[error("invalid version requirement {requirement:?} for dependency {name:?}: {source}")]
    InvalidDependencyRequirement {
        name: String,
        requirement: String,
        #[source]
        source: semver::Error,
    },

    #[error(
        "target {target:?} declares `type = \"cpp_header_only\"` but lists `sources`; header-only libraries ship only `include_dirs`"
    )]
    HeaderOnlyDeclaresSources { target: String },

    // -----------------------------------------------------------------
    // Dependency-kind errors.
    // -----------------------------------------------------------------
    #[error(
        "dependency {name:?} in {section} sets `system = true` but also declares {field:?}; {detail}"
    )]
    SystemConflictsWith {
        name: String,
        section: &'static str,
        field: &'static str,
        detail: &'static str,
    },

    #[error("system dependency {name:?} requires a `version` requirement string")]
    SystemDependencyMissingVersion { name: String },

    #[error("target-specific dependency table {section:?} is not supported")]
    TargetSpecificDependenciesNotSupported { section: String },

    #[error(
        "optional dependencies are not supported in {section}; declared optional dependency: {name:?}",
        section = kind.manifest_section(),
    )]
    OptionalNotSupportedForKind {
        name: String,
        kind: cabin_core::DependencyKind,
    },

    #[error(
        "dependency {name:?} declares an empty feature name in `features`; feature names must be non-empty"
    )]
    EmptyDependencyFeatureName { name: String },

    #[error("invalid target cfg expression in `[target.{raw}]`: {source}")]
    InvalidTargetCfg {
        raw: String,
        #[source]
        source: cabin_core::ConditionParseError,
    },

    #[error("invalid `[target.{raw}]` table: {source}")]
    InvalidConditionalTargetTable {
        raw: String,
        #[source]
        source: Box<toml::de::Error>,
    },

    #[error(
        "dependency {name:?} uses workspace = true inside `[target.cfg({condition})]`; workspace inheritance is not currently supported under target-conditional tables — declare the dependency unconditionally and gate its use with features instead"
    )]
    WorkspaceInsideConditionalTarget { name: String, condition: String },

    #[error(transparent)]
    Validation(#[from] ValidationError),

    // -----------------------------------------------------------------
    // Profile errors.
    // -----------------------------------------------------------------
    #[error(
        "invalid profile name {value:?}; profile names must be non-empty, must not start with `.`, must not be `.` or `..`, and may only contain ASCII alphanumerics, `_`, `-`, or `.`"
    )]
    InvalidProfileName { value: String },

    #[error(
        "profile {profile:?} declares `inherits = {value:?}`, which is not a valid profile name"
    )]
    InvalidInheritedProfileName { profile: String, value: String },

    // -----------------------------------------------------------------
    // Toolchain / build-flag errors.
    // -----------------------------------------------------------------
    /// `[toolchain].cc` / `cxx` / `ar` was set to an empty or
    /// whitespace-only string.
    #[error("[toolchain] tool spec must be a non-empty string")]
    EmptyToolSpec,

    /// `[profile]` defines or include directories failed
    /// validation.
    #[error("invalid [profile] table: {0}")]
    InvalidBuildFlags(#[source] cabin_core::BuildFlagsValidationError),

    /// A `[profile.cache] compiler-wrapper = "<value>"` declaration
    /// could not be turned into a typed
    /// [`cabin_core::CompilerWrapperRequest`] (empty value or an
    /// unsupported wrapper name).
    #[error("invalid {section}.compiler-wrapper: {source}")]
    InvalidCompilerWrapper {
        /// The TOML section the bad value lived under, e.g.
        /// `"[profile.cache]"` or
        /// `"[target.'cfg(unix)'.profile.cache]"`. Echoes the
        /// user input so the error points at exactly the table
        /// they edited.
        section: String,
        #[source]
        source: cabin_core::CompilerWrapperParseError,
    },

    /// A `[patch]` table entry could not be turned into a typed
    /// [`cabin_core::PatchSource`]. The wrapping variant carries
    /// the offending package name so the user sees which row in
    /// the table needs attention.
    #[error("invalid `[patch]` entry for `{package}`: {source}")]
    InvalidPatch {
        package: String,
        #[source]
        source: cabin_core::PatchValidationError,
    },
}

/// Source-annotated form of a TOML parse failure. Carries the
/// original file path, the full source text, and the offending
/// byte span so the diagnostic renderer can draw a snippet with
/// a caret at the failing region.
///
/// The struct is reachable from outside as
/// [`ManifestError::TomlAt`]; callers that already own the
/// path / source text construct it through [`ManifestParseError::from_toml`].
#[derive(Debug, Error, Diagnostic)]
#[error("could not parse Cabin manifest at {path}", path = path.display())]
#[diagnostic(
    code(cabin::manifest::parse_error),
    help("check that the manifest is valid TOML; the caret above marks where the parser stopped")
)]
pub struct ManifestParseError {
    pub path: PathBuf,
    #[source_code]
    pub source_text: miette::NamedSource,
    /// Stable label text the diagnostic renderer prints next to
    /// the caret. Precomputed from the inner `toml::de::Error`'s
    /// message so the user sees the actual cause — for example,
    /// "unknown field `required`" — rather than a generic
    /// "syntax error".
    pub label_message: String,
    #[label("{label_message}")]
    pub span: miette::SourceSpan,
    #[source]
    pub source: toml::de::Error,
}

impl From<ManifestParseError> for ManifestError {
    fn from(value: ManifestParseError) -> Self {
        Self::TomlAt(Box::new(value))
    }
}

impl ManifestParseError {
    /// Build a [`ManifestParseError`] from a `toml::de::Error`
    /// plus the path and full text Cabin already has at parse
    /// time. Falls back to a zero-width span at byte 0 when the
    /// underlying error has no `.span()`.
    pub fn from_toml(path: PathBuf, source_text: String, source: toml::de::Error) -> Self {
        let span = source.span().map_or_else(
            || miette::SourceSpan::new(0.into(), 0.into()),
            |range| miette::SourceSpan::new(range.start.into(), range.len().into()),
        );
        let display_path = path.display().to_string();
        let label_message = source.message().to_owned();
        Self {
            path,
            source_text: miette::NamedSource::new(display_path, source_text),
            label_message,
            span,
            source,
        }
    }
}

fn supported_target_types() -> String {
    let mut out = String::new();
    for (i, kind) in TargetKind::all().iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push('"');
        out.push_str(kind.as_str());
        out.push('"');
    }
    out
}
