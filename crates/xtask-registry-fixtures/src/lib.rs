//! Publish-conformance fixtures generated with the IN-TREE `cabin`
//! binary: real `cabin package` archive + canonical-metadata pairs, so
//! the registry's publish validation is tested against exactly what the
//! client uploads and the two sides can never silently drift.
//!
//! The frozen pair under `registry/tests/fixtures/` is a checked-in copy
//! of the `withdep` output; regenerate it here if the canonical metadata
//! format changes intentionally.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// A fixture package: a manifest and the single source file it names.
struct Package {
    /// Also the directory it is authored in.
    name: &'static str,
    manifest: &'static str,
    source: &'static str,
    code: &'static str,
}

/// Scoped names, so the produced filenames carry the flattened
/// `<scope>-<name>` stem: `smoke-nodep-0.1.0.zip` and its `.json`.
const PACKAGES: &[Package] = &[
    Package {
        name: "nodep",
        manifest: "\
[package]
name = \"smoke/nodep\"
version = \"0.1.0\"
c-standard = \"c11\"

[target.nodep]
type = \"library\"
sources = [\"src/nodep.c\"]
",
        source: "src/nodep.c",
        code: "int nodep(void) { return 0; }\n",
    },
    Package {
        name: "withdep",
        manifest: "\
[package]
name = \"smoke/withdep\"
version = \"0.2.0\"
cxx-standard = \"c++20\"

[dependencies]
\"smoke/nodep\" = \"^0.1\"

[target.withdep]
type = \"library\"
sources = [\"src/withdep.cc\"]
interface-cxx-standard = \"c++17\"
links = \"withdep-native\"
",
        source: "src/withdep.cc",
        code: "void withdep() {}\n",
    },
    Package {
        name: "withupstream",
        manifest: "\
[package]
name = \"smoke/withupstream\"
version = \"0.3.0\"
c-standard = \"c11\"

[package.upstream]
url = \"https://example.com/withupstream-0.3.0.tar.gz\"
checksum = \"sha256:9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23\"
format = \"tar.gz\"
strip-prefix = \"withupstream-0.3.0\"
patches = [\"patches/0001-fix.patch\"]

[[package.upstream.copy]]
from = \"scripts/config.h.prebuilt\"
to = \"config.h\"

[target.withupstream]
type = \"library\"
sources = [\"src/withupstream.c\"]
",
        source: "src/withupstream.c",
        code: "int withupstream(void) { return 0; }\n",
    },
];

/// The declared patch must exist in the tree: `cabin package` refuses to
/// stage a manifest whose declared patch file is absent, and the
/// conformance leg proves the Worker accepts patches-bearing metadata.
const UPSTREAM_PATCH: &str = "\
--- a/src/withupstream.c
+++ b/src/withupstream.c
@@ -1,1 +1,1 @@
-int withupstream(void) { return 1; }
+int withupstream(void) { return 0; }
";

/// The repository this tool was built from.
///
/// Resolved from the crate's own manifest directory rather than the
/// working directory: the Cargo aliases are run from the repository
/// root, but nothing here should depend on that.
#[must_use]
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Build `cabin`, author the fixture packages, and package them into
/// `out`.
///
/// # Errors
///
/// If `out` cannot be created, if `cargo build` or `cabin package`
/// fails, or if the authored sources cannot be written.
pub fn generate(out: &Path) -> Result<()> {
    let root = repo_root();
    std::fs::create_dir_all(out).with_context(|| format!("create {}", out.display()))?;

    step("building the in-tree cabin binary");
    run(Command::new("cargo")
        .args(["build", "--locked", "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .args(["--bin", "cabin"]))?;
    let cabin = root
        .join("target/debug")
        .join(format!("cabin{}", std::env::consts::EXE_SUFFIX));

    let sources = tempfile::tempdir().context("create a scratch directory")?;
    step("authoring the fixture packages");
    author(sources.path())?;

    for package in PACKAGES {
        step(&format!("packaging {}", package.name));
        run(Command::new(&cabin)
            .arg("package")
            .arg("--manifest-path")
            .arg(sources.path().join(package.name).join("cabin.toml"))
            .arg("--output-dir")
            .arg(out))?;
    }

    step(&format!("fixtures written to {}", out.display()));
    list(out)
}

/// Write the fixture packages under `sources`, one directory each.
///
/// # Errors
///
/// If any fixture file cannot be written.
fn author(sources: &Path) -> Result<()> {
    for package in PACKAGES {
        let directory = sources.join(package.name);
        write(&directory.join("cabin.toml"), package.manifest)?;
        write(&directory.join(package.source), package.code)?;
    }
    write(
        &sources.join("withupstream/patches/0001-fix.patch"),
        UPSTREAM_PATCH,
    )
}

fn write(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("write {}", path.display()))
}

fn step(message: &str) {
    println!("==> {message}");
}

fn run(command: &mut Command) -> Result<()> {
    let program = command.get_program().to_owned();
    let status = command
        .status()
        .with_context(|| format!("run {}", program.to_string_lossy()))?;
    if !status.success() {
        bail!("{} failed: {status}", program.to_string_lossy());
    }
    Ok(())
}

/// The fixtures actually written, sorted so the listing does not depend
/// on directory order.
fn list(out: &Path) -> Result<()> {
    let mut written = Vec::new();
    for entry in std::fs::read_dir(out).with_context(|| format!("read {}", out.display()))? {
        let entry = entry.with_context(|| format!("read {}", out.display()))?;
        let size = entry
            .metadata()
            .with_context(|| format!("stat {}", entry.path().display()))?
            .len();
        written.push((entry.file_name().to_string_lossy().into_owned(), size));
    }
    written.sort();
    for (name, size) in written {
        println!("{size:>9}  {name}");
    }
    Ok(())
}
