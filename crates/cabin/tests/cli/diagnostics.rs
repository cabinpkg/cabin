use super::*;

/// Replace the tempdir path in `text` with `<TMPDIR>`, in both its
/// canonical and given spellings: macOS canonicalizes `/tmp/...` to
/// `/private/tmp/...`.
fn normalize(text: &str, tmpdir: &std::path::Path) -> String {
    let canonical = tmpdir
        .canonicalize()
        .unwrap_or_else(|_| tmpdir.to_path_buf());
    let canonical_str = canonical.to_string_lossy();
    let original_str = tmpdir.to_string_lossy();
    let mut out = text.replace(canonical_str.as_ref(), "<TMPDIR>");
    out = out.replace(original_str.as_ref(), "<TMPDIR>");
    out
}

#[test]
fn missing_manifest_emits_typed_diagnostic_with_help() {
    let dir = TempDir::new().unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    let normalized = normalize(&stderr, dir.path());
    // `ManifestNotFound` carries no `io::Error` source, so the raw
    // `os error 2` text must not appear.
    assert!(
        normalized.contains("cabin::workspace::manifest_not_found"),
        "missing code: {normalized:?}"
    );
    assert!(
        normalized.contains(&host_path(
            "× could not find a Cabin workspace at <TMPDIR>/cabin.toml"
        )),
        "missing primary message: {normalized:?}"
    );
    assert!(
        normalized.contains("help: run `cabin init`"),
        "missing help: {normalized:?}"
    );
    assert!(
        !normalized.contains("os error 2"),
        "raw OS error must not appear: {normalized:?}"
    );
}

#[test]
fn invalid_toml_manifest_renders_source_snippet() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str("[package\nname = broken\n")
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    // The span the toml parser reports varies between releases, so
    // only position-independent parts of the report are asserted.
    assert!(
        stderr.contains("cabin::manifest::parse_error"),
        "missing parse_error code: {stderr}"
    );
    assert!(
        stderr.contains("× could not parse Cabin manifest"),
        "missing primary message: {stderr}"
    );
    assert!(stderr.contains("╭─["), "missing snippet header: {stderr}");
    assert!(stderr.contains("[package"), "missing source line: {stderr}");
    assert!(
        stderr.contains("help: check that the manifest is valid TOML"),
        "missing help: {stderr}"
    );
}

#[test]
fn cabin_help_works_outside_workspace() {
    // clap handles `--help` before dispatch, so no `cabin.toml` is
    // needed.
    let dir = TempDir::new().unwrap();
    cabin()
        .current_dir(dir.path())
        .arg("--help")
        .assert()
        .success();
}

#[test]
fn cabin_subcommand_help_works_outside_workspace() {
    // Pins the failure where a subcommand loads the workspace before
    // clap handles `--help`.  `all_subcommand_names` is read from clap
    // and includes hidden subcommands.
    let dir = TempDir::new().unwrap();
    for sub in all_subcommand_names() {
        cabin()
            .current_dir(dir.path())
            .args([sub.as_str(), "--help"])
            .assert()
            .success();
    }
}

#[test]
fn manifest_path_pointing_at_directory_emits_unreadable_diagnostic() {
    // `--manifest-path <dir>` is not a missing manifest:
    // the path canonicalizes fine but the subsequent
    // `read_to_string` in the manifest crate returns
    // `IsADirectory`.
    let dir = TempDir::new().unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("cabin::manifest::unreadable"),
        "expected manifest unreadable diagnostic code, got: {stderr}"
    );
    // The renderer is configured `.without_cause_chain()`, so the OS
    // error appears in the message only, not again as a `╰─▶` cause
    // line.  The OS message is platform-specific (Windows reports
    // `Access is denied.` for opening a directory).
    let os_error = manifest_dir_read_error();
    let occurrences = stderr.matches(os_error).count();
    assert_eq!(
        occurrences, 1,
        "expected one `{os_error}` occurrence (no chain dup), got {occurrences}: {stderr}"
    );
}

#[test]
#[cfg(unix)]
fn permission_denied_manifest_emits_unreadable_diagnostic() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("cabin.toml");
    assert_fs::fixture::ChildPath::new(&manifest)
        .write_str(
            r#"[package]
name = "x"
version = "0.1.0"
"#,
        )
        .unwrap();
    // Mode 000 leaves `canonicalize` succeeding (it needs no
    // permission on the file itself) and makes the manifest crate's
    // `read_to_string` fail with PermissionDenied.
    let mut perms = std::fs::metadata(&manifest).unwrap().permissions();
    perms.set_mode(0o000);
    std::fs::set_permissions(&manifest, perms).unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    let mut restore = std::fs::metadata(&manifest).unwrap().permissions();
    restore.set_mode(0o644);
    let _ = std::fs::set_permissions(&manifest, restore);
    assert!(
        stderr.contains("cabin::manifest::unreadable"),
        "expected manifest unreadable code, got: {stderr}"
    );
}

/// A local package index is third-party data down to its JSON keys, and
/// the parse error quotes the rejected key back.
#[test]
fn index_parse_errors_cannot_smuggle_terminal_escapes() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    // Valid JSON whose sole key is unknown, so `deny_unknown_fields`
    // quotes the key back; `\u001b` decodes to ESC, and `ESC [ 2 K`
    // erases the line the terminal is on.
    assert_fs::fixture::ChildPath::new(index.join("dep.json"))
        .write_str(r#"{"\u001b[2Khidden":1}"#)
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
dep = ">=1.0.0"

[target.app]
type = "executable"
sources = ["src/main.cc"]
cxx-standard = "c++17"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();

    let assertion = cabin()
        .args(["resolve", "--color", "never", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(&index)
        .assert()
        .failure()
        .stderr(predicate::str::contains("\u{1b}").not());
    assert!(
        stderr_with_wrapping_joined(&assertion).contains(r"\u{1b}[2Khidden"),
        "expected the escaped key in stderr: {}",
        String::from_utf8_lossy(&assertion.get_output().stderr)
    );
}
