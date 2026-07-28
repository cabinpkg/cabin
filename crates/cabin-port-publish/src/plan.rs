//! Recipe discovery and publication planning.
//!
//! Scans the committed `ports/` directory, converts every recipe
//! (see [`crate::convert`]), and orders the results so every port
//! is published after the ports it depends on.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use cabin_core::{DependencySource, PackageName};
use cabin_port::PortDescriptor;
use semver::Version;

use crate::convert::{
    ConvertRequest, RecipeSummary, convert_overlay, published_version, summarize,
};

/// Sidecar file a recipe directory may carry to publish a packaging
/// revision (`ports/<name>/<version>/packaging-revision`, an
/// integer of at least 1).  Repository-only: `cabin-port`'s
/// embedding ignores it, so the builtin port layer is unaffected.
pub const REVISION_FILENAME: &str = "packaging-revision";

/// One recipe, converted and ready to materialize.
#[derive(Debug)]
pub struct PortConversion {
    /// `ports/<name>/<version>/` recipe directory.
    pub recipe_dir: PathBuf,
    /// Parsed `port.toml`.
    pub descriptor: PortDescriptor,
    /// Scoped registry name (`cabin-ports/<lowercase>`).
    pub scoped_name: PackageName,
    /// Version the conversion publishes (upstream version plus any
    /// packaging revision).
    pub published_version: Version,
    /// Converted manifest text.
    pub manifest: String,
    /// Inter-port dependencies the conversion rewrote — the
    /// publication-order edges, requirement included so ordering can
    /// wait for a *satisfying* version, not just any version of the
    /// name.
    pub dependencies: Vec<PortDependencyEdge>,
}

/// One rewritten inter-port dependency edge.
#[derive(Debug, Clone)]
pub struct PortDependencyEdge {
    /// Scoped registry name of the dependency.
    pub scoped: PackageName,
    /// Version requirement the conversion carries for it.
    pub req: semver::VersionReq,
}

/// Scan `ports_dir`, convert every recipe, and return the
/// conversions in publication order (dependencies first; name-sorted
/// within a rank, versions ascending within a name).
///
/// # Errors
/// Returns an error when the directory cannot be read, any recipe
/// fails to load or convert, converted names collide, or the
/// inter-port dependency graph has a cycle.
pub fn load_conversions(ports_dir: &Path) -> Result<Vec<PortConversion>> {
    let recipes = discover_recipes(ports_dir)?;
    if recipes.is_empty() {
        bail!("no recipes found under {}", ports_dir.display());
    }

    // First pass: parse everything and summarize each port for
    // cross-port dependency rewriting.
    let mut summaries: BTreeMap<String, RecipeSummary> = BTreeMap::new();
    let mut loaded = Vec::new();
    for recipe_dir in recipes {
        let descriptor = cabin_port::load_port(recipe_dir.join("port.toml"))
            .with_context(|| format!("loading {}", recipe_dir.join("port.toml").display()))?;
        let overlay_path = recipe_dir.join(&descriptor.overlay.relative_path);
        let overlay_text = fs::read_to_string(&overlay_path)
            .with_context(|| format!("reading {}", overlay_path.display()))?;
        let parsed = cabin_manifest::parse_manifest_str(&overlay_text)
            .with_context(|| format!("parsing {}", overlay_path.display()))?;
        let package = parsed
            .package
            .ok_or_else(|| anyhow!("{} has no [package] table", overlay_path.display()))?;
        let port_name = descriptor.name.as_str().to_owned();
        let summary = summarize(&port_name, &package)?;
        if let Some(previous) = summaries.get(&port_name) {
            // Two versions of one port summarize identically as long
            // as their converted target sets agree; a disagreement
            // would make dependency rewriting ambiguous.
            if previous.library_like_target_keys != summary.library_like_target_keys {
                bail!(
                    "recipe versions of port `{port_name}` disagree on their library-like \
                     targets; dependency rewriting needs one consistent target set"
                );
            }
        } else {
            summaries.insert(port_name.clone(), summary);
        }
        let revision = read_revision(&recipe_dir)?;
        loaded.push((recipe_dir, descriptor, overlay_text, revision, package));
    }

    ensure_distinct_scoped_names(&summaries)?;
    ensure_port_requirements_satisfiable(&loaded)?;

    // Second pass: convert.
    let mut conversions = Vec::new();
    for (recipe_dir, descriptor, overlay_text, revision, package) in loaded {
        let request = ConvertRequest {
            descriptor: &descriptor,
            overlay_text: &overlay_text,
            revision,
            summaries: &summaries,
        };
        let manifest = convert_overlay(&request)
            .with_context(|| format!("converting {}", recipe_dir.display()))?;
        let dependencies = package
            .dependencies
            .iter()
            .filter_map(|dep| match &dep.source {
                DependencySource::Port(cabin_core::PortDepSource::Builtin {
                    version_req, ..
                }) => Some((dep, version_req.clone())),
                _ => None,
            })
            .map(|(dep, req)| {
                summaries
                    .get(dep.name.as_str())
                    .map(|summary| PortDependencyEdge {
                        scoped: summary.scoped.clone(),
                        req,
                    })
                    .ok_or_else(|| anyhow!("unknown port dependency `{}`", dep.name.as_str()))
            })
            .collect::<Result<Vec<_>>>()?;
        let scoped_name = summaries[descriptor.name.as_str()].scoped.clone();
        let published_version = published_version(&descriptor.version, revision)?;
        conversions.push(PortConversion {
            recipe_dir,
            descriptor,
            scoped_name,
            published_version,
            manifest,
            dependencies,
        });
    }

    ensure_unique_identities(&conversions)?;
    order_by_dependencies(conversions)
}

/// One first-pass recipe: directory, descriptor, overlay text,
/// optional packaging revision, parsed overlay package.
type LoadedRecipe = (
    PathBuf,
    PortDescriptor,
    String,
    Option<u32>,
    cabin_core::Package,
);

/// Two distinct recipe names must not fold onto one scoped name
/// (`cJSON/` next to `cjson/` would silently merge under
/// `cabin-ports/cjson`).
fn ensure_distinct_scoped_names(summaries: &BTreeMap<String, RecipeSummary>) -> Result<()> {
    let mut scoped_owners: BTreeMap<&str, &str> = BTreeMap::new();
    for (port_name, summary) in summaries {
        if let Some(previous) = scoped_owners.insert(summary.scoped.as_str(), port_name) {
            bail!(
                "ports `{previous}` and `{port_name}` both convert to `{}`; converted names \
                 must stay distinct",
                summary.scoped.as_str()
            );
        }
    }
    Ok(())
}

/// Every rewritten requirement must be satisfiable by a version
/// converted in the same run, or the preflight (and every consumer)
/// would fail resolution later with a worse error.
fn ensure_port_requirements_satisfiable(loaded: &[LoadedRecipe]) -> Result<()> {
    let mut versions_by_port: BTreeMap<String, Vec<Version>> = BTreeMap::new();
    for (_, descriptor, _, _, _) in loaded {
        versions_by_port
            .entry(descriptor.name.as_str().to_owned())
            .or_default()
            .push(descriptor.version.clone());
    }
    for (recipe_dir, _, _, _, package) in loaded {
        for dep in &package.dependencies {
            let DependencySource::Port(cabin_core::PortDepSource::Builtin { version_req, .. }) =
                &dep.source
            else {
                continue;
            };
            let satisfied = versions_by_port
                .get(dep.name.as_str())
                .is_some_and(|versions| versions.iter().any(|v| version_req.matches(v)));
            if !satisfied {
                bail!(
                    "{} depends on `{}` with requirement `{version_req}`, which no committed \
                     recipe version satisfies",
                    recipe_dir.display(),
                    dep.name.as_str()
                );
            }
        }
    }
    Ok(())
}

/// `ports/<name>/<version>/` directories that hold a `port.toml`.
fn discover_recipes(ports_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut recipes = Vec::new();
    let names = read_sorted_dirs(ports_dir)?;
    for name_dir in names {
        for version_dir in read_sorted_dirs(&name_dir)? {
            if version_dir.join("port.toml").is_file() {
                recipes.push(version_dir);
            }
        }
    }
    Ok(recipes)
}

fn read_sorted_dirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("reading {}", dir.display()))?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    Ok(dirs)
}

/// Read the optional packaging-revision sidecar.
///
/// # Errors
/// Returns an error when the file exists but does not hold an
/// integer >= 1.
fn read_revision(recipe_dir: &Path) -> Result<Option<u32>> {
    let path = recipe_dir.join(REVISION_FILENAME);
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let revision: u32 = text
        .trim()
        .parse()
        .with_context(|| format!("{} must hold an integer >= 1", path.display()))?;
    if revision == 0 {
        bail!(
            "{} must hold an integer >= 1; the unrevised publication is revision zero",
            path.display()
        );
    }
    Ok(Some(revision))
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
                "two recipes convert to `{} {}`; converted identities must be unique",
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
        // Requirement matching ignores build metadata, so packaging
        // revisions never affect readiness.
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

    const SHA: &str = "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23";

    fn write_recipe(
        root: &assert_fs::fixture::ChildPath,
        name: &str,
        version: &str,
        overlay_extra: &str,
    ) {
        let dir = root.child(format!("{name}/{version}"));
        dir.child("port.toml")
            .write_str(&format!(
                "[port]\nname = \"{name}\"\nversion = \"{version}\"\n\n[source]\ntype = \
                 \"archive\"\nurl = \"https://example.com/{name}-{version}.tar.gz\"\nsha256 = \
                 \"{SHA}\"\nstrip_prefix = \"{name}-{version}\"\n\n[overlay]\nmanifest = \
                 \"cabin.toml\"\n"
            ))
            .unwrap();
        dir.child("cabin.toml")
            .write_str(&format!(
                "[package]\nname = \"{name}\"\nversion = \"{version}\"\n{overlay_extra}\n\
                 [target.{name}]\ntype = \"library\"\nsources = [\"{name}.c\"]\nc-standard = \
                 \"c11\"\n"
            ))
            .unwrap();
    }

    #[test]
    fn orders_dependencies_before_dependents() {
        let dir = TempDir::new().unwrap();
        let ports = dir.child("ports");
        // `apng` sorts before `zlib` but depends on it, so the order
        // must invert the name sort.
        write_recipe(
            &ports,
            "apng",
            "1.0.0",
            "\n[dependencies]\nzlib = { port = true, version = \"^1.3\" }\n",
        );
        write_recipe(&ports, "zlib", "1.3.1", "");
        let conversions = load_conversions(ports.path()).unwrap();
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

    /// Readiness is per satisfying version: `apng` requires `^2` of
    /// zlib, and zlib 2.0.0 itself waits on xxhash, so `apng` must
    /// order after zlib 2.0.0 - even though zlib's name already
    /// published version 1.3.1 in the first rank and `apng` sorts
    /// before `zlib` inside a rank.
    #[test]
    fn dependents_wait_for_a_satisfying_version_not_just_the_name() {
        let dir = TempDir::new().unwrap();
        let ports = dir.child("ports");
        write_recipe(
            &ports,
            "apng",
            "1.0.0",
            "\n[dependencies]\nzlib = { port = true, version = \"^2\" }\n",
        );
        write_recipe(&ports, "zlib", "1.3.1", "");
        write_recipe(
            &ports,
            "zlib",
            "2.0.0",
            "\n[dependencies]\nxxhash = { port = true, version = \"^0.8\" }\n",
        );
        write_recipe(&ports, "xxhash", "0.8.3", "");
        let conversions = load_conversions(ports.path()).unwrap();
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

    #[test]
    fn reads_the_packaging_revision_sidecar() {
        let dir = TempDir::new().unwrap();
        let ports = dir.child("ports");
        write_recipe(&ports, "zlib", "1.3.1", "");
        ports
            .child("zlib/1.3.1")
            .child(REVISION_FILENAME)
            .write_str("2\n")
            .unwrap();
        let conversions = load_conversions(ports.path()).unwrap();
        assert_eq!(
            conversions[0].published_version.to_string(),
            "1.3.1+cabin.2"
        );
        assert_eq!(conversions[0].descriptor.version.to_string(), "1.3.1");
    }

    #[test]
    fn rejects_a_malformed_revision_sidecar() {
        let dir = TempDir::new().unwrap();
        let ports = dir.child("ports");
        write_recipe(&ports, "zlib", "1.3.1", "");
        for bad in ["0", "one", "-1", ""] {
            ports
                .child("zlib/1.3.1")
                .child(REVISION_FILENAME)
                .write_str(bad)
                .unwrap();
            let err = load_conversions(ports.path()).unwrap_err();
            assert!(
                format!("{err:#}").contains(REVISION_FILENAME),
                "{bad:?}: {err:#}"
            );
        }
    }

    #[test]
    fn detects_dependency_cycles() {
        let dir = TempDir::new().unwrap();
        let ports = dir.child("ports");
        write_recipe(
            &ports,
            "aport",
            "1.0.0",
            "\n[dependencies]\nbport = { port = true, version = \"^1\" }\n",
        );
        write_recipe(
            &ports,
            "bport",
            "1.0.0",
            "\n[dependencies]\naport = { port = true, version = \"^1\" }\n",
        );
        let err = load_conversions(ports.path()).unwrap_err();
        assert!(format!("{err:#}").contains("cycle"), "{err:#}");
    }

    /// The committed recipes are the tool's real inputs; converting
    /// them end-to-end (no archives needed — conversion is pure) pins
    /// the requirements that name concrete ports.
    #[test]
    fn committed_recipes_all_convert() {
        let ports_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("cabin-port")
            .join("ports");
        let conversions = load_conversions(&ports_dir).unwrap();
        assert!(
            conversions.len() >= 17,
            "expected every committed recipe, got {}",
            conversions.len()
        );

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

        // libpng orders after zlib and rewrites its dependency to the
        // bare scoped shorthand.
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

        // Provenance is stamped everywhere, and no converted manifest
        // keeps a port dependency or an upper-case identity.
        for conversion in &conversions {
            assert!(
                conversion.manifest.contains("[package.upstream]"),
                "{} lacks provenance",
                conversion.scoped_name.as_str()
            );
            assert!(
                !conversion.manifest.contains("port = true"),
                "{} keeps a port dependency",
                conversion.scoped_name.as_str()
            );
            assert_eq!(
                conversion.scoped_name.as_str(),
                conversion.scoped_name.as_str().to_lowercase(),
                "scoped names are lowercase"
            );
        }

        // Deterministic: a second scan converts byte-identically.
        let again = load_conversions(&ports_dir).unwrap();
        for (a, b) in conversions.iter().zip(&again) {
            assert_eq!(a.scoped_name, b.scoped_name);
            assert_eq!(a.manifest, b.manifest);
        }
    }
}
