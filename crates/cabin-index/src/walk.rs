//! The dependency walk both index sources share: the file loader
//! reads each package document from disk by name, and
//! `cabin-index-http` fetches it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use cabin_core::PackageName;

use crate::model::IndexEntry;

/// Load the packages reachable from `roots` through non-optional
/// dependency edges, calling `fetch` at most once per package and
/// never for a name in `known`.
///
/// Every non-yanked version's edges are followed, because resolution
/// may select any of them; a yanked version is never selected, so its
/// edges are not.  Optional edges are left for resolution to request
/// once a feature enables one.  `fetch` returns `None` for a package
/// the index does not have, which is recorded as an entry without
/// versions: whether a missing package matters is the resolver's
/// call, since an unselected version's dependency must not fail the
/// walk.
///
/// # Errors
/// Returns the first error `fetch` returns.
pub fn walk_dependencies<E>(
    roots: &[PackageName],
    known: &BTreeSet<PackageName>,
    mut fetch: impl FnMut(&PackageName) -> Result<Option<IndexEntry>, E>,
) -> Result<BTreeMap<PackageName, IndexEntry>, E> {
    let mut packages: BTreeMap<PackageName, IndexEntry> = BTreeMap::new();
    let mut queue: VecDeque<PackageName> = roots.iter().cloned().collect();
    while let Some(name) = queue.pop_front() {
        if packages.contains_key(&name) || known.contains(&name) {
            continue;
        }
        let entry = fetch(&name)?.unwrap_or_else(|| IndexEntry {
            name: name.clone(),
            versions: BTreeMap::new(),
        });
        for meta in entry.versions.values().filter(|meta| !meta.yanked) {
            queue.extend(
                meta.dependencies
                    .iter()
                    .filter(|(dep, edge)| !edge.optional && !packages.contains_key(*dep))
                    .map(|(dep, _)| dep.clone()),
            );
        }
        packages.insert(name, entry);
    }
    Ok(packages)
}
