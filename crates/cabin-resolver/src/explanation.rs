//! Convert `PubGrub`'s `NoSolution` derivation tree into a
//! Cabin-owned [`ResolveError::Conflict`].
//!
//! `PubGrub` reports unsolvable inputs as a `DerivationTree` plus
//! a default reporter that renders human-readable prose.  Cabin
//! normalizes both sides of that output so the rendered
//! diagnostic is deterministic and Cabin's stable
//! [`ResolveError`] surface does not leak `PubGrub` types to
//! callers.

use cabin_core::PackageName;
use pubgrub::{DefaultStringReporter, DerivationTree, Ranges, Reporter};
use semver::Version;

use crate::error::ResolveError;

/// Build a [`ResolveError::Conflict`] from `PubGrub`'s
/// `NoSolution` derivation tree.
///
/// The tree is collapsed first so unused "no versions" branches
/// do not appear in the rendered explanation, then routed
/// through [`DefaultStringReporter`] and normalized so the
/// resulting message is byte-stable across runs.
pub(crate) fn explain_no_solution(
    mut tree: DerivationTree<PackageName, Ranges<Version>, String>,
    root_name: &PackageName,
) -> ResolveError {
    tree.collapse_no_versions();
    let detail = DefaultStringReporter::report(&tree);
    ResolveError::Conflict {
        package: pick_conflict_package(&tree, root_name),
        detail: normalize_explanation(&detail),
    }
}

/// Pick a representative package name to attach to a
/// `Conflict` error.  Walks the derivation tree and returns the
/// alphabetically-first non-root package mentioned, or falls
/// back to the root name when only the root appears.
fn pick_conflict_package(
    tree: &DerivationTree<PackageName, Ranges<Version>, String>,
    root: &PackageName,
) -> String {
    let mut names: Vec<&PackageName> = tree.packages().into_iter().filter(|p| *p != root).collect();
    names.sort();
    names
        .first()
        .map_or_else(|| root.as_str().to_owned(), |p| p.as_str().to_owned())
}

/// Normalize `PubGrub`'s reporter output for deterministic
/// inclusion in error messages: trim trailing whitespace per
/// line and at the end, leave line breaks intact otherwise.
fn normalize_explanation(detail: &str) -> String {
    detail
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_owned()
}
