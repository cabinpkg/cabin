use cabin_core::PackageName;
use miette::Diagnostic;
use thiserror::Error;

/// Errors produced by [`crate::resolve`].
///
/// Each variant carries the actionable user context (package
/// name, locked version, observed constraints, …) the
/// `cabin-diagnostics` layer renders through `miette`. The
/// stable diagnostic code is
/// [`cabin_diagnostics::code::RESOLVER_ERROR`];
/// per-variant `help` text complements the message body.
#[non_exhaustive]
#[derive(Debug, Error, Diagnostic)]
pub enum ResolveError {
    #[error("package {0:?} was not found in index")]
    #[diagnostic(
        code(cabin::resolver::error),
        help(
            "check the dependency name in cabin.toml, verify it is published in the configured registry, and confirm the index source with `--index-path` or `--index-url`"
        )
    )]
    UnknownPackage(String),

    #[error(
        "no version of {package:?} satisfies the following constraints: {}",
        format_constraints(.constraints)
    )]
    #[diagnostic(
        code(cabin::resolver::error),
        help(
            "relax the version requirement in cabin.toml, publish a version that satisfies it, or update other dependencies whose constraints are tighter than necessary"
        )
    )]
    NoMatchingVersion {
        package: String,
        constraints: Vec<ResolverConstraint>,
    },

    #[error("all matching versions of {0:?} are yanked")]
    #[diagnostic(
        code(cabin::resolver::error),
        help(
            "loosen the version requirement so a non-yanked release is in range, or contact the package maintainer to republish"
        )
    )]
    AllMatchingVersionsYanked(String),

    #[error("dependency resolution failed for {package:?}:\n{detail}")]
    #[diagnostic(
        code(cabin::resolver::error),
        help(
            "review the listed version requirements; loosening or aligning them usually resolves the conflict"
        )
    )]
    Conflict { package: String, detail: String },

    // ---- locked-mode failures ----
    #[error(
        "cabin.lock has no entry for required package {0:?}; run `cabin resolve` or `cabin update` to refresh it"
    )]
    #[diagnostic(
        code(cabin::resolver::error),
        help(
            "run `cabin update` (or drop `--locked`) so the lockfile picks up the missing package"
        )
    )]
    LockfileMissingPackage(String),

    #[error("locked package {name:?} {version} is not present in the index")]
    #[diagnostic(
        code(cabin::resolver::error),
        help(
            "run `cabin update` to refresh the lockfile, or restore the missing version in the index"
        )
    )]
    LockedVersionMissing { name: String, version: String },

    #[error("locked package {name:?} {version} is yanked")]
    #[diagnostic(
        code(cabin::resolver::error),
        help("run `cabin update` to pick a non-yanked replacement")
    )]
    LockedVersionYanked { name: String, version: String },

    #[error(
        "locked version of {name:?} ({version}) does not satisfy the current constraint(s): {}",
        format_constraints(.constraints)
    )]
    #[diagnostic(
        code(cabin::resolver::error),
        help(
            "run `cabin update` to refresh the lockfile, or loosen the constraint so the locked version still applies"
        )
    )]
    LockedVersionViolatesConstraint {
        name: String,
        version: String,
        constraints: Vec<ResolverConstraint>,
    },

    #[error(
        "checksum mismatch for locked package {name:?} {version}: lockfile says {expected:?}, index says {actual:?}"
    )]
    #[diagnostic(
        code(cabin::resolver::error),
        help(
            "investigate the source-archive change before continuing; run `cabin update` only after confirming the index entry is the intended one"
        )
    )]
    LockedChecksumMismatch {
        name: String,
        version: String,
        expected: String,
        actual: String,
    },

    #[error("unsupported version requirement for package {package:?}: {requirement}")]
    #[diagnostic(
        code(cabin::resolver::error),
        help("update Cabin or use a version requirement syntax supported by this release")
    )]
    UnsupportedVersionRequirement {
        package: String,
        requirement: String,
    },
}
/// One constraint observed by the resolver, carrying the requirement and
/// the package that imposed it. Surfaced inside
/// [`ResolveError::NoMatchingVersion`] and
/// [`ResolveError::LockedVersionViolatesConstraint`] so the user can see
/// *why* nothing matched.
#[derive(Debug, Clone)]
pub struct ResolverConstraint {
    pub origin: PackageName,
    pub requirement: semver::VersionReq,
}

fn format_constraints(constraints: &[ResolverConstraint]) -> String {
    let mut parts: Vec<String> = constraints
        .iter()
        .map(|c| format!("{} requires {}", c.origin.as_str(), c.requirement))
        .collect();
    parts.sort();
    parts.join("; ")
}
