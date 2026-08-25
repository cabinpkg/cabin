//! End-to-end coverage for Cabin's two version-reporting surfaces.

use super::*;

fn run_version(args: &[&str]) -> String {
    let assertion = cabin().args(args).assert().success();
    let out = assertion.get_output();
    assert!(out.stderr.is_empty(), "cabin {args:?} wrote to stderr");
    String::from_utf8(out.stdout.clone()).expect("stdout should be utf-8")
}

#[test]
fn short_version_alias_prints_the_compatibility_line() {
    assert_eq!(run_version(&["-V"]), format!("cabin {CABIN_VERSION}\n"));
}

#[test]
fn version_works_outside_a_workspace() {
    let dir = TempDir::new().unwrap();
    let assertion = cabin()
        .current_dir(dir.path())
        .arg("version")
        .assert()
        .success();
    assert_eq!(
        assertion.get_output().stdout,
        format!("cabin {CABIN_VERSION}\n").as_bytes()
    );
}

#[test]
fn verbose_version_reports_release_metadata() {
    let stdout = run_version(&["version", "--verbose"]);
    assert!(stdout.starts_with(&format!("cabin {CABIN_VERSION}\n")));
    assert!(stdout.contains(&format!("release: {CABIN_VERSION}\n")));
}

#[test]
fn quiet_does_not_suppress_requested_version_output() {
    assert_eq!(
        run_version(&["version", "--quiet"]),
        format!("cabin {CABIN_VERSION}\n")
    );
}
