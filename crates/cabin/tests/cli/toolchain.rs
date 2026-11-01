//! End-to-end coverage for explicit toolchain selection and
//! conditional build flags.

use super::*;
// The shell-script fakes below are Unix-only and are the only use of
// `PathBuf` in this module.
#[cfg(unix)]
use std::path::PathBuf;

/// Write a fake compiler/archiver `name` into `dir` and return its
/// absolute path.  The fake is a POSIX shell script that exits 0, so
/// `--cxx /path/to/it` resolves; `cabin metadata`'s fail-soft
/// compiler detection runs it, but nothing compiles with it.
#[cfg(unix)]
fn fake_tool(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    write_executable(&path, "#!/bin/sh\nexit 0\n");
    path
}

#[cfg(unix)]
#[test]
fn metadata_reports_default_toolchain_source() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let cxx = &value["toolchain"]["tools"]["cxx"];
    assert_eq!(cxx["kind"].as_str(), Some("cxx"));
    assert_eq!(cxx["source"].as_str(), Some("default"));
}

#[test]
fn metadata_requires_resolvable_cxx_before_detection() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    let empty_path = TempDir::new().unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("PATH", empty_path.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("no usable C++ compiler found on PATH"),
        "expected missing-CXX diagnostic, got: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn cli_cxx_flag_overrides_default() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    let bin = TempDir::new().unwrap();
    let cxx = fake_tool(bin.path(), "my-cxx");
    let _ar = fake_tool(bin.path(), "ar");
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--cxx"])
        .arg(&cxx)
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let entry = &value["toolchain"]["tools"]["cxx"];
    assert_eq!(entry["source"].as_str(), Some("cli"));
    assert_eq!(entry["spec"].as_str().unwrap(), cxx.to_str().unwrap());
}

#[cfg(unix)]
#[test]
fn cxx_env_var_is_respected_when_no_cli_flag() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    let bin = TempDir::new().unwrap();
    let cxx = fake_tool(bin.path(), "env-cxx");
    let _ar = fake_tool(bin.path(), "ar");
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("PATH", bin.path())
        .env("CXX", &cxx)
        .env_remove("CC")
        .env_remove("AR")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let entry = &value["toolchain"]["tools"]["cxx"];
    assert_eq!(entry["source"].as_str(), Some("env"));
}

#[cfg(unix)]
#[test]
fn missing_explicit_cxx_errors_clearly() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    let bin = TempDir::new().unwrap();
    let _ar = fake_tool(bin.path(), "ar");
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--cxx", "definitely-not-a-real-compiler-99"])
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("definitely-not-a-real-compiler-99"),
        "expected toolchain error mentioning the spec, got: {stderr}",
    );
    assert!(
        stderr.contains("could not be found"),
        "expected `could not be found` wording, got: {stderr}",
    );
}

#[cfg(unix)]
#[test]
fn manifest_toolchain_table_is_honored_when_no_cli_or_env() {
    let dir = TempDir::new().unwrap();
    let bin = TempDir::new().unwrap();
    let _g = fake_tool(bin.path(), "g++");
    let _c = fake_tool(bin.path(), "clang++");
    let _ar = fake_tool(bin.path(), "ar");
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[toolchain]
cxx = "clang++"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let entry = &value["toolchain"]["tools"]["cxx"];
    assert_eq!(entry["source"].as_str(), Some("manifest"));
    assert_eq!(entry["spec"].as_str(), Some("clang++"));
}

#[test]
fn unsupported_toolchain_field_is_rejected() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[toolchain]
compiler-family = "clang"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("compiler-family"),
        "expected unknown-field error mentioning compiler-family, got: {stderr}",
    );
}

#[test]
fn invalid_include_path_with_parent_traversal_is_rejected() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[profile]
include-dirs = ["../sneaky"]
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("..") || stderr.contains("sneaky"),
        "expected include-dir traversal rejection, got: {stderr}",
    );
}

#[cfg(unix)]
#[test]
fn target_conditioned_build_flags_apply_to_compile_commands() {
    require_cxx_build_tools();
    let host_os = std::env::consts::OS;
    let other_os = if host_os == "linux" { "macos" } else { "linux" };
    let dir = TempDir::new().unwrap();
    let manifest = format!(
        r#"[package]
name = "hello"
version = "0.1.0"
cxx-standard = "c++17"

[target.hello]
type = "executable"
sources = ["src/main.cc"]

[target.'cfg(os = "{host_os}")'.profile]
defines = ["CABIN_HOST_MATCHED"]

[target.'cfg(os = "{other_os}")'.profile]
defines = ["CABIN_HOST_NOT_MATCHED"]
"#
    );
    dir.child("cabin.toml").write_str(&manifest).unwrap();
    dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();

    let build_dir = dir.path().join("build");
    cabin()
        .current_dir(dir.path())
        .args(["build", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();
    let cc = std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
    assert!(
        cc.contains("-DCABIN_HOST_MATCHED"),
        "expected matching cfg define present: {cc}"
    );
    assert!(
        !cc.contains("CABIN_HOST_NOT_MATCHED"),
        "expected non-matching cfg define absent: {cc}"
    );
}

#[cfg(unix)]
#[test]
fn build_includes_dirs_from_build_table() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "hello"
version = "0.1.0"
cxx-standard = "c++17"

[target.hello]
type = "executable"
sources = ["src/main.cc"]

[profile]
defines = ["CABIN_BUILD_DEFINE"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
    dir.child("include/.gitkeep").write_str("").unwrap();

    let build_dir = dir.path().join("build");
    cabin()
        .current_dir(dir.path())
        .args(["build", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();
    let cc = std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
    assert!(
        cc.contains("-DCABIN_BUILD_DEFINE"),
        "expected build define in compile_commands: {cc}"
    );
    assert!(
        cc.contains("/include"),
        "expected include dir in compile_commands: {cc}"
    );
}

#[test]
fn member_manifest_with_toolchain_table_is_rejected() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/app"]
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[toolchain]
cxx = "clang++"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("toolchain"),
        "expected member-toolchain rejection, got: {stderr}",
    );
}

/// Compilers replaced at the same paths rebuild the package though no
/// command line changed: every edge depends on the toolchain identity,
/// which leaves paths to the commands that use them.
#[cfg(unix)]
#[test]
fn compilers_replaced_at_the_same_paths_rebuild() {
    require_c_and_cxx_build_tools();
    let find = |names: [&'static str; 3]| names.into_iter().find(|n| command_exists(n)).unwrap();
    let (real_cc, real_cxx) = (
        find(["cc", "clang", "gcc"]),
        find(["c++", "clang++", "g++"]),
    );
    let dir = TempDir::new().unwrap();
    let root = assert_fs::fixture::ChildPath::new(dir.path());
    root.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"
cxx-standard = "c++17"

[target.capp]
type = "executable"
sources = ["src/main.c"]

[target.cxxapp]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    root.child("src/main.c")
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    root.child("src/main.cc")
        .write_str("int main() { return 0; }\n")
        .unwrap();
    let bin = TempDir::new().unwrap();
    let (cc, cxx) = (bin.path().join("cc"), bin.path().join("cxx"));
    let install = |version: &str| {
        for (fake, real) in [(&cc, real_cc), (&cxx, real_cxx)] {
            write_executable(
                fake,
                &format!(
                    "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'clang version {version}'; exit 0; fi\nexec {real} \"$@\"\n"
                ),
            );
        }
    };
    let build = |cc: &Path| {
        cabin()
            .current_dir(dir.path())
            .arg("build")
            .arg("--cc")
            .arg(cc)
            .arg("--cxx")
            .arg(&cxx)
            .assert()
            .success();
    };
    let modified = || {
        ["capp", "cxxapp"].map(|exe| {
            let path = dir.path().join("build/dev/packages/app").join(exe);
            fs::metadata(path).unwrap().modified().unwrap()
        })
    };

    install("17.0.1");
    build(&cc);
    let first = modified();
    build(&cc);
    assert_eq!(modified(), first, "unchanged compilers must not relink");

    install("17.0.2");
    build(&cc);
    let replaced = modified();
    assert!(
        first.iter().zip(&replaced).all(|(a, b)| a != b),
        "replaced compilers must relink both executables"
    );

    let moved = bin.path().join("cc-moved");
    write_executable(&moved, &fs::read_to_string(&cc).unwrap());
    build(&moved);
    assert_eq!(
        modified()[1],
        replaced[1],
        "a C compiler at another path must not relink the C++ program"
    );
}

/// A relative compiler path names a file under the current directory,
/// though Ninja runs each compile from the build directory.
#[cfg(unix)]
#[test]
fn relative_compiler_paths_resolve_against_the_current_directory() {
    require_c_and_cxx_build_tools();
    let find = |names: [&'static str; 3]| names.into_iter().find(|n| command_exists(n)).unwrap();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"
cxx-standard = "c++17"

[target.capp]
type = "executable"
sources = ["src/main.c"]

[target.cxxapp]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    dir.child("src/main.c")
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    dir.child("src/main.cc")
        .write_str("int main() { return 0; }\n")
        .unwrap();
    fs::create_dir(dir.path().join("bin")).unwrap();
    for (fake, real) in [
        ("cc", find(["cc", "clang", "gcc"])),
        ("cxx", find(["c++", "clang++", "g++"])),
    ] {
        write_executable(
            &dir.path().join("bin").join(fake),
            &format!("#!/bin/sh\nexec {real} \"$@\"\n"),
        );
    }
    cabin()
        .current_dir(dir.path())
        .args(["build", "--cc", "./bin/cc", "--cxx", "./bin/cxx"])
        .assert()
        .success();
}

/// `cl` searches `INCLUDE` and `link` searches `LIB`, which no command
/// line names, so pointing either somewhere else rebuilds, and an
/// unchanged environment rebuilds nothing.
#[cfg(windows)]
#[test]
fn msvc_include_and_lib_changes_rebuild() {
    require_c_and_cxx_build_tools();
    let (Some(include), Some(lib)) = (std::env::var_os("INCLUDE"), std::env::var_os("LIB")) else {
        eprintln!("skipping: no activated MSVC environment");
        return;
    };
    let dir = TempDir::new().unwrap();
    let root = assert_fs::fixture::ChildPath::new(dir.path());
    root.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"
cxx-standard = "c++17"

[target.app]
type = "executable"
sources = ["src/main.cc", "src/value.c"]
"#,
        )
        .unwrap();
    root.child("src/value.c")
        .write_str("#include <envdep.h>\nint c_value(void) { return ENVDEP; }\n")
        .unwrap();
    root.child("src/main.cc")
        .write_str(
            "#include <cstdio>\n#include <envdep.h>\nextern \"C\" int c_value(void);\n\
             int main() { std::printf(\"value=%d,%d\\n\", ENVDEP, c_value()); }\n",
        )
        .unwrap();
    for (name, value) in [("one", 1), ("two", 2)] {
        root.child(format!("{name}/envdep.h"))
            .write_str(&format!("#define ENVDEP {value}\n"))
            .unwrap();
    }
    let prepend = |dir: &Path, list: &std::ffi::OsStr| {
        let mut joined = dir.as_os_str().to_owned();
        joined.push(";");
        joined.push(list);
        joined
    };
    let cabin_with = |headers: &str, lib: &std::ffi::OsStr, args: &[&str]| {
        cabin()
            .args(args)
            .env("INCLUDE", prepend(&dir.path().join(headers), &include))
            .env("LIB", lib)
            .current_dir(dir.path())
            .assert()
            .success()
    };
    cabin_with("one", &lib, &["run"]).stdout(predicate::str::contains("value=1,1"));
    cabin_with("two", &lib, &["run"]).stdout(predicate::str::contains("value=2,2"));
    cabin_with("two", &lib, &["-v", "build"])
        .stdout(predicate::str::contains("ninja: no work to do."));
    let other_lib = prepend(dir.path(), &lib);
    cabin_with("two", &other_lib, &["-v", "build"])
        .stdout(predicate::str::contains("ninja: no work to do.").not());
}
