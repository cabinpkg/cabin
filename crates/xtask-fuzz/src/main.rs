//! Fuzz-target maintenance (`fuzz/README.md`).
//!
//!   `cargo fuzz-seeds`  regenerate `fuzz/seeds/` from in-tree packages
//!                       and manifests

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

/// Examples staged through the real packager into seed archives: C and
/// C++ packages, with and without a library target and tests.
const SEED_PACKAGES: &[&str] = &[
    "examples/hello-c",
    "examples/hello-cpp",
    "examples/header-only-lib",
    "examples/library-with-tests",
];

/// A published archive with a dependency (`cargo gen-fixtures`).
const SEED_ARCHIVE: &str = "registry/tests/fixtures/smoke-withdep-0.2.0.zip";

/// The fuzz harness's `FALLBACK_MANIFEST` (`fuzz/src/lib.rs`), staged
/// into a seed archive of its own; the harness's test checks the copy.
const FALLBACK_MANIFEST: &str = "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n";

/// Trees whose every `cabin.toml` seeds the manifest parser.
const MANIFEST_TREES: &[&str] = &["examples", "ports"];

#[derive(Parser)]
enum Cli {
    /// Regenerate the seed corpora under `fuzz/seeds/`.
    Seeds,
}

fn main() -> ExitCode {
    let result = match Cli::parse() {
        Cli::Seeds => seeds(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn seeds(root: &Path) -> Result<()> {
    let out = root.join("fuzz/seeds");
    if out.exists() {
        fs::remove_dir_all(&out).with_context(|| format!("removing {}", out.display()))?;
    }

    let mut archives = Vec::new();
    for dir in SEED_PACKAGES {
        let manifest = root.join(dir).join("cabin.toml");
        let staged = cabin_package::stage_with_project(
            &manifest,
            None,
            None,
            &cabin_core::WorkspaceDepRequirements::default(),
        )
        .with_context(|| format!("staging {}", manifest.display()))?;
        archives.push((format!("{}.zip", seed_name(dir)), staged.archive_bytes));
    }
    let fixture = root.join(SEED_ARCHIVE);
    let bytes = fs::read(&fixture).with_context(|| format!("reading {}", fixture.display()))?;
    archives.push(("smoke-withdep-0.2.0.zip".to_owned(), bytes));
    archives.push(("demo-1.2.3.zip".to_owned(), stage_fallback_package()?));
    write_seeds(&out.join("inspect_zip"), &archives)?;

    let mut manifests = Vec::new();
    for tree in MANIFEST_TREES {
        collect_manifests(root, &root.join(tree), &mut manifests)?;
    }
    write_seeds(&out.join("parse_manifest"), &manifests)
}

/// A C package of [`FALLBACK_MANIFEST`], staged through the packager
/// from a scratch directory.
fn stage_fallback_package() -> Result<Vec<u8>> {
    let dir = tempfile::tempdir().context("creating a scratch directory")?;
    let dir = dir.path();
    fs::create_dir(dir.join("src")).context("creating the scratch package")?;
    fs::write(dir.join("cabin.toml"), FALLBACK_MANIFEST)?;
    fs::write(dir.join("src/main.c"), "int main(void) { return 0; }\n")?;
    let staged = cabin_package::stage_with_project(
        &dir.join("cabin.toml"),
        None,
        None,
        &cabin_core::WorkspaceDepRequirements::default(),
    )
    .context("staging the fallback package")?;
    Ok(staged.archive_bytes)
}

/// `examples/hello-c` -> `examples-hello-c`.
fn seed_name(rel: &str) -> String {
    rel.replace('/', "-")
}

fn collect_manifests(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()
        .with_context(|| format!("reading {}", dir.display()))?;
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_manifests(root, &path, out)?;
        } else if path.file_name().is_some_and(|name| name == "cabin.toml") {
            let rel = path
                .parent()
                .and_then(|parent| parent.strip_prefix(root).ok())
                .context("a manifest outside the repository")?;
            let components: Vec<_> = rel.iter().map(|part| part.to_string_lossy()).collect();
            let name = format!("{}.toml", components.join("-"));
            let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            out.push((name, bytes));
        }
    }
    Ok(())
}

fn write_seeds(dir: &Path, seeds: &[(String, Vec<u8>)]) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    for (name, bytes) in seeds {
        let path = dir.join(name);
        fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}
