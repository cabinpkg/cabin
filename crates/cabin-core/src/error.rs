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
        "package name {0:?} is not valid; package names must consist only of ASCII letters, ASCII digits, `_`, `-`, and `.`, must be non-empty, must not start with `.` or `-`, and must not be `.` or `..`"
    )]
    UnsafePackageName(String),

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
