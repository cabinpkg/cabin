use super::*;

#[test]
fn cli_sources_do_not_write_directly_to_stderr() {
    fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read source directory") {
            let entry = entry.expect("read source entry");
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }

    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs_files(&src_dir, &mut files);

    let mut offenders = Vec::new();
    for path in files {
        let body = fs::read_to_string(&path).expect("read source file");
        if body.contains("eprintln!(") {
            offenders.push(
                path.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
            );
        }
    }

    assert!(
        offenders.is_empty(),
        "production CLI sources must route human output through Reporter or cabin-diagnostics, not direct eprintln!: {offenders:#?}",
    );
}

/// Replace the absolute test-tempdir path in `text` with a
/// stable placeholder so a golden assertion is byte-stable
/// across CI / developer machines. macOS canonicalizes
/// `/tmp/...` to `/private/tmp/...`, so we strip both
/// prefixes.
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
    // miette's fancy renderer emits the stable code on its
    // own line, then a blank line, then `  × <message>`,
    // and finally `  help: <help text>`. Pin all three
    // components plus the no-cause-chain invariant: the
    // raw `os error 2` must not appear anywhere because
    // the typed error sets its own message.
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
    // The exact byte position the toml parser flags varies
    // between releases, so we assert on stable invariants:
    // - the `parse_error` code,
    // - the primary `× could not parse Cabin manifest`
    //   line,
    // - miette's box-drawing snippet header `╭─[path:l:c]`,
    // - the offending source line embedded in the snippet,
    // - a `help:` line.
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
    // A user invoking `cabin --help` should get the help
    // text whether or not they are inside a Cabin
    // workspace. clap short-circuits `--help` before
    // dispatch, so we expect SUCCESS even from a tempdir
    // that has no `cabin.toml`.
    let dir = TempDir::new().unwrap();
    cabin()
        .current_dir(dir.path())
        .arg("--help")
        .assert()
        .success();
}

#[test]
fn cabin_subcommand_help_works_outside_workspace() {
    // Same regression for `cabin <cmd> --help`. The classic
    // failure mode is: dispatcher tries to load the
    // workspace before clap sees the `--help` flag, and the
    // missing manifest fails the help invocation. Every
    // top-level subcommand is exercised — including the
    // hidden distribution helpers — so a regression in any
    // one of them surfaces here.  The list is derived from
    // clap so a future subcommand is covered automatically.
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
    // `IsADirectory`. The diagnostic must be a typed
    // `cabin::manifest::unreadable` with no chain
    // duplication.
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
    // The OS error must appear once, not twice. The old
    // anyhow chain rendered "failed to read X: failed to
    // read X: Is a directory: Is a directory" — and the
    // miette renderer is configured `.without_cause_chain()`
    // so it doesn't re-emit `╰─▶ Is a directory` either. The
    // OS message itself is platform-specific (Windows reports
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
    // Strip every permission bit so `std::fs::canonicalize`
    // (the workspace loader's first read) returns
    // PermissionDenied rather than NotFound.
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
