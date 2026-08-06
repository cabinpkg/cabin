//! The release-packaging contracts a published archive depends on
//! (cargo-binstall declares `pkg-fmt = "txz"` against these names),
//! extracted from the retired shell-vs-port differentials.
//!
//! Unix-only: the archive on this path is `tar -cJf`'s, and `tar` is
//! what unpacks it for the layout assertion.
#![cfg(unix)]

use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Command;

use sha2::{Digest as _, Sha256};

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

/// `checksums` writes `sha256.sum` over every release archive in the
/// working directory and prints the same summary it wrote.
#[test]
fn checksums_print_what_they_write() {
    let root = assert_fs::TempDir::new().unwrap();
    fs::write(root.path().join("cabin-0.14.0-triple.tar.xz"), b"archive").unwrap();

    let run = tool(root.path(), &["checksums"]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let digest = Sha256::digest(b"archive")
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
    let line = format!("{digest} *cabin-0.14.0-triple.tar.xz\n");
    let written = fs::read(root.path().join("sha256.sum")).expect("sha256.sum");
    assert_eq!(written, line.as_bytes());
    assert_eq!(run.stdout, written);
    let sidecar = fs::read(root.path().join("cabin-0.14.0-triple.tar.xz.sha256"))
        .expect("the per-archive sidecar");
    assert_eq!(sidecar, line.as_bytes());
}
