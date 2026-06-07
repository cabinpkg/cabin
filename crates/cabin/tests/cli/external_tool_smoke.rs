use super::*;

fn assert_process_success(tool: &Path, args: &[&str], label: &str) {
    let output = std::process::Command::new(tool)
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("failed to spawn {label} at {}: {err}", tool.display()));
    assert!(
        output.status.success(),
        "{label} at {} exited with {:?}\nstdout:\n{}\nstderr:\n{}",
        tool.display(),
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_cpp_project(root: &TempDir, manifest_tail: &str, source: &str) {
    root.child("cabin.toml")
        .write_str(&format!("{VALID_MANIFEST}\n{manifest_tail}"))
        .unwrap();
    root.child("src/main.cc").write_str(source).unwrap();
}

#[test]
fn ninja_is_available() {
    require_external_tool("ninja");
    assert_process_success(Path::new("ninja"), &["--version"], "ninja");
}

#[test]
#[cfg_attr(windows, ignore = "pkg-config is not available on Windows runners")]
fn pkg_config_is_available() {
    require_external_tool("pkg-config");
    assert_process_success(Path::new("pkg-config"), &["--version"], "pkg-config");
}

#[test]
fn cabin_fmt_reaches_real_formatter() {
    let dir = TempDir::new().unwrap();
    write_cpp_project(&dir, "", "int main() { return 0; }\n");
    dir.child(".clang-format")
        .write_str("BasedOnStyle: LLVM\n")
        .unwrap();

    require_external_tool("clang-format");
    cabin()
        .current_dir(dir.path())
        .args(["fmt", "--check"])
        .assert()
        .success();
}

/// `cabin lint` was removed alongside the cpplint wrapper.
/// Pin the absence so a regression cannot reintroduce a
/// hidden command path or alias.
#[test]
fn cabin_lint_subcommand_no_longer_exists() {
    let dir = TempDir::new().unwrap();
    write_cpp_project(&dir, "", "int main() { return 0; }\n");
    cabin()
        .current_dir(dir.path())
        .arg("lint")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand"));
}

#[test]
#[cfg_attr(windows, ignore = "run-clang-tidy is not available on Windows runners")]
fn cabin_tidy_reaches_real_tidy() {
    let dir = TempDir::new().unwrap();
    write_cpp_project(&dir, "", "int main() { return 0; }\n");
    dir.child(".clang-tidy")
        .write_str("Checks: '-*,clang-diagnostic-*,clang-analyzer-core.*'\n")
        .unwrap();

    require_external_tool("run-clang-tidy");
    require_cxx_build_tools();
    cabin()
        .current_dir(dir.path())
        .arg("tidy")
        .assert()
        .success();
}
