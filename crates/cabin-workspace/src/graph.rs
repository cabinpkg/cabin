use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use cabin_core::standard_compatibility::{ConsumerStandards, dependency_attributes};
use cabin_core::{
    CStandard, CompilerWrapperRequest, Condition, CxxStandard, DependencyKind, Package,
    PatchManifestSettings, ProfileDefinition, ProfileName, ToolchainSettings,
    resolve_language_standards,
};

/// Root-manifest policy settings that apply workspace-wide even
/// when the entry manifest is a pure `[workspace]` manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RootSettings {
    pub profiles: BTreeMap<ProfileName, ProfileDefinition>,
    pub toolchain: ToolchainSettings,
    pub compiler_wrapper: Option<CompilerWrapperRequest>,
    pub patches: PatchManifestSettings,
}

impl From<cabin_manifest::RootSettings> for RootSettings {
    fn from(value: cabin_manifest::RootSettings) -> Self {
        Self {
            profiles: value.profiles,
            toolchain: value.toolchain,
            compiler_wrapper: value.compiler_wrapper,
            patches: value.patches,
        }
    }
}

/// A loaded set of local Cabin packages with their dependency edges
/// resolved against the local filesystem.
///
/// Packages appear in topological order: a package's local dependencies
/// always appear before the package itself in [`PackageGraph::packages`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageGraph {
    /// Path to the manifest the user passed (canonicalized to absolute).
    pub root_manifest_path: PathBuf,
    /// Directory containing the root manifest.
    pub root_dir: PathBuf,
    /// Whether the root manifest declares a `[workspace]` table.
    pub is_workspace_root: bool,
    /// If the root manifest itself is a package (i.e. has a `[package]`
    /// table), the index of that package in [`PackageGraph::packages`].
    pub root_package: Option<usize>,
    /// Root-manifest policy settings.  For package roots this
    /// mirrors the root package's root-owned fields; for pure
    /// workspace roots this is the only place those settings are
    /// exposed.
    pub root_settings: RootSettings,
    /// Indices of packages that count as "primary" - i.e. would be built
    /// when no narrower package selection is given.
    ///
    /// For a single package this is the root.  For a workspace root it
    /// is every member declared by `[workspace.members]`.  Path dependencies
    /// pulled in transitively are *not* primary.
    pub primary_packages: Vec<usize>,
    /// Indices of packages listed under
    /// `[workspace.default-members]`, validated to be members.  Empty
    /// when the workspace declares no defaults - callers fall back to
    /// the documented "all members" behavior.  Always a subset of
    /// `primary_packages`.
    pub default_members: Vec<usize>,
    /// Relative paths under `root_dir` for any directories
    /// dropped by `[workspace.exclude]`.  Carried through purely for
    /// metadata reporting; the loader has already removed them from
    /// `primary_packages`.
    pub excluded_members: Vec<PathBuf>,
    /// All loaded packages, in topological order.
    pub packages: Vec<WorkspacePackage>,
}

impl PackageGraph {
    /// Find a package by name.  Linear scan; package counts are small.
    pub fn package_by_name(&self, name: &str) -> Option<&WorkspacePackage> {
        self.packages
            .iter()
            .find(|p| p.package.name.as_str() == name)
    }

    /// Index of a package by name.  Returned together with the reference
    /// for callers that need to record edges by index.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.packages
            .iter()
            .position(|p| p.package.name.as_str() == name)
    }

    /// The consumer standards for standard-aware version preference
    /// (`docs/design/standard-compatibility/preference-mode.md`
    /// section 1): per language, the minimum effective implementation
    /// standard (spec D6 `impl_L`) across the targets of `members` that
    /// implement it.  `None` for a language none of them compiles - it
    /// then imposes nothing on candidate ordering.
    ///
    /// `members` must be the package set the resolve is actually for -
    /// the selected closure
    /// ([`ResolvedSelection::closure`](crate::ResolvedSelection::closure)),
    /// not the whole graph - so an unselected member never lowers the
    /// consumer standard for a scoped resolve.  Within each member the
    /// targets this invocation can build count: default-buildable kinds
    /// always, plus dev-only (`test` / `example`) targets for packages
    /// named in `dev_for` (the set whose `[dev-dependencies]` this
    /// invocation activates, e.g. `cabin test`), and in both cases only
    /// when their `required-features` are satisfied by `enabled_features`
    /// (keyed by package index).  A target gated behind an unenabled
    /// feature, or a `test` / `example` under a plain `cabin build`, does
    /// not lower the consumer standard.  Dev-only targets are counted
    /// whenever `dev_for` activates them, without a per-target reachability
    /// walk - the same conservative over-approximation applied to a path
    /// dependency's libraries (below): it can only prefer an older, more
    /// broadly compatible version, never lock one a built target (such as
    /// an example a selected target references in `deps`) cannot consume.
    ///
    /// The set is deliberately every default-buildable (plus `dev_for`)
    /// target of the selected packages, **not** the single target a
    /// `--bin` / `--example` / test-name narrows a later build to.
    /// `cabin.lock` is shared per project, so its versions must suit
    /// every target `cabin build` compiles; scoping resolution to one
    /// run/test target would under-constrain the shared lock for its
    /// siblings.  Which target is finally compiled is a build-time
    /// decision, downstream of resolution.
    ///
    /// This is the Cargo-style workspace-level approximation used during
    /// a partial solve: exactness is not required because the
    /// post-resolution validation remains the correctness authority.
    ///
    /// `primary` is the originally selected package set
    /// ([`ResolvedSelection::packages`](crate::ResolvedSelection::packages)),
    /// a subset of `members`: `members` also holds the transitive
    /// path-dependency packages the closure pulls in.  A path dependency
    /// is built only for the library targets its consumers link, never
    /// for its own executables/tests, so a non-primary member counts
    /// only its archive-producing (library) targets.  Whether each such
    /// library is in turn *reachable* (linked by a consumer target) is
    /// deliberately not computed here: that per-target build-graph walk
    /// is the planner's post-resolution job, and counting a path
    /// dependency's archive targets is a conservative over-approximation
    /// in the safe direction - it can only prefer an older, more broadly
    /// compatible version, never cause a wrong build.
    ///
    /// This extends to a path dependency reached only through a
    /// feature-disabled optional edge: the loader records optional path
    /// edges unconditionally (only disabled optional *registry* deps are
    /// pruned), and this walk does no package-level feature-reachability
    /// pruning of `members`.  That is deliberate and equally safe - each
    /// added member contributes only to the per-language `min`, which
    /// extra targets can lower but never raise, so an unbuilt optional
    /// dependency can at most prefer an older, more broadly compatible
    /// version. Pruning it would only ever raise the preferred version
    /// and never changes solvability, so it is left to the planner.
    #[must_use]
    pub fn consumer_standards(
        &self,
        members: &BTreeSet<usize>,
        primary: &[usize],
        enabled_features: &HashMap<usize, BTreeSet<String>>,
        dev_for: &BTreeSet<String>,
    ) -> ConsumerStandards {
        let empty = BTreeSet::new();
        let mut c: Option<CStandard> = None;
        let mut cxx: Option<CxxStandard> = None;
        for &index in members {
            let member = &self.packages[index];
            let is_primary = primary.contains(&index);
            let enabled = enabled_features.get(&index).unwrap_or(&empty);
            let dev_active = dev_for.contains(member.package.name.as_str());
            let resolved = resolve_language_standards(&member.package.language);
            for target in &member.package.targets {
                // A header-only target has no translation units, so as a
                // consumer it compiles nothing (spec D7 `langs = empty`)
                // and imposes no consumer level - even though
                // `dependency_attributes` reports its header-only
                // inference on the dependency side.
                if target.kind.is_header_only() {
                    continue;
                }
                // Only targets this invocation can compile count.  A
                // primary package builds its default-buildable targets
                // and, under `dev_for` (`cabin test`), its dev-only
                // (`test` / `example`) targets; a path-dep member builds
                // only the library targets it is linked for.  Dev-only
                // targets are counted whenever `dev_for` activates them,
                // without walking which are actually reachable from a
                // selected target - the same safe over-approximation
                // applied to a path dependency's libraries below.  A
                // selected target may reference an `example` in its
                // `deps` (the planner then compiles it), so excluding
                // examples could raise the consumer above a built
                // example's standard and lock a version it cannot
                // consume; counting one that a run does not reach only
                // lowers the consumer (an older, more broadly compatible
                // pick), never raises it.
                let built = if is_primary {
                    target.kind.is_default_buildable() || (dev_active && target.kind.is_dev_only())
                } else {
                    target.kind.produces_archive()
                };
                if !built || !target.missing_required_features(enabled).is_empty() {
                    continue;
                }
                let attributes = dependency_attributes(target, &resolved, &member.package.language);
                if let Some(level) = attributes.impl_c {
                    c = Some(c.map_or(level, |current| current.min(level)));
                }
                if let Some(level) = attributes.impl_cxx {
                    cxx = Some(cxx.map_or(level, |current| current.min(level)));
                }
            }
        }
        ConsumerStandards { c, cxx }
    }
}

/// A single loaded package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePackage {
    pub package: Package,
    /// Absolute path to this package's `cabin.toml`.
    pub manifest_path: PathBuf,
    /// Absolute path to the directory containing `manifest_path`.
    pub manifest_dir: PathBuf,
    /// Resolved package-dependency edges, in declaration order.
    /// Each edge carries the index of the depended-on package
    /// inside [`PackageGraph::packages`] together with the
    /// [`DependencyKind`] under which it was declared.
    ///
    /// `Normal`-kind edges always appear here.  `Dev`-kind edges
    /// appear only when the loader was asked to activate this
    /// package's `[dev-dependencies]` via
    /// `WorkspaceLoadOptions::include_dev_for` - `cabin test` does
    /// that for the selected packages; ordinary commands keep dev
    /// deps declaration-only.  The kind is preserved per-edge so
    /// consumers can filter appropriately.
    pub deps: Vec<DependencyEdge>,
    /// Whether this package was loaded from a local source tree
    /// or from an extracted registry archive.
    pub kind: PackageKind,
    /// Whether this package is a prepared foundation port (its
    /// source tree was materialized from a `port.toml` recipe).
    /// Ports are also [`PackageKind::Local`] - this flag is what
    /// distinguishes them from ordinary `path` dependencies so
    /// `cabin tree` / `explain` can tag them `[port]`.
    pub is_port: bool,
}

impl WorkspacePackage {
    /// Iterate dependency edges of a single kind.  Used by the
    /// build planner, which resolves ordinary targets through
    /// `Normal`-kind edges only and additionally lets dev-only
    /// targets (`test` / `example`) see activated `Dev`-kind edges.
    pub fn deps_of_kind(&self, kind: DependencyKind) -> impl Iterator<Item = usize> + '_ {
        self.deps
            .iter()
            .filter(move |edge| edge.kind == kind)
            .map(|edge| edge.index)
    }

    /// Iterate all dependency edges as bare indices, in
    /// declaration order.  Used by closure walks (resolve / fetch /
    /// metadata) that include every package-graph-resident kind.
    pub fn all_dep_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.deps.iter().map(|edge| edge.index)
    }
}

/// A single resolved package-dependency edge in the package graph.
///
/// The graph only contains edges that *could* be active on the
/// evaluation platform (the loader filters out non-matching
/// `[target.'cfg(...)'.<kind>]` entries before constructing the
/// graph), so consumers never need to re-check the condition
/// against a different platform - the loader already did.  The
/// edge still records the originating condition for diagnostics
/// and metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyEdge {
    /// Index of the depended-on package in [`PackageGraph::packages`].
    pub index: usize,
    /// Which manifest section this edge was declared under.
    pub kind: DependencyKind,
    /// `Some` when this edge originated from a
    /// `[target.'cfg(...)'.<kind>]` table that matched the
    /// evaluation platform; `None` for unconditional edges.
    pub condition: Option<Condition>,
    /// Whether the declaration opted this edge out of the
    /// experimental `standard-compat` check with
    /// `ignore-interface-standard = true`.  Per-edge by design;
    /// inert unless the check runs.
    pub ignore_interface_standard: bool,
}

/// Where a [`WorkspacePackage`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    /// A local-filesystem package: the workspace root or a member, a
    /// `path = "..."` dependency, a `[patch]`ed package, or a prepared
    /// foundation port.
    ///
    /// `Local` is the trust boundary used when deciding whether to honor
    /// a package's own raw `[profile]` compiler/linker flags: every
    /// `Local` source is user-controlled.  Root / members / path deps are
    /// local working trees; patches are local override copies; and a
    /// port's build flags come from its trusted overlay recipe (bundled
    /// or user-pinned), not the downloaded source archive.  The loader
    /// guarantees a downloaded registry archive can never introduce a
    /// `Local` package, because it rejects `path` / `port` dependencies
    /// declared by a [`PackageKind::Registry`] package.
    Local,
    /// A registry package whose source archive was already fetched and
    /// extracted into the artifact cache.  Untrusted: its own `[profile]`
    /// `cflags` / `cxxflags` / `ldflags` are dropped during build-flag
    /// resolution.
    Registry,
}

/// Synthesize a root identity for resolving over a pure-workspace
/// root (no `[package]`).  The name is a deterministic
/// `__workspace_<dirname>` value the resolver uses for diagnostic
/// output only; nothing else relies on it being canonical.  Lives
/// here because it is derived purely from a [`PackageGraph`]'s
/// `root_dir`, keeping the synthetic-root naming rule out of the CLI.
///
/// # Panics
/// Panics only if the constructed name were rejected by
/// `PackageName::new`, which cannot happen: `sanitized` always begins
/// with the literal `__workspace_` prefix (so it is non-empty) and
/// every appended character is ASCII alphanumeric, `_`, or `-`.
pub fn synthetic_root_identity(graph: &PackageGraph) -> (cabin_core::PackageName, semver::Version) {
    let dirname = graph
        .root_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("workspace");
    let mut sanitized = String::with_capacity(dirname.len() + 12);
    sanitized.push_str("__workspace_");
    for c in dirname.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
            sanitized.push(c);
        } else {
            sanitized.push('_');
        }
    }
    let name =
        cabin_core::PackageName::new(sanitized).expect("synthesized name is non-empty and ASCII");
    let version = semver::Version::new(0, 0, 0);
    (name, version)
}

#[cfg(test)]
mod consumer_standards_tests {
    use super::*;
    use cabin_core::{
        CxxStandard, Features, LanguageStandardSettings, PackageConfigInput, PackageName,
        StandardDeclaration, Target, TargetKind, TargetName,
    };
    use camino::Utf8PathBuf;

    fn compiled_target(name: &str, ext: &str, language: LanguageStandardSettings) -> Target {
        Target {
            name: TargetName::new(name).unwrap(),
            kind: TargetKind::Library,
            sources: vec![Utf8PathBuf::from(format!("src/{name}.{ext}"))],
            include_dirs: Vec::new(),
            defines: Vec::new(),
            deps: Vec::new(),
            required_features: Vec::new(),
            language,
        }
    }

    fn gated_target(
        name: &str,
        ext: &str,
        language: LanguageStandardSettings,
        required_features: &[&str],
    ) -> Target {
        Target {
            required_features: required_features.iter().map(|f| (*f).to_owned()).collect(),
            ..compiled_target(name, ext, language)
        }
    }

    fn header_only_target(name: &str, language: LanguageStandardSettings) -> Target {
        Target {
            kind: TargetKind::HeaderOnly,
            sources: Vec::new(),
            include_dirs: vec![Utf8PathBuf::from("include")],
            ..compiled_target(name, "h", language)
        }
    }

    fn executable_target(name: &str, ext: &str, language: LanguageStandardSettings) -> Target {
        Target {
            kind: TargetKind::Executable,
            ..compiled_target(name, ext, language)
        }
    }

    fn member(name: &str, targets: Vec<Target>) -> WorkspacePackage {
        member_with_features(name, targets, Features::default())
    }

    fn member_with_features(
        name: &str,
        targets: Vec<Target>,
        features: Features,
    ) -> WorkspacePackage {
        let package = Package::with_config(PackageConfigInput {
            name: PackageName::new(name).unwrap(),
            version: semver::Version::new(1, 0, 0),
            targets,
            dependencies: Vec::new(),
            system_dependencies: Vec::new(),
            features,
        })
        .unwrap();
        WorkspacePackage {
            package,
            manifest_path: PathBuf::from(format!("/ws/{name}/cabin.toml")),
            manifest_dir: PathBuf::from(format!("/ws/{name}")),
            deps: Vec::new(),
            kind: PackageKind::Local,
            is_port: false,
        }
    }

    fn graph(packages: Vec<WorkspacePackage>) -> PackageGraph {
        PackageGraph {
            root_manifest_path: PathBuf::from("/ws/cabin.toml"),
            root_dir: PathBuf::from("/ws"),
            is_workspace_root: true,
            root_package: None,
            root_settings: RootSettings::default(),
            primary_packages: (0..packages.len()).collect(),
            default_members: Vec::new(),
            excluded_members: Vec::new(),
            packages,
        }
    }

    fn cxx(level: CxxStandard) -> LanguageStandardSettings {
        LanguageStandardSettings {
            cxx_standard: Some(StandardDeclaration::Declared(level)),
            ..Default::default()
        }
    }

    fn c(level: CStandard) -> LanguageStandardSettings {
        LanguageStandardSettings {
            c_standard: Some(StandardDeclaration::Declared(level)),
            ..Default::default()
        }
    }

    /// The consumer standard is the per-language minimum implementation
    /// standard across every member target, and `None` for a language
    /// no member compiles.
    #[test]
    fn consumer_standard_is_the_workspace_minimum_per_language() {
        let workspace = graph(vec![
            member(
                "a",
                vec![compiled_target("a", "cc", cxx(CxxStandard::Cxx20))],
            ),
            member(
                "b",
                vec![
                    compiled_target("b", "cc", cxx(CxxStandard::Cxx17)),
                    compiled_target("bc", "c", c(CStandard::C17)),
                ],
            ),
        ]);
        let all: BTreeSet<usize> = (0..workspace.packages.len()).collect();
        let consumer =
            workspace.consumer_standards(&all, &[0, 1], &HashMap::new(), &BTreeSet::new());
        assert_eq!(consumer.cxx, Some(CxxStandard::Cxx17));
        assert_eq!(consumer.c, Some(CStandard::C17));
    }

    /// Only the given members count: scoping to the C++20 member alone
    /// keeps the consumer at C++20 even though a C++17 member exists in
    /// the graph - a scoped resolve is not lowered by an unselected
    /// member.
    #[test]
    fn consumer_standard_is_scoped_to_the_given_members() {
        let workspace = graph(vec![
            member(
                "app20",
                vec![compiled_target("app20", "cc", cxx(CxxStandard::Cxx20))],
            ),
            member(
                "other17",
                vec![compiled_target("other17", "cc", cxx(CxxStandard::Cxx17))],
            ),
        ]);
        let only_app20: BTreeSet<usize> = [0].into_iter().collect();
        let consumer =
            workspace.consumer_standards(&only_app20, &[0], &HashMap::new(), &BTreeSet::new());
        assert_eq!(consumer.cxx, Some(CxxStandard::Cxx20));
    }

    /// A target gated behind an unenabled feature does not lower the
    /// consumer standard; enabling its feature counts it.
    #[test]
    fn feature_gated_target_does_not_lower_consumer_until_enabled() {
        let features = Features::new(
            Vec::new(),
            [("legacy".to_owned(), Vec::new())].into_iter().collect(),
        )
        .unwrap();
        let workspace = graph(vec![member_with_features(
            "app",
            vec![
                compiled_target("app", "cc", cxx(CxxStandard::Cxx20)),
                gated_target("legacy", "cc", cxx(CxxStandard::Cxx17), &["legacy"]),
            ],
            features,
        )]);
        let members: BTreeSet<usize> = [0].into_iter().collect();

        // Feature off: the c++17 target is not built, so the consumer
        // stays at c++20.
        assert_eq!(
            workspace
                .consumer_standards(&members, &[0], &HashMap::new(), &BTreeSet::new())
                .cxx,
            Some(CxxStandard::Cxx20)
        );

        // Feature on: the c++17 target is built and lowers the consumer.
        let enabled: HashMap<usize, BTreeSet<String>> =
            [(0, ["legacy".to_owned()].into_iter().collect())]
                .into_iter()
                .collect();
        assert_eq!(
            workspace
                .consumer_standards(&members, &[0], &enabled, &BTreeSet::new())
                .cxx,
            Some(CxxStandard::Cxx17)
        );
    }

    /// A dev-only (`test`) target counts only when this invocation
    /// activates the package's dev-dependencies (`dev_for`), matching
    /// `cabin test`; a plain build does not let it lower the consumer.
    #[test]
    fn dev_only_target_counts_only_for_dev_for_packages() {
        let test_target = Target {
            kind: TargetKind::Test,
            ..compiled_target("app_test", "cc", cxx(CxxStandard::Cxx17))
        };
        let workspace = graph(vec![member(
            "app",
            vec![
                compiled_target("app", "cc", cxx(CxxStandard::Cxx20)),
                test_target,
            ],
        )]);
        let members: BTreeSet<usize> = [0].into_iter().collect();

        // Plain build: the c++17 test target is not built.
        assert_eq!(
            workspace
                .consumer_standards(&members, &[0], &HashMap::new(), &BTreeSet::new())
                .cxx,
            Some(CxxStandard::Cxx20)
        );

        // `cabin test` on this package: the test target is built and
        // lowers the consumer.
        let dev_for: BTreeSet<String> = ["app".to_owned()].into_iter().collect();
        assert_eq!(
            workspace
                .consumer_standards(&members, &[0], &HashMap::new(), &dev_for)
                .cxx,
            Some(CxxStandard::Cxx17)
        );
    }

    /// An `example` is dev-only: it counts under `dev_for` (`cabin test`)
    /// exactly like a `test` target.  A selected target can reference an
    /// example in its `deps`, so the planner may compile it; counting
    /// every activated example (the safe over-approximation) keeps the
    /// consumer low enough that a built example never gets a version it
    /// cannot consume.  A plain build does not activate it.
    #[test]
    fn example_target_counts_under_dev_for() {
        let example_target = Target {
            kind: TargetKind::Example,
            ..compiled_target("app_example", "cc", cxx(CxxStandard::Cxx17))
        };
        let workspace = graph(vec![member(
            "app",
            vec![
                compiled_target("app", "cc", cxx(CxxStandard::Cxx20)),
                example_target,
            ],
        )]);
        let members: BTreeSet<usize> = [0].into_iter().collect();

        // Plain build: the c++17 example is not built.
        assert_eq!(
            workspace
                .consumer_standards(&members, &[0], &HashMap::new(), &BTreeSet::new())
                .cxx,
            Some(CxxStandard::Cxx20)
        );

        // `cabin test` activates dev-only targets: the c++17 example
        // lowers the consumer standard.
        let dev_for: BTreeSet<String> = ["app".to_owned()].into_iter().collect();
        assert_eq!(
            workspace
                .consumer_standards(&members, &[0], &HashMap::new(), &dev_for)
                .cxx,
            Some(CxxStandard::Cxx17)
        );
    }

    /// A header-only target has no translation units, so it imposes no
    /// consumer standard - even though `dependency_attributes` reports
    /// its header-only inference on the dependency side.
    #[test]
    fn header_only_target_imposes_no_consumer_standard() {
        // A package whose only target is header-only compiles nothing.
        let workspace = graph(vec![member(
            "hdr",
            vec![header_only_target("hdr", cxx(CxxStandard::Cxx20))],
        )]);
        let members: BTreeSet<usize> = [0].into_iter().collect();
        let consumer =
            workspace.consumer_standards(&members, &[0], &HashMap::new(), &BTreeSet::new());
        assert_eq!(consumer.cxx, None);
        assert_eq!(consumer.c, None);

        // A header-only c++17 target beside a compiled c++20 library does
        // not lower the consumer below c++20.
        let workspace = graph(vec![member(
            "app",
            vec![
                header_only_target("hdr", cxx(CxxStandard::Cxx17)),
                compiled_target("app", "cc", cxx(CxxStandard::Cxx20)),
            ],
        )]);
        assert_eq!(
            workspace
                .consumer_standards(&members, &[0], &HashMap::new(), &BTreeSet::new())
                .cxx,
            Some(CxxStandard::Cxx20)
        );
    }

    /// A transitive path-dependency package is built only for the
    /// library targets its consumers link; its own executable is never
    /// built, so a non-primary member's executable does not lower the
    /// consumer standard.
    #[test]
    fn path_dependency_executable_does_not_lower_consumer() {
        let workspace = graph(vec![
            member(
                "app",
                vec![compiled_target("app", "cc", cxx(CxxStandard::Cxx20))],
            ),
            member(
                "dep",
                vec![
                    compiled_target("dep", "cc", cxx(CxxStandard::Cxx20)),
                    executable_target("dep_bin", "cc", cxx(CxxStandard::Cxx17)),
                ],
            ),
        ]);
        // `app` is the selected primary; `dep` is a transitive path
        // dependency (in the closure, not primary).
        let members: BTreeSet<usize> = [0, 1].into_iter().collect();
        let consumer =
            workspace.consumer_standards(&members, &[0], &HashMap::new(), &BTreeSet::new());
        assert_eq!(consumer.cxx, Some(CxxStandard::Cxx20));
    }

    /// No members imposes nothing.
    #[test]
    fn empty_member_set_has_no_consumer_standard() {
        let workspace = graph(vec![member(
            "a",
            vec![compiled_target("a", "cc", cxx(CxxStandard::Cxx20))],
        )]);
        let consumer =
            workspace.consumer_standards(&BTreeSet::new(), &[], &HashMap::new(), &BTreeSet::new());
        assert_eq!(consumer.c, None);
        assert_eq!(consumer.cxx, None);
    }
}
