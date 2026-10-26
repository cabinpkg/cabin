//! Fuzz-target maintenance (`fuzz/README.md`).
//!
//!   `cargo fuzz-seeds`     regenerate `fuzz/seeds/` from in-tree
//!                          packages and manifests
//!   `cargo fuzz-check`     lint and test the fuzz workspace and replay
//!                          every target's seeds
//!   `cargo fuzz-campaign`  fuzz every target on its kept corpus, then
//!                          minimize that corpus

use std::fs::{self, File};
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

/// How long `cargo fuzz-campaign` fuzzes each target.
const CAMPAIGN_SECONDS: u32 = 300;

/// The harness's `PATCH_TREE` (`fuzz/src/lib.rs`), which the seed
/// patches are diffs of; the harness's test checks that they apply.
const PATCH_TREE: &[(&str, &str)] = &[
    (
        "cabin.toml",
        "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n",
    ),
    ("include/demo.h", "#pragma once\nint demo(void);"),
    (
        "src/demo.c",
        "#include \"demo.h\"\n\nint demo(void) {\n    return 0;\n}\n",
    ),
    (
        "src/main.cpp",
        "#include \"demo.h\"\n\nint main() {\n    return demo();\n}\n",
    ),
];

/// A path under [`PATCH_TREE`]'s root and its new contents, `None` deleting it.
type Edit = (&'static str, Option<&'static str>);

/// Each patch seed's edits to [`PATCH_TREE`].  The seed files carry no
/// extension: global gitignores commonly ignore `*.patch` and `*.diff`.
const PATCH_SEEDS: &[(&str, &[Edit])] = &[
    (
        "modify-c",
        &[(
            "src/demo.c",
            Some("#include \"demo.h\"\n\nint demo(void) {\n    return 1;\n}\n"),
        )],
    ),
    (
        "modify-no-newline",
        &[(
            "include/demo.h",
            Some("#pragma once\nint demo(void);\nint other(void);"),
        )],
    ),
    (
        "add-newline",
        &[("include/demo.h", Some("#pragma once\nint demo(void);\n"))],
    ),
    (
        "create",
        &[("src/extra.c", Some("int extra(void) { return 2; }\n"))],
    ),
    // git quotes a non-ASCII path, as a C string literal.
    (
        "create-quoted",
        &[("src/caf\u{e9}.c", Some("int cafe(void) { return 3; }\n"))],
    ),
    ("delete", &[("src/main.cpp", None)]),
    (
        "two-files",
        &[
            ("src/demo.c", Some("int demo(void) {\n    return 0;\n}\n")),
            (
                "src/main.cpp",
                Some("#include \"demo.h\"\n\nint main() {\n    return demo() + 1;\n}\n"),
            ),
        ],
    ),
];

#[derive(Parser)]
enum Cli {
    /// Regenerate the seed corpora under `fuzz/seeds/`.
    Seeds,
    /// Lint and test `fuzz/` and replay every target's seeds (needs
    /// `cargo-fuzz`).
    Check,
    /// Fuzz every target on `fuzz/corpus/` and its seeds, then minimize
    /// that corpus (needs `cargo-fuzz`).
    Campaign,
}

fn main() -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let result = match Cli::parse() {
        Cli::Seeds => seeds(&root),
        Cli::Check => check(&root.join("fuzz")),
        Cli::Campaign => campaign(&root.join("fuzz")),
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
    let mut specs = Vec::new();
    for dir in SEED_PACKAGES {
        specs.push((seed_name(dir), package_spec(&root.join(dir))?));
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

    write_seeds(&out.join("apply_patch"), &patch_seeds()?)?;
    write_seeds(&out.join("package_round_trip"), &specs)?;

    let mut manifests = Vec::new();
    for tree in MANIFEST_TREES {
        collect_manifests(root, &root.join(tree), &mut manifests)?;
    }
    write_seeds(&out.join("parse_manifest"), &manifests)
}

/// The package in `dir` as the round-trip target reads one: its
/// `cabin.toml`, then each other file the packager collects as a NUL,
/// its path, a newline, and its contents.
fn package_spec(dir: &Path) -> Result<Vec<u8>> {
    let files = cabin_package::archive::collect_package_files(dir, None)
        .with_context(|| format!("listing {}", dir.display()))?;
    let manifest = dir.join("cabin.toml");
    let mut spec =
        fs::read(&manifest).with_context(|| format!("reading {}", manifest.display()))?;
    for file in files.iter().filter(|file| file.rel_path != "cabin.toml") {
        let bytes = fs::read(&file.abs_path)
            .with_context(|| format!("reading {}", file.abs_path.display()))?;
        ensure!(
            !bytes.contains(&0),
            "{} holds a NUL byte",
            file.abs_path.display()
        );
        spec.push(0);
        spec.extend(file.rel_path.as_bytes());
        spec.push(b'\n');
        spec.extend(bytes);
    }
    Ok(spec)
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

/// [`PATCH_SEEDS`] as `git diff` writes them, the producer upstream
/// patches come from, plus the first without git's own header lines,
/// the shape a plain `diff -u` writes.
fn patch_seeds() -> Result<Vec<(String, Vec<u8>)>> {
    let mut seeds = Vec::new();
    for (name, edits) in PATCH_SEEDS {
        let repo = tempfile::tempdir().context("creating a scratch directory")?;
        let repo = repo.path();
        for (path, text) in PATCH_TREE {
            let path = repo.join(path);
            fs::create_dir_all(path.parent().context("a tree path has a parent")?)?;
            fs::write(path, text)?;
        }
        git(repo, &["init", "-q", "--object-format=sha1"])?;
        git(repo, &["add", "."])?;
        for (path, text) in *edits {
            match text {
                Some(text) => fs::write(repo.join(path), text)?,
                None => fs::remove_file(repo.join(path))?,
            }
            // Intent to add, so the diff shows a created file.
            if PATCH_TREE.iter().all(|(tree_path, _)| tree_path != path) {
                git(repo, &["add", "-N", path])?;
            }
        }
        let diff = git(repo, &["diff", "--full-index"])?;
        ensure!(!diff.is_empty(), "the {name} patch seed is empty");
        seeds.push(((*name).to_owned(), diff));
    }
    let plain = seeds[0]
        .1
        .split_inclusive(|byte| *byte == b'\n')
        .filter(|line| !line.starts_with(b"diff --git ") && !line.starts_with(b"index "))
        .flatten()
        .copied()
        .collect();
    seeds.push(("plain-modify-c".to_owned(), plain));
    Ok(seeds)
}

/// `git` in `repo`, without the user's or the system's config, which
/// changes what `git diff` writes; its stdout.
fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    // No inherited `GIT_*` variable, config, or attributes file shapes the seed bytes.
    let output = Command::new("git")
        .current_dir(repo)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .args(args)
        .output()
        .with_context(|| format!("running `git {}`", args.join(" ")))?;
    ensure!(output.status.success(), "`git {}` failed", args.join(" "));
    Ok(output.stdout)
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

    // A target without seeds or a dictionary fails here: libFuzzer
    // refuses a missing corpus directory or dictionary file.
    for target in targets(fuzz)? {
        let corpus = corpus(&target);
        let seeds = format!("seeds/{corpus}");
        let dict = format!("-dict=dicts/{corpus}.dict");
        cargo(
            fuzz,
            &["fuzz", "run", &target, &seeds, "--", "-runs=0", &dict],
        )?;
    }
    Ok(())
}

/// Fuzzes every target on `corpus/<target>`, which keeps what it
/// finds, and its seeds, then minimizes that corpus.  A target's
/// output can quote the input that crashed it (`README.md`,
/// "Crashes"), so it goes to a scratch log: the console gets
/// libFuzzer's numeric final stats, or the name of a target that
/// failed.  A failed target's corpus is dropped too, since it can hold
/// that input and the workflow caches the corpus where any pull
/// request can read it.
fn campaign(fuzz: &Path) -> Result<()> {
    cargo(fuzz, &["fuzz", "build"])?;
    let logs = tempfile::tempdir().context("creating a scratch directory")?;
    let max_time = format!("-max_total_time={CAMPAIGN_SECONDS}");
    let mut failed = Vec::new();
    for target in targets(fuzz)? {
        let kept = format!("corpus/{target}");
        fs::create_dir_all(fuzz.join(&kept)).with_context(|| format!("creating {kept}"))?;
        let corpus = corpus(&target);
        let seeds = format!("seeds/{corpus}");
        let dict = format!("-dict=dicts/{corpus}.dict");
        let run = [
            "fuzz",
            "run",
            &target,
            &kept,
            &seeds,
            "--",
            &dict,
            &max_time,
            "-timeout=60",
            "-print_final_stats=1",
        ];
        // `cargo fuzz cmin` exits 0 when its merge fails, so the merge
        // runs here, into a directory that replaces the corpus.
        let merged = tempfile::tempdir_in(fuzz).context("creating a scratch directory")?;
        let merged_dir = merged
            .path()
            .to_str()
            .context("the fuzz path is not UTF-8")?;
        let merge = ["fuzz", "run", &target, merged_dir, &kept, "--", "-merge=1"];
        // Stats come from the run's own log: a run that exits 0 dumped no
        // input, but a merge carries on past a crash it has dumped.
        let log = logs.path().join(format!("{target}.log"));
        let merge_log = logs.path().join(format!("{target}.merge.log"));
        let passed = quiet_cargo(fuzz, &run, &log)? && quiet_cargo(fuzz, &merge, &merge_log)?;
        fs::remove_dir_all(fuzz.join(&kept)).with_context(|| format!("removing {kept}"))?;
        if passed {
            fs::rename(merged.keep(), fuzz.join(&kept))
                .with_context(|| format!("replacing {kept}"))?;
            let log = fs::read_to_string(&log).context("reading a fuzz log")?;
            for stat in log.lines().filter(|line| line.starts_with("stat::")) {
                println!("{target}: {stat}");
            }
        } else {
            println!("{target}: failed (output withheld)");
            failed.push(target);
        }
    }
    ensure!(
        failed.is_empty(),
        "{} failed; fuzz each locally to reproduce (fuzz/README.md)",
        failed.join(", ")
    );
    Ok(())
}

/// The targets `cargo fuzz list` names.
fn targets(fuzz: &Path) -> Result<Vec<String>> {
    let listed = fuzz_cargo(fuzz)
        .args(["fuzz", "list"])
        .output()
        .context("running `cargo fuzz list`")?;
    ensure!(listed.status.success(), "`cargo fuzz list` failed");
    let listed = String::from_utf8(listed.stdout).context("`cargo fuzz list` printed non-UTF-8")?;
    Ok(listed.lines().map(str::to_owned).collect())
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

/// `cargo` with its output in `log`; whether it succeeded.
fn quiet_cargo(fuzz: &Path, args: &[&str], log: &Path) -> Result<bool> {
    let log = File::create(log).context("creating a fuzz log")?;
    let status = fuzz_cargo(fuzz)
        .args(args)
        .stdout(log.try_clone().context("opening a fuzz log")?)
        .stderr(log)
        .status()
        .with_context(|| format!("running `cargo {}`", args.join(" ")))?;
    Ok(status.success())
}

fn cargo(fuzz: &Path, args: &[&str]) -> Result<()> {
    let status = fuzz_cargo(fuzz)
        .args(args)
        .status()
        .with_context(|| format!("running `cargo {}`", args.join(" ")))?;
    ensure!(status.success(), "`cargo {}` failed", args.join(" "));
    Ok(())
}
