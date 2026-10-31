use super::*;
use std::path::PathBuf;

/// Sentinel marker appended by the bundled fake clang-format.
/// A file whose trimmed contents end with this marker is
/// treated as "already formatted"; a file that lacks it is
/// treated as "would be reformatted".
const MARKER: &str = "/* FORMATTED */";

fn cabin_with_fake_formatter() -> Command {
    let mut cmd = cabin();
    let path = fake_formatter_path();
    cmd.env("CABIN_FMT", path);
    // Poison the environment with registry credentials: the fake
    // formatter exits 4 when it sees either variable, so every test
    // in this module enforces the child-env scrub.
    cmd.env("CABIN_REGISTRY_TOKEN", "cabin_secretToken1234");
    cmd.env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "runner-oidc-secret");
    cmd
}

fn fake_formatter_path() -> PathBuf {
    workspace_test_bin("cabin-fmt-fake-formatter")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

#[test]
fn write_mode_formats_in_place() {
    let dir = TempDir::new().unwrap();
    write_minimal_project(dir.path());

    cabin_with_fake_formatter()
        .current_dir(dir.path())
        .arg("fmt")
        .assert()
        .success()
        .stdout(predicate::str::contains("Formatted 1 file"));

    let body = read(&dir.path().join("src/main.cc"));
    assert!(body.contains(MARKER), "expected marker, got: {body:?}");
}

#[test]
fn check_mode_passes_when_already_formatted() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
    dir.child("src/main.cc")
        .write_str("int main() { return 0; }\n/* FORMATTED */\n")
        .unwrap();

    cabin_with_fake_formatter()
        .current_dir(dir.path())
        .args(["fmt", "--check"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already up to date"));

    let body = read(&dir.path().join("src/main.cc"));
    assert!(body.contains(MARKER));
}

#[test]
fn check_mode_fails_when_files_would_be_reformatted() {
    let dir = TempDir::new().unwrap();
    write_minimal_project(dir.path());

    // At default verbosity a failing `--check` reports both the
    // formatter's per-file diagnostic on stderr and a Cabin-owned
    // banner on stdout; the banner appears even when a custom
    // `CABIN_FMT` wrapper writes nothing to stderr.
    let assertion = cabin_with_fake_formatter()
        .current_dir(dir.path())
        .args(["fmt", "--check"])
        .assert()
        .failure();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("would be reformatted"),
        "expected formatter diagnostic on stderr, got: {stderr}"
    );
    assert!(
        stdout.contains("Failed") && stdout.contains("cabin fmt --check"),
        "expected Cabin-owned failure banner on stdout, got: {stdout}"
    );

    let body = read(&dir.path().join("src/main.cc"));
    assert!(!body.contains(MARKER), "check mode mutated file: {body}");
}

#[test]
fn exclude_path_skips_named_file() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
    dir.child("src/main.cc")
        .write_str("int main() {}\n")
        .unwrap();
    dir.child("src/generated.cc")
        .write_str("int gen() {}\n")
        .unwrap();

    cabin_with_fake_formatter()
        .current_dir(dir.path())
        // Spell the exclude with the host separator so it matches the
        // walker's discovered paths on Windows (`src\generated.cc`).
        .arg("fmt")
        .arg("--exclude")
        .arg(host_path("src/generated.cc"))
        .assert()
        .success();

    assert!(read(&dir.path().join("src/main.cc")).contains(MARKER));
    assert!(!read(&dir.path().join("src/generated.cc")).contains(MARKER));
}

#[test]
fn exclude_path_skips_directory_descendants() {
    // docs/fmt.md: "a directory entry skips every descendant".
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
    dir.child("src/main.cc")
        .write_str("int main() {}\n")
        .unwrap();
    dir.child("vendored/dep.cc")
        .write_str("int dep() {}\n")
        .unwrap();

    cabin_with_fake_formatter()
        .current_dir(dir.path())
        .arg("fmt")
        .arg("--exclude")
        .arg("vendored")
        .assert()
        .success();

    assert!(read(&dir.path().join("src/main.cc")).contains(MARKER));
    assert!(!read(&dir.path().join("vendored/dep.cc")).contains(MARKER));
}

#[test]
fn repeated_exclude_accumulates() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
    dir.child("src/main.cc")
        .write_str("int main() {}\n")
        .unwrap();
    dir.child("src/a.cc").write_str("int a() {}\n").unwrap();
    dir.child("src/b.cc").write_str("int b() {}\n").unwrap();

    cabin_with_fake_formatter()
        .current_dir(dir.path())
        // Host-separator excludes so they match the walker's
        // discovered paths on Windows.
        .arg("fmt")
        .arg("--exclude")
        .arg(host_path("src/a.cc"))
        .arg("--exclude")
        .arg(host_path("src/b.cc"))
        .assert()
        .success();

    assert!(read(&dir.path().join("src/main.cc")).contains(MARKER));
    assert!(!read(&dir.path().join("src/a.cc")).contains(MARKER));
    assert!(!read(&dir.path().join("src/b.cc")).contains(MARKER));
}

#[test]
fn vcs_ignored_files_are_skipped_by_default() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
    dir.child("src/main.cc")
        .write_str("int main() {}\n")
        .unwrap();
    dir.child("src/generated.cc")
        .write_str("int gen() {}\n")
        .unwrap();
    dir.child(".gitignore")
        .write_str("src/generated.cc\n")
        .unwrap();
    // Touch a `.git/HEAD` so the ignore crate's git-aware
    // walker activates without us needing to shell out to
    // `git init`.
    dir.child(".git/HEAD")
        .write_str("ref: refs/heads/main\n")
        .unwrap();

    cabin_with_fake_formatter()
        .current_dir(dir.path())
        .arg("fmt")
        .assert()
        .success();

    assert!(read(&dir.path().join("src/main.cc")).contains(MARKER));
    assert!(!read(&dir.path().join("src/generated.cc")).contains(MARKER));
}

#[test]
fn no_ignore_vcs_includes_gitignored_files() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
    dir.child("src/main.cc")
        .write_str("int main() {}\n")
        .unwrap();
    dir.child("src/generated.cc")
        .write_str("int gen() {}\n")
        .unwrap();
    dir.child(".gitignore")
        .write_str("src/generated.cc\n")
        .unwrap();
    dir.child(".git/HEAD")
        .write_str("ref: refs/heads/main\n")
        .unwrap();

    cabin_with_fake_formatter()
        .current_dir(dir.path())
        .args(["fmt", "--no-ignore-vcs"])
        .assert()
        .success();

    assert!(read(&dir.path().join("src/main.cc")).contains(MARKER));
    assert!(read(&dir.path().join("src/generated.cc")).contains(MARKER));
}

#[test]
fn build_directory_is_not_formatted() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
    dir.child("src/main.cc")
        .write_str("int main() {}\n")
        .unwrap();
    // Drop a fake artifact under the default build dir.
    dir.child("build/dev/scratch.cc")
        .write_str("int scratch() {}\n")
        .unwrap();

    cabin_with_fake_formatter()
        .current_dir(dir.path())
        .arg("fmt")
        .assert()
        .success();

    assert!(read(&dir.path().join("src/main.cc")).contains(MARKER));
    assert!(!read(&dir.path().join("build/dev/scratch.cc")).contains(MARKER));
}

#[test]
fn custom_build_directory_is_not_formatted() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
    dir.child("src/main.cc")
        .write_str("int main() {}\n")
        .unwrap();
    dir.child("out/dev/scratch.cc")
        .write_str("int s() {}\n")
        .unwrap();

    cabin_with_fake_formatter()
        .current_dir(dir.path())
        .args(["fmt", "--build-dir", "out"])
        .assert()
        .success();

    assert!(read(&dir.path().join("src/main.cc")).contains(MARKER));
    assert!(!read(&dir.path().join("out/dev/scratch.cc")).contains(MARKER));
}

#[test]
fn relative_build_directory_resolves_against_the_current_directory() {
    // `cabin build --manifest-path pkg/cabin.toml --build-dir pkg/gen`
    // writes `pkg/gen`, so `cabin fmt` given the same arguments must
    // skip it rather than a `gen` below the package.
    let dir = TempDir::new().unwrap();
    dir.child("pkg/cabin.toml")
        .write_str(VALID_MANIFEST)
        .unwrap();
    dir.child("pkg/src/main.cc")
        .write_str("int main() {}\n")
        .unwrap();
    dir.child("pkg/gen/dev/scratch.cc")
        .write_str("int scratch() {}\n")
        .unwrap();

    cabin_with_fake_formatter()
        .current_dir(dir.path())
        .args(["fmt", "--manifest-path", "pkg/cabin.toml"])
        .args(["--build-dir", "pkg/gen"])
        .assert()
        .success();

    assert!(read(&dir.path().join("pkg/src/main.cc")).contains(MARKER));
    assert!(!read(&dir.path().join("pkg/gen/dev/scratch.cc")).contains(MARKER));
}

#[test]
fn vendor_and_cache_directories_are_not_formatted() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
    dir.child("src/main.cc")
        .write_str("int main() {}\n")
        .unwrap();
    // `target`, `dist`, and `node_modules` are in source
    // discovery's built-in excluded-name set.
    dir.child("target/leftover.cc")
        .write_str("int t() {}\n")
        .unwrap();
    dir.child("dist/staging.cc")
        .write_str("int d() {}\n")
        .unwrap();
    dir.child("node_modules/dep/main.cc")
        .write_str("int dep() {}\n")
        .unwrap();

    cabin_with_fake_formatter()
        .current_dir(dir.path())
        .arg("fmt")
        .assert()
        .success();

    assert!(read(&dir.path().join("src/main.cc")).contains(MARKER));
    for skipped in [
        "target/leftover.cc",
        "dist/staging.cc",
        "node_modules/dep/main.cc",
    ] {
        assert!(
            !read(&dir.path().join(skipped)).contains(MARKER),
            "{skipped} was unexpectedly formatted"
        );
    }
}

#[test]
fn missing_formatter_produces_actionable_error() {
    let dir = TempDir::new().unwrap();
    write_minimal_project(dir.path());

    let assertion = cabin()
        .current_dir(dir.path())
        .env("CABIN_FMT", "/no-such/clang-format-binary")
        .arg("fmt")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("/no-such/clang-format-binary"),
        "diagnostic should name the missing executable: {stderr}"
    );
    assert!(
        stderr.contains("install `clang-format`"),
        "diagnostic should include the install hint: {stderr}"
    );
    assert!(
        stderr.contains("CABIN_FMT"),
        "diagnostic should reference the override env var: {stderr}"
    );
}

#[test]
fn cabin_fmt_env_override_routes_through_named_binary() {
    // `CABIN_FMT` names the binary Cabin spawns in place of the
    // system clang-format; only the fake formatter writes `MARKER`.
    let dir = TempDir::new().unwrap();
    write_minimal_project(dir.path());

    cabin()
        .current_dir(dir.path())
        .env("CABIN_FMT", fake_formatter_path())
        .arg("fmt")
        .assert()
        .success();
    assert!(read(&dir.path().join("src/main.cc")).contains(MARKER));
}

#[test]
fn quiet_mode_suppresses_normal_status_lines() {
    let dir = TempDir::new().unwrap();
    write_minimal_project(dir.path());

    let assertion = cabin_with_fake_formatter()
        .current_dir(dir.path())
        .args(["fmt", "--quiet"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stdout.is_empty() && stderr.is_empty(),
        "quiet fmt run must produce no output, got stdout={stdout:?} stderr={stderr:?}"
    );
}

#[test]
fn verbose_mode_lists_selected_package_and_file_count() {
    let dir = TempDir::new().unwrap();
    write_minimal_project(dir.path());

    let assertion = cabin_with_fake_formatter()
        .current_dir(dir.path())
        .args(["fmt", "--verbose"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    assert!(
        stdout.contains("formatting 1 file"),
        "verbose fmt must report file count: {stdout}"
    );
    assert!(
        stdout.contains("hello"),
        "verbose fmt must name the selected package: {stdout}"
    );
}

#[test]
fn very_verbose_lists_command_lines() {
    let dir = TempDir::new().unwrap();
    write_minimal_project(dir.path());

    let assertion = cabin_with_fake_formatter()
        .current_dir(dir.path())
        .args(["fmt", "-vv"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    assert!(
        stdout.contains("--style=file"),
        "very-verbose fmt must echo the formatter command: {stdout}"
    );
    assert!(
        stdout.contains("-i"),
        "very-verbose fmt must echo the write-mode flag: {stdout}"
    );
}

#[test]
fn nested_workspace_member_is_not_walked_from_outer_root() {
    // A member nested inside another package's directory is walked
    // only when it is itself selected.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["outer", "outer/nested"]
"#,
        )
        .unwrap();
    dir.child("outer/cabin.toml")
        .write_str(
            r#"[package]
name = "outer"
version = "0.1.0"
"#,
        )
        .unwrap();
    dir.child("outer/src/main.cc")
        .write_str("int outer() {}\n")
        .unwrap();
    dir.child("outer/nested/cabin.toml")
        .write_str(
            r#"[package]
name = "nested"
version = "0.1.0"
"#,
        )
        .unwrap();
    dir.child("outer/nested/src/main.cc")
        .write_str("int nested() {}\n")
        .unwrap();

    cabin_with_fake_formatter()
        .current_dir(dir.path())
        .args(["fmt", "-p", "outer"])
        .assert()
        .success();

    assert!(read(&dir.path().join("outer/src/main.cc")).contains(MARKER));
    assert!(!read(&dir.path().join("outer/nested/src/main.cc")).contains(MARKER));
}
