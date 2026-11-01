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

    /// [`ManifestError::Toml`] as returned by `load_manifest`, which
    /// knows the on-disk path and original text.  Wraps a
    /// [`ManifestParseError`] that owns the source / span metadata
    /// the diagnostic renderer needs to draw a caret, under the same
    /// `parse_error` code.  Boxed because it carries the full source
    /// text.
    #[error(transparent)]
    #[diagnostic(transparent)]
    TomlAt(Box<ManifestParseError>),

    #[error("manifest contains neither a [package] nor a [workspace] table")]
    EmptyManifest,

    #[error(
        "`{table}` flags compile this manifest's own package, but it has no [package]; put them in the member manifests that need them (`[profile.<name>]` flags here reach every package in the build, registry dependencies included)"
    )]
    ProfileFlagsWithoutPackage { table: String },

    #[error(
        "`{table}` applies to this manifest's own package, but it has no [package]; declare it in the member manifests that need it"
    )]
    PackageTableWithoutPackage { table: String },

    #[error("invalid version {value:?} in [package]: {source}")]
    Version {
        value: String,
        #[source]
        source: semver::Error,
    },

    #[error(
        "invalid `[package] include` entry {value:?}: expected a forward-slash relative path with no `.` or `..` components"
    )]
    InvalidInclude { value: String },

    #[error("invalid `[package.upstream]`: {source}")]
    Upstream {
        #[source]
        source: cabin_core::UpstreamError,
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

    #[error("invalid version requirement {requirement:?} for dependency {name:?}: {source}")]
    InvalidDependencyRequirement {
        name: String,
        requirement: String,
        #[source]
        source: semver::Error,
    },

    #[error(
        "target {target:?} declares `type = \"header-only\"` but lists `sources`; header-only libraries ship only `include-dirs`"
    )]
    HeaderOnlyDeclaresSources { target: String },

    #[error(
        "target {target:?} declares `type = \"header-only\"` but lists `defines`; a header-only library compiles nothing, so definitions its consumers need go in `interface-defines`"
    )]
    HeaderOnlyDeclaresDefines { target: String },

    #[error(
        "target {target:?} has an invalid `interface-defines` entry {value:?}; expected `NAME` or `NAME=value` with a C identifier for NAME, no control or bidirectional-formatting characters, and each NAME defined once"
    )]
    InvalidInterfaceDefine { target: String, value: String },

    #[error(
        "`[target.{condition:?}.sources]` names {target:?}, which is not a target of this package"
    )]
    ConditionalSourcesUnknownTarget { condition: String, target: String },

    #[error(
        "`[target.{condition:?}.sources]` may only test the platform (`os`, `arch`, `family`, `env`, `abi`, `target`); a target's sources must not depend on features or the detected compiler"
    )]
    ConditionalSourcesNotPlatformOnly { condition: String },

    // -----------------------------------------------------------------
    // Dependency-kind errors.
    // -----------------------------------------------------------------
    #[error(
        "dependency {name:?} in {section} sets `system = true` but also declares {field:?}; the field is incompatible with `system = true`"
    )]
    SystemConflictsWith {
        name: String,
        section: &'static str,
        field: &'static str,
    },

    #[error("system dependency {name:?} requires a `version` requirement string")]
    SystemDependencyMissingVersion { name: String },

    #[error("target-specific dependency table {section:?} is not supported")]
    TargetSpecificDependenciesNotSupported { section: String },

    #[error(
        "`cfg(feature = ...)` in `[target.{condition:?}]` may only gate a `.profile` table, not the `{table}` table; feature resolution runs over the dependency graph, so a feature-gated dependency or toolchain would be circular"
    )]
    FeatureConditionNotAllowedHere {
        condition: String,
        table: &'static str,
    },

    #[error(
        "compiler conditions (`cfg(cc = ...)`, `cfg(cxx = ...)`, and their `_version` forms) in `[target.{condition:?}]` may only gate a `.profile` flag table, not the `{table}` table; compiler identity is detected from the resolved toolchain, which is not available when dependencies or the toolchain itself are selected"
    )]
    CompilerConditionNotAllowedHere {
        condition: String,
        table: &'static str,
    },

    #[error(
        "system dependencies are not supported in {section}; declared system dependency: {name:?}",
        section = kind.manifest_section(),
    )]
    SystemNotSupportedForKind {
        name: String,
        kind: cabin_core::DependencyKind,
    },

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
    #[error(transparent)]
    InvalidProfileName(cabin_core::InvalidProfileName),

    #[error("profile {profile:?} has an invalid `inherits` value: {source}")]
    InvalidInheritedProfileName {
        profile: String,
        source: cabin_core::InvalidProfileName,
    },

    #[error(
        "`inherits` is not allowed in {table}; profile inheritance must be defined by `[profile.{profile}]`"
    )]
    NamedTargetProfileInherits { table: String, profile: String },

    #[error(
        "`{field}` is not allowed in {table}; target-specific named profile overlays may only contain array flag fields"
    )]
    NamedTargetProfileField { table: String, field: String },

    #[error(
        "unknown field `{field}` in {table}; supported fields are defines, include-dirs, cflags, cxxflags, ldflags, link-libs, and frameworks"
    )]
    UnknownNamedTargetProfileField { table: String, field: String },

    // -----------------------------------------------------------------
    // Toolchain / build-flag errors.
    // -----------------------------------------------------------------
    /// `[toolchain].cc` / `cxx` / `ar` was set to an empty or
    /// whitespace-only string.
    #[error("[toolchain] tool spec must be a non-empty string")]
    EmptyToolSpec,

    /// A `[profile]` flag table's defines, include directories, link
    /// libraries, or frameworks failed validation.
    #[error("invalid [profile] table: {0}")]
    InvalidBuildFlags(#[source] cabin_core::BuildFlagsValidationError),

    /// A `c-standard` / `cxx-standard` / `interface-c-standard` /
    /// `interface-cxx-standard` value is not a recognized language
    /// standard.  The source error lists the valid spellings.
    #[error("{0}")]
    InvalidLanguageStandard(#[source] cabin_core::LanguageStandardParseError),

    /// A target-level interface field was declared on a target kind
    /// that has no public interface for consumers.
    #[error(
        "target {target:?} is `{kind}`; `{field}` describes a library's public interface and only applies to `library` / `header-only` targets"
    )]
    InterfaceFieldOnNonLibrary {
        target: String,
        kind: String,
        field: &'static str,
    },

    /// A target compiles sources of a language with no effective
    /// implementation standard: neither the target nor `[package]`
    /// declares one.  Cabin has no built-in default; a
    /// `{ workspace = true }` opt-in counts as declaring.
    #[error(
        "target {target:?} compiles {language} sources but no {language} standard is declared; add `{field} = \"<standard>\"` to `[package]` or `[target.{target}]`, or opt into a workspace default with `{field} = {{ workspace = true }}` on `[package]`"
    )]
    MissingLanguageStandard {
        target: String,
        language: &'static str,
        field: &'static str,
    },

    /// A header-only target declares no interface standard at any
    /// level, so consumers cannot know what standard its public
    /// headers require.
    #[error(
        "header-only target {target:?} declares no interface language standard; add `interface-c-standard` or `interface-cxx-standard` to `[package]` or `[target.{target}]`, or opt into a workspace default with the field set to `{{ workspace = true }}` on `[package]`"
    )]
    HeaderOnlyMissingInterfaceStandard { target: String },

    /// A `[package]`-level standard field set `workspace = false`.
    #[error(
        "`{field}` sets `workspace = false`; either remove the field or declare a literal standard value"
    )]
    WorkspaceStandardExplicitlyDisabled { field: &'static str },

    /// A `{ min, max }` range table on an implementation-standard
    /// field.  Sources compile at exactly one standard; only the
    /// interface fields describe a range of acceptable consumers.
    #[error(
        "`{field}` selects the single standard the sources compile with; `min` / `max` range tables are only valid on `interface-c-standard` / `interface-cxx-standard`"
    )]
    RangeOnImplementationStandard { field: &'static str },

    /// An interface range table without the required `min` key.
    #[error(
        "`{field}` range table is missing `min`; declare the inclusive bounds as `{field} = {{ min = \"...\", max = \"...\" }}` (omit `max` for a minimum-only requirement)"
    )]
    RangeMissingMin { field: &'static str },

    /// `{ workspace = true }` on a `[workspace]`-level standard
    /// field, which declares the defaults members opt into.
    #[error(
        "`{field} = {{ workspace = true }}` is not valid on the `[workspace]` table; declare the literal default value members opt into"
    )]
    WorkspaceMarkerOnWorkspaceTable { field: &'static str },

    /// `{ workspace = true }` on a `[target.<name>]` standard
    /// field.  Workspace standard inheritance is a `[package]`-level
    /// feature.
    #[error(
        "target {target:?} sets `{field} = {{ workspace = true }}`; workspace standard inheritance only applies to [package]-level fields"
    )]
    WorkspaceStandardOnTarget { target: String, field: &'static str },

    /// A `[build] compiler-wrapper = "<value>"` declaration could
    /// not be turned into a typed
    /// [`cabin_core::CompilerWrapperRequest`].
    #[error("invalid [build].compiler-wrapper: {source}")]
    InvalidCompilerWrapper {
        #[source]
        source: cabin_core::CompilerWrapperParseError,
    },

    /// A `[patch]` table entry could not be turned into a typed
    /// [`cabin_core::PatchSource`].  The wrapping variant carries
    /// the offending package name so the user sees which row in
    /// the table needs attention.
    #[error("invalid `[patch]` entry for `{package}`: {source}")]
    InvalidPatch {
        package: String,
        #[source]
        source: cabin_core::PatchValidationError,
    },

    /// A `[system-provided]` entry is not one valid module or a
    /// non-empty table of valid modules per target.
    #[error("invalid `[system-provided]` entry for `{package}`: {source}")]
    InvalidSystemProvider {
        package: String,
        #[source]
        source: cabin_core::SystemProviderError,
    },
}

/// Source-annotated form of a TOML parse failure.  Carries the
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
    pub source_text: miette::NamedSource<String>,
    /// Stable label text the diagnostic renderer prints next to
    /// the caret.  Precomputed from the inner `toml::de::Error`'s
    /// message so the user sees the actual cause - for example,
    /// "unknown field `required`" - rather than a generic
    /// "syntax error".
    pub label_message: String,
    #[label("{label_message}")]
    pub span: miette::SourceSpan,
    #[source]
    pub source: toml::de::Error,
}

impl ManifestParseError {
    /// Build a [`ManifestParseError`] from a `toml::de::Error`
    /// plus the path and full text Cabin already has at parse
    /// time.  Falls back to a zero-width span at byte 0 when the
    /// underlying error has no `.span()`.
    pub fn from_toml(path: PathBuf, source_text: String, source: toml::de::Error) -> Self {
        let span = source.span().map_or_else(
            || miette::SourceSpan::new(0_usize.into(), 0),
            |range| miette::SourceSpan::new(range.start.into(), range.len()),
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
    TargetKind::all()
        .iter()
        .map(|kind| format!("\"{}\"", kind.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}
