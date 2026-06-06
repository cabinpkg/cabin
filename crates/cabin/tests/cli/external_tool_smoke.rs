use super::*;

fn external_tool(real: &str, fake: &str) -> PathBuf {
    if use_fake_external_tools() {
        workspace_test_bin(fake)
    } else {
        require_external_tool(real);
        PathBuf::from(real)
    }
}

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
fn ninja_is_available_or_fake_backend_is_selected() {
    let tool = external_tool("ninja", "cabin-ninja-fake-ninja");
    assert_process_success(&tool, &["--version"], "ninja");
}

#[test]
fn pkg_config_is_available_or_fake_probe_is_selected() {
    let tool = external_tool("pkg-config", "cabin-system-deps-fake-pkg-config");
    assert_process_success(&tool, &["--version"], "pkg-config");
}

#[test]
fn cabin_fmt_reaches_real_formatter_or_fake_formatter() {
    let dir = TempDir::new().unwrap();
    let source = if use_fake_external_tools() {
        "int main() { return 0; }\n/* FORMATTED */\n"
    } else {
        "int main() { return 0; }\n"
    };
    write_cpp_project(&dir, "", source);
    dir.child(".clang-format")
        .write_str("BasedOnStyle: LLVM\n")
        .unwrap();

    let mut cmd = cabin();
    if use_fake_external_tools() {
        cmd.env("CABIN_FMT", workspace_test_bin("cabin-fmt-fake-formatter"));
    } else {
        require_external_tool("clang-format");
    }
    cmd.current_dir(dir.path())
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
fn cabin_tidy_reaches_real_tidy_or_fake_tidy() {
    let dir = TempDir::new().unwrap();
    write_cpp_project(&dir, "", "int main() { return 0; }\n");
    dir.child(".clang-tidy")
        .write_str("Checks: '-*,clang-diagnostic-*,clang-analyzer-core.*'\n")
        .unwrap();

    let mut cmd = cabin();
    if use_fake_external_tools() {
        cmd.env("CABIN_TIDY", workspace_test_bin("cabin-tidy-fake-tidy"));
        let dummy_tool = workspace_test_bin("cabin-ninja-fake-ninja");
        cmd.env("CXX", &dummy_tool);
        cmd.env("CC", &dummy_tool);
        cmd.env("AR", &dummy_tool);
    } else {
        require_external_tool("run-clang-tidy");
        assert!(
            build_tools_available(),
            "real `cabin tidy` smoke test requires ninja and a C++ compiler; install them or set {SKIP_EXTERNAL_TOOL_TESTS_ENV}=1 to use bundled fake tools"
        );
    }
    cmd.current_dir(dir.path()).arg("tidy").assert().success();
}
