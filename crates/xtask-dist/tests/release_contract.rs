//! The release-packaging contracts a published archive depends on
//! (cargo-binstall declares `pkg-fmt = "txz"` against these names),
//! extracted from the retired shell-vs-port differentials.
//!
//! Unix-only: the archive on this path is `tar -cJf`'s, and `tar` is
//! what unpacks it for the layout assertion.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Command;

fn tool(dir: &Path, arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_xtask-dist"))
        .args(arguments)
        .current_dir(dir)
        .output()
        .expect("the tool is runnable")
}

/// A tag build names the package after the tag, prints the archive
/// path to stdout, and the archive holds exactly the staged trio under
/// the package directory.
#[test]
fn a_tag_build_packages_the_staged_trio() {
    let root = assert_fs::TempDir::new().unwrap();
    let release = root.path().join("target/triple/release");
    fs::create_dir_all(&release).unwrap();
    fs::write(release.join("cabin"), b"binary bytes").unwrap();
    fs::set_permissions(release.join("cabin"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(root.path().join("README.md"), b"readme").unwrap();
    fs::write(root.path().join("LICENSE"), b"license").unwrap();

    let run = tool(
        root.path(),
        &[
            "package",
            "--target",
            "triple",
            "--ref-name",
            "0.14.0",
            "--ref-type",
            "tag",
            "--sha",
            "0123456789abcdef",
        ],
    );
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "cabin-0.14.0-triple.tar.xz\n"
    );

    let listing = Command::new("tar")
        .args(["-tf", "cabin-0.14.0-triple.tar.xz"])
        .current_dir(root.path())
        .output()
        .expect("tar is runnable");
    assert!(listing.status.success());
    let mut entries: Vec<&str> = std::str::from_utf8(&listing.stdout)
        .expect("tar lists UTF-8 names")
        .lines()
        .collect();
    entries.sort_unstable();
    assert_eq!(
        entries,
        [
            "cabin-0.14.0-triple/",
            "cabin-0.14.0-triple/LICENSE",
            "cabin-0.14.0-triple/README.md",
            "cabin-0.14.0-triple/cabin",
        ]
    );

    // The executable bit rides the archive: cargo-binstall unpacks and
    // runs the binary as-is.
    let unpacked = root.path().join("unpacked");
    fs::create_dir(&unpacked).unwrap();
    let extract = Command::new("tar")
        .args(["-xf", "../cabin-0.14.0-triple.tar.xz"])
        .current_dir(&unpacked)
        .status()
        .expect("tar is runnable");
    assert!(extract.success());
    let mode = fs::metadata(unpacked.join("cabin-0.14.0-triple/cabin"))
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0, "the binary lost its executable bit");
}

/// `check` unpacks what `package` made and runs the binary inside it:
/// one that reports the workspace version passes, any other fails.
#[test]
fn check_runs_the_packaged_binary_and_compares_its_version() {
    let version = env!("CARGO_PKG_VERSION");
    for (prints, passes) in [(version, true), ("0.0.0", false)] {
        let root = assert_fs::TempDir::new().unwrap();
        let release = root.path().join("target/triple/release");
        fs::create_dir_all(&release).unwrap();
        fs::write(
            release.join("cabin"),
            format!("#!/bin/sh\necho 'cabin {prints}'\n"),
        )
        .unwrap();
        fs::set_permissions(release.join("cabin"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(root.path().join("README.md"), b"readme").unwrap();
        fs::write(root.path().join("LICENSE"), b"license").unwrap();
        let package = tool(
            root.path(),
            &[
                "package",
                "--target",
                "triple",
                "--ref-name",
                "main",
                "--sha",
                "0123456789abcdef",
            ],
        );
        assert!(package.status.success());
        let archive = String::from_utf8(package.stdout).unwrap();

        let check = tool(
            root.path(),
            &["check", archive.trim_end(), "--ref-name", "main"],
        );
        let stderr = String::from_utf8_lossy(&check.stderr);
        assert_eq!(check.status.success(), passes, "{prints}: {stderr}");
        assert_eq!(passes, !stderr.contains("not \"cabin "), "{stderr}");
    }
}
