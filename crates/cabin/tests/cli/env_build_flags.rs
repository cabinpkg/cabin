use super::*;

/// `metadata`'s JSON view exposes per-package build flags
/// under `toolchain.build_flags_per_package`.  Returns the
/// first package's block; tests in this module declare one
/// primary package.
fn package_build_flags(view: &serde_json::Value) -> &serde_json::Value {
    let per_package = view["toolchain"]["build_flags_per_package"]
        .as_object()
        .expect("toolchain.build_flags_per_package object");
    per_package
        .values()
        .next()
        .expect("at least one package with build flags")
}

/// Read the build-configuration fingerprint of the
/// (sole) primary package via `cabin explain build-config`.
/// `metadata`'s JSON view only exposes per-package
/// configurations when features are non-empty; `explain
/// build-config` always renders the full configuration block
/// including the fingerprint, so we route the fingerprint
/// assertions through it.
fn fingerprint_for(dir: &Path, cmd_env: &[(&str, &str)], package: &str) -> String {
    let mut cmd = cabin();
    for (k, v) in cmd_env {
        cmd.env(k, v);
    }
    let assertion = cmd
        .current_dir(dir)
        .args(["explain", "build-config", package, "--format", "json"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("explain JSON");
    v["configuration"]["fingerprint"]
        .as_str()
        .expect("fingerprint string")
        .to_owned()
}

fn metadata_view(cmd_env: &[(&str, &str)], dir: &Path) -> serde_json::Value {
    let mut cmd = cabin();
    for (k, v) in cmd_env {
        cmd.env(k, v);
    }
    let assertion = cmd.current_dir(dir).arg("metadata").assert().success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    serde_json::from_str(&stdout).expect("metadata stdout must be JSON")
}

#[test]
fn cppflags_appear_in_language_neutral_compile_args() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let view = metadata_view(
        &[("CPPFLAGS", "-DENV_FROM_CPP=1 -I/opt/include")],
        dir.path(),
    );
    let pkg = package_build_flags(&view);
    let extras: Vec<String> = pkg["extra_compile_args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(
        extras.contains(&"-DENV_FROM_CPP=1".to_owned()),
        "CPPFLAGS must reach language-neutral bucket: {extras:?}",
    );
    assert!(
        extras.contains(&"-I/opt/include".to_owned()),
        "CPPFLAGS tokens preserved verbatim: {extras:?}",
    );
    // CPPFLAGS must not leak into the C-only / C++-only
    // buckets - that would defeat the documented per-bucket
    // routing.
    let c_only: Vec<String> = pkg["cflags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    let cxx_only: Vec<String> = pkg["cxxflags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(
        c_only.is_empty(),
        "CPPFLAGS must not enter C-only bucket: {c_only:?}"
    );
    assert!(
        cxx_only.is_empty(),
        "CPPFLAGS must not enter C++-only bucket: {cxx_only:?}"
    );
}

#[test]
fn cflags_only_reach_c_compile_bucket() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let view = metadata_view(&[("CFLAGS", "-std=c11 -Wmissing-prototypes")], dir.path());
    let pkg = package_build_flags(&view);
    let c_only: Vec<String> = pkg["cflags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(c_only, vec!["-std=c11", "-Wmissing-prototypes"]);
    let cxx_only: Vec<String> = pkg["cxxflags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(
        cxx_only.is_empty(),
        "CFLAGS must never reach C++ bucket: {cxx_only:?}"
    );
    let link: Vec<String> = pkg["ldflags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(
        link.is_empty(),
        "CFLAGS must never reach link bucket: {link:?}"
    );
}

#[test]
fn cxxflags_only_reach_cxx_compile_bucket() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let view = metadata_view(&[("CXXFLAGS", "-fno-rtti -fno-exceptions")], dir.path());
    let pkg = package_build_flags(&view);
    let cxx_only: Vec<String> = pkg["cxxflags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(cxx_only, vec!["-fno-rtti", "-fno-exceptions"]);
    let c_only: Vec<String> = pkg["cflags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(
        c_only.is_empty(),
        "CXXFLAGS must never reach C bucket: {c_only:?}"
    );
}

#[test]
fn ldflags_only_reach_link_bucket() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let view = metadata_view(&[("LDFLAGS", "-L/opt/lib -lextra")], dir.path());
    let pkg = package_build_flags(&view);
    let link: Vec<String> = pkg["ldflags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(link, vec!["-L/opt/lib", "-lextra"]);
    let extras: Vec<String> = pkg["extra_compile_args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(
        extras.is_empty(),
        "LDFLAGS must never reach compile bucket: {extras:?}"
    );
}

#[test]
fn empty_and_whitespace_env_vars_are_ignored() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    // Metadata only emits a per-package build-flags block
    // when the package contributes at least one non-empty
    // bucket.  Empty / whitespace env vars must produce no
    // contribution, so the map stays empty and the
    // fingerprint matches the no-env baseline.
    let view_empty = metadata_view(
        &[
            ("CPPFLAGS", ""),
            ("CFLAGS", "  \t  "),
            ("CXXFLAGS", "\n"),
            ("LDFLAGS", ""),
        ],
        dir.path(),
    );
    let per_package = view_empty["toolchain"]["build_flags_per_package"]
        .as_object()
        .expect("toolchain.build_flags_per_package object");
    assert!(
        per_package.is_empty(),
        "empty / whitespace env vars must produce no flag contribution: {per_package:?}",
    );

    // Mirror through the fingerprint: identical to the
    // unset baseline.
    let base_fp = fingerprint_for(dir.path(), &[], "hello");
    let empty_fp = fingerprint_for(
        dir.path(),
        &[
            ("CPPFLAGS", ""),
            ("CFLAGS", "  \t  "),
            ("CXXFLAGS", "\n"),
            ("LDFLAGS", ""),
        ],
        "hello",
    );
    assert_eq!(base_fp, empty_fp);
}

#[test]
fn quoted_and_escaped_arguments_parse_correctly() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    // Single-quoted run preserves spaces verbatim; the whole
    // -DNAME="hello world" is one argv element.
    let view = metadata_view(&[("CXXFLAGS", "-DNAME='hello world' -O\\ 2")], dir.path());
    let pkg = package_build_flags(&view);
    let cxx: Vec<String> = pkg["cxxflags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        cxx,
        vec!["-DNAME=hello world".to_owned(), "-O 2".to_owned(),],
    );
}

#[test]
fn malformed_quote_errors_name_variable() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let assertion = cabin()
        .current_dir(dir.path())
        .env("CXXFLAGS", "'oops")
        .arg("metadata")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("CXXFLAGS"),
        "error must name CXXFLAGS: {stderr}"
    );
    assert!(
        stderr.contains("shell"),
        "error must explain the parse issue: {stderr}",
    );
}

#[test]
fn malformed_escape_errors_name_variable() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let assertion = cabin()
        .current_dir(dir.path())
        .env("LDFLAGS", "-L/lib\\")
        .arg("metadata")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("LDFLAGS"),
        "error must name LDFLAGS: {stderr}"
    );
    assert!(
        stderr.contains("shell"),
        "error must explain the parse issue: {stderr}",
    );
}

#[test]
fn order_preserved_within_a_single_variable() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let view = metadata_view(&[("CPPFLAGS", "-Dfirst -Dsecond -Dthird")], dir.path());
    let pkg = package_build_flags(&view);
    let extras: Vec<String> = pkg["extra_compile_args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(extras, vec!["-Dfirst", "-Dsecond", "-Dthird"]);
}

#[test]
fn env_flags_append_after_manifest_layer() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "hello"
version = "0.1.0"

[target.hello]
type = "executable"
sources = ["src/main.cc"]

[profile]
cxxflags = ["-DFROM_MANIFEST"]
ldflags = ["-Wl,--as-needed"]
"#,
        )
        .unwrap();
    dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
    let view = metadata_view(
        &[("CXXFLAGS", "-DFROM_ENV"), ("LDFLAGS", "-L/from/env")],
        dir.path(),
    );
    let pkg = package_build_flags(&view);
    let cxx: Vec<String> = pkg["cxxflags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        cxx,
        vec!["-DFROM_MANIFEST", "-DFROM_ENV"],
        "env flags must append *after* manifest [profile] flags",
    );
    let link: Vec<String> = pkg["ldflags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(link, vec!["-Wl,--as-needed", "-L/from/env"]);
}

#[test]
fn fingerprint_changes_when_env_flag_changes() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());

    let base_fp = fingerprint_for(dir.path(), &[], "hello");

    let cpp_fp = fingerprint_for(dir.path(), &[("CPPFLAGS", "-DENV1=1")], "hello");
    assert_ne!(base_fp, cpp_fp, "CPPFLAGS change must move the fingerprint");

    let cflags_fp = fingerprint_for(dir.path(), &[("CFLAGS", "-std=c11")], "hello");
    assert_ne!(
        base_fp, cflags_fp,
        "CFLAGS change must move the fingerprint"
    );
    assert_ne!(
        cpp_fp, cflags_fp,
        "CFLAGS and CPPFLAGS must hash into different fingerprints because they route to different buckets",
    );

    let cxx_fp = fingerprint_for(dir.path(), &[("CXXFLAGS", "-fno-rtti")], "hello");
    assert_ne!(base_fp, cxx_fp, "CXXFLAGS change must move the fingerprint");

    let ld_fp = fingerprint_for(dir.path(), &[("LDFLAGS", "-L/opt")], "hello");
    assert_ne!(base_fp, ld_fp, "LDFLAGS change must move the fingerprint");
}

#[test]
fn fingerprint_is_deterministic_for_identical_env() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let env: &[(&str, &str)] = &[
        ("CPPFLAGS", "-DSHARED=1"),
        ("CFLAGS", "-std=c11"),
        ("CXXFLAGS", "-std=c++20"),
        ("LDFLAGS", "-L/opt/lib"),
    ];
    assert_eq!(
        fingerprint_for(dir.path(), env, "hello"),
        fingerprint_for(dir.path(), env, "hello"),
    );
}

#[cfg(unix)]
#[test]
fn cabin_build_emits_cppflags_into_compile_commands_for_cxx_sources() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let build_dir = dir.path().join("build");
    cabin()
        .current_dir(dir.path())
        .env("CPPFLAGS", "-DBUILD_FROM_ENV")
        .args(["build", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();
    let cc = std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
    assert!(
        cc.contains("-DBUILD_FROM_ENV"),
        "CPPFLAGS must appear in compile_commands.json: {cc}",
    );
}

#[cfg(unix)]
#[test]
fn cabin_build_emits_cxxflags_only_for_cxx_translation_units() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "mixed"
version = "0.1.0"

[target.mixed]
type = "executable"
sources = ["src/main.cc", "src/helper.c"]
"#,
        )
        .unwrap();
    dir.child("src/main.cc")
        .write_str("extern \"C\" void helper(void);\nint main() { helper(); return 0; }\n")
        .unwrap();
    dir.child("src/helper.c")
        .write_str("void helper(void) {}\n")
        .unwrap();
    let build_dir = dir.path().join("build");
    cabin()
        .current_dir(dir.path())
        .env("CXXFLAGS", "-DSEEN_BY_CXX_ONLY")
        .env("CFLAGS", "-DSEEN_BY_C_ONLY")
        .args(["build", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();
    let cc = std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
    let view: serde_json::Value = serde_json::from_str(&cc).unwrap();
    let entries = view.as_array().expect("compile_commands is an array");
    let mut cxx_seen = false;
    let mut c_seen = false;
    for entry in entries {
        let file = entry["file"].as_str().unwrap();
        // The compile DB stores each invocation as a single
        // `command` string; the planner does not emit the
        // alternate `arguments` array form.
        let command = entry["command"].as_str().unwrap();
        if file.ends_with("main.cc") {
            cxx_seen = true;
            assert!(
                command.contains("-DSEEN_BY_CXX_ONLY"),
                "C++ compile must include CXXFLAGS: {command}",
            );
            assert!(
                !command.contains("-DSEEN_BY_C_ONLY"),
                "C++ compile must NOT include CFLAGS: {command}",
            );
        } else if file.ends_with("helper.c") {
            c_seen = true;
            assert!(
                command.contains("-DSEEN_BY_C_ONLY"),
                "C compile must include CFLAGS: {command}",
            );
            assert!(
                !command.contains("-DSEEN_BY_CXX_ONLY"),
                "C compile must NOT include CXXFLAGS: {command}",
            );
        }
    }
    assert!(
        cxx_seen && c_seen,
        "expected both C/C++ entries in the compile DB"
    );
}

#[cfg(unix)]
#[test]
fn cabin_build_ldflags_appear_in_ninja_link_command() {
    require_cxx_build_tools();
    // Use a benign LDFLAG the host linker accepts silently
    // so the build phase succeeds and we can read the
    // generated artifacts. `-L<path>` adds a library search
    // path with no requirement that the path exist.
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let build_dir = dir.path().join("build");
    let distinctive = "-L/this/path/should/not/exist/very-distinctive";
    cabin()
        .current_dir(dir.path())
        .env("LDFLAGS", distinctive)
        .args(["build", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();
    let ninja_text = std::fs::read_to_string(build_dir.join("dev").join("build.ninja")).unwrap();
    assert!(
        ninja_text.contains(distinctive),
        "LDFLAGS must reach the link command in build.ninja: {ninja_text}",
    );
    // And must NOT contaminate compile lines.
    let cc = std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
    assert!(
        !cc.contains(distinctive),
        "LDFLAGS must NOT appear in compile_commands.json: {cc}",
    );
}

#[cfg(unix)]
#[test]
fn ninja_rebuilds_when_cxxflags_change() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let build_dir = dir.path().join("build");
    cabin()
        .current_dir(dir.path())
        .env("CXXFLAGS", "-DFIRST_BUILD")
        .args(["build", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();
    let first = std::fs::read_to_string(build_dir.join("dev").join("build.ninja")).unwrap();
    cabin()
        .current_dir(dir.path())
        .env("CXXFLAGS", "-DSECOND_BUILD")
        .args(["build", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();
    let second = std::fs::read_to_string(build_dir.join("dev").join("build.ninja")).unwrap();
    assert!(
        first.contains("-DFIRST_BUILD") && !first.contains("-DSECOND_BUILD"),
        "first build.ninja should pin the first flag value",
    );
    assert!(
        second.contains("-DSECOND_BUILD") && !second.contains("-DFIRST_BUILD"),
        "second build.ninja should pin the second flag value",
    );
}

#[cfg(unix)]
#[test]
fn cabin_run_build_phase_uses_env_flags() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let build_dir = dir.path().join("build");
    cabin()
        .current_dir(dir.path())
        .env("CPPFLAGS", "-DRUN_PHASE_FLAG")
        .args(["run", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();
    let cc = std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
    assert!(
        cc.contains("-DRUN_PHASE_FLAG"),
        "cabin run must propagate CPPFLAGS to the build phase: {cc}",
    );
}

#[cfg(unix)]
#[test]
fn cabin_test_build_phase_uses_env_flags() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "hello"
version = "0.1.0"

[target.smoke]
type = "test"
sources = ["src/test.cc"]
"#,
        )
        .unwrap();
    dir.child("src/test.cc")
        .write_str("int main() { return 0; }\n")
        .unwrap();
    let build_dir = dir.path().join("build");
    cabin()
        .current_dir(dir.path())
        .env("CXXFLAGS", "-DTEST_PHASE_FLAG")
        .args(["test", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();
    let cc = std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
    assert!(
        cc.contains("-DTEST_PHASE_FLAG"),
        "cabin test must propagate CXXFLAGS to the build phase: {cc}",
    );
}

/// `cabin tidy` regenerates the compile database from the
/// same build planner the other commands use, so env flags
/// must reach the on-disk `compile_commands.json` it writes.
#[cfg(unix)]
#[test]
fn cabin_tidy_compile_db_sees_env_flags() {
    require_cxx_build_tools();
    // Use the fake tidy so the test does not require a real
    // clang-tidy install; cabin still regenerates the
    // compile database before invoking the tool. `cabin
    // tidy` reads the build directory via `CABIN_BUILD_DIR`.
    let fake_tidy = fake_tidy_path();
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let build_dir = dir.path().join("build");
    cabin()
        .current_dir(dir.path())
        .env("CABIN_TIDY", &fake_tidy)
        .env("CABIN_BUILD_DIR", &build_dir)
        .env("CPPFLAGS", "-DTIDY_DB_SEES_THIS")
        .arg("tidy")
        .assert()
        .success();
    let cc = std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
    assert!(
        cc.contains("-DTIDY_DB_SEES_THIS"),
        "cabin tidy compile DB must include CPPFLAGS: {cc}",
    );
}

/// Mirrors the bundled fake-binary lookup the tidy module
/// uses; we keep it local rather than re-export across mod
/// boundaries.  Only the Unix-only `cabin_tidy_compile_db_sees_env_flags`
/// test uses it.
#[cfg(unix)]
fn fake_tidy_path() -> std::path::PathBuf {
    let test_exe = std::env::current_exe().expect("current_exe");
    let mut dir = test_exe
        .parent()
        .expect("test exe should live in a directory")
        .to_path_buf();
    if dir.file_name().and_then(|n| n.to_str()) == Some("deps") {
        dir.pop();
    }
    let candidate = dir.join(format!(
        "cabin-tidy-fake-tidy{}",
        std::env::consts::EXE_SUFFIX
    ));
    assert!(
        candidate.is_file(),
        "expected fake tidy at {}; build cabin-tidy with `--features test-fake-tidy`",
        candidate.display(),
    );
    candidate
}

/// pkg-config and env-flag layers must coexist deterministically.
/// pkg-config goes in first (already merged into
/// `ResolvedProfileFlags` by `augment_build_flags_with_system_deps`),
/// then env flags append.
#[test]
fn pkg_config_and_env_flags_coexist_in_documented_order() {
    let fixtures = system_deps_pkg_config::Fixtures::new();
    fixtures.write(
        "zlib",
        r#"{
                "version": "1.2.13",
                "cflags": "-I/opt/zlib/include -DZLIB_CONST",
                "libs": "-L/opt/zlib/lib -lz"
            }"#,
    );
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "hello"
version = "0.1.0"

[target.hello]
type = "executable"
sources = ["src/main.cc"]

[dependencies]
zlib = { version = "", system = true }
"#,
        )
        .unwrap();
    dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();

    let mut cmd = system_deps_pkg_config::cabin_with_fake_pkg_config(&fixtures);
    cmd.env("CPPFLAGS", "-DFROM_ENV");
    cmd.env("LDFLAGS", "-L/from/env");
    let assertion = cmd
        .current_dir(dir.path())
        .arg("metadata")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    let view: serde_json::Value = serde_json::from_str(&stdout).expect("metadata JSON");
    let pkg = package_build_flags(&view);
    let extras: Vec<String> = pkg["extra_compile_args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    // pkg-config contributed `-DZLIB_CONST`; env adds
    // `-DFROM_ENV`.  The pkg-config entry must come first.
    let env_pos = extras
        .iter()
        .position(|s| s == "-DFROM_ENV")
        .expect("env CPPFLAGS present");
    let pkg_pos = extras
        .iter()
        .position(|s| s == "-DZLIB_CONST")
        .expect("pkg-config define present");
    assert!(
        pkg_pos < env_pos,
        "pkg-config define must precede env CPPFLAGS in deterministic order: {extras:?}",
    );

    let link: Vec<String> = pkg["ldflags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    let env_link = link
        .iter()
        .position(|s| s == "-L/from/env")
        .expect("env LDFLAGS present in link args");
    let pkg_link = link
        .iter()
        .position(|s| s == "-L/opt/zlib/lib")
        .expect("pkg-config -L present in link args");
    assert!(
        pkg_link < env_link,
        "pkg-config link flags must precede env LDFLAGS: {link:?}",
    );
}

/// `cabin fmt` must ignore the build-flag environment.
/// Regression: a bad CFLAGS should not block formatter
/// invocations.
#[test]
fn cabin_fmt_unaffected_by_build_flag_env() {
    // `cabin fmt --check` may succeed or fail depending on
    // whether a real `clang-format` is on PATH; either is
    // acceptable.  The only behavior we forbid is the
    // env-flag parser leaking through - stderr must never
    // name CXXFLAGS for an `fmt` invocation.
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let assertion = cabin()
        .current_dir(dir.path())
        .env("CXXFLAGS", "'never parsed")
        .arg("fmt")
        .arg("--check")
        .assert();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        !stderr.contains("CXXFLAGS"),
        "cabin fmt must not parse CXXFLAGS: {stderr}",
    );
}

/// `cabin clean`, `cabin new`, `cabin init` are workspace /
/// scaffold commands that must not be affected by build
/// flags.
#[test]
fn cabin_new_unaffected_by_build_flag_env() {
    let dir = TempDir::new().unwrap();
    let assertion = cabin()
        .current_dir(dir.path())
        .env("CXXFLAGS", "'unterminated")
        .args(["new", "demo", "--bin"])
        .assert();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        !stderr.contains("CXXFLAGS"),
        "cabin new must not parse CXXFLAGS: {stderr}",
    );
}

#[test]
fn cabin_init_unaffected_by_build_flag_env() {
    let dir = TempDir::new().unwrap();
    let assertion = cabin()
        .current_dir(dir.path())
        .env("CPPFLAGS", "'unterminated")
        .args(["init", "--bin"])
        .assert();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        !stderr.contains("CPPFLAGS"),
        "cabin init must not parse CPPFLAGS: {stderr}",
    );
}

#[test]
fn cabin_clean_unaffected_by_build_flag_env() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let assertion = cabin()
        .current_dir(dir.path())
        .env("LDFLAGS", "-L/lib\\")
        .arg("clean")
        .assert();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        !stderr.contains("LDFLAGS"),
        "cabin clean must not parse LDFLAGS: {stderr}",
    );
}

/// `cabin metadata --format json` must stay parseable JSON
/// on stdout even with env flags active and verbose enabled.
#[test]
fn metadata_stdout_stays_clean_with_env_flags_under_verbose() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let assertion = cabin()
        .current_dir(dir.path())
        .env("CPPFLAGS", "-DCHATTY=1")
        .env("LDFLAGS", "-L/opt/lib")
        .args(["-v", "metadata"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    let _view: serde_json::Value =
        serde_json::from_str(&stdout).expect("metadata stdout JSON under -v");
}
