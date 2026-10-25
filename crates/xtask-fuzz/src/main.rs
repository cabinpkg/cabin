//! Fuzz-target maintenance (`fuzz/README.md`).
//!
//!   `cargo fuzz-seeds`  regenerate `fuzz/seeds/` from in-tree packages
//!                       and manifests
//!   `cargo fuzz-check`  lint and test the fuzz workspace and replay
//!                       every target's seeds

use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, ensure};
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

/// The top directory the extraction seeds nest every file under, as
/// an upstream release archive does; the harness's `SEED_PREFIX`,
/// whose test checks the seeds against it.
const SEED_PREFIX: &str = "pkg";

/// Trees whose every `cabin.toml` seeds the manifest parser.
const MANIFEST_TREES: &[&str] = &["examples", "ports"];

#[derive(Parser)]
enum Cli {
    /// Regenerate the seed corpora under `fuzz/seeds/`.
    Seeds,
    /// Lint and test `fuzz/` and replay every target's seeds (needs
    /// `cargo-fuzz`).
    Check,
}

fn main() -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let result = match Cli::parse() {
        Cli::Seeds => seeds(&root),
        Cli::Check => check(&root.join("fuzz")),
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

    let mut zips = Vec::new();
    let mut tars = Vec::new();
    for (name, archive) in &archives {
        let files = archive_files(archive).with_context(|| format!("reading {name}"))?;
        let stem = name.trim_end_matches(".zip");
        zips.push((name.clone(), prefixed_zip(&files)?));
        tars.push((format!("{stem}.tar"), prefixed_tar(&files)?));
    }
    write_seeds(&out.join("extract_zip"), &zips)?;
    write_seeds(&out.join("extract_tar_gz"), &tars)?;

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

/// Every file of a package archive, in archive order.
fn archive_files(archive: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))?;
    let mut files = Vec::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let name = String::from_utf8(entry.name_raw().to_vec())?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        files.push((name, bytes));
    }
    Ok(files)
}

/// `files` under [`SEED_PREFIX`] in a zip, behind the prefix's own
/// directory entry.
fn prefixed_zip(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    zip.add_directory(format!("{SEED_PREFIX}/"), options)?;
    for (name, bytes) in files {
        zip.start_file(format!("{SEED_PREFIX}/{name}"), options)?;
        zip.write_all(bytes)?;
    }
    Ok(zip.finish()?.into_inner())
}

/// `files` under [`SEED_PREFIX`] in a tar, behind the prefix's own
/// directory entry, with no timestamps or owners.
fn prefixed_tar(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut tar = tar::Builder::new(Vec::new());
    let mut dir = tar::Header::new_ustar();
    dir.set_entry_type(tar::EntryType::Directory);
    dir.set_mode(0o755);
    dir.set_size(0);
    tar.append_data(&mut dir, format!("{SEED_PREFIX}/"), io::empty())?;
    for (name, bytes) in files {
        let mut header = tar::Header::new_ustar();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o644);
        header.set_size(bytes.len() as u64);
        tar.append_data(
            &mut header,
            format!("{SEED_PREFIX}/{name}"),
            bytes.as_slice(),
        )?;
    }
    Ok(tar.into_inner()?)
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

fn check(fuzz: &Path) -> Result<()> {
    cargo(fuzz, &["fmt", "--check"])?;
    cargo(
        fuzz,
        &[
            "clippy",
            "--locked",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    cargo(fuzz, &["test", "--locked"])?;

    let listed = fuzz_cargo(fuzz)
        .args(["fuzz", "list"])
        .output()
        .context("running `cargo fuzz list`")?;
    ensure!(listed.status.success(), "`cargo fuzz list` failed");
    let listed = String::from_utf8(listed.stdout).context("`cargo fuzz list` printed non-UTF-8")?;
    // A target without seeds or a dictionary fails here: libFuzzer
    // refuses a missing corpus directory or dictionary file.
    for target in listed.lines() {
        let corpus = corpus(target);
        let seeds = format!("seeds/{corpus}");
        let dict = format!("-dict=dicts/{corpus}.dict");
        cargo(
            fuzz,
            &["fuzz", "run", target, &seeds, "--", "-runs=0", &dict],
        )?;
    }
    Ok(())
}

/// The seeds and dictionary `target` runs on.
fn corpus(target: &str) -> &str {
    match target {
        "inspect_zip_wrong_digest" => "inspect_zip",
        target => target,
    }
}

/// `cargo` in `fuzz/`. The rustup proxy that ran this xtask exports
/// its own toolchain, which would override `fuzz/rust-toolchain.toml`.
fn fuzz_cargo(fuzz: &Path) -> Command {
    let mut cargo = Command::new("cargo");
    cargo.current_dir(fuzz).env_remove("RUSTUP_TOOLCHAIN");
    cargo
}

fn cargo(fuzz: &Path, args: &[&str]) -> Result<()> {
    let status = fuzz_cargo(fuzz)
        .args(args)
        .status()
        .with_context(|| format!("running `cargo {}`", args.join(" ")))?;
    ensure!(status.success(), "`cargo {}` failed", args.join(" "));
    Ok(())
}
