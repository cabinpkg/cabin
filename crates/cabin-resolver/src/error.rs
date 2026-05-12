use cabin_core::PackageName;
use thiserror::Error;

/// Errors produced by [`crate::resolve`].
#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("package {0:?} was not found in index")]
    UnknownPackage(String),

    #[error(
        "no version of {package:?} satisfies the following constraints: {}",
        format_constraints(.constraints)
    )]
    NoMatchingVersion {
        package: String,
        constraints: Vec<ResolverConstraint>,
    },

    #[error("all matching versions of {0:?} are yanked")]
    AllMatchingVersionsYanked(String),

    #[error("dependency resolution failed for {package:?}: {detail}")]
    Conflict { package: String, detail: String },

    // ---- locked-mode failures ----
    #[error(
        "cabin.lock has no entry for required package {0:?}; run `cabin resolve` or `cabin update` to refresh it"
    )]
    LockfileMissingPackage(String),

    #[error("locked package {name:?} {version} is not present in the index")]
    LockedVersionMissing { name: String, version: String },

    #[error("locked package {name:?} {version} is yanked")]
    LockedVersionYanked { name: String, version: String },

    #[error(
        "locked version of {name:?} ({version}) does not satisfy the current constraint(s): {}",
        format_constraints(.constraints)
    )]
    LockedVersionViolatesConstraint {
        name: String,
        version: String,
        constraints: Vec<ResolverConstraint>,
    },

    #[error(
        "checksum mismatch for locked package {name:?} {version}: lockfile says {expected:?}, index says {actual:?}"
    )]
    LockedChecksumMismatch {
        name: String,
        version: String,
        expected: String,
        actual: String,
    },
}

/// One constraint observed by the resolver, carrying the requirement and
/// the package that imposed it. Surfaced inside [`ResolveError::NoMatchingVersion`]
/// so the user can see *why* nothing matched.
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
