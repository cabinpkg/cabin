use thiserror::Error;

use crate::config::InvalidFeatureEntryKind;
use crate::model::DependencyKind;

/// Errors produced when validating values that compose the internal package
/// model.  These are kept independent from manifest-parsing or CLI-specific
/// errors so future producers (registry, lockfile, build graph) can reuse the
/// same validation surface.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("package name must not be empty")]
    EmptyPackageName,

    #[error("package name must not contain whitespace: {0:?}")]
    PackageNameContainsWhitespace(String),

    #[error(
        "package name {0:?} is not valid; a package name is `<name>` or `<scope>/<name>`, where `<name>` matches `[a-z0-9][a-z0-9_-]*` (lowercase ASCII letters, ASCII digits, `_`, and `-`, starting with a letter or digit){hint}",
        hint = lowercase_hint(.0)
    )]
    InvalidPackageName(String),

    #[error(
        "package name {0:?} is not valid; a scoped package name is `<scope>/<name>` with exactly one `/`"
    )]
    PackageNameTooManySlashes(String),

    #[error(
        "package scope {scope:?} in {name:?} is not valid; scopes must consist only of ASCII lowercase letters, ASCII digits, and `-`, must not start or end with `-`, and must be at most 39 characters{hint}",
        hint = lowercase_hint(name)
    )]
    InvalidPackageScope { name: String, scope: String },

    #[error("target name must not be empty")]
    EmptyTargetName,

    #[error("target name must not contain whitespace: {0:?}")]
    TargetNameContainsWhitespace(String),

    #[error(
        "target name {0:?} is not valid; target names must consist only of ASCII letters, ASCII digits, `_`, `-`, and `.`, must be non-empty, must not start with `.` or `-`, and must not be `.` or `..`"
    )]
    UnsafeTargetName(String),

    #[error("duplicate target name: {0:?}")]
    DuplicateTargetName(String),

    #[error(
        "system dependency name {0:?} is not valid; a pkg-config module name must consist only of ASCII letters, ASCII digits, `_`, `-`, and `.`, must not start with `.` or `-`, and must not be `.` or `..`"
    )]
    InvalidSystemDependencyName(String),

    #[error(
        "invalid `links` value {0:?}; a native-library identity must be non-empty and consist only of ASCII letters, ASCII digits, `.`, `_`, `+`, and `-`"
    )]
    InvalidLinksValue(String),

    #[error(
        "target {target:?} ({kind}) cannot claim `links` {links:?}; only `library` targets produce a linkable artifact that can embody a native-library identity"
    )]
    LinksOnNonLibraryTarget {
        target: String,
        kind: String,
        links: String,
    },

    #[error(
        "targets {first:?} and {second:?} both claim the native-library identity {links:?}; a package may claim each `links` value at most once"
    )]
    DuplicateTargetLinks {
        links: String,
        first: String,
        second: String,
    },

    #[error(
        "target {target:?} lists instruction sets for {path:?}, which is not one of its C or C++ sources; each key must be a path from the target's `sources` or a `[target.'cfg(...)'.sources]` list"
    )]
    InstructionSetsForUnknownSource {
        target: String,
        path: camino::Utf8PathBuf,
    },

    #[error(
        "target {target:?} include directory {path:?} must be relative to the manifest directory; absolute paths are not allowed"
    )]
    TargetIncludeDirIsAbsolute { target: String, path: String },

    #[error(
        "target {target:?} include directory {path:?} must not contain `..`; include search paths cannot escape the package root"
    )]
    TargetIncludeDirHasParent { target: String, path: String },

    #[error("duplicate dependency {name:?} in {section}", section = kind.manifest_section())]
    DuplicateDependency { name: String, kind: DependencyKind },

    #[error("duplicate system dependency: {0:?}")]
    DuplicateSystemDependency(String),

    // ---- Features ----------------------------------------------
    #[error("{0} name must not be empty")]
    EmptyConfigName(&'static str),

    #[error("invalid {kind} name {value:?}")]
    InvalidConfigName { kind: &'static str, value: String },

    #[error("the feature name {0:?} is reserved")]
    ReservedFeatureName(String),

    #[error("feature {referrer:?} references unknown feature {referenced:?}")]
    UnknownFeatureReference {
        referrer: String,
        referenced: String,
    },

    #[error("feature definitions contain a cycle: {}", .0.join(" -> "))]
    FeatureCycle(Vec<String>),

    #[error(
        "invalid entry {entry:?} in feature {referrer:?}: {}",
        reason.message()
    )]
    InvalidFeatureEntry {
        referrer: String,
        entry: String,
        reason: InvalidFeatureEntryKind,
    },

    #[error("unknown feature {feature:?} for package {package:?}")]
    UnknownFeature { package: String, feature: String },

    #[error(
        "target {target:?} requires unknown feature {feature:?}; `required-features` entries must name features declared in this package's `[features]` table"
    )]
    UnknownRequiredFeature { target: String, feature: String },
}

/// `; did you mean "boa"?` when lowercasing alone makes `name` a valid
/// package name.  Only a suggestion: package names are never folded.
fn lowercase_hint(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower != name && crate::PackageName::new(lower.as_str()).is_ok() {
        format!("; did you mean {lower:?}?")
    } else {
        String::new()
    }
}
