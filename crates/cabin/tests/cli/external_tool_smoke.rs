use super::*;

fn write_cpp_project(root: &TempDir, manifest_tail: &str, source: &str) {
    root.child("cabin.toml")
        .write_str(&format!("{VALID_MANIFEST}\n{manifest_tail}"))
        .unwrap();
    root.child("src/main.cc").write_str(source).unwrap();
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
