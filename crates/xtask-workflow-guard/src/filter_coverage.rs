//! The change filters' coverage guard (`cargo workflow-filter-coverage`),
//! run by `.github/workflows/rust.yml`'s lint job,
//! `.github/workflows/registry.yml`'s conformance job, and `cargo ci`.
//!
//! Two lists in `.github/path-filters.yml` scope jobs by what those jobs
//! build instead of by every crate:
//!
//! - `registry-build` (registry.yml's build and Kani jobs): the registry
//!   workspace and `REGISTRY_ROOTS`, plus `REGISTRY_INPUTS`.
//! - `ports-dry-run` (ports-publish.yml's pull-request dry-run): the
//!   `cabin` binary and the publisher, plus `PORTS_INPUTS`.
//!
//! The coarse `registry` list (the conformance job and the main-push
//! deploy) is checked against `registry-build`'s closure and inputs too,
//! so every change the Worker builds from also deploys.
//!
//! A list is sound only while it covers the directory of every package
//! inside the repository that its roots reach in Cargo's resolved
//! dependency graph (`cargo metadata --locked`, every dependency kind),
//! and every non-Cargo input its jobs read: `SHARED_INPUTS` and the
//! list's own. The resolved graph is what Cargo builds, so a `[patch]`,
//! `[replace]`, `paths` override, or vendored source from any manifest
//! or config file lands in it with the directory it builds from; a
//! package outside the repository is pinned by `Cargo.lock`, which every
//! list covers. A reached package inside the repository must also be a
//! member of the root or the registry workspace: what it inherits
//! (`workspace = true`) then comes from `Cargo.toml` or
//! `registry/Cargo.toml`, both covered, where another workspace's root
//! manifest would be a path no list is checked for; and each of its
//! targets must lie inside its directory, for the same reason. The
//! guard fails when a list misses a path. The roots and the non-Cargo
//! inputs are hand-kept: a new cargo alias, run-time `cargo build -p`,
//! file the jobs read, or file a crate includes from outside its own
//! directory (`include!` and its variants, `#[path]`, a build script's
//! reads) belongs in this file, since
//! the guard cannot see one.
//!
//! Only literal entries are decidable without re-implementing
//! dorny/paths-filter's matcher, so a checked list may hold nothing but
//! a file path, `<dir>/**`, and `!<dir>/**`, each segment made of ASCII
//! letters, digits, `.`, `_`, and `-` (`canonical`). Under the gate's
//! `some-with-excludes` rule a path then matches when an include names
//! it or is `<dir>/**` above it and no exclusion's directory is above
//! it; a directory counts as covered only when no exclusion reaches
//! into it either.
//!
//! The CI call sites run the guard where the `rust` list (the lint job)
//! or the `registry` list (the conformance job) matches, and the lint
//! job also on any edit to the filter file, through the gate's inline
//! `filter-file` output, so an edit to the lists cannot skip it. A
//! closure changes only through an edit to a package in it or to
//! `SHARED_INPUTS`, so the guard also fails on any of those paths
//! neither list covers, and checks the `rust` list for literal entries
//! too.
//!
//! Failure direction: every unanswered question fails the step - a
//! failed `cargo metadata`, a missing, malformed, or non-literal list,
//! a root missing from the workspace, a package missing from the
//! resolved graph, and a reached package inside the repository outside
//! both workspaces or with a target outside its directory.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

use crate::superseded::{FILTER_FILE, filter_entries};

/// Root-workspace packages registry.yml's `registry-build`-gated jobs
/// run through cargo aliases or build at run time.
const REGISTRY_ROOTS: &[&str] = &[
    // check-sql, check-r2, check-deploy, check-kani-report.
    "xtask-registry-guard",
    // registry-smoke, which builds cabin-registry-verify at run time.
    "xtask-registry-smoke",
    "cabinpkg-registry-verify",
];

/// The `cabin` binary the dry-run builds the ports with.
const CLI: &str = "cabinpkg";
/// The port publisher the dry-run runs.
const PUBLISHER: &str = "xtask-port-publish";

/// Non-Cargo inputs every scoped job reads; directories end in `/`.
const SHARED_INPUTS: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ".cargo/",
    ".github/workflows/changes.yml",
    FILTER_FILE,
];
const REGISTRY_INPUTS: &[&str] = &[
    "registry/",
    ".github/workflows/registry.yml",
    ".github/actions/required-gate/",
];
const PORTS_INPUTS: &[&str] = &[
    "ports/",
    // Decides the patch bytes the Windows leg checks out.
    ".gitattributes",
    ".github/workflows/ports-publish.yml",
];

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
    resolve: Resolve,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    manifest_path: PathBuf,
    targets: Vec<Target>,
}

#[derive(Deserialize)]
struct Target {
    src_path: PathBuf,
}

#[derive(Deserialize)]
struct Resolve {
    nodes: Vec<Node>,
}

#[derive(Deserialize)]
struct Node {
    id: String,
    deps: Vec<NodeDep>,
}

#[derive(Deserialize)]
struct NodeDep {
    pkg: String,
}

/// Fail unless each scoped list covers every package directory and
/// non-Cargo input its jobs build from.
///
/// # Errors
///
/// When a list misses a path, or when the answer cannot be established
/// (module docs, "Failure direction").
pub fn run() -> Result<()> {
    let repo = std::fs::canonicalize(".").context("resolving the working directory")?;
    let root = metadata(&repo)?;
    // From inside the workspace, so Cargo reads its config as the
    // registry jobs do.
    let registry = metadata(&repo.join("registry"))?;
    let text =
        std::fs::read_to_string(FILTER_FILE).with_context(|| format!("reading {FILTER_FILE}"))?;

    let members = registry.workspace_members.iter().map(String::as_str);
    let mut registry_packages = closure(&registry, members.collect())?;
    registry_packages.extend(closure(&root, members_named(&root, REGISTRY_ROOTS)?)?);
    let ports_packages = closure(&root, members_named(&root, &[CLI, PUBLISHER])?)?;
    let dirs =
        |packages: &[&Package]| resolved(packages.iter().map(|&package| dir(package)).collect());
    let (registry_dirs, ports_dirs) = (dirs(&registry_packages)?, dirs(&ports_packages)?);
    let member_dirs = [&root, &registry].into_iter().flat_map(|metadata| {
        let members = metadata.packages.iter();
        members
            .filter(|package| metadata.workspace_members.contains(&package.id))
            .map(dir)
    });
    let member_dirs: HashSet<PathBuf> = resolved(member_dirs.collect())?.into_iter().collect();
    let reached: BTreeSet<&PathBuf> = registry_dirs.iter().chain(&ports_dirs).collect();

    let mut report = String::new();
    for dir in reached
        .iter()
        .filter(|dir| dir.starts_with(&repo) && !member_dirs.contains(**dir))
    {
        writeln!(
            report,
            "  outside both workspaces: {}",
            dir.strip_prefix(&repo).unwrap_or(dir).display()
        )?;
    }
    let reached_packages = registry_packages.iter().chain(&ports_packages).copied();
    for src in escaping(&repo, reached_packages)? {
        writeln!(
            report,
            "  target outside its package: {}",
            src.strip_prefix(&repo).unwrap_or(&src).display()
        )?;
    }
    let mut lists = HashMap::new();
    for name in ["registry-build", "registry", "ports-dry-run", "rust"] {
        let entries = filter_entries(&text, name)
            .with_context(|| format!("no usable `{name}` list in {FILTER_FILE}"))?;
        for entry in entries.iter().filter(|entry| !canonical(entry)) {
            writeln!(report, "  {name}: unsupported entry {entry:?}")?;
        }
        lists.insert(name, entries);
    }
    for (name, dirs, inputs) in [
        ("registry-build", &registry_dirs, REGISTRY_INPUTS),
        ("registry", &registry_dirs, REGISTRY_INPUTS),
        ("ports-dry-run", &ports_dirs, PORTS_INPUTS),
    ] {
        for path in missing(&repo, dirs, inputs, &lists[name])? {
            writeln!(report, "  {name}: {path}")?;
        }
    }
    let reached: Vec<&PathBuf> = reached.into_iter().collect();
    for path in unguarded(&repo, &reached, &lists["rust"], &lists["registry"])? {
        writeln!(
            report,
            "  `rust` or `registry` (the guard's own triggers): {path}"
        )?;
    }
    if !report.is_empty() {
        bail!(
            "these entries or paths break the lists in {FILTER_FILE} that scope jobs by what \
             they build; use literal entries, include the paths or drop the exclusion, and \
             keep every package inside the repository a workspace member with its targets \
             inside its directory:\n\
             {report}"
        );
    }
    Ok(())
}

/// The resolved metadata of the workspace in `dir`.
fn metadata(dir: &Path) -> Result<Metadata> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--locked"])
        .current_dir(dir)
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("spawning `cargo metadata` in {}", dir.display()))?;
    if !output.status.success() {
        bail!(
            "`cargo metadata` in {} failed ({})",
            dir.display(),
            output.status
        );
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("parsing `cargo metadata` in {}", dir.display()))
}

/// The ids of the named workspace members.
fn members_named<'a>(metadata: &'a Metadata, names: &[&str]) -> Result<Vec<&'a str>> {
    names
        .iter()
        .map(|name| {
            metadata
                .packages
                .iter()
                .find(|package| {
                    package.name == *name && metadata.workspace_members.contains(&package.id)
                })
                .map(|package| package.id.as_str())
                .with_context(|| format!("`{name}` is not a root-workspace member"))
        })
        .collect()
}

/// Every package `roots` reach in the resolved graph.
fn closure<'a>(metadata: &'a Metadata, mut pending: Vec<&'a str>) -> Result<Vec<&'a Package>> {
    let packages: HashMap<&str, &Package> = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect();
    let nodes: HashMap<&str, &Node> = metadata
        .resolve
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut seen = HashSet::new();
    let mut reached = Vec::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let package = packages
            .get(id)
            .with_context(|| format!("`{id}` is not in the metadata"))?;
        reached.push(*package);
        let node = nodes
            .get(id)
            .with_context(|| format!("`{id}` is not in the resolved graph"))?;
        pending.extend(node.deps.iter().map(|dep| dep.pkg.as_str()));
    }
    Ok(reached)
}

fn dir(package: &Package) -> &Path {
    package.manifest_path.parent().unwrap_or(Path::new(""))
}

/// The sources of the targets of `packages` inside `repo` that lie
/// outside their package's directory, resolved.
fn escaping<'a>(
    repo: &Path,
    packages: impl Iterator<Item = &'a Package>,
) -> Result<BTreeSet<PathBuf>> {
    let mut escaping = BTreeSet::new();
    for package in packages {
        let dir = std::fs::canonicalize(dir(package))
            .with_context(|| format!("resolving {}", dir(package).display()))?;
        if !dir.starts_with(repo) {
            continue;
        }
        for target in &package.targets {
            let src = std::fs::canonicalize(&target.src_path)
                .with_context(|| format!("resolving {}", target.src_path.display()))?;
            if !src.starts_with(&dir) {
                escaping.insert(src);
            }
        }
    }
    Ok(escaping)
}

/// `dirs` resolved: Cargo reports a directory source's packages
/// unnormalized (`registry/../crates/...`), and `missing` compares
/// paths lexically.
fn resolved(dirs: Vec<&Path>) -> Result<Vec<PathBuf>> {
    dirs.into_iter()
        .map(|dir| {
            std::fs::canonicalize(dir).with_context(|| format!("resolving {}", dir.display()))
        })
        .collect()
}

/// The paths `entries` does not cover: the directories in `dirs` inside
/// `repo`, sorted, then `SHARED_INPUTS` and `inputs`.
fn missing(
    repo: &Path,
    dirs: &[impl AsRef<Path>],
    inputs: &[&str],
    entries: &[String],
) -> Result<Vec<String>> {
    let mut inside = BTreeSet::new();
    for dir in dirs {
        if let Ok(relative) = dir.as_ref().strip_prefix(repo) {
            let relative = relative
                .to_str()
                .with_context(|| format!("{} is not UTF-8", relative.display()))?;
            inside.insert(format!("{}/", relative.replace('\\', "/")));
        }
    }
    let mut paths: Vec<String> = inside.into_iter().collect();
    paths.extend(
        SHARED_INPUTS
            .iter()
            .chain(inputs)
            .map(|&path| path.to_owned()),
    );
    paths.retain(|path| !covered(entries, path));
    Ok(paths)
}

/// The paths in `dirs` and `SHARED_INPUTS` that neither `rust` nor
/// `registry` covers, so an edit there skips the guard (module docs).
fn unguarded(
    repo: &Path,
    dirs: &[impl AsRef<Path>],
    rust: &[String],
    registry: &[String],
) -> Result<Vec<String>> {
    let mut paths = missing(repo, dirs, &[], rust)?;
    paths.retain(|path| !covered(registry, path));
    Ok(paths)
}

/// Whether `entry` is a file path, `<dir>/**`, or `!<dir>/**` of
/// literal segments (module docs).
fn canonical(entry: &str) -> bool {
    let (excluded, rest) = entry
        .strip_prefix('!')
        .map_or((false, entry), |rest| (true, rest));
    let (dir, path) = rest
        .strip_suffix("/**")
        .map_or((false, rest), |path| (true, path));
    (dir || !excluded)
        && path.split('/').all(|segment| {
            !matches!(segment, "" | "." | "..")
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        })
}

/// Whether canonical `entries` match everything under `path`, a file or
/// a directory ending in `/`.
fn covered(entries: &[String], path: &str) -> bool {
    let dirs = |excluded: bool| {
        entries.iter().filter_map(move |entry| {
            let entry = if excluded {
                entry.strip_prefix('!')?
            } else if entry.starts_with('!') {
                return None;
            } else {
                entry
            };
            entry.strip_suffix("/**").map(|dir| format!("{dir}/"))
        })
    };
    let named = !path.ends_with('/') && entries.iter().any(|entry| entry == path);
    (named || dirs(false).any(|dir| path.starts_with(&dir)))
        && !dirs(true).any(|dir| path.starts_with(&dir) || dir.starts_with(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resolved workspace at `/repo`: `(name, directory, deps)`, the
    /// directory relative to `/repo` unless absolute, and the
    /// packages named in `members` its members.
    fn workspace(packages: &[(&str, &str, &[&str])], members: &[&str]) -> Metadata {
        Metadata {
            packages: packages
                .iter()
                .map(|(name, dir, _)| Package {
                    id: (*name).to_owned(),
                    name: (*name).to_owned(),
                    manifest_path: Path::new("/repo").join(dir).join("Cargo.toml"),
                    targets: Vec::new(),
                })
                .collect(),
            workspace_members: members.iter().map(|&name| name.to_owned()).collect(),
            resolve: Resolve {
                nodes: packages
                    .iter()
                    .map(|(name, _, deps)| Node {
                        id: (*name).to_owned(),
                        deps: deps
                            .iter()
                            .map(|&dep| NodeDep {
                                pkg: dep.to_owned(),
                            })
                            .collect(),
                    })
                    .collect(),
            },
        }
    }

    fn list(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|&entry| entry.to_owned()).collect()
    }

    /// `entries` plus a covering entry for every non-Cargo input.
    fn with_inputs(entries: &[&str]) -> Vec<String> {
        let inputs = SHARED_INPUTS
            .iter()
            .chain(REGISTRY_INPUTS)
            .chain(PORTS_INPUTS);
        let globs = inputs.map(|path| {
            let glob = if path.ends_with('/') { "**" } else { "" };
            format!("{path}{glob}")
        });
        list(entries).into_iter().chain(globs).collect()
    }

    /// `cabinpkg` reaches `cabin-build` and `cabin-tidy`, both of which
    /// reach `cabin-core`; the publisher reaches `cabin-publish` and the
    /// fetched `anyhow`; `xtask-ci` is outside the closure.
    fn ports_workspace() -> Metadata {
        workspace(
            &[
                (CLI, "crates/cabin", &["cabin-build", "cabin-tidy"]),
                ("cabin-build", "crates/cabin-build", &["cabin-core"]),
                ("cabin-core", "crates/cabin-core", &[]),
                ("cabin-tidy", "crates/cabin-tidy", &["cabin-core"]),
                ("cabin-publish", "crates/cabin-publish", &["cabin-core"]),
                (
                    PUBLISHER,
                    "crates/xtask-port-publish",
                    &["cabin-publish", "anyhow"],
                ),
                ("anyhow", "/home/.cargo/registry/src/anyhow-1.0.0", &[]),
                ("xtask-ci", "crates/xtask-ci", &["cabin-core"]),
            ],
            &[
                CLI,
                "cabin-build",
                "cabin-core",
                "cabin-tidy",
                "cabin-publish",
                PUBLISHER,
                "xtask-ci",
            ],
        )
    }

    fn ports_missing(metadata: &Metadata, entries: &[String]) -> Vec<String> {
        let roots = members_named(metadata, &[CLI, PUBLISHER]).unwrap();
        let dirs: Vec<&Path> = closure(metadata, roots)
            .unwrap()
            .into_iter()
            .map(dir)
            .collect();
        missing(Path::new("/repo"), &dirs, PORTS_INPUTS, entries).unwrap()
    }

    #[test]
    fn a_crate_outside_the_closure_may_be_excluded() {
        let entries = with_inputs(&["crates/**", "!crates/xtask-ci/**"]);
        let missing = ports_missing(&ports_workspace(), &entries);
        assert!(missing.is_empty(), "{missing:?}");
    }

    #[test]
    fn a_crate_inside_the_closure_may_not() {
        for (excluded, expected) in [
            // A direct and a transitive dependency of the binary.
            ("!crates/cabin-tidy/**", "crates/cabin-tidy/"),
            ("!crates/cabin-core/**", "crates/cabin-core/"),
            // The publisher's own dependency, and the two roots.
            ("!crates/cabin-publish/**", "crates/cabin-publish/"),
            ("!crates/cabin/**", "crates/cabin/"),
            (
                "!crates/xtask-port-publish/**",
                "crates/xtask-port-publish/",
            ),
        ] {
            assert_eq!(
                ports_missing(&ports_workspace(), &with_inputs(&["crates/**", excluded])),
                vec![expected.to_owned()],
                "{excluded}"
            );
        }
    }

    #[test]
    fn every_non_cargo_input_must_be_covered() {
        let full = with_inputs(&["crates/**"]);
        for input in SHARED_INPUTS.iter().chain(PORTS_INPUTS) {
            let without: Vec<String> = full
                .iter()
                .filter(|entry| !entry.starts_with(input))
                .cloned()
                .collect();
            assert_eq!(
                ports_missing(&ports_workspace(), &without),
                vec![(*input).to_owned()],
                "{input}"
            );
        }
        // Excluding part of an input directory uncovers all of it.
        assert_eq!(
            ports_missing(
                &ports_workspace(),
                &with_inputs(&["crates/**", "!ports/zlib/**"])
            ),
            vec!["ports/".to_owned()]
        );
    }

    #[test]
    fn every_resolved_edge_counts_whatever_its_kind() {
        let metadata: Metadata = serde_json::from_str(
            r#"{"workspace_members": ["cli", "publisher"],
                "packages": [
                  {"id": "cli", "name": "cabinpkg",
                   "manifest_path": "/repo/crates/cabin/Cargo.toml", "targets": []},
                  {"id": "build", "name": "build",
                   "manifest_path": "/repo/crates/build/Cargo.toml", "targets": []},
                  {"id": "dev", "name": "dev",
                   "manifest_path": "/repo/crates/dev/Cargo.toml", "targets": []},
                  {"id": "publisher", "name": "xtask-port-publish",
                   "manifest_path": "/repo/crates/xtask-port-publish/Cargo.toml", "targets": []}],
                "resolve": {"nodes": [
                  {"id": "cli", "deps": [
                    {"pkg": "build", "dep_kinds": [{"kind": "build", "target": null}]},
                    {"pkg": "dev", "dep_kinds": [{"kind": "dev", "target": "cfg(windows)"}]}]},
                  {"id": "build", "deps": []},
                  {"id": "dev", "deps": []},
                  {"id": "publisher", "deps": []}]}}"#,
        )
        .unwrap();
        assert_eq!(
            ports_missing(
                &metadata,
                &with_inputs(&["crates/cabin/**", "crates/xtask-port-publish/**"])
            ),
            ["crates/build/", "crates/dev/"]
        );
    }

    #[test]
    fn a_missing_root_or_node_fails() {
        let mut metadata = ports_workspace();
        metadata.workspace_members.retain(|id| id != PUBLISHER);
        assert!(members_named(&metadata, &[CLI, PUBLISHER]).is_err());

        let mut metadata = ports_workspace();
        metadata
            .resolve
            .nodes
            .retain(|node| node.id != "cabin-core");
        let roots = members_named(&metadata, &[CLI]).unwrap();
        assert!(closure(&metadata, roots).is_err());
    }

    #[test]
    fn every_closure_edit_must_reach_the_guard() {
        // The publisher reaches a package outside `crates/`.
        let mut metadata = ports_workspace();
        let anyhow = metadata
            .packages
            .iter_mut()
            .find(|package| package.name == "anyhow")
            .unwrap();
        anyhow.manifest_path = PathBuf::from("/repo/local/anyhow/Cargo.toml");
        let roots = members_named(&metadata, &[CLI, PUBLISHER]).unwrap();
        let dirs: Vec<&Path> = closure(&metadata, roots)
            .unwrap()
            .into_iter()
            .map(dir)
            .collect();
        let rust = with_inputs(&["crates/**"]);
        let registry = list(&["registry/**", "crates/**"]);
        assert_eq!(
            unguarded(Path::new("/repo"), &dirs, &rust, &registry).unwrap(),
            ["local/anyhow/"]
        );
        // Either list may see it.
        let registry = list(&["registry/**", "crates/**", "local/**"]);
        assert!(
            unguarded(Path::new("/repo"), &dirs, &rust, &registry)
                .unwrap()
                .is_empty()
        );
        // And so must every shared input.
        let rust = list(&["crates/**", "local/**"]);
        assert_eq!(
            unguarded(Path::new("/repo"), &dirs, &rust, &list(&["Cargo.lock"])).unwrap(),
            SHARED_INPUTS
                .iter()
                .filter(|&&input| input != "Cargo.lock")
                .copied()
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_target_outside_its_package_fails() {
        // Real files, since the check resolves paths: a package rooted
        // at this crate's `src/`, whose second target only lexically
        // lies inside it.
        let repo = std::fs::canonicalize(env!("CARGO_MANIFEST_DIR")).unwrap();
        let target = |path: &str| Target {
            src_path: repo.join(path),
        };
        let package = Package {
            id: String::new(),
            name: String::new(),
            manifest_path: repo.join("src/Cargo.toml"),
            targets: vec![target("src/lib.rs"), target("src/../Cargo.toml")],
        };
        assert_eq!(
            escaping(&repo, [&package].into_iter()).unwrap(),
            BTreeSet::from([repo.join("Cargo.toml")])
        );
        // Outside the repository, nothing is checked.
        assert!(
            escaping(Path::new("/elsewhere"), [&package].into_iter())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn only_literal_entries_are_supported() {
        for entry in [
            "Cargo.toml",
            ".cargo/**",
            "crates/**",
            "!crates/cabin-tidy/**",
            ".github/actions/required-gate/**",
        ] {
            assert!(canonical(entry), "{entry}");
        }
        for entry in [
            "crates/*/tests/**",
            "!**/*.rs",
            "!./crates/cabin-core/**",
            "./!crates/cabin-core/**",
            "!crates/cabin-config/**|crates/cabin-core/**",
            "crates/cabin-core/",
            "crates/cabin-core**",
            "!Cargo.toml",
            "../crates/**",
            "",
        ] {
            assert!(!canonical(entry), "{entry}");
        }
    }

    #[test]
    fn an_exclusion_reaching_into_a_path_uncovers_it() {
        let all = list(&["crates/**", "!crates/cabin-tidy/**"]);
        assert!(covered(&all, "crates/cabin-core/"));
        assert!(!covered(&all, "crates/cabin-tidy/"));
        // A sibling sharing the name prefix is not under the entry.
        assert!(covered(&all, "crates/cabin-tidy-extra/"));
        // An exclusion below a directory uncovers the directory.
        assert!(!covered(
            &list(&["crates/**", "!crates/cabin-core/src/**"]),
            "crates/cabin-core/"
        ));
        // Only `<dir>/**` covers a directory; a file is covered by its
        // own name.
        assert!(!covered(
            &list(&["crates/cabin-core"]),
            "crates/cabin-core/"
        ));
        assert!(covered(&list(&["Cargo.lock"]), "Cargo.lock"));
    }
}
