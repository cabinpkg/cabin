//! The release archive, ported one-to-one from the `run:` body of the
//! "Package binary" step of `.github/workflows/dist.yml`.
//!
//! ```text
//! L1  set -euo pipefail
//! L2  target="${{ matrix.target }}"
//! L3  version="${GITHUB_REF_NAME}"
//! L4  if [[ "${GITHUB_REF_TYPE:-}" != "tag" ]]; then
//! L5    version="dev-${GITHUB_SHA::12}"
//! L6  fi
//! L7  bin="cabin"
//! L8  if [[ "$RUNNER_OS" == "Windows" ]]; then
//! L9    bin="cabin.exe"
//! L10 fi
//! L11 package="cabin-${version}-${target}"
//! L12 mkdir -p "$package"
//! L13 cp "target/${target}/release/${bin}" "$package/"
//! L14 cp README.md LICENSE "$package/"
//! L15 if [[ "$RUNNER_OS" == "Windows" ]]; then
//! L16   package_path="${package}.zip"
//! L17   powershell -NoProfile -Command \
//! L18     "Compress-Archive -Path '${package}' -DestinationPath '${package_path}' -Force"
//! L19 else
//! L20   package_path="${package}.tar.xz"
//! L21   tar -cJf "${package_path}" "$package"
//! L22 fi
//! L23 echo "PACKAGE_PATH=${package_path}" >> "$GITHUB_ENV"
//! ```
//!
//! Inherited properties, preserved rather than fixed - a port is not a
//! place to change behavior. Unlike the guards ported before, this
//! block sets its own options at L1, so `-u` and `-o pipefail` are on
//! here as well as `-e`:
//!
//! - **`-e` makes every step fatal.** No substitution in the block sits
//!   in a condition, so a failing `cp` at L13 or L14, or a failing
//!   archiver at L18 or L21, ended the step then and there and L23
//!   never ran. `-o pipefail` never bites: the block has no pipeline.
//! - **`-u` separates unset from empty.** L3 and L5 read
//!   `${GITHUB_REF_NAME}` and `${GITHUB_SHA::12}` with no default, so an
//!   unset name or SHA killed the step before anything was staged -
//!   which is why both arrive here as required arguments. A
//!   set-but-empty one is a value, and is spliced in as-is: an empty
//!   ref name on a tag build names the package `cabin--<target>`. L4's
//!   `:-` conversely accepts an unset ref type as the empty string,
//!   which is not `tag`.
//! - **The version is the ref name only for a tag.** L4 compares
//!   against an unquoted literal, which `[[` does not glob, so the test
//!   is an exact string comparison and every other ref type - and the
//!   empty one - takes L5.
//! - **`${GITHUB_SHA::12}` never errors on a short value.** Substring
//!   expansion takes what there is, so a 4-character SHA yields
//!   `dev-` plus those four and an empty one yields `dev-`.
//! - **L13 and L14 are two separate `cp` invocations.** L13's failure
//!   ended the step before `README.md` and `LICENSE` were looked at,
//!   while L14 attempts *both* of its sources and only then exits 1 - a
//!   run that dies on a missing `README.md` still leaves `LICENSE`
//!   staged. Each names its copy after the source's last component,
//!   which is what the trailing `/` on the destination asks for.
//! - **L12 accepts an existing directory**, and creates missing parents.
//! - **L18's single quotes are the only quoting around `${package}`.**
//!   A version carrying a `'` breaks out of the `Compress-Archive`
//!   argument. The port builds the same string and passes the same
//!   argv; closing that would be a behavior change, not a port.
//!
//! Stated ceilings:
//!
//! - **The archivers stay child processes with the shell's exact
//!   argv.** `crates/cabin/Cargo.toml` declares `pkg-fmt = "txz"`, so
//!   these archive names and formats are a cargo-binstall contract, and
//!   `AGENTS.md` puts binstall behavior off limits to unrelated work.
//!   Compressing in Rust instead would change the published bytes for
//!   no gain - the same reasoning that keeps `gh`, `git` and `jq` as
//!   child processes in `xtask-workflow-guard`.
//! - **`$RUNNER_OS` becomes `cfg!(windows)`.** Its only uses are L8 and
//!   L15, the binary's suffix and zip-versus-tar. Every Windows target
//!   in the matrix runs on a Windows runner and every other target does
//!   not, so on each row the host this tool compiles and runs on is the
//!   `RUNNER_OS` the shell tested. The branch is threaded as an
//!   argument internally so both shapes stay testable from one host.
//! - **L23 is the workflow's again.** Writing `$GITHUB_ENV` is
//!   `xtask-workflow-guard`'s reserved surface, so this port prints the
//!   package path - the whole of stdout, one line - and the step wraps
//!   it back into the `PACKAGE_PATH=` assignment it always was.
//! - **Diagnostic wording.** The original's stderr came from `cp`,
//!   `tar`, `powershell` and `bash` itself; this port writes its own
//!   wording for the failures it detects, and lets the archiver's own
//!   stderr through inherited. The control flow is identical in each
//!   case, and the workflow reads the step's status.
//! - **Exit statuses collapse to 1.** The shell died with the failing
//!   tool's own code - 1 from `cp`, 1 or 2 from `tar`, 127 for a
//!   missing binary - and nothing downstream reads more than pass or
//!   fail.
//! - **The SHA is sliced by character, not by byte.** `bash` counts
//!   characters in a UTF-8 locale and bytes in the C locale; a SHA is
//!   ASCII hex, where the two agree.
//! - **Permission bits.** [`std::fs::copy`] copies the source's mode,
//!   where `cp` applies the process umask to a file it creates. Both
//!   leave a 0755 binary executable under the runners' 0022 umask, and
//!   the executable bit is what the archive has to carry.

use std::io;
use std::path::Path;
use std::process::Command;

use anyhow::{Result, anyhow, bail};

/// The command line the `cargo dist-package` alias reaches.
pub const USAGE: &str = "\
usage: xtask-dist package --target <TRIPLE> --ref-name <NAME> --sha <SHA>
                          [--ref-type <TYPE>]

Release packaging steps for .github/workflows/dist.yml, run from a
workflow step through their Cargo aliases.

commands:
  package          stage target/<TRIPLE>/release/cabin, README.md and
                   LICENSE into cabin-<VERSION>-<TRIPLE>/, archive that
                   directory, and print the archive's path to stdout
                   (`cargo dist-package`)

options:
  --target <TRIPLE>  the target triple the release binary was built for
  --ref-name <NAME>  $GITHUB_REF_NAME, the version for a tag build
  --ref-type <TYPE>  $GITHUB_REF_TYPE; anything but `tag` versions the
                     package `dev-<SHA[..12]>` (default: empty)
  --sha <SHA>        $GITHUB_SHA
  -h, --help         show this help
";

/// L7 and L9.
const BINARY: &str = "cabin";
const BINARY_WINDOWS: &str = "cabin.exe";

/// L14's sources, in the order that one `cp` took them.
const DOCUMENTS: [&str; 2] = ["README.md", "LICENSE"];

/// L5's `${GITHUB_SHA::12}`.
const SHORT_SHA: usize = 12;

/// Stage the release archive for one matrix row and print its path.
///
/// # Errors
///
/// Whatever ended the step under `set -e`: a source that could not be
/// staged, or an archiver that failed to run or ran and failed.
pub fn run(arguments: &[String]) -> Result<()> {
    let path = package(Path::new(""), &parse(arguments)?, cfg!(windows), &mut spawn)?;
    println!("{path}");
    Ok(())
}

/// The subcommand's arguments. Every value the original spliced from
/// the run's environment arrives here instead, because reading
/// `GITHUB_*` is `xtask-workflow-guard`'s reserved surface.
#[derive(Debug, PartialEq, Eq)]
struct Arguments {
    target: String,
    ref_name: String,
    ref_type: String,
    sha: String,
}

/// L2..L21, computed before the filesystem is touched.
#[derive(Debug)]
struct Plan {
    /// L11.
    package: String,
    /// L16 or L20 - what L23 recorded and what this port prints.
    package_path: String,
    /// L13's source.
    binary: String,
    /// L17/L18 or L21, argv for argv.
    archiver: Vec<String>,
}

/// L2..L22. `root` is the directory the original's relative paths
/// resolved against: empty in production, where that is the process's
/// working directory, and the fixture root under test. The archiver's
/// argv is relative to it too, which is why the tests script `archive`
/// rather than spawning one.
fn package(
    root: &Path,
    arguments: &Arguments,
    windows: bool,
    archive: &mut dyn FnMut(&[String]) -> io::Result<bool>,
) -> Result<String> {
    let plan = plan(arguments, windows);

    // L12..L14.
    let directory = root.join(&plan.package);
    if let Err(error) = std::fs::create_dir_all(&directory) {
        bail!("cannot create {}: {error}", directory.display());
    }
    copy_into(root, &directory, &[plan.binary.as_str()])?;
    copy_into(root, &directory, &DOCUMENTS)?;

    // L15..L22. `set -e` killed the step with the archiver's own status.
    match archive(&plan.archiver) {
        Ok(true) => Ok(plan.package_path),
        Ok(false) => bail!("{} failed to write {}", plan.archiver[0], plan.package_path),
        Err(error) => bail!("{}: {error}", plan.archiver[0]),
    }
}

/// L3..L21.
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

/// L3..L6.
fn version(ref_name: &str, ref_type: &str, sha: &str) -> String {
    if ref_type == "tag" {
        return ref_name.to_owned();
    }
    let short: String = sha.chars().take(SHORT_SHA).collect();
    format!("dev-{short}")
}

/// One `cp SRC... DIR/`: every source is attempted, each failure is
/// reported, and the command fails once if any did.
fn copy_into(root: &Path, directory: &Path, sources: &[&str]) -> Result<()> {
    let failures: Vec<String> = sources
        .iter()
        .filter_map(|source| {
            // The trailing `/` on the destination: the copy is named
            // after the source's last component.
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

/// The production archiver: a real child process, found on `PATH`
/// exactly as the workflow's `tar` and `powershell` were, with its
/// output inherited.
fn spawn(argv: &[String]) -> io::Result<bool> {
    Command::new(&argv[0])
        .args(&argv[1..])
        .status()
        .map(|status| status.success())
}

/// The flags carrying what the original read from the run's
/// environment. Order is free and a repeated flag keeps its last value,
/// as a repeated assignment would have.
fn parse(arguments: &[String]) -> Result<Arguments> {
    let (mut target, mut ref_name, mut ref_type, mut sha) = (None, None, None, None);
    let mut arguments = arguments.iter();
    while let Some(flag) = arguments.next() {
        let slot = match flag.as_str() {
            "--target" => &mut target,
            "--ref-name" => &mut ref_name,
            "--ref-type" => &mut ref_type,
            "--sha" => &mut sha,
            other => bail!("unexpected argument: {other}\n\n{USAGE}"),
        };
        let Some(value) = arguments.next() else {
            bail!("{flag} takes a value\n\n{USAGE}");
        };
        *slot = Some(value.clone());
    }
    Ok(Arguments {
        target: required(target, "--target")?,
        ref_name: required(ref_name, "--ref-name")?,
        // `${GITHUB_REF_TYPE:-}`: absent reads as the empty string.
        ref_type: ref_type.unwrap_or_default(),
        sha: required(sha, "--sha")?,
    })
}

/// `${VAR}` under `-u`: an unset name killed the step where an empty
/// one was a value, so the flag must be given and may be empty.
fn required(value: Option<String>, flag: &str) -> Result<String> {
    value.ok_or_else(|| anyhow!("{flag} is required\n\n{USAGE}"))
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

    /// A checkout with everything L13 and L14 copy, for `windows`'s
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

    /// A scripted archiver: records the argv it was handed and answers
    /// as told.
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
        // An empty ref name is a value under `-u`, spliced in as-is.
        assert_eq!(version("", "tag", "0123456789abcdef"), "");
    }

    #[test]
    fn every_other_ref_type_is_versioned_by_the_short_sha() {
        // L4's `:-` default, a branch, and a ref type that only looks
        // like the literal L4 compares against.
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
        // `${GITHUB_SHA::12}` never errors on a value shorter than the
        // slice; an empty SHA is set, so `-u` let it through too.
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
    fn the_archivers_take_the_shells_argv() {
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
    fn an_existing_package_directory_is_not_an_error() {
        // `mkdir -p`.
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
        // L13 is its own `cp`, so L14 never ran.
        assert!(!root.child("cabin-0.17.0-triple/README.md").path().exists());
        assert!(!root.child("cabin-0.17.0-triple/LICENSE").path().exists());
    }

    #[test]
    fn a_missing_readme_still_stages_the_license() {
        // L14 is one `cp` over both sources: it reports the missing one
        // and copies the other before exiting 1.
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

    #[test]
    fn the_flags_carry_the_context_the_shell_read_from_the_environment() {
        let given = [
            "--target",
            "t",
            "--ref-name",
            "n",
            "--ref-type",
            "tag",
            "--sha",
            "s",
        ]
        .map(str::to_owned);
        assert_eq!(
            parse(&given).expect("the arguments"),
            arguments("t", "n", "tag", "s")
        );
    }

    #[test]
    fn an_absent_ref_type_reads_as_empty() {
        let given = [
            "--target",
            "t",
            "--ref-name",
            "n",
            "--sha",
            "0123456789abcdef",
        ]
        .map(str::to_owned);
        let parsed = parse(&given).expect("the arguments");
        assert_eq!(parsed.ref_type, "");
        assert_eq!(
            version(&parsed.ref_name, &parsed.ref_type, &parsed.sha),
            "dev-0123456789ab"
        );
    }

    #[test]
    fn an_empty_value_is_a_value() {
        let given = [
            "--target",
            "",
            "--ref-name",
            "",
            "--ref-type",
            "",
            "--sha",
            "",
        ]
        .map(str::to_owned);
        assert_eq!(
            parse(&given).expect("the arguments"),
            arguments("", "", "", "")
        );
    }

    #[test]
    fn a_missing_required_flag_is_where_the_shell_died_on_an_unset_variable() {
        for flag in ["--target", "--ref-name", "--sha"] {
            let given: Vec<String> = ["--target", "t", "--ref-name", "n", "--sha", "s"]
                .chunks(2)
                .filter(|pair| pair[0] != flag)
                .flat_map(|pair| pair.iter().map(|argument| (*argument).to_owned()))
                .collect();
            let error = parse(&given).expect_err(flag);
            assert!(error.to_string().contains(flag), "{error}");
        }
    }

    #[test]
    fn an_unknown_flag_and_a_valueless_one_are_usage_errors() {
        let unknown = ["--build-dir", "x"].map(str::to_owned);
        assert!(
            parse(&unknown)
                .expect_err("the unknown flag")
                .to_string()
                .contains("unexpected argument: --build-dir")
        );
        let valueless = ["--sha".to_owned()];
        assert!(
            parse(&valueless)
                .expect_err("the valueless flag")
                .to_string()
                .contains("--sha takes a value")
        );
    }
}
