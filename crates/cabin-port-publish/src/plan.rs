//! Port discovery and publication planning.
//!
//! Scans the committed `ports/` directory and orders the results so
//! every port is published after the ports it depends on.  Every
//! committed port is a provenance-bearing package manifest, published
//! verbatim.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use cabin_core::{DependencySource, PackageName, UpstreamProvenance};
use semver::Version;

/// Registry scope every port publishes under.
const REGISTRY_SCOPE: &str = "cabin-ports";

/// One committed port, ready to materialize and publish.
#[derive(Debug)]
pub struct PortConversion {
    /// `ports/<name>/<version>/` source directory.
    pub committed_dir: PathBuf,
    /// The declared provenance the shared materializer runs.
    pub upstream: Box<UpstreamProvenance>,
    /// Scoped registry name (`cabin-ports/<lowercase>`).
    pub scoped_name: PackageName,
    /// Version the conversion publishes: the upstream version,
    /// verbatim.  Packaging corrections republish the same version as
    /// a new registry revision (derived from the archive bytes), so
    /// no version-string axis exists here.
    pub published_version: Version,
    /// The manifest text that publishes: the committed file,
    /// verbatim.
    pub manifest: String,
    /// Inter-port dependencies this package publishes with — the
    /// publication-order edges, requirement included so ordering can
    /// wait for a *satisfying* version, not just any version of the
    /// name.
    pub dependencies: Vec<PortDependencyEdge>,
    /// Language standards a probe consumer must use to satisfy every
    /// probed target's declared interface requirement.  `None` keeps
    /// the probe's defaults.
    pub probe_standards: ProbeStandards,
    /// Published keys of the library-like targets the preflight probe
    /// links, referenced through explicit `package:target` selectors
    /// unless [`Self::sole_library_target`] allows the bare-package
    /// shorthand.
    pub library_like_target_keys: Vec<String>,
    /// Whether the published package declares exactly one
    /// library-like target *in total*.  Only then may the probe use
    /// the bare-package shorthand: Cabin resolves a bare `deps` entry
    /// against every library-like target the dependency's manifest
    /// declares - including one gated behind a non-default feature,
    /// which the probe leaves unlinked - and refuses the reference as
    /// ambiguous when there is more than one.
    pub sole_library_target: bool,
}

/// The `c-standard` / `cxx-standard` a generated probe consumer must
/// declare, and which probed targets each consumer links.  `None`
/// means "no declared interface requirement to satisfy", and that
/// probe keeps its default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeStandards {
    /// Lowest C standard satisfying `c_targets`' declared
    /// `interface-c-standard`.
    pub c: Option<cabin_core::CStandard>,
    /// Lowest C++ standard satisfying the remaining probed targets'
    /// declared `interface-cxx-standard`.
    pub cxx: Option<cabin_core::CxxStandard>,
    /// Probed targets that declare `interface-cxx-standard = "none"`:
    /// not consumable from C++ at all, so they are linked by a C
    /// consumer instead.  Every other probed target goes to the C++
    /// consumer, so a package mixing the two is probed from both.
    pub c_targets: Vec<String>,
}

/// One inter-port dependency edge.
#[derive(Debug, Clone)]
pub struct PortDependencyEdge {
    /// Scoped registry name of the dependency.
    pub scoped: PackageName,
    /// Version requirement the conversion carries for it.
    pub req: semver::VersionReq,
}

/// The scoped registry name for a port: `cabin-ports/<lowercase>`.
///
/// # Errors
/// Returns an error when the lowercased name does not satisfy the
/// canonical registry grammar (`[a-z0-9][a-z0-9_-]*`), which the
/// hosted registry and the publish gates both enforce.
fn scoped_package_name(port_name: &str) -> Result<PackageName> {
    let lower = port_name.to_lowercase();
    let canonical = lower
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && lower
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if !canonical {
        bail!(
            "port name `{port_name}` does not lowercase to a canonical registry package name \
             (`[a-z0-9][a-z0-9_-]*`)"
        );
    }
    PackageName::new(format!("{REGISTRY_SCOPE}/{lower}"))
        .map_err(|err| anyhow!("scoped name for port `{port_name}` is invalid: {err}"))
}

/// Scan `ports_dir` and return every committed port in publication
/// order (dependencies first; name-sorted within a rank, versions
/// ascending within a name).
///
/// # Errors
/// Returns an error when the directory cannot be read, any port fails
/// to load, published names collide, or the inter-port dependency
/// graph has a cycle.
pub fn load_conversions(ports_dir: &Path) -> Result<Vec<PortConversion>> {
    let packages = discover_ports(ports_dir)?;
    if packages.is_empty() {
        bail!("no ports found under {}", ports_dir.display());
    }

    let mut scoped_by_dir: BTreeMap<String, PackageName> = BTreeMap::new();
    let mut conversions = Vec::new();
    for package_dir in packages {
        let conversion = load_package(&package_dir)?;
        let dir_name = dir_name(package_dir.parent().unwrap_or(&package_dir))?;
        scoped_by_dir
            .entry(dir_name)
            .or_insert_with(|| conversion.scoped_name.clone());
        conversions.push(conversion);
    }

    ensure_distinct_scoped_names(&scoped_by_dir)?;
    ensure_requirements_satisfiable(&conversions)?;
    ensure_unique_identities(&conversions)?;
    order_by_dependencies(conversions)
}

/// Load one committed port directory: the committed manifest is the
/// published manifest, so nothing is rewritten.  The directory layout
/// is the identity, and a disagreement fails loudly rather than
/// publishing under a surprising name.
fn load_package(package_dir: &Path) -> Result<PortConversion> {
    let manifest_path = package_dir.join("cabin.toml");
    // The manifest is the package's identity and provenance, so it
    // must be the committed regular file - the same rule the
    // directory walk and the patch resolver already apply.  Following
    // a symlink here would let published metadata depend on
    // checkout-local state.
    let marker = fs::symlink_metadata(&manifest_path)
        .with_context(|| format!("inspecting {}", manifest_path.display()))?;
    if !marker.is_file() {
        bail!(
            "{} is not a regular file; a migrated port's manifest must be committed \
             directly, not through a symlink",
            manifest_path.display()
        );
    }
    let manifest = fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let parsed = cabin_manifest::parse_manifest_str(&manifest)
        .with_context(|| format!("parsing {}", manifest_path.display()))?;
    let package = parsed
        .package
        .ok_or_else(|| anyhow!("{} has no [package] table", manifest_path.display()))?;

    let version_dir = dir_name(package_dir)?;
    let name_dir = match package_dir.parent() {
        Some(parent) => dir_name(parent)?,
        None => String::new(),
    };
    if package.name.scope() != Some(REGISTRY_SCOPE) {
        bail!(
            "{} declares `{}`; port packages must publish under the `{}` scope",
            manifest_path.display(),
            package.name.as_str(),
            REGISTRY_SCOPE
        );
    }
    let expected = scoped_package_name(&name_dir)
        .with_context(|| format!("package directory `{name_dir}/`"))?;
    if package.name != expected {
        bail!(
            "{} declares `{}`, which disagrees with its `{name_dir}/` directory",
            manifest_path.display(),
            package.name.as_str()
        );
    }
    if package.version.to_string() != version_dir {
        bail!(
            "{} declares version {}, which disagrees with its `{version_dir}/` directory",
            manifest_path.display(),
            package.version
        );
    }
    let Some(upstream) = package.upstream.clone() else {
        bail!(
            "{} declares no [package.upstream] provenance; port packages are materialized \
             from their pinned upstream archives",
            manifest_path.display()
        );
    };

    let dependencies = registry_edges(&package, &manifest_path)?;

    // The preflight probe depends on the staged package the ordinary
    // way, so the package's default features are on and no others.
    // A target whose `required-features` that set satisfies is
    // therefore linkable and must be probed - dropping it would let
    // a target that does not even compile pass preflight.  One
    // gated behind a non-default feature is left out of the link
    // set (it still publishes); linking it would fail the very
    // build the probe exists to prove.
    let default_features = package
        .features
        .expand(&package.features.default.iter().cloned().collect());
    let library_like: Vec<&cabin_core::Target> = package
        .targets
        .iter()
        .filter(|target| target.kind.is_library_like())
        .collect();
    let library_like_target_keys: Vec<String> = library_like
        .iter()
        .filter(|target| {
            target
                .missing_required_features(&default_features)
                .is_empty()
        })
        .map(|target| target.name.as_str().to_owned())
        .collect();

    let probe_standards = probe_standards(&package, &library_like_target_keys).map_err(|why| {
        anyhow!(
            "{}: the probed targets' declared interface standards cannot be satisfied by one \
             consumer ({why}); the preflight probe builds one consumer per language for all of \
             them",
            manifest_path.display()
        )
    })?;

    Ok(PortConversion {
        committed_dir: package_dir.to_path_buf(),
        upstream: Box::new(upstream),
        probe_standards,
        scoped_name: package.name.clone(),
        published_version: package.version.clone(),
        manifest,
        dependencies,
        sole_library_target: library_like.len() == 1,
        library_like_target_keys,
    })
}

/// The publication-order edges a published manifest declares, and the
/// validation every port package's dependency table must pass.
fn registry_edges(
    package: &cabin_core::Package,
    manifest_path: &Path,
) -> Result<Vec<PortDependencyEdge>> {
    // System dependencies live in their own field, so the loop below
    // would never see them - and a port package resolving through
    // host pkg-config would make its published build host-dependent.
    if let Some(system) = package.system_dependencies.first() {
        bail!(
            "{} declares the system dependency `{}`; port packages carry registry \
             dependencies only",
            manifest_path.display(),
            system.name.as_str()
        );
    }

    let activated_by_default = default_activated_optional_deps(package);

    let mut dependencies = Vec::new();
    for dep in &package.dependencies {
        if dep.condition.is_some() {
            bail!(
                "{} declares `{}` under a cfg-conditional table; publication ordering reads \
                 the declared edges without evaluating conditions, so port packages carry \
                 unconditional dependencies only",
                manifest_path.display(),
                dep.name.as_str()
            );
        }
        match &dep.source {
            DependencySource::Version(req) => {
                if dep.name.scope() != Some(REGISTRY_SCOPE) {
                    bail!(
                        "{} depends on `{}`; port packages may only depend on other `{}/*` \
                         packages",
                        manifest_path.display(),
                        dep.name.as_str(),
                        REGISTRY_SCOPE
                    );
                }
                // Every dependency is validated above, whatever its
                // shape, but only one that is actually resolved
                // becomes a publication-order edge.  A consumer's
                // resolution of a transitive registry package drops
                // its dev dependencies (`docs/dependency-kinds.md`)
                // and the optional ones its default features do not
                // activate, and the preflight probe depends on the
                // package the ordinary way - so those are inactive
                // in both, and two packages referencing each other
                // only that way are valid, not a cycle.  An optional
                // dependency the default closure DOES activate is
                // resolved and must be published first.
                // (Conditional edges never get here: ordering does
                // not evaluate conditions, so they are refused
                // outright above.)
                //
                // Ceiling, deliberate: activation is read from THIS
                // package's own default closure.  A `features = [..]`
                // request on an edge can also activate optional
                // dependencies inside the *dependency*, and ordering
                // does not follow that - resolving it means running
                // `cabin-feature`'s cross-package activation, which
                // would duplicate that engine inside scaffolding the
                // migration deletes.  No committed port requests
                // features on a dependency (the tree has exactly one
                // inter-port edge, libpng -> zlib); if one ever does,
                // preflight fails loudly on the missing package
                // rather than publishing anything wrong.
                if dep.kind == cabin_core::DependencyKind::Normal
                    && (!dep.optional || activated_by_default.contains(dep.name.as_str()))
                {
                    dependencies.push(PortDependencyEdge {
                        scoped: dep.name.clone(),
                        req: req.clone(),
                    });
                }
            }
            _ => bail!(
                "{} depends on `{}` through a non-registry source; port packages carry \
                 registry dependencies only",
                manifest_path.display(),
                dep.name.as_str()
            ),
        }
    }
    Ok(dependencies)
}

/// Names of the optional dependencies a package's DEFAULT feature
/// closure activates, as written in the manifest.  Both activation
/// forms count, matching `cabin-feature`'s resolver: a `dep:<name>`
/// entry, and a `<dep>/<feature>` entry naming an optional
/// dependency.  Unparsable entries are ignored here - manifest
/// validation owns that diagnosis.
fn default_activated_optional_deps(package: &cabin_core::Package) -> BTreeSet<String> {
    let roots: BTreeSet<String> = package.features.default.iter().cloned().collect();
    let mut entries: Vec<&String> = package.features.default.iter().collect();
    for feature in package.features.expand(&roots) {
        if let Some(values) = package.features.features.get(&feature) {
            entries.extend(values.iter());
        }
    }

    let optional: BTreeSet<&str> = package
        .dependencies
        .iter()
        .filter(|dep| dep.optional)
        .map(|dep| dep.name.as_str())
        .collect();
    let mut activated = BTreeSet::new();
    for entry in entries {
        match cabin_core::FeatureEntry::parse(entry) {
            Ok(cabin_core::FeatureEntry::OptionalDep(name)) => {
                activated.insert(name);
            }
            Ok(cabin_core::FeatureEntry::DepFeature { dep, .. })
                if optional.contains(dep.as_str()) =>
            {
                activated.insert(dep);
            }
            _ => {}
        }
    }
    activated
}

/// The lowest standard a probe consumer can declare and still
/// satisfy every target it links, per language.  The requirements
/// are joined through `cabin_core::Requirement` - the
/// standard-compatibility engine's own intersection - rather than
/// compared here, so the publisher cannot drift from the rule the
/// build enforces.  `None` means nothing was declared and that probe
/// keeps its default; a requirement no consumer level can satisfy
/// (`Forbidden`) is refused by the caller.
fn probe_standards(
    package: &cabin_core::Package,
    probed: &[String],
) -> Result<ProbeStandards, &'static str> {
    let targets: Vec<&cabin_core::Target> = package
        .targets
        .iter()
        .filter(|t| probed.iter().any(|key| key == t.name.as_str()))
        .collect();

    // Effective-standard precedence: a target-level declaration wins,
    // and the package-level one applies where the target is silent.
    let declared = |target: &cabin_core::Target| {
        (
            target
                .language
                .interface_c_standard
                .or(package.language.interface_c_standard)
                .and_then(cabin_core::StandardDeclaration::value),
            target
                .language
                .interface_cxx_standard
                .or(package.language.interface_cxx_standard)
                .and_then(cabin_core::StandardDeclaration::value),
        )
    };

    // A target declaring `interface-cxx-standard = "none"` is not
    // consumable from C++ at all, so it is linked by a C consumer;
    // every other target is linked by the C++ one.  Splitting them
    // keeps a package that mixes both shapes probeable - one
    // consumer of the whole set could satisfy neither side.
    let (c_targets, cxx_targets): (Vec<_>, Vec<_>) = targets
        .into_iter()
        .partition(|t| matches!(declared(t).1, Some(cabin_core::InterfaceRequirement::None)));

    // Only the language a given probe actually compiles is joined.
    // Each probe is a single `main.cc` / `main.c` consumer, so Cabin
    // only ever checks that language's interface - a C++ library
    // declaring `interface-c-standard = "none"` is valid and must not
    // be refused because of a C requirement no probe exercises.
    //
    // Ceiling, deliberate: these are the DECLARED interfaces only.
    // Cabin also composes requirements across public target edges
    // (and infers them for header-only targets); replicating that
    // here would duplicate the standard-compatibility engine inside
    // scaffolding the migration deletes.  A composed floor above the
    // probe's therefore surfaces as Cabin's own compatibility error
    // at preflight rather than as a diagnosis here.
    Ok(ProbeStandards {
        c: join_interface(c_targets.iter().map(|t| declared(t).0))?,
        cxx: join_interface(cxx_targets.iter().map(|t| declared(t).1))?,
        c_targets: c_targets.iter().map(|t| t.name.to_string()).collect(),
    })
}

/// Join declared interface requirements and return the lowest
/// satisfying consumer level, or `Err` when no level satisfies them
/// together.
fn join_interface<S: Copy + Ord + cabin_core::StandardLevel>(
    declared: impl Iterator<Item = Option<cabin_core::InterfaceRequirement<S>>>,
) -> Result<Option<S>, &'static str> {
    let mut any = false;
    let joined = cabin_core::Requirement::join_all(declared.flatten().map(|req| {
        any = true;
        match req {
            cabin_core::InterfaceRequirement::None => cabin_core::Requirement::Forbidden,
            cabin_core::InterfaceRequirement::Requirement(r) => {
                cabin_core::Requirement::from_declared(r)
            }
        }
    }));
    if !any {
        return Ok(None);
    }
    match joined {
        cabin_core::Requirement::Forbidden => Err("no consumer standard satisfies them together"),
        cabin_core::Requirement::Unconstrained => Ok(None),
        other => Ok(other.lower_bound()),
    }
}

fn dir_name(dir: &Path) -> Result<String> {
    dir.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("{} has no UTF-8 directory name", dir.display()))
}

/// Two distinct port directory names must not fold onto one scoped
/// name (`cJSON/` next to `cjson/` would silently merge under
/// `cabin-ports/cjson`).
fn ensure_distinct_scoped_names(scoped_by_dir: &BTreeMap<String, PackageName>) -> Result<()> {
    let mut scoped_owners: BTreeMap<&str, &str> = BTreeMap::new();
    for (port_name, scoped) in scoped_by_dir {
        if let Some(previous) = scoped_owners.insert(scoped.as_str(), port_name) {
            bail!(
                "ports `{previous}` and `{port_name}` both publish as `{}`; published names \
                 must stay distinct",
                scoped.as_str()
            );
        }
    }
    Ok(())
}

/// Every declared requirement must be satisfiable by a version
/// published in the same run, or the preflight (and every consumer)
/// would fail resolution later with a worse error.  An unsatisfiable
/// edge must fail here with its real diagnosis, not surface as a
/// phantom "dependency cycle" when ordering never drains the node.
fn ensure_requirements_satisfiable(conversions: &[PortConversion]) -> Result<()> {
    let mut published: BTreeMap<&str, Vec<&Version>> = BTreeMap::new();
    for conversion in conversions {
        published
            .entry(conversion.scoped_name.base_name())
            .or_default()
            .push(&conversion.published_version);
    }
    for conversion in conversions {
        for dep in &conversion.dependencies {
            let satisfied = published
                .get(dep.scoped.base_name())
                .is_some_and(|versions| versions.iter().any(|v| dep.req.matches(v)));
            if !satisfied {
                bail!(
                    "{} depends on `{}` with requirement `{}`, which no committed port \
                     version satisfies",
                    conversion.committed_dir.display(),
                    dep.scoped.as_str(),
                    dep.req
                );
            }
        }
    }
    Ok(())
}

/// Every `ports/<name>/<version>/` directory holding a committed
/// `cabin.toml` - the one shape a port is committed in.
fn discover_ports(ports_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut packages = Vec::new();
    let names = read_sorted_dirs(ports_dir)?;
    for name_dir in names {
        for version_dir in read_sorted_dirs(&name_dir)? {
            if version_dir.join("cabin.toml").is_file() {
                packages.push(version_dir);
            }
        }
    }
    Ok(packages)
}

fn read_sorted_dirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry
            .with_context(|| format!("reading {}", dir.display()))?
            .path();
        // Committed regular directories only: a symlinked name or
        // version directory would source manifests and patch bytes
        // from outside the committed tree.
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("inspecting {}", path.display()))?;
        if metadata.file_type().is_symlink() {
            if fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
                bail!(
                    "{} is a symlinked directory; ports must be committed as regular \
                     directories",
                    path.display()
                );
            }
            continue;
        }
        if metadata.is_dir() {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn ensure_unique_identities(conversions: &[PortConversion]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for conversion in conversions {
        let identity = (
            conversion.scoped_name.clone(),
            conversion.published_version.to_string(),
        );
        if !seen.insert(identity) {
            bail!(
                "two ports publish as `{} {}`; published identities must be unique",
                conversion.scoped_name.as_str(),
                conversion.published_version
            );
        }
    }
    Ok(())
}

/// Kahn's algorithm over the scoped-name dependency edges.  Ready
/// nodes are drained in name order (versions ascending within a
/// name), so the publication order is deterministic.
fn order_by_dependencies(conversions: Vec<PortConversion>) -> Result<Vec<PortConversion>> {
    let mut remaining: BTreeMap<(String, Version), PortConversion> = conversions
        .into_iter()
        .map(|c| {
            (
                (
                    c.scoped_name.as_str().to_owned(),
                    c.published_version.clone(),
                ),
                c,
            )
        })
        .collect();
    let mut ordered = Vec::with_capacity(remaining.len());
    let mut published: BTreeMap<String, Vec<Version>> = BTreeMap::new();
    while !remaining.is_empty() {
        // Readiness is per requirement, not per name: a dependent
        // waits until a *satisfying* version of each dependency has
        // published (a name may convert several versions, and a
        // dependent may require one that publishes in a later rank).
        let ready: Vec<(String, Version)> = remaining
            .iter()
            .filter(|(_, c)| {
                c.dependencies.iter().all(|dep| {
                    published
                        .get(dep.scoped.as_str())
                        .is_some_and(|versions| versions.iter().any(|v| dep.req.matches(v)))
                })
            })
            .map(|(key, _)| key.clone())
            .collect();
        if ready.is_empty() {
            let stuck: Vec<&str> = remaining.keys().map(|(name, _)| name.as_str()).collect();
            bail!("inter-port dependency cycle among: {}", stuck.join(", "));
        }
        for key in ready {
            let conversion = remaining.remove(&key).expect("key came from the map");
            published
                .entry(key.0.clone())
                .or_default()
                .push(conversion.published_version.clone());
            ordered.push(conversion);
        }
    }
    Ok(ordered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    #[test]
    fn scoped_names_lowercase_under_the_ports_scope() {
        assert_eq!(
            scoped_package_name("cJSON").unwrap().as_str(),
            "cabin-ports/cjson"
        );
        assert_eq!(
            scoped_package_name("nlohmann_json").unwrap().as_str(),
            "cabin-ports/nlohmann_json"
        );
        // `.` is valid in a local package name but not in the
        // registry grammar, so the publisher refuses it up front.
        assert!(scoped_package_name("foo.bar").is_err());
    }

    /// A stray `packaging-revision` sidecar (the removed mechanism)
    /// is just an unknown file in the port directory: discovery
    /// ignores it and the published version stays the upstream one.
    #[test]
    fn stray_packaging_revision_sidecars_are_ignored() {
        let dir = TempDir::new().unwrap();
        let ports = dir.child("ports");
        write_package(
            ports.path(),
            "zlib",
            "1.3.1",
            &package_manifest("cabin-ports/zlib", "1.3.1", ""),
        );
        ports
            .child("zlib/1.3.1")
            .child("packaging-revision")
            .write_str("2\n")
            .unwrap();
        let conversions = load_conversions(ports.path()).unwrap();
        assert_eq!(conversions[0].published_version.to_string(), "1.3.1");
    }

    /// The committed ports are the tool's real inputs; loading them
    /// end-to-end (no archives needed — conversion is pure) pins the
    /// requirements that name concrete ports.
    ///
    /// Also pins the SHAPE of the committed tree: every port is a
    /// provenance-bearing package.  Discovery now ignores a version
    /// directory that holds only a `port.toml`, so a reintroduced
    /// recipe would publish as nothing at all rather than fail - this
    /// assertion is the check that catches it.
    #[test]
    fn committed_ports_all_load() {
        let ports_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("cabin-port")
            .join("ports");
        let conversions = load_conversions(&ports_dir).unwrap();
        assert!(
            conversions.len() >= 17,
            "expected every committed port, got {}",
            conversions.len()
        );
        let mut pending = vec![ports_dir.clone()];
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    assert!(
                        path.file_name() != Some("port.toml".as_ref()),
                        "{} is a recipe; every committed port is a provenance-bearing package",
                        path.display()
                    );
                }
            }
        }

        let by_name: BTreeMap<&str, &PortConversion> = conversions
            .iter()
            .map(|c| (c.scoped_name.as_str(), c))
            .collect();

        // zlib publishes a sole library target named `z`.
        let zlib = by_name["cabin-ports/zlib"];
        assert!(zlib.manifest.contains("[target.z]"), "{}", zlib.manifest);
        assert!(
            !zlib.manifest.contains("[target.zlib]"),
            "{}",
            zlib.manifest
        );

        // The mixed-case identities normalize coherently: package
        // name and target key agree on the lowercase spelling.
        let cjson = by_name["cabin-ports/cjson"];
        assert!(
            cjson.manifest.contains("[target.cjson]"),
            "{}",
            cjson.manifest
        );
        let cli11 = by_name["cabin-ports/cli11"];
        assert!(
            cli11.manifest.contains("[target.cli11]"),
            "{}",
            cli11.manifest
        );

        // libpng orders after zlib, which its manifest names through
        // the bare scoped shorthand.
        let zlib_pos = conversions
            .iter()
            .position(|c| c.scoped_name.as_str() == "cabin-ports/zlib")
            .unwrap();
        let libpng_pos = conversions
            .iter()
            .position(|c| c.scoped_name.as_str() == "cabin-ports/libpng")
            .unwrap();
        assert!(zlib_pos < libpng_pos);
        let libpng = by_name["cabin-ports/libpng"];
        assert!(
            libpng.manifest.contains("\"cabin-ports/zlib\" = \"^1.3\""),
            "{}",
            libpng.manifest
        );
        assert!(
            libpng.manifest.contains("deps = [\"cabin-ports/zlib\"]"),
            "{}",
            libpng.manifest
        );
        assert!(
            libpng.manifest.contains("[target.png]"),
            "{}",
            libpng.manifest
        );
        assert!(
            libpng.manifest.contains("[[package.upstream.copy]]"),
            "{}",
            libpng.manifest
        );

        // Provenance is declared everywhere, and no published
        // manifest keeps an upper-case identity.
        for conversion in &conversions {
            assert!(
                conversion.manifest.contains("[package.upstream]"),
                "{} lacks provenance",
                conversion.scoped_name.as_str()
            );
            assert_eq!(
                conversion.scoped_name.as_str(),
                conversion.scoped_name.as_str().to_lowercase(),
                "scoped names are lowercase"
            );
        }

        // Deterministic: a second scan loads byte-identically.
        let again = load_conversions(&ports_dir).unwrap();
        for (a, b) in conversions.iter().zip(&again) {
            assert_eq!(a.scoped_name, b.scoped_name);
            assert_eq!(a.manifest, b.manifest);
        }
    }

    /// Scaffolding for the package-shape validation tests: one
    /// committed package directory under a temp ports tree, loaded
    /// through the real `load_conversions` entry.
    fn write_package(dir: &std::path::Path, name: &str, version: &str, manifest: &str) {
        let package_dir = dir.join(name).join(version);
        std::fs::create_dir_all(&package_dir).unwrap();
        std::fs::write(package_dir.join("cabin.toml"), manifest).unwrap();
    }

    fn package_manifest(scoped: &str, version: &str, extra: &str) -> String {
        format!(
            "[package]\nname = \"{scoped}\"\nversion = \"{version}\"\n\n\
             [package.upstream]\nurl = \"https://ports.invalid/a.tar.gz\"\nsha256 = \"{}\"\n\
             format = \"tar.gz\"\n\n{extra}\
             [target.t]\ntype = \"library\"\nsources = [\"a.c\"]\ninclude-dirs = [\".\"]\n\
             c-standard = \"c11\"\n",
            "a".repeat(64)
        )
    }

    #[test]
    fn a_package_whose_name_disagrees_with_its_directory_is_refused() {
        let dir = assert_fs::TempDir::new().unwrap();
        write_package(
            dir.path(),
            "fmt",
            "12.2.0",
            &package_manifest("cabin-ports/notfmt", "12.2.0", ""),
        );
        let err = load_conversions(dir.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("disagrees with its `fmt/` directory"),
            "{err:#}"
        );
    }

    #[test]
    fn a_package_whose_version_disagrees_with_its_directory_is_refused() {
        let dir = assert_fs::TempDir::new().unwrap();
        write_package(
            dir.path(),
            "fmt",
            "12.2.0",
            &package_manifest("cabin-ports/fmt", "12.2.1", ""),
        );
        let err = load_conversions(dir.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("disagrees with its `12.2.0/` directory"),
            "{err:#}"
        );
    }

    #[test]
    fn a_package_outside_the_ports_scope_is_refused() {
        let dir = assert_fs::TempDir::new().unwrap();
        write_package(
            dir.path(),
            "fmt",
            "12.2.0",
            &package_manifest("otherscope/fmt", "12.2.0", ""),
        );
        let err = load_conversions(dir.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("must publish under the `cabin-ports` scope"),
            "{err:#}"
        );
    }

    #[test]
    fn a_package_without_upstream_provenance_is_refused() {
        let dir = assert_fs::TempDir::new().unwrap();
        let manifest = "[package]\nname = \"cabin-ports/fmt\"\nversion = \"12.2.0\"\n\n\
                        [target.t]\ntype = \"library\"\nsources = [\"a.c\"]\ninclude-dirs = \
                        [\".\"]\nc-standard = \"c11\"\n"
            .to_owned();
        write_package(dir.path(), "fmt", "12.2.0", &manifest);
        let err = load_conversions(dir.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("declares no [package.upstream]"),
            "{err:#}"
        );
    }

    /// Two migrated versions may expose different library targets:
    /// each publishes verbatim and is probed on its own targets, and
    /// nothing consumes a port through a port-wide summary, so there
    /// is no port-wide target set to keep consistent.
    #[test]
    fn migrated_versions_may_expose_different_targets() {
        let dir = assert_fs::TempDir::new().unwrap();
        write_package(
            dir.path(),
            "fmt",
            "12.2.0",
            &package_manifest("cabin-ports/fmt", "12.2.0", ""),
        );
        let manifest =
            package_manifest("cabin-ports/fmt", "12.3.0", "").replace("[target.t]", "[target.u]");
        write_package(dir.path(), "fmt", "12.3.0", &manifest);
        let conversions = load_conversions(dir.path()).unwrap();
        assert_eq!(conversions.len(), 2);
        let mut keys: Vec<&str> = conversions
            .iter()
            .flat_map(|c| c.library_like_target_keys.iter().map(String::as_str))
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["t", "u"]);
    }

    #[test]
    fn orders_dependencies_before_dependents() {
        let dir = assert_fs::TempDir::new().unwrap();
        // `apng` sorts before `zlib` but depends on it, so the order
        // must invert the name sort.
        write_package(
            dir.path(),
            "apng",
            "1.0.0",
            &package_manifest(
                "cabin-ports/apng",
                "1.0.0",
                "[dependencies]\n\"cabin-ports/zlib\" = \"^1.3\"\n\n",
            ),
        );
        write_package(
            dir.path(),
            "zlib",
            "1.3.1",
            &package_manifest("cabin-ports/zlib", "1.3.1", ""),
        );
        let conversions = load_conversions(dir.path()).unwrap();
        let names: Vec<&str> = conversions.iter().map(|c| c.scoped_name.as_str()).collect();
        assert_eq!(names, ["cabin-ports/zlib", "cabin-ports/apng"]);
        assert_eq!(
            conversions[1]
                .dependencies
                .iter()
                .map(|dep| (dep.scoped.as_str(), dep.req.to_string()))
                .collect::<Vec<_>>(),
            [("cabin-ports/zlib", "^1.3".to_owned())]
        );
    }

    #[test]
    fn detects_dependency_cycles() {
        let dir = assert_fs::TempDir::new().unwrap();
        write_package(
            dir.path(),
            "aport",
            "1.0.0",
            &package_manifest(
                "cabin-ports/aport",
                "1.0.0",
                "[dependencies]\n\"cabin-ports/bport\" = \"^1\"\n\n",
            ),
        );
        write_package(
            dir.path(),
            "bport",
            "1.0.0",
            &package_manifest(
                "cabin-ports/bport",
                "1.0.0",
                "[dependencies]\n\"cabin-ports/aport\" = \"^1\"\n\n",
            ),
        );
        let err = load_conversions(dir.path()).unwrap_err();
        assert!(format!("{err:#}").contains("cycle"), "{err:#}");
    }

    /// Ordering waits for a version that *satisfies* the requirement,
    /// not merely for the dependency's name: `apng` needs zlib `^2`,
    /// so publishing zlib 1.3.1 first does not unblock it.
    #[test]
    fn dependents_wait_for_a_satisfying_version_not_just_the_name() {
        let dir = assert_fs::TempDir::new().unwrap();
        write_package(
            dir.path(),
            "apng",
            "1.0.0",
            &package_manifest(
                "cabin-ports/apng",
                "1.0.0",
                "[dependencies]\n\"cabin-ports/zlib\" = \"^2\"\n\n",
            ),
        );
        write_package(
            dir.path(),
            "zlib",
            "1.3.1",
            &package_manifest("cabin-ports/zlib", "1.3.1", ""),
        );
        write_package(
            dir.path(),
            "zlib",
            "2.0.0",
            &package_manifest(
                "cabin-ports/zlib",
                "2.0.0",
                "[dependencies]\n\"cabin-ports/xxhash\" = \"^0.8\"\n\n",
            ),
        );
        write_package(
            dir.path(),
            "xxhash",
            "0.8.3",
            &package_manifest("cabin-ports/xxhash", "0.8.3", ""),
        );
        let conversions = load_conversions(dir.path()).unwrap();
        let order: Vec<String> = conversions
            .iter()
            .map(|c| format!("{} {}", c.scoped_name.as_str(), c.published_version))
            .collect();
        let position = |needle: &str| {
            order
                .iter()
                .position(|entry| entry == needle)
                .unwrap_or_else(|| panic!("{needle} missing from {order:?}"))
        };
        assert!(position("cabin-ports/zlib 2.0.0") < position("cabin-ports/apng 1.0.0"));
        assert!(position("cabin-ports/xxhash 0.8.3") < position("cabin-ports/zlib 2.0.0"));
        assert!(position("cabin-ports/zlib 1.3.1") < position("cabin-ports/zlib 2.0.0"));
    }

    /// Two migrated packages may dev-depend on each other: a
    /// consumer's resolution drops a transitive package's dev
    /// dependencies and the preflight probe is an ordinary build, so
    /// neither edge is active and neither orders publication.
    /// An unactivated optional dependency is dropped by a consumer's
    /// resolution and by the preflight probe alike, so it must not
    /// order publication either - two packages optionally depending
    /// on each other are valid, not a cycle.
    #[test]
    fn optional_dependencies_do_not_order_publication() {
        let dir = assert_fs::TempDir::new().unwrap();
        write_package(
            dir.path(),
            "alpha",
            "1.0.0",
            &package_manifest(
                "cabin-ports/alpha",
                "1.0.0",
                "[dependencies]\n\"cabin-ports/beta\" = { version = \"1.0.0\", optional = \
                 true }\n\n",
            ),
        );
        write_package(
            dir.path(),
            "beta",
            "1.0.0",
            &package_manifest(
                "cabin-ports/beta",
                "1.0.0",
                "[dependencies]\n\"cabin-ports/alpha\" = { version = \"1.0.0\", optional = \
                 true }\n\n",
            ),
        );
        let conversions = load_conversions(dir.path()).unwrap();
        assert_eq!(conversions.len(), 2);
        assert!(
            conversions.iter().all(|c| c.dependencies.is_empty()),
            "optional edges must not order publication"
        );
    }

    /// An optional dependency the DEFAULT feature closure activates
    /// is resolved by the preflight probe, so it must publish first.
    /// Both activation forms count (`dep:<name>` and an optional
    /// `<dep>/<feature>`), and the closure is transitive.
    #[test]
    fn default_activated_optional_dependencies_order_publication() {
        for extra in [
            "[features]\ndefault = [\"ssl\"]\nssl = [\"dep:cabin-ports/beta\"]\n\n\
             [dependencies]\n\"cabin-ports/beta\" = { version = \"1.0.0\", optional = true }\n\n",
            // Transitive: default -> outer -> ssl -> dep:beta.
            "[features]\ndefault = [\"outer\"]\nouter = [\"ssl\"]\nssl = \
             [\"dep:cabin-ports/beta\"]\n\n[dependencies]\n\"cabin-ports/beta\" = \
             { version = \"1.0.0\", optional = true }\n\n",
            // The `<dep>/<feature>` form activates an optional dep too.
            "[features]\ndefault = [\"ssl\"]\nssl = [\"cabin-ports/beta/fast\"]\n\n\
             [dependencies]\n\"cabin-ports/beta\" = { version = \"1.0.0\", optional = true }\n\n",
        ] {
            let dir = assert_fs::TempDir::new().unwrap();
            write_package(
                dir.path(),
                "alpha",
                "1.0.0",
                &package_manifest("cabin-ports/alpha", "1.0.0", extra),
            );
            write_package(
                dir.path(),
                "beta",
                "1.0.0",
                &package_manifest("cabin-ports/beta", "1.0.0", ""),
            );
            let conversions = load_conversions(dir.path()).unwrap();
            // beta publishes before alpha, and the edge exists.
            let names: Vec<&str> = conversions.iter().map(|c| c.scoped_name.as_str()).collect();
            assert_eq!(names, ["cabin-ports/beta", "cabin-ports/alpha"], "{extra}");
            let alpha = conversions.last().unwrap();
            assert_eq!(alpha.dependencies.len(), 1, "{extra}");
        }
    }

    #[test]
    fn dev_dependencies_do_not_order_publication() {
        let dir = assert_fs::TempDir::new().unwrap();
        write_package(
            dir.path(),
            "alpha",
            "1.0.0",
            &package_manifest(
                "cabin-ports/alpha",
                "1.0.0",
                "[dev-dependencies]\n\"cabin-ports/beta\" = \"1.0.0\"\n\n",
            ),
        );
        write_package(
            dir.path(),
            "beta",
            "1.0.0",
            &package_manifest(
                "cabin-ports/beta",
                "1.0.0",
                "[dev-dependencies]\n\"cabin-ports/alpha\" = \"1.0.0\"\n\n",
            ),
        );
        let conversions = load_conversions(dir.path()).unwrap();
        assert_eq!(conversions.len(), 2);
        assert!(
            conversions.iter().all(|c| c.dependencies.is_empty()),
            "dev edges must not order publication"
        );
    }

    /// Publication ordering reads declared edges without evaluating
    /// cfg conditions, so a conditional dependency is refused rather
    /// than recorded as an unconditional edge (two packages with
    /// mutually exclusive platform edges would otherwise false-cycle).
    #[test]
    fn a_package_with_a_conditional_dependency_is_refused() {
        let dir = assert_fs::TempDir::new().unwrap();
        write_package(
            dir.path(),
            "alpha",
            "1.0.0",
            &package_manifest(
                "cabin-ports/alpha",
                "1.0.0",
                "[target.'cfg(os = \"linux\")'.dependencies]\n\
                 \"cabin-ports/beta\" = \"1.0.0\"\n\n",
            ),
        );
        let err = load_conversions(dir.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("cfg-conditional table"),
            "{err:#}"
        );
    }

    /// A library target whose `required-features` the package's
    /// DEFAULT features satisfy is linkable by the probe, so it must
    /// stay in the link set - otherwise a target that does not
    /// compile could pass preflight and publish.
    #[test]
    fn default_enabled_feature_gated_targets_stay_in_the_probe_link_set() {
        let dir = assert_fs::TempDir::new().unwrap();
        let manifest = package_manifest(
            "cabin-ports/fmt",
            "12.2.0",
            "[features]\ndefault = [\"ssl\"]\nssl = []\n\n",
        )
        .replace(
            "c-standard = \"c11\"\n",
            "c-standard = \"c11\"\nrequired-features = [\"ssl\"]\n",
        );
        write_package(dir.path(), "fmt", "12.2.0", &manifest);
        let conversions = load_conversions(dir.path()).unwrap();
        assert_eq!(conversions[0].library_like_target_keys, ["t"]);
    }

    /// A library target gated behind a NON-default feature stays out
    /// of the probe's link set: the probe enables no extra features,
    /// so linking it would fail the build it exists to prove.
    #[test]
    fn feature_gated_targets_stay_out_of_the_probe_link_set() {
        let dir = assert_fs::TempDir::new().unwrap();
        let manifest = package_manifest("cabin-ports/fmt", "12.2.0", "[features]\nextra = []\n\n")
            .replace(
                "c-standard = \"c11\"\n",
                "c-standard = \"c11\"\nrequired-features = [\"extra\"]\n",
            );
        write_package(dir.path(), "fmt", "12.2.0", &manifest);
        let conversions = load_conversions(dir.path()).unwrap();
        assert_eq!(conversions.len(), 1);
        assert!(
            conversions[0].library_like_target_keys.is_empty(),
            "{:?}",
            conversions[0].library_like_target_keys
        );
    }

    /// A gated sibling still publishes, so it still counts against
    /// the bare-package shorthand: Cabin resolves a bare `deps` entry
    /// against the whole declared target set, not the probe's link
    /// set.
    #[test]
    fn a_gated_sibling_target_forbids_the_bare_shorthand() {
        let dir = assert_fs::TempDir::new().unwrap();
        let manifest = format!(
            "{}\n[target.gated]\ntype = \"library\"\nsources = [\"b.c\"]\n\
             include-dirs = [\".\"]\nc-standard = \"c11\"\nrequired-features = [\"extra\"]\n",
            package_manifest("cabin-ports/fmt", "12.2.0", "[features]\nextra = []\n\n")
        );
        write_package(dir.path(), "fmt", "12.2.0", &manifest);
        let conversions = load_conversions(dir.path()).unwrap();
        assert_eq!(conversions[0].library_like_target_keys, ["t"]);
        assert!(!conversions[0].sole_library_target);
    }

    /// The probe consumes every probed target at once, so its
    /// standards must satisfy their joined declared interface
    /// requirements - a package whose interface floor is above the
    /// probe's default must raise it, or preflight would reject the
    /// probe edge and the package could never publish.
    #[test]
    fn probe_standards_rise_to_the_declared_interface_floor() {
        let dir = assert_fs::TempDir::new().unwrap();
        let manifest = package_manifest("cabin-ports/fmt", "12.2.0", "").replace(
            "c-standard = \"c11\"\n",
            "c-standard = \"c11\"\ninterface-cxx-standard = \"c++20\"\n",
        );
        write_package(dir.path(), "fmt", "12.2.0", &manifest);
        let conversions = load_conversions(dir.path()).unwrap();
        assert_eq!(
            conversions[0].probe_standards.cxx,
            Some(cabin_core::CxxStandard::Cxx20)
        );
    }

    /// A package-level interface declaration applies where the
    /// target is silent, matching Cabin's effective-standard
    /// precedence - reading only the target would leave the probe on
    /// its default and reject a valid package.
    #[test]
    fn package_level_interface_standards_reach_the_probe() {
        let dir = assert_fs::TempDir::new().unwrap();
        let manifest = package_manifest("cabin-ports/fmt", "12.2.0", "").replace(
            "version = \"12.2.0\"\n",
            "version = \"12.2.0\"\ninterface-cxx-standard = \"c++20\"\n",
        );
        write_package(dir.path(), "fmt", "12.2.0", &manifest);
        let conversions = load_conversions(dir.path()).unwrap();
        assert_eq!(
            conversions[0].probe_standards.cxx,
            Some(cabin_core::CxxStandard::Cxx20)
        );
    }

    /// A target explicitly not consumable from C++ is probed by a C
    /// consumer rather than refused: the port is valid, it just has
    /// no C++ interface.
    #[test]
    fn c_only_interfaces_are_probed_from_c() {
        let dir = assert_fs::TempDir::new().unwrap();
        let manifest = package_manifest("cabin-ports/zlib", "1.3.1", "").replace(
            "c-standard = \"c11\"\n",
            "c-standard = \"c11\"\ninterface-cxx-standard = \"none\"\n\
             interface-c-standard = \"c99\"\n",
        );
        write_package(dir.path(), "zlib", "1.3.1", &manifest);
        let conversions = load_conversions(dir.path()).unwrap();
        assert_eq!(conversions[0].probe_standards.c_targets, ["t"]);
        assert_eq!(
            conversions[0].probe_standards.c,
            Some(cabin_core::CStandard::C99)
        );
        assert_eq!(conversions[0].probe_standards.cxx, None);
    }

    /// A package mixing a C-only library with a C++-consumable one is
    /// probed from both languages instead of being refused: no single
    /// consumer could link the two, but each half has one.
    #[test]
    fn mixed_language_targets_are_split_across_probes() {
        let dir = assert_fs::TempDir::new().unwrap();
        let manifest = format!(
            "{}\n[target.u]\ntype = \"library\"\nsources = [\"b.cc\"]\n\
             include-dirs = [\".\"]\ncxx-standard = \"c++17\"\n\
             interface-cxx-standard = \"c++20\"\n",
            package_manifest("cabin-ports/mixed", "1.0.0", "").replace(
                "c-standard = \"c11\"\n",
                "c-standard = \"c11\"\ninterface-cxx-standard = \"none\"\n\
                 interface-c-standard = \"c99\"\n",
            )
        );
        write_package(dir.path(), "mixed", "1.0.0", &manifest);
        let conversions = load_conversions(dir.path()).unwrap();
        assert_eq!(conversions[0].probe_standards.c_targets, ["t"]);
        assert_eq!(
            conversions[0].probe_standards.c,
            Some(cabin_core::CStandard::C99)
        );
        assert_eq!(
            conversions[0].probe_standards.cxx,
            Some(cabin_core::CxxStandard::Cxx20)
        );
    }

    /// A C++ library may declare `interface-c-standard = "none"`:
    /// the probe compiles C++, so Cabin never checks its C level and
    /// the package must not be refused for a C requirement no probe
    /// exercises.
    #[test]
    fn a_cxx_library_forbidding_c_consumption_is_accepted() {
        let dir = assert_fs::TempDir::new().unwrap();
        let manifest = package_manifest("cabin-ports/fmt", "12.2.0", "").replace(
            "c-standard = \"c11\"\n",
            "c-standard = \"c11\"\ninterface-c-standard = \"none\"\n\
             interface-cxx-standard = \"c++14\"\n",
        );
        write_package(dir.path(), "fmt", "12.2.0", &manifest);
        let conversions = load_conversions(dir.path()).unwrap();
        assert!(conversions[0].probe_standards.c_targets.is_empty());
        assert_eq!(conversions[0].probe_standards.c, None);
        assert_eq!(
            conversions[0].probe_standards.cxx,
            Some(cabin_core::CxxStandard::Cxx14)
        );
    }

    /// Nothing declared leaves the probe on its defaults.
    #[test]
    fn probe_standards_default_when_no_interface_is_declared() {
        let dir = assert_fs::TempDir::new().unwrap();
        write_package(
            dir.path(),
            "fmt",
            "12.2.0",
            &package_manifest("cabin-ports/fmt", "12.2.0", ""),
        );
        let conversions = load_conversions(dir.path()).unwrap();
        assert_eq!(conversions[0].probe_standards, ProbeStandards::default());
    }

    #[test]
    fn a_package_with_a_system_dependency_is_refused() {
        let dir = assert_fs::TempDir::new().unwrap();
        write_package(
            dir.path(),
            "fmt",
            "12.2.0",
            &package_manifest(
                "cabin-ports/fmt",
                "12.2.0",
                "[dependencies]\nzlib = { version = \"1.3\", system = true }\n\n",
            ),
        );
        let err = load_conversions(dir.path()).unwrap_err();
        assert!(format!("{err:#}").contains("system dependency"), "{err:#}");
    }

    #[test]
    fn a_package_with_a_non_registry_dependency_is_refused() {
        let dir = assert_fs::TempDir::new().unwrap();
        write_package(
            dir.path(),
            "fmt",
            "12.2.0",
            &package_manifest(
                "cabin-ports/fmt",
                "12.2.0",
                "[dependencies]\nzlib = { path = \"../zlib\" }\n\n",
            ),
        );
        let err = load_conversions(dir.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("non-registry source"),
            "{err:#}"
        );
    }

    #[test]
    fn a_package_with_a_registry_invalid_name_is_refused() {
        let dir = assert_fs::TempDir::new().unwrap();
        write_package(
            dir.path(),
            "foo.bar",
            "1.0.0",
            &package_manifest("cabin-ports/foo.bar", "1.0.0", ""),
        );
        let err = load_conversions(dir.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("canonical registry package name"),
            "{err:#}"
        );
    }

    /// The manifest carries the package's identity and provenance,
    /// so it must be the committed regular file - following a
    /// symlink would let published metadata come from outside the
    /// version directory.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_migrated_manifest_is_refused() {
        let dir = assert_fs::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("outside.toml"),
            package_manifest("cabin-ports/fmt", "12.2.0", ""),
        )
        .unwrap();
        let package_dir = dir.path().join("ports/fmt/12.2.0");
        std::fs::create_dir_all(&package_dir).unwrap();
        std::os::unix::fs::symlink(
            dir.path().join("outside.toml"),
            package_dir.join("cabin.toml"),
        )
        .unwrap();
        let err = load_conversions(&dir.path().join("ports")).unwrap_err();
        assert!(format!("{err:#}").contains("not a regular file"), "{err:#}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_port_directory_is_refused() {
        let dir = assert_fs::TempDir::new().unwrap();
        let outside = dir.path().join("outside/fmt");
        std::fs::create_dir_all(outside.join("12.2.0")).unwrap();
        std::fs::write(
            outside.join("12.2.0/cabin.toml"),
            package_manifest("cabin-ports/fmt", "12.2.0", ""),
        )
        .unwrap();
        let ports = dir.path().join("ports");
        std::fs::create_dir_all(&ports).unwrap();
        std::os::unix::fs::symlink(&outside, ports.join("fmt")).unwrap();
        let err = load_conversions(&ports).unwrap_err();
        assert!(
            format!("{err:#}").contains("symlinked directory"),
            "{err:#}"
        );
    }

    #[test]
    fn an_unsatisfiable_package_dependency_is_diagnosed_as_such() {
        let dir = assert_fs::TempDir::new().unwrap();
        write_package(
            dir.path(),
            "fmt",
            "12.2.0",
            &package_manifest(
                "cabin-ports/fmt",
                "12.2.0",
                "[dependencies]\n\"cabin-ports/zlib\" = \"^2\"\n\n",
            ),
        );
        write_package(
            dir.path(),
            "zlib",
            "1.3.1",
            &package_manifest("cabin-ports/zlib", "1.3.1", ""),
        );
        let err = load_conversions(dir.path()).unwrap_err();
        // The real diagnosis, never Kahn's phantom "cycle".
        let message = format!("{err:#}");
        assert!(
            message.contains("which no committed port version satisfies"),
            "{message}"
        );
        assert!(!message.contains("cycle"), "{message}");
    }
}
