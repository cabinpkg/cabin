//! Convert `PubGrub`'s `NoSolution` derivation tree into a
//! Cabin-owned [`ResolveError::Conflict`].
//!
//! `PubGrub` reports unsolvable inputs as a `DerivationTree` plus
//! a default reporter that renders human-readable prose.  Cabin
//! normalizes both sides of that output so the rendered
//! diagnostic is deterministic and Cabin's stable
//! [`ResolveError`] surface does not leak `PubGrub` types to
//! callers.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use pubgrub::{DefaultStringReporter, DerivationTree, External, Ranges, Reporter};
use semver::Version;

use crate::error::ResolveError;
use crate::input::ResolveInput;
use crate::provider::Node;

type Tree = DerivationTree<Node, Ranges<Version>, String>;

/// The largest derivation, counted as a tree, that is collapsed
/// before reporting.  `collapse_no_versions` unshares every shared
/// derivation (`Arc::make_mut`), so it costs the tree size, which a
/// deep conflict makes exponential in the graph it shares.  At this
/// limit collapsing takes tens of milliseconds and megabytes; above
/// it the explanation keeps its "no versions" lines, true but noisy.
const COLLAPSE_LIMIT: u64 = 100_000;

/// Build a [`ResolveError::Conflict`] from `PubGrub`'s
/// `NoSolution` derivation tree.
///
/// A tree within [`COLLAPSE_LIMIT`] is collapsed first, so unused
/// "no versions" branches stay out of the explanation and the
/// named package; a larger one is reported as it is.  The report
/// comes from [`DefaultStringReporter`], which follows shared
/// derivations, normalized so the message is byte-stable across
/// runs.  A conflict involving a locally supplied package ends with
/// a note naming its one version, which no requirement can move.
pub(crate) fn explain_no_solution(mut tree: Tree, input: &ResolveInput) -> ResolveError {
    if tree_size(&tree, &mut HashMap::new()) <= COLLAPSE_LIMIT {
        tree.collapse_no_versions();
    }
    let mut nodes = BTreeSet::new();
    collect_nodes(&tree, &mut HashSet::new(), &mut nodes);
    let names: BTreeSet<_> = nodes
        .iter()
        .filter(|node| !matches!(node, Node::Root(_)))
        .map(|node| node.package_name())
        .collect();
    let mut detail = normalize_explanation(&DefaultStringReporter::report(&tree));
    let locals: Vec<String> = names
        .iter()
        .filter_map(|name| {
            let version = input.local_packages.get(name)?.versions.keys().next()?;
            Some(format!("{name} {version}"))
        })
        .collect();
    if !locals.is_empty() {
        detail += "\n\nnote: supplied locally (workspace member, path dependency, or `[patch]`), so no other version can be selected: ";
        detail += &locals.join(", ");
    }
    ResolveError::Conflict {
        package: names
            .first()
            .copied()
            .unwrap_or(&input.root_name)
            .as_str()
            .to_owned(),
        detail,
    }
}

/// The node count of `tree` expanded as a tree, saturating, with
/// each shared derivation counted once per reference but walked
/// once.
fn tree_size(tree: &Tree, sizes: &mut HashMap<*const Tree, u64>) -> u64 {
    let DerivationTree::Derived(derived) = tree else {
        return 1;
    };
    [&derived.cause1, &derived.cause2]
        .into_iter()
        .fold(1, |total, cause| {
            let size = if let Some(&size) = sizes.get(&Arc::as_ptr(cause)) {
                size
            } else {
                let size = tree_size(cause, sizes);
                sizes.insert(Arc::as_ptr(cause), size);
                size
            };
            total.saturating_add(size)
        })
}

/// Every node the derivation graph mentions, walking each shared
/// derivation once (`DerivationTree::packages` revisits them).  The
/// conflict's representative package is the alphabetically-first
/// non-root one, falling back to the root when only the root appears.
fn collect_nodes<'a>(
    tree: &'a Tree,
    walked: &mut HashSet<*const Tree>,
    nodes: &mut BTreeSet<&'a Node>,
) {
    match tree {
        DerivationTree::External(External::FromDependencyOf(p, _, p2, _)) => {
            nodes.extend([p, p2]);
        }
        DerivationTree::External(
            External::NoVersions(p, _) | External::NotRoot(p, _) | External::Custom(p, _, _),
        ) => {
            nodes.insert(p);
        }
        DerivationTree::Derived(derived) => {
            nodes.extend(derived.terms.keys());
            for cause in [&derived.cause1, &derived.cause2] {
                if walked.insert(Arc::as_ptr(cause)) {
                    collect_nodes(cause, walked, nodes);
                }
            }
        }
    }
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
