//! End-to-end coverage for the experimental `standard-compat`
//! post-resolution warning pass (`-Z standard-compat`).
//!
//! Each violation class of
//! `docs/design/standard-compatibility/spec.md` D9 gets a fixture:
//! a plain interface-minimum violation (row 2), forbidden via
//! `interface-cxx-standard = "none"` (row 1), transitive provenance
//! through a public edge (D10), header-only inference (row 3), and
//! a mixed-language consumer violating both languages on one edge
//! (D13).  The feature-off runs pin the no-op guarantee, and the
//! registry fixture covers both the fresh-resolution and the
//! lockfile-load paths.
//!
//! The warnings are diagnostic-only.  Where the always-on
//! build-time interface enforcement also trips (it deliberately
//! differs from the resolver-level model), the command still
//! fails, and the warning must render *before* that failure.  The
//! `"none"` fixtures succeed, proving warnings never gate a
//! command.

use super::*;

/// Collapse miette's graphical wrapping so phrase assertions hold
/// regardless of where long messages (which embed tempdir paths)
/// wrap: drop the box-drawing gutter and rejoin whitespace.
fn flatten(stderr: &str) -> String {
    stderr
        .replace(['│', '╰', '╭', '─', '·'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

const WARNING_CODE: &str = "cabin::language::standard_compat_violation";

/// app (c++17) -> lib declaring `interface-cxx-standard = "c++20"`.
fn write_direct_minimum_fixture(dir: &Path) {
    assert_fs::fixture::ChildPath::new(dir.join("lib/cabin.toml"))
        .write_str(
            r#"[package]
name = "lib"
version = "0.1.0"

[target.lib]
type = "library"
sources = ["src/lib.cc"]
include-dirs = ["include"]
cxx-standard = "c++20"
interface-cxx-standard = "c++20"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("lib/include/lib.h"))
        .write_str("#pragma once\nint lib_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("lib/src/lib.cc"))
        .write_str("#include \"lib.h\"\nint lib_value() { return 7; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
lib = { path = "../lib" }

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["lib"]
cxx-standard = "c++17"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("app/src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
}

#[test]
fn direct_minimum_violation_warns_before_the_build_fails() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_direct_minimum_fixture(dir.path());
    let assertion = cabin()
        .args(["build", "-Z", "standard-compat", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        // The always-on build-time enforcement still fails this
        // graph; the warning renders first.
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    let flat = flatten(&stderr);
    assert!(
        stderr.contains(WARNING_CODE),
        "expected the stable code in: {stderr}"
    );
    assert!(
        flat.contains(
            "target `app:app` compiles C++ as `c++17`, but its dependency `lib:lib` requires \
             C++ consumers at `c++20` or newer"
        ),
        "expected the core warning sentence in: {flat}"
    );
    assert!(
        flat.contains("`interface-cxx-standard` in"),
        "expected the origin declaration citation in: {flat}"
    );
    // The label points at the consumer's own standard declaration.
    assert!(
        stderr.contains(r#"cxx-standard = "c++17""#),
        "expected the consumer manifest snippet in: {stderr}"
    );
    assert!(
        flat.contains("`app:app` compiles C++ as `c++17`"),
        "expected the snippet label in: {flat}"
    );
    assert!(
        flat.contains("raise `app:app`'s C++ standard to at least `c++20`"),
        "expected the raise remedy in: {flat}"
    );
}

/// The no-op guarantee: without `-Z standard-compat`, the same
/// graph produces no trace of the pass.
#[test]
fn feature_off_emits_no_warning() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_direct_minimum_fixture(dir.path());
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        !stderr.contains("standard_compat"),
        "the pass must leave no trace when off: {stderr}"
    );
    assert!(
        !stderr.contains('⚠'),
        "no warning may render when the feature is off: {stderr}"
    );
}

/// `interface-cxx-standard = "none"` warns as forbidden, and -
/// unlike the minimum-violation fixtures - the build succeeds:
/// warnings never gate a command.
#[test]
fn declared_none_warns_without_failing_the_build() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("lib/cabin.toml"))
        .write_str(
            r#"[package]
name = "lib"
version = "0.1.0"

[target.lib]
type = "library"
sources = ["src/lib.cc"]
include-dirs = ["include"]
cxx-standard = "c++17"
interface-cxx-standard = "none"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("lib/include/lib.h"))
        .write_str("#pragma once\nint lib_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("lib/src/lib.cc"))
        .write_str("#include \"lib.h\"\nint lib_value() { return 7; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
lib = { path = "../lib" }

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["lib"]
cxx-standard = "c++17"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
    let assertion = cabin()
        .args(["build", "-Z", "standard-compat", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    let flat = flatten(&stderr);
    assert!(
        stderr.contains(WARNING_CODE),
        "expected the stable code in: {stderr}"
    );
    assert!(
        flat.contains(
            "its dependency `lib:lib` cannot be consumed from C++: C++ consumption was \
             disabled by `interface-cxx-standard = \"none\"`"
        ),
        "expected the disabled-consumption wording in: {flat}"
    );
    assert!(
        flat.contains("`lib:lib` cannot be consumed from C++ at any standard level"),
        "expected the forbidden help in: {flat}"
    );
}

/// A requirement declared two public edges down reaches the
/// consumer, and the warning names the chain and the origin.
#[test]
fn transitive_public_requirement_names_the_chain() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("libb/cabin.toml"))
        .write_str(
            r#"[package]
name = "libb"
version = "0.1.0"

[target.libb]
type = "library"
sources = ["src/b.cc"]
include-dirs = ["include"]
cxx-standard = "c++20"
interface-cxx-standard = "c++20"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("libb/include/b.h"))
        .write_str("#pragma once\nint b_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("libb/src/b.cc"))
        .write_str("#include \"b.h\"\nint b_value() { return 2; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("liba/cabin.toml"))
        .write_str(
            r#"[package]
name = "liba"
version = "0.1.0"

[dependencies]
libb = { path = "../libb" }

[target.liba]
type = "library"
sources = ["src/a.cc"]
include-dirs = ["include"]
deps = [{ name = "libb", public = true }]
cxx-standard = "c++20"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("liba/include/a.h"))
        .write_str("#pragma once\nint a_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("liba/src/a.cc"))
        .write_str("#include \"a.h\"\n#include \"b.h\"\nint a_value() { return b_value() + 1; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
liba = { path = "../liba" }

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["liba"]
cxx-standard = "c++17"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
    let assertion = cabin()
        .args(["build", "-Z", "standard-compat", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    let flat = flatten(&stderr);
    assert!(
        stderr.contains(WARNING_CODE),
        "expected the stable code in: {stderr}"
    );
    assert!(
        flat.contains(
            "its dependency `liba:liba` requires C++ consumers at `c++20` or newer, imposed \
             by `libb:libb` via public dependency chain `liba:liba` -> `libb:libb`"
        ),
        "expected the provenance chain in: {flat}"
    );
}

/// A header-only dependency without a C++ interface declaration
/// infers its minimum from its implementation standard, and the
/// warning is marked as inferred.
#[test]
fn header_only_inference_is_marked_as_inferred() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // The C interface declaration satisfies the header-only
    // at-least-one-interface rule while leaving the C++ side to
    // inference (spec D9 row 3).
    assert_fs::fixture::ChildPath::new(dir.path().join("hdr/cabin.toml"))
        .write_str(
            r#"[package]
name = "hdr"
version = "0.1.0"

[target.hdr]
type = "header-only"
include-dirs = ["include"]
cxx-standard = "c++20"
interface-c-standard = "c99"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("hdr/include/hdr.h"))
        .write_str("#pragma once\n#define HDR_VALUE 3\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
hdr = { path = "../hdr" }

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["hdr"]
cxx-standard = "c++17"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
    let assertion = cabin()
        .args(["build", "-Z", "standard-compat", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    let flat = flatten(&stderr);
    assert!(
        stderr.contains(WARNING_CODE),
        "expected the stable code in: {stderr}"
    );
    assert!(
        flat.contains(
            "its dependency `hdr:hdr` requires C++ consumers at `c++20` or newer (inferred \
             from implementation standard: `cxx-standard` in"
        ),
        "expected the inference marker in: {flat}"
    );
}

/// A mixed-language consumer reports each violated language as its
/// own warning on the same edge, C first.
#[test]
fn mixed_language_consumer_warns_per_language() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("w/cabin.toml"))
        .write_str(
            r#"[package]
name = "w"
version = "0.1.0"

[target.w]
type = "library"
sources = ["src/w.c"]
include-dirs = ["include"]
c-standard = "c17"
interface-c-standard = "c17"
interface-cxx-standard = "c++23"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("w/include/w.h"))
        .write_str("#pragma once\nint w_value(void);\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("w/src/w.c"))
        .write_str("#include \"w.h\"\nint w_value(void) { return 5; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"
cxx-standard = "c++20"

[dependencies]
w = { path = "../w" }

[target.app]
type = "executable"
sources = ["src/part.c", "src/main.cc"]
deps = ["w"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/part.c"))
        .write_str("int part_value(void) { return 1; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
    let assertion = cabin()
        .args(["build", "-Z", "standard-compat", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    let flat = flatten(&stderr);
    assert!(
        flat.contains(
            "target `app:app` compiles C as `c11`, but its dependency `w:w` requires C \
             consumers at `c17` or newer"
        ),
        "expected the C-side warning in: {flat}"
    );
    assert!(
        flat.contains(
            "target `app:app` compiles C++ as `c++20`, but its dependency `w:w` requires \
             C++ consumers at `c++23` or newer"
        ),
        "expected the C++-side warning in: {flat}"
    );
    // Two warnings on the one edge: the code renders once each.
    assert_eq!(
        stderr.matches(WARNING_CODE).count(),
        2,
        "expected exactly two warnings in: {stderr}"
    );
}

/// A registry dependency warns on the fresh-resolution build, gets
/// the pin remedy, and warns again on the follow-up build that
/// loads the existing lockfile.
#[test]
fn registry_dependency_warns_on_fresh_and_lockfile_paths() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();

    let pkg_root = dir.path().join("pkg");
    assert_fs::fixture::ChildPath::new(pkg_root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "libnone"
version = "0.1.0"
cxx-standard = "c++17"
interface-cxx-standard = "none"

[target.libnone]
type = "library"
sources = ["src/libnone.cc"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join("include/libnone.h"))
        .write_str("#pragma once\nint libnone_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join("src/libnone.cc"))
        .write_str("#include \"libnone.h\"\nint libnone_value() { return 9; }\n")
        .unwrap();
    let registry = dir.path().join("registry");
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();

    let app_root = dir.path().join("app");
    assert_fs::fixture::ChildPath::new(app_root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[dependencies]
libnone = "0.1.0"

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["libnone"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(app_root.join("src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();

    let build = |build_dir: &str| {
        cabin()
            .args(["build", "-Z", "standard-compat", "--manifest-path"])
            .arg(app_root.join("cabin.toml"))
            .arg("--index-path")
            .arg(&registry)
            .arg("--cache-dir")
            .arg(dir.path().join("cache"))
            .arg("--build-dir")
            .arg(app_root.join(build_dir))
            .assert()
            .success()
    };

    // Fresh resolution: no lockfile exists yet, so the warning
    // carries no lockfile note.
    assert!(!app_root.join("cabin.lock").exists());
    let first = build("build");
    let first_stderr = String::from_utf8_lossy(&first.get_output().stderr).to_string();
    let first_flat = flatten(&first_stderr);
    assert!(
        first_stderr.contains(WARNING_CODE),
        "expected the warning on the fresh-resolution path: {first_stderr}"
    );
    assert!(
        first_flat.contains("C++ consumption was disabled by `interface-cxx-standard = \"none\"`"),
        "expected the disabled-consumption wording in: {first_flat}"
    );
    // The registry dependency gets the pin remedy.
    assert!(
        first_flat.contains("or pin `libnone` to an older version (currently 0.1.0)"),
        "expected the pin remedy in: {first_flat}"
    );
    assert!(
        !first_flat.contains("cabin update"),
        "a fresh-resolution warning must not mention the lockfile: {first_flat}"
    );

    // The first build wrote the lockfile; the second run resolves
    // through it, warns identically, and adds the staleness note.
    assert!(app_root.join("cabin.lock").exists());
    let second = build("build2");
    let second_stderr = String::from_utf8_lossy(&second.get_output().stderr).to_string();
    let second_flat = flatten(&second_stderr);
    assert!(
        second_stderr.contains(WARNING_CODE),
        "expected the warning on the lockfile-load path: {second_stderr}"
    );
    assert!(
        second_flat.contains(
            "this dependency's resolved version was loaded from cabin.lock, which records version"
        ),
        "expected the lockfile note on the lockfile-load path: {second_flat}"
    );
}

/// A lockfile generated while every standard was compatible keeps
/// its bytes when a manifest later lowers the consumer's standard:
/// the lockfile-loaded `cabin build` and `cabin test` both warn
/// with the staleness explanation and the `cabin update` remedy,
/// and the pass never rewrites `cabin.lock` - which records
/// version pins only, no standards and no toolchain information.
#[test]
fn lockfile_loaded_violation_explains_staleness_and_suggests_update() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();

    let pkg_root = dir.path().join("pkg");
    assert_fs::fixture::ChildPath::new(pkg_root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "libiface"
version = "0.1.0"
cxx-standard = "c++20"
interface-cxx-standard = "c++20"

[target.libiface]
type = "library"
sources = ["src/libiface.cc"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join("include/libiface.h"))
        .write_str("#pragma once\nint libiface_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join("src/libiface.cc"))
        .write_str("#include \"libiface.h\"\nint libiface_value() { return 4; }\n")
        .unwrap();
    let registry = dir.path().join("registry");
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();

    let app_root = dir.path().join("app");
    let app_manifest = |cxx_standard: &str| {
        format!(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "{cxx_standard}"

[dependencies]
libiface = "0.1.0"

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["libiface"]

[target.app_test]
type = "test"
sources = ["tests/app_test.cc"]
deps = ["libiface"]
"#
        )
    };
    assert_fs::fixture::ChildPath::new(app_root.join("cabin.toml"))
        .write_str(&app_manifest("c++20"))
        .unwrap();
    assert_fs::fixture::ChildPath::new(app_root.join("src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(app_root.join("tests/app_test.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();

    let run = |subcommand: &str, build_dir: &str| {
        cabin()
            .args([subcommand, "-Z", "standard-compat", "--manifest-path"])
            .arg(app_root.join("cabin.toml"))
            .arg("--index-path")
            .arg(&registry)
            .arg("--cache-dir")
            .arg(dir.path().join("cache"))
            .arg("--build-dir")
            .arg(app_root.join(build_dir))
            .assert()
    };

    // Compatible standards: the build succeeds silently and writes
    // the lockfile.
    let clean = run("build", "build");
    clean.success();
    let lock_path = app_root.join("cabin.lock");
    let locked_bytes = std::fs::read(&lock_path).unwrap();
    let lock_text = String::from_utf8(locked_bytes.clone()).unwrap();
    // Version pins only: standards, toolchains, and fingerprints
    // never reach the lockfile, so it stays shareable across
    // platforms and toolchains even though every manifest in this
    // fixture declares a standard.
    for needle in ["standard", "c++", "toolchain", "fingerprint"] {
        assert!(
            !lock_text.contains(needle),
            "the lockfile must not record `{needle}`: {lock_text}"
        );
    }

    // Lower the consumer's standard *after* the lockfile was
    // generated - exactly the staleness story the note explains.
    assert_fs::fixture::ChildPath::new(app_root.join("cabin.toml"))
        .write_str(&app_manifest("c++17"))
        .unwrap();

    for (subcommand, build_dir) in [("build", "build2"), ("test", "build3")] {
        let assertion = run(subcommand, build_dir);
        // The always-on build-time enforcement still fails the
        // command; the warning renders first.
        let assertion = assertion.failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        let flat = flatten(&stderr);
        assert!(
            stderr.contains(WARNING_CODE),
            "expected the warning from `cabin {subcommand}` on the lockfile path: {stderr}"
        );
        assert!(
            flat.contains(
                "this dependency's resolved version was loaded from cabin.lock, which \
                 records version pins only"
            ),
            "expected the pins-only explanation from `cabin {subcommand}`: {flat}"
        );
        assert!(
            flat.contains(
                "if a standard declaration changed in a manifest after the lockfile was \
                 generated, run `cabin update` to re-resolve"
            ),
            "expected the staleness cause and update remedy from `cabin {subcommand}`: {flat}"
        );
    }

    // The warning pass validated a lockfile-loaded graph twice and
    // never rewrote the lockfile.
    assert_eq!(
        std::fs::read(&lock_path).unwrap(),
        locked_bytes,
        "validation must not modify cabin.lock"
    );
}
