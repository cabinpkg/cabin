use cabin_core::{PackageName, SourceLanguage};
use pubgrub::SelectedDependencies;
use semver::Version;
use serde::Serialize;

use crate::input::ResolveInput;

/// Resolution result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveOutput {
    /// Resolved packages, including the root, sorted with the root first
    /// and registry packages alphabetical by name.
    pub packages: Vec<ResolvedPackage>,
    /// Packages whose newest in-range version was passed over because
    /// [`IncompatibleStandards::Fallback`](cabin_core::IncompatibleStandards::Fallback)
    /// found it standard-incompatible with the workspace.  Empty under
    /// `Allow`, and whenever selection matched the pure-semver newest
    /// (ordinary semver hold-backs are never reported).  Sorted by
    /// package name.
    pub held_back: Vec<HeldBack>,
}

/// A standard-compatibility note about a resolved package.  Three
/// shapes, distinguished by `newest` and `blocked_by`:
/// - a newer version was passed over because it is **incompatible**
///   (`newest` = `Some`, `blocked_by` non-empty);
/// - a newer version was passed over only because it is **undeclared**
///   and preference ranks a declared-compatible version above it
///   (`newest` = `Some`, `blocked_by` empty);
/// - the selected version is **itself incompatible** because nothing in
///   range satisfies the consumer (`newest` = `None`, the rule-2 case of
///   `preference-mode.md` section 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldBack {
    pub name: PackageName,
    /// The version preference mode selected.
    pub selected: Version,
    /// The newer in-range version passed over, or `None` when the
    /// selected version is the newest and is itself incompatible.
    pub newest: Option<Version>,
    /// The blocking interface requirement(s), in a fixed C-before-C++
    /// order for deterministic output - of `newest` when it is
    /// incompatible, of `selected` in the rule-2 case, and empty when
    /// the newer version was passed over only for being undeclared.
    pub blocked_by: Vec<BlockedRequirement>,
}

impl HeldBack {
    /// The update-facing one-line message, e.g.
    /// `foo v1.4.0 (available: v2.3.0, requires interface c++20)`;
    /// `foo v1.4.0 (available: v2.3.0, preferred as declared-compatible
    /// over the undeclared newer version)`; or
    /// `foo v2.0.0 (requires interface c++20; no compatible version in
    /// range)` when the selected version is itself incompatible.
    #[must_use]
    pub fn message(&self) -> String {
        let clauses = self
            .blocked_by
            .iter()
            .map(BlockedRequirement::clause)
            .collect::<Vec<_>>()
            .join("; ");
        match (&self.newest, self.blocked_by.is_empty()) {
            (Some(newest), false) => format!(
                "{} v{} (available: v{newest}, {clauses})",
                self.name.as_str(),
                self.selected,
            ),
            (Some(newest), true) => format!(
                "{} v{} (available: v{newest}, preferred as declared-compatible over the undeclared newer version)",
                self.name.as_str(),
                self.selected,
            ),
            (None, _) => format!(
                "{} v{} ({clauses}; no compatible version in range)",
                self.name.as_str(),
                self.selected,
            ),
        }
    }
}

/// One interface requirement of a held-back version that the
/// workspace consumer fails to satisfy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedRequirement {
    /// The consumer language whose requirement is unmet.
    pub language: SourceLanguage,
    /// The declared minimum standard (e.g. `c++20`), or `None` when
    /// the interface is declared `"none"` (unconsumable at any level).
    pub minimum: Option<String>,
}

impl BlockedRequirement {
    /// The requirement clause used inside [`HeldBack::message`].
    #[must_use]
    fn clause(&self) -> String {
        match &self.minimum {
            Some(level) => format!("requires interface {level}"),
            None => format!("declares interface {} = \"none\"", self.language_str()),
        }
    }

    fn language_str(&self) -> &'static str {
        match self.language {
            SourceLanguage::C => "c",
            SourceLanguage::Cxx => "c++",
        }
    }
}

/// One package selected by the resolver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPackage {
    pub name: PackageName,
    pub version: semver::Version,
    pub source: ResolvedSource,
}

/// Where a [`ResolvedPackage`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedSource {
    /// The root package being resolved for.
    Root,
    /// Selected from the local JSON package index.
    Index,
}

impl ResolvedSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ResolvedSource::Root => "root",
            ResolvedSource::Index => "index",
        }
    }
}

/// Assemble the public [`ResolveOutput`] from `PubGrub`'s
/// [`SelectedDependencies`] map.
///
/// The output keeps the root package first and sorts the
/// remaining registry packages alphabetically by name (with
/// version as a secondary key for the deterministic-ordering
/// invariant).  `PubGrub` types stay confined to this
/// boundary - callers see only Cabin-owned [`ResolvedPackage`]
/// values.
pub(crate) fn selected_dependencies_to_output(
    input: &ResolveInput,
    solution: SelectedDependencies<PackageName, Version>,
) -> ResolveOutput {
    let mut others: Vec<ResolvedPackage> = solution
        .into_iter()
        .filter(|(name, _)| name != &input.root_name)
        .map(|(name, version)| ResolvedPackage {
            name,
            version,
            source: ResolvedSource::Index,
        })
        .collect();
    others.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));

    let mut packages = Vec::with_capacity(others.len() + 1);
    packages.push(ResolvedPackage {
        name: input.root_name.clone(),
        version: input.root_version.clone(),
        source: ResolvedSource::Root,
    });
    packages.extend(others);
    ResolveOutput {
        packages,
        held_back: Vec::new(),
    }
}
