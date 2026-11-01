//! The release archive: the "Package binary" step of
//! `.github/workflows/dist.yml`.
//!
//! - **Every step is fatal.** A failing copy or archiver ends the step
//!   then and there, and no path is printed.
//! - **Unset is not empty.** The ref name and the SHA are required
//!   arguments. A set-but-empty one is a value, and is spliced in
//!   as-is: an empty ref name on a tag build names the package
//!   `cabin--<target>`. An absent ref type reads as the empty string,
//!   which is not `tag`.
//! - **The version is the ref name only for a tag.** The ref type is
//!   compared exactly against `tag`, so every other ref type - and the
//!   empty one - is versioned `dev-<sha[..12]>`.
//! - **A short SHA never errors.** The slice takes what there is, so a
//!   4-character SHA yields `dev-` plus those four and an empty one
//!   yields `dev-`.
//! - **The binary and the documents are two separate copies.** A
//!   missing binary ends the step before `README.md` and `LICENSE` are
//!   looked at, while the documents' copy attempts *both* of its
//!   sources and only then fails - a run that dies on a missing
//!   `README.md` still leaves `LICENSE` staged. Each copy is named
//!   after the source's last component.
//! - **An existing package directory is accepted**, and missing parents
//!   are created.
//! - **Single quotes are the only quoting around the package name in
//!   the `Compress-Archive` command string,** so a version carrying a
//!   `'` would break out of it. A tag name with a character outside
//!   `[A-Za-z0-9._+-]` is therefore refused before anything is staged.
//!
//! Stated ceilings:
//!
//! - **The archivers stay child processes.** `crates/cabin/Cargo.toml`
//!   declares `pkg-fmt = "txz"`, so these archive names and formats are
//!   a cargo-binstall contract, and `AGENTS.md` puts binstall behavior
//!   off limits to unrelated work. Compressing in Rust instead would
//!   change the published bytes for no gain - the same reasoning that
//!   keeps `gh`, `git` and `jq` as child processes in
//!   `xtask-workflow-guard`.
//! - **The runner's OS is `cfg!(windows)`.** It decides the binary's
//!   suffix and zip-versus-tar. Every Windows target in the matrix runs
//!   on a Windows runner and every other target does not, so on each
//!   row the host this tool compiles and runs on is Windows exactly
//!   when the target is. The branch is threaded as an argument
//!   internally so both shapes stay testable from one host.
//! - **The workflow records the path.** Writing `$GITHUB_ENV` is
//!   `xtask-workflow-guard`'s reserved surface, so this prints the
//!   package path - the whole of stdout, one line - and the step wraps
//!   it into the `PACKAGE_PATH=` assignment.
//! - **Exit statuses collapse to 1.** The failing tool's own code is
//!   not propagated, and nothing downstream reads more than pass or
//!   fail.
//! - **Permission bits.** [`std::fs::copy`] copies the source's mode,
//!   so a 0755 binary stays executable, and the executable bit is what
//!   the archive has to carry.

use std::io;
use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

const BINARY: &str = "cabin";
const BINARY_WINDOWS: &str = "cabin.exe";

const DOCUMENTS: [&str; 2] = ["README.md", "LICENSE"];

const SHORT_SHA: usize = 12;

/// Stage the release archive for one matrix row and print its path.
///
/// # Errors
///
/// A source that could not be staged, or an archiver that failed to
/// run or ran and failed.
pub fn run(arguments: &Arguments) -> Result<()> {
    let path = package(Path::new(""), arguments, cfg!(windows), &mut spawn)?;
    println!("{path}");
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub struct Arguments {
    pub target: String,
    pub ref_name: String,
    /// `${GITHUB_REF_TYPE:-}`: absent reads as the empty string.
    pub ref_type: String,
    pub sha: String,
}

/// Computed before the filesystem is touched.
#[derive(Debug)]
struct Plan {
    package: String,
    /// What the step prints.
    package_path: String,
    binary: String,
    archiver: Vec<String>,
}

/// `root` is the directory the relative paths resolve against: empty
/// in production, where that is the process's working directory, and
/// the fixture root under test. The archiver's argv is relative to it
/// too, which is why the tests script `archive` rather than spawning
/// one.
fn package(
    root: &Path,
    arguments: &Arguments,
    windows: bool,
    archive: &mut dyn FnMut(&[String]) -> io::Result<bool>,
) -> Result<String> {
    // The Windows archiver splices the tag name into a PowerShell
    // command string, so a tag like `1.2.3';<cmd>;'` would run `<cmd>`
    // on the Windows legs; a version never needs more than these
    // characters.
    if arguments.ref_type == "tag"
        && !arguments
            .ref_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
    {
        bail!(
            "refusing tag name with characters outside [A-Za-z0-9._+-]: {:?}",
            arguments.ref_name
        );
    }
    let plan = plan(arguments, windows);

    let directory = root.join(&plan.package);
    if let Err(error) = std::fs::create_dir_all(&directory) {
        bail!("cannot create {}: {error}", directory.display());
    }
    copy_into(root, &directory, &[plan.binary.as_str()])?;
    copy_into(root, &directory, &DOCUMENTS)?;

    match archive(&plan.archiver) {
        Ok(true) => Ok(plan.package_path),
        Ok(false) => bail!("{} failed to write {}", plan.archiver[0], plan.package_path),
        Err(error) => bail!("{}: {error}", plan.archiver[0]),
    }
}

fn plan(arguments: &Arguments, windows: bool) -> Plan {
    let target = &arguments.target;
    let version = version(&arguments.ref_name, &arguments.ref_type, &arguments.sha);
    let package = format!("cabin-{version}-{target}");
    let binary = format!(
        "target/{target}/release/{}",
        if windows { BINARY_WINDOWS } else { BINARY }
    );
    let package_path = if windows {
        format!("{package}.zip")
    } else {
        format!("{package}.tar.xz")
    };
    let archiver = if windows {
        vec![
            "powershell".to_owned(),
            "-NoProfile".to_owned(),
            "-Command".to_owned(),
            format!("Compress-Archive -Path '{package}' -DestinationPath '{package_path}' -Force"),
        ]
    } else {
        vec![
            "tar".to_owned(),
            "-cJf".to_owned(),
            package_path.clone(),
            package.clone(),
        ]
    };
    Plan {
        package,
        package_path,
        binary,
        archiver,
    }
}

fn version(ref_name: &str, ref_type: &str, sha: &str) -> String {
    if ref_type == "tag" {
        return ref_name.to_owned();
    }
    let short: String = sha.chars().take(SHORT_SHA).collect();
    format!("dev-{short}")
}

/// Every source is attempted, each failure is reported, and the copy
/// fails once if any did.
fn copy_into(root: &Path, directory: &Path, sources: &[&str]) -> Result<()> {
    let failures: Vec<String> = sources
        .iter()
        .filter_map(|source| {
            let name = Path::new(source).file_name().unwrap_or_default();
            let error = std::fs::copy(root.join(source), directory.join(name)).err()?;
            Some(format!("{source}: {error}"))
        })
        .collect();
    if failures.is_empty() {
        return Ok(());
    }
    bail!(
        "cannot stage into {}: {}",
        directory.display(),
        failures.join("; ")
    );
}

/// The production archiver: a real child process, found on `PATH`,
/// with its output inherited.
fn spawn(argv: &[String]) -> io::Result<bool> {
    Command::new(&argv[0])
        .args(&argv[1..])
        .status()
        .map(|status| status.success())
}

#[cfg(test)]
mod tests {
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    use super::*;

    fn arguments(target: &str, ref_name: &str, ref_type: &str, sha: &str) -> Arguments {
        Arguments {
            target: target.to_owned(),
            ref_name: ref_name.to_owned(),
            ref_type: ref_type.to_owned(),
            sha: sha.to_owned(),
        }
    }

    /// A checkout with everything the step copies, for `windows`'s
    /// binary name.
    fn checkout(windows: bool) -> TempDir {
        let root = TempDir::new().expect("a temporary directory");
        let binary = if windows { BINARY_WINDOWS } else { BINARY };
        root.child("target/triple/release")
            .create_dir_all()
            .expect("the build directory");
        root.child(format!("target/triple/release/{binary}"))
            .write_str("binary")
            .expect("the staged binary");
        root.child("README.md").write_str("readme").expect("README");
        root.child("LICENSE").write_str("license").expect("LICENSE");
        root
    }

    /// A scripted archiver: answers as told, and panics if run twice.
    fn archiver(answer: io::Result<bool>) -> impl FnMut(&[String]) -> io::Result<bool> {
        let mut answer = Some(answer);
        move |_| {
            answer
                .take()
                .unwrap_or_else(|| panic!("the archiver ran more than once"))
        }
    }

    #[test]
    fn a_tag_is_versioned_by_its_ref_name() {
        assert_eq!(version("0.17.0", "tag", "0123456789abcdef"), "0.17.0");
        // An empty ref name is a value, spliced in as-is.
        assert_eq!(version("", "tag", "0123456789abcdef"), "");
    }

    #[test]
    fn every_other_ref_type_is_versioned_by_the_short_sha() {
        // The empty default, a branch, and ref types that only look
        // like `tag`.
        for ref_type in ["", "branch", "Tag", "tags"] {
            assert_eq!(
                version("main", ref_type, "0123456789abcdef"),
                "dev-0123456789ab",
                "{ref_type}"
            );
        }
    }

    #[test]
    fn a_short_sha_is_taken_as_far_as_it_goes() {
        // The slice never errors on a value shorter than twelve
        // characters, and an empty SHA is a value too.
        assert_eq!(version("main", "branch", "0123"), "dev-0123");
        assert_eq!(version("main", "branch", ""), "dev-");
        assert_eq!(
            version("main", "branch", "0123456789ab"),
            "dev-0123456789ab"
        );
    }

    #[test]
    fn the_package_is_named_for_the_version_and_the_target() {
        let plan = plan(
            &arguments("aarch64-apple-darwin", "0.17.0", "tag", "abc"),
            false,
        );
        assert_eq!(plan.package, "cabin-0.17.0-aarch64-apple-darwin");
        assert_eq!(
            plan.package_path,
            "cabin-0.17.0-aarch64-apple-darwin.tar.xz"
        );
        assert_eq!(plan.binary, "target/aarch64-apple-darwin/release/cabin");
    }

    #[test]
    fn a_windows_row_packages_the_exe_into_a_zip() {
        let plan = plan(
            &arguments(
                "x86_64-pc-windows-msvc",
                "main",
                "branch",
                "0123456789abcdef",
            ),
            true,
        );
        assert_eq!(
            plan.package,
            "cabin-dev-0123456789ab-x86_64-pc-windows-msvc"
        );
        assert_eq!(
            plan.package_path,
            "cabin-dev-0123456789ab-x86_64-pc-windows-msvc.zip"
        );
        assert_eq!(
            plan.binary,
            "target/x86_64-pc-windows-msvc/release/cabin.exe"
        );
    }

    #[test]
    fn the_archivers_take_the_exact_argv() {
        let unix = plan(&arguments("triple", "0.17.0", "tag", "abc"), false);
        assert_eq!(
            unix.archiver,
            [
                "tar",
                "-cJf",
                "cabin-0.17.0-triple.tar.xz",
                "cabin-0.17.0-triple"
            ]
        );

        let windows = plan(&arguments("triple", "0.17.0", "tag", "abc"), true);
        assert_eq!(
            windows.archiver,
            [
                "powershell",
                "-NoProfile",
                "-Command",
                "Compress-Archive -Path 'cabin-0.17.0-triple' -DestinationPath 'cabin-0.17.0-triple.zip' -Force",
            ]
        );
    }

    #[test]
    fn staging_copies_the_binary_and_both_documents() {
        for windows in [false, true] {
            let root = checkout(windows);
            let path = package(
                root.path(),
                &arguments("triple", "0.17.0", "tag", "abc"),
                windows,
                &mut archiver(Ok(true)),
            )
            .expect("a staged package");

            let suffix = if windows { "zip" } else { "tar.xz" };
            assert_eq!(path, format!("cabin-0.17.0-triple.{suffix}"));

            let staged = root.child("cabin-0.17.0-triple");
            staged
                .child(if windows { BINARY_WINDOWS } else { BINARY })
                .assert("binary");
            staged.child("README.md").assert("readme");
            staged.child("LICENSE").assert("license");
        }
    }

    #[test]
    fn a_tag_name_that_could_escape_the_powershell_literal_is_refused() {
        for windows in [false, true] {
            let root = checkout(windows);
            let error = package(
                root.path(),
                &arguments("triple", "1.2.3';calc;'", "tag", "abc"),
                windows,
                &mut |_: &[String]| panic!("the archiver ran"),
            )
            .expect_err("a refused tag name");
            assert!(error.to_string().contains("1.2.3';calc;'"), "{error}");
            assert!(!root.path().join("cabin-1.2.3';calc;'-triple").exists());
        }
        // Only a tag is a version: a branch of the same spelling is
        // versioned by the SHA and never reaches the archiver's argv.
        let root = checkout(false);
        package(
            root.path(),
            &arguments("triple", "1.2.3';calc;'", "branch", "abc"),
            false,
            &mut archiver(Ok(true)),
        )
        .expect("a branch build");
    }

    #[test]
    fn an_existing_package_directory_is_not_an_error() {
        let root = checkout(false);
        root.child("cabin-0.17.0-triple/stale")
            .write_str("left over")
            .expect("a stale package directory");
        package(
            root.path(),
            &arguments("triple", "0.17.0", "tag", "abc"),
            false,
            &mut archiver(Ok(true)),
        )
        .expect("a staged package");
    }

    #[test]
    fn a_missing_binary_fails_before_the_documents_are_touched() {
        let root = TempDir::new().expect("a temporary directory");
        root.child("README.md").write_str("readme").expect("README");
        root.child("LICENSE").write_str("license").expect("LICENSE");
        let error = package(
            root.path(),
            &arguments("triple", "0.17.0", "tag", "abc"),
            false,
            &mut archiver(Ok(true)),
        )
        .expect_err("the missing binary");
        assert!(
            error.to_string().contains("target/triple/release/cabin"),
            "{error}"
        );
        // The binary is copied on its own, so the documents never were.
        assert!(!root.child("cabin-0.17.0-triple/README.md").path().exists());
        assert!(!root.child("cabin-0.17.0-triple/LICENSE").path().exists());
    }

    #[test]
    fn a_missing_readme_still_stages_the_license() {
        // The documents are one copy over both sources: it reports the
        // missing one and copies the other before failing.
        let root = checkout(false);
        std::fs::remove_file(root.child("README.md").path()).expect("removing README");
        let error = package(
            root.path(),
            &arguments("triple", "0.17.0", "tag", "abc"),
            false,
            &mut archiver(Ok(true)),
        )
        .expect_err("the missing README");
        assert!(error.to_string().contains("README.md"), "{error}");
        root.child("cabin-0.17.0-triple/LICENSE").assert("license");
    }

    #[test]
    fn a_missing_license_is_reported_too() {
        let root = checkout(false);
        std::fs::remove_file(root.child("LICENSE").path()).expect("removing LICENSE");
        let error = package(
            root.path(),
            &arguments("triple", "0.17.0", "tag", "abc"),
            false,
            &mut archiver(Ok(true)),
        )
        .expect_err("the missing LICENSE");
        assert!(error.to_string().contains("LICENSE"), "{error}");
        root.child("cabin-0.17.0-triple/README.md").assert("readme");
    }

    #[test]
    fn a_failing_archiver_fails_the_step() {
        let root = checkout(false);
        let error = package(
            root.path(),
            &arguments("triple", "0.17.0", "tag", "abc"),
            false,
            &mut archiver(Ok(false)),
        )
        .expect_err("the failing archiver");
        assert!(
            error.to_string().contains("cabin-0.17.0-triple.tar.xz"),
            "{error}"
        );
    }

    #[test]
    fn an_archiver_that_cannot_be_spawned_fails_the_step() {
        let root = checkout(false);
        let missing = io::Error::new(io::ErrorKind::NotFound, "no such file");
        let error = package(
            root.path(),
            &arguments("triple", "0.17.0", "tag", "abc"),
            false,
            &mut archiver(Err(missing)),
        )
        .expect_err("the missing archiver");
        assert!(error.to_string().contains("tar"), "{error}");
    }
}
