//! A `cabin` that a program `cabin run` or `cabin test` started runs
//! against the same build directory, never a `--target` subtree, and
//! refuses to clean outputs its ancestor keeps in use rather than wait
//! for it forever.

use std::time::Duration;

use super::output_namespaces::build_machine_triple;
use super::*;

/// An executable and a test that both run `$NESTED_CABIN` through the
/// shell and exit with its status.
fn write_project(dir: &Path) {
    let root = assert_fs::fixture::ChildPath::new(dir);
    root.child("cabin.toml")
        .write_str(
            r#"[package]
name = "outer"
version = "0.1.0"
c-standard = "c11"

[target.outer]
type = "executable"
sources = ["src/main.c"]

[target.nested]
type = "test"
sources = ["src/main.c"]
"#,
        )
        .unwrap();
    root.child("src/main.c")
        .write_str(
            "#include <stdio.h>\n#include <stdlib.h>\nint main(void) {\n  const char *nested = getenv(\"NESTED_CABIN\");\n  fflush(stdout);\n  return nested != NULL && system(nested) == 0 ? 0 : 1;\n}\n",
        )
        .unwrap();
}

/// `cabin <args>` in `dir`, its program running `cabin <nested>`.
/// The timeout turns a deadlock into a failure.
fn cabin_nesting(dir: &Path, args: &[&str], nested: &str) -> assert_cmd::assert::Assert {
    let program = assert_cmd::cargo::cargo_bin("cabin");
    cabin()
        .current_dir(dir)
        .env(
            "NESTED_CABIN",
            format!("\"{}\" {nested}", program.display()),
        )
        .args(args)
        .timeout(Duration::from_secs(300))
        .assert()
}

#[test]
fn a_nested_cabin_under_target_builds_in_the_same_build_dir() {
    require_c_and_cxx_build_tools();
    let triple = build_machine_triple();
    if !cabin_core::TargetPlatform::from_triple(&triple)
        .unwrap()
        .runs_on_build_machine()
    {
        // A Windows Arm runner's MSVC builds x64, which `cabin run
        // --target` does not run without a `[runner]`.
        eprintln!("skipped: the build machine does not run {triple} executables");
        return;
    }
    for command in ["run", "test"] {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        write_project(dir);
        let target = dir.join("build/.targets").join(&triple);
        cabin_nesting(dir, &[command, "--target", &triple], "build").success();
        assert!(dir.join("build/dev/build.ninja").is_file(), "{command}");
        cabin_nesting(
            dir,
            &[command, "--target", &triple],
            &format!("build --target {triple}"),
        )
        .success();
        assert!(!target.join(".targets").exists(), "{command}");
        cabin()
            .current_dir(dir)
            .args(["build", "-v", "--target", &triple])
            .assert()
            .success()
            .stdout(predicate::str::contains("ninja: no work to do."));
    }
}

#[test]
fn a_nested_clean_of_outputs_in_use_fails_instead_of_waiting() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    write_project(dir);
    // `:` separates a Unix path list's entries; Windows refuses it.
    let build = if cfg!(unix) { "build:nested" } else { "build" };
    let exe = dir
        .join(build)
        .join("dev/packages/outer")
        .join(host_exe("outer"));
    for command in ["run", "test"] {
        for nested in ["clean", "clean --gc"] {
            cabin_nesting(dir, &[command, "--build-dir", build], nested)
                .failure()
                .stderr(predicate::str::contains("refusing to remove outputs under"));
            assert!(exe.is_file(), "{command} / {nested}");
        }
    }
    // Once nothing uses them, the same clean removes them.
    cabin()
        .current_dir(dir)
        .args(["clean", "--build-dir", build])
        .assert()
        .success();
    assert!(!exe.exists());
}
