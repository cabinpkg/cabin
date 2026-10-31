//! The dependency walk both index sources share: the file loader
//! reads each package document from disk by name, and
//! `cabin-index-http` fetches it.

use std::collections::{BTreeMap, BTreeSet};

use cabin_core::PackageName;

use crate::model::IndexEntry;

/// Load the packages reachable from `roots` through non-optional
/// dependency edges, never asking for a name in `known` or one asked
/// for before.
///
/// The walk goes in waves: `fetch` gets every name the previous wave
/// newly reached, in sorted order, and returns their entries in that
/// order, so a caller can fetch a wave concurrently.  Every non-yanked
/// version's edges are followed, because resolution may select any of
/// them; a yanked version is never selected, so its edges are not.
/// Optional edges are left for resolution to request once a feature
/// enables one.  `fetch` returns `None` for a package the index does
/// not have, which is recorded as an entry without versions: whether a
/// missing package matters is the resolver's call, since an unselected
/// version's dependency must not fail the walk.
///
/// # Errors
/// Returns the first error `fetch` returns.
pub fn walk_dependencies<E>(
    roots: &[PackageName],
    known: &BTreeSet<PackageName>,
    mut fetch: impl FnMut(&[PackageName]) -> Result<Vec<Option<IndexEntry>>, E>,
) -> Result<BTreeMap<PackageName, IndexEntry>, E> {
    let mut packages: BTreeMap<PackageName, IndexEntry> = BTreeMap::new();
    let mut wave: BTreeSet<PackageName> = roots.iter().cloned().collect();
    loop {
        wave.retain(|name| !packages.contains_key(name) && !known.contains(name));
        if wave.is_empty() {
            return Ok(packages);
        }
        let names: Vec<PackageName> = std::mem::take(&mut wave).into_iter().collect();
        let entries = fetch(&names)?;
        for (name, entry) in names.into_iter().zip(entries) {
            let entry = entry.unwrap_or_else(|| IndexEntry {
                name: name.clone(),
                versions: BTreeMap::new(),
            });
            for meta in entry.versions.values().filter(|meta| !meta.yanked) {
                wave.extend(
                    meta.dependencies
                        .iter()
                        .filter(|(_, edge)| !edge.optional)
                        .map(|(dep, _)| dep.clone()),
                );
            }
            packages.insert(name, entry);
        }
    }
}
