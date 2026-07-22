//! End-to-end coverage for the post-resolution standard-compatibility
//! check.
//!
//! Each violation class of
//! `docs/design/standard-compatibility/spec.md` D9 gets a fixture:
//! a plain interface-minimum violation (row 2), forbidden via
//! `interface-cxx-standard = "none"` (row 1), transitive provenance
//! through a public edge (D10), header-only inference (row 3), and
//! a mixed-language consumer violating both languages on one edge
//! (D13).  The registry fixture covers both the fresh-resolution and
//! the lockfile-load paths.
//!
//! The check always runs; there is no opt-in flag.  Violations are
//! errors: they render with the provenance chain (manifest
//! `path:line` references) and fail the command with exit code 1.
//! The `"none"` fixtures - which the always-on build-time
//! enforcement deliberately accepts - prove the gating comes from
//! this check alone.  The one escape hatch is a per-edge
//! `ignore-interface-standard = true` dependency override, which
//! downgrades exactly that edge to an unchecked-edge note.  The
//! removed `standard-compat` feature name is now an ordinary
//! unknown `-Z` value.

use super::*;

/// Collapse miette's graphical wrapping so failure messages stay
/// readable: drop the box-drawing gutter, normalize Windows path
/// separators, and rejoin whitespace.
fn flatten(stderr: &str) -> String {
    stderr
        .replace(['│', '╰', '╭', '─', '·'], " ")
        .replace('\\', "/")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whitespace-insensitive phrase containment.  miette wraps long
/// messages (which embed tempdir paths) at spaces *and* at hyphens
/// inside words like `interface-cxx-standard`, so the only stable
/// comparison strips all whitespace from both sides.  Shared with
/// sibling CLI test modules that assert on wrapped diagnostics.
pub(super) fn flat_contains(stderr: &str, phrase: &str) -> bool {
    fn squash(text: &str) -> String {
        text.replace('\\', "/")
            .chars()
            .filter(|c| !c.is_whitespace() && !matches!(c, '│' | '╰' | '╭' | '─' | '·'))
            .collect()
    }
    squash(stderr).contains(&squash(phrase))
}

const VIOLATION_CODE: &str = "cabin::language::standard_compat_violation";
const UNCHECKED_CODE: &str = "cabin::language::standard_compat_unchecked_edge";

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

/// One library package whose C++ interface is declared `"none"`,
/// plus headers/sources so the build itself is clean: the
/// always-on build-time enforcement accepts these graphs, so any
/// failure is the standard-compat gate itself.
fn write_none_library(dir: &Path, name: &str) {
    assert_fs::fixture::ChildPath::new(dir.join(format!("{name}/cabin.toml")))
        .write_str(&format!(
            r#"[package]
name = "{name}"
version = "0.1.0"

[target.{name}]
type = "library"
sources = ["src/{name}.cc"]
include-dirs = ["include"]
cxx-standard = "c++17"
interface-cxx-standard = "none"
"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join(format!("{name}/include/{name}.h")))
        .write_str(&format!("#pragma once\nint {name}_value();\n"))
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join(format!("{name}/src/{name}.cc")))
        .write_str(&format!(
            "#include \"{name}.h\"\nint {name}_value() {{ return 7; }}\n"
        ))
        .unwrap();
}

/// A violated dependency edge fails the build with exit code 1:
/// the error carries the provenance arrow with `path:line`
/// references into both manifests, and the remedies read raise ->
/// override (a path dependency offers no pin).
#[test]
fn direct_minimum_violation_fails_with_provenance_error() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_direct_minimum_fixture(dir.path());
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .failure()
        .code(1);
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    let flat = flatten(&stderr);
    assert!(
        stderr.contains(VIOLATION_CODE),
        "expected the stable code in: {stderr}"
    );
    // Diagnostics v2: `consumer (standard, path:line) -> dependency
    // requires ... (field, path:line)`.  `cxx-standard` sits on
    // line 12 of app/cabin.toml, `interface-cxx-standard` on line
    // 10 of lib/cabin.toml.
    assert!(
        flat_contains(&stderr, "`app:app` (c++17,"),
        "expected the consumer half of the provenance arrow in: {flat}"
    );
    assert!(
        flat_contains(
            &stderr,
            "app/cabin.toml:12) -> `lib:lib` requires C++ consumers at `c++20` or newer \
             (`interface-cxx-standard`,"
        ),
        "expected the line-referenced provenance arrow in: {flat}"
    );
    assert!(
        flat_contains(&stderr, "lib/cabin.toml:10)"),
        "expected the origin declaration line in: {flat}"
    );
    // The label points at the consumer's own standard declaration.
    assert!(
        stderr.contains(r#"cxx-standard = "c++17""#),
        "expected the consumer manifest snippet in: {stderr}"
    );
    assert!(
        flat_contains(
            &stderr,
            "raise `app:app`'s C++ standard to at least `c++20`"
        ),
        "expected the raise remedy in: {flat}"
    );
    // The always-on build-time enforcement also rejects this
    // minimum violation, so the override remedy - which could not
    // unblock the command - is withheld.
    assert!(
        !flat_contains(&stderr, "ignore-interface-standard"),
        "a minimum violation must not offer the override: {flat}"
    );
    assert!(
        flat_contains(&stderr, "1 standard compatibility violation"),
        "expected the gating summary in: {flat}"
    );
}

/// The check now always runs, so the removed `standard-compat`
/// feature name is an ordinary unknown `-Z` value: naming it is
/// rejected exactly like any other unrecognized feature, with no
/// special-casing and no migration hint.  (Every fixture above
/// exercises the check without passing `-Z`.)
#[test]
fn removed_feature_name_is_an_ordinary_unknown_feature() {
    // Rejected at argument-parse time (exit 2), before any manifest
    // is needed, so this needs no build tools or fixture.
    let removed = cabin()
        .args(["build", "-Z", "standard-compat"])
        .assert()
        .failure()
        .code(2);
    let removed_stderr = String::from_utf8_lossy(&removed.get_output().stderr).to_string();
    assert!(
        removed_stderr.contains("unknown experimental feature 'standard-compat'"),
        "the removed name must read as an unknown feature: {removed_stderr}"
    );
    assert!(
        !removed_stderr.contains("migration") && !removed_stderr.contains("standard-compat-errors"),
        "the removed name must carry no migration diagnostics: {removed_stderr}"
    );
    // Byte-for-byte the same treatment as any other unknown value,
    // save for the echoed name.
    let other = cabin()
        .args(["build", "-Z", "frobnicate"])
        .assert()
        .failure()
        .code(2);
    let other_stderr = String::from_utf8_lossy(&other.get_output().stderr).to_string();
    assert!(
        other_stderr.contains("unknown experimental feature 'frobnicate'"),
        "an arbitrary unknown feature must read the same way: {other_stderr}"
    );
    assert_eq!(
        removed_stderr.replace("standard-compat", "frobnicate"),
        other_stderr,
        "the removed name must be handled identically to any other unknown feature"
    );
}

/// `interface-cxx-standard = "none"` is a violation the always-on
/// build-time enforcement deliberately does not share, so the exit
/// code isolates the promotion: the command fails with exit code 1
/// purely because of the standard-compat error.
#[test]
fn declared_none_fails_the_build_with_an_error() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_none_library(dir.path(), "lib");
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
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .failure()
        .code(1);
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    let flat = flatten(&stderr);
    assert!(
        stderr.contains(VIOLATION_CODE),
        "expected the stable code in: {stderr}"
    );
    assert!(stderr.contains('×'), "expected error severity in: {stderr}");
    assert!(
        flat_contains(
            &stderr,
            "-> `lib:lib` cannot be consumed from C++: C++ consumption was disabled by \
             `interface-cxx-standard = \"none\"`"
        ),
        "expected the disabled-consumption wording in: {flat}"
    );
    assert!(
        flat_contains(
            &stderr,
            "`lib:lib` cannot be consumed from C++ at any standard level"
        ),
        "expected the forbidden help in: {flat}"
    );
    assert!(
        flat_contains(&stderr, "1 standard compatibility violation"),
        "expected the gating summary in: {flat}"
    );
}

/// The per-edge override suppresses exactly one edge: the
/// overridden edge downgrades to an unchecked-edge note while the
/// second violated edge still fails the command.
#[test]
fn override_suppresses_exactly_one_edge() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_none_library(dir.path(), "liba");
    write_none_library(dir.path(), "libb");
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
liba = { path = "../liba", ignore-interface-standard = true }
libb = { path = "../libb" }

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["liba", "libb"]
cxx-standard = "c++17"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .failure()
        .code(1);
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    let flat = flatten(&stderr);
    // Exactly one edge errors (libb) and exactly one goes
    // unchecked (liba).
    assert_eq!(
        stderr.matches(VIOLATION_CODE).count(),
        1,
        "only the non-overridden edge may error: {stderr}"
    );
    assert_eq!(
        stderr.matches(UNCHECKED_CODE).count(),
        1,
        "the overridden edge downgrades to one note: {stderr}"
    );
    assert!(
        flat_contains(&stderr, "-> `libb:libb` cannot be consumed from C++"),
        "expected the libb error in: {flat}"
    );
    assert!(
        flat_contains(
            &stderr,
            "dependency edge `app:app` -> `liba:liba` is unchecked: \
             `ignore-interface-standard = true` is set for `liba` in"
        ),
        "expected the unchecked-edge note in: {flat}"
    );
    assert!(
        flat_contains(&stderr, "1 standard compatibility violation"),
        "the suppressed edge must not count toward the gate: {flat}"
    );
}

/// With every violated edge overridden, the command succeeds and
/// the downgraded note is the only trace: the edge is unchecked,
/// not silently forgotten.
#[test]
fn override_downgrades_to_note_and_succeeds() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_none_library(dir.path(), "liba");
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
liba = { path = "../liba", ignore-interface-standard = true }

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
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    let flat = flatten(&stderr);
    assert!(
        stderr.contains(UNCHECKED_CODE),
        "expected the unchecked-edge note in: {stderr}"
    );
    assert!(
        !stderr.contains(VIOLATION_CODE),
        "no violation may render for an overridden edge: {stderr}"
    );
    assert!(
        !stderr.contains('×') && !stderr.contains('⚠'),
        "the note renders below warning severity: {stderr}"
    );
    assert!(
        flat_contains(
            &stderr,
            "remove `ignore-interface-standard` from the `[dependencies]` entry to \
             re-enable the check"
        ),
        "expected the re-enable help in: {flat}"
    );
}

/// A requirement declared two public edges down reaches the
/// consumer, and the error names the origin and its manifest line.
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
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    let flat = flatten(&stderr);
    assert!(
        stderr.contains(VIOLATION_CODE),
        "expected the stable code in: {stderr}"
    );
    assert!(
        flat_contains(
            &stderr,
            "-> `liba:liba` requires C++ consumers at `c++20` or newer via public \
             dependency `libb:libb` (`interface-cxx-standard`,"
        ),
        "expected the provenance chain in: {flat}"
    );
    assert!(
        flat_contains(&stderr, "libb/cabin.toml:10)"),
        "expected the origin declaration line in: {flat}"
    );
}

/// A header-only dependency without a C++ interface declaration
/// infers its minimum from its implementation standard, and the
/// error is marked as inferred.
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
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    let flat = flatten(&stderr);
    assert!(
        stderr.contains(VIOLATION_CODE),
        "expected the stable code in: {stderr}"
    );
    assert!(
        flat_contains(
            &stderr,
            "-> `hdr:hdr` requires C++ consumers at `c++20` or newer (inferred from \
             implementation standard `cxx-standard`,"
        ),
        "expected the inference marker in: {flat}"
    );
}

/// A mixed-language consumer reports each violated language as its
/// own error on the same edge, C first, and the gating summary
/// counts both.
#[test]
fn mixed_language_consumer_errors_per_language() {
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
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    let flat = flatten(&stderr);
    assert!(
        flat_contains(&stderr, "`app:app` (c11,")
            && flat_contains(&stderr, "-> `w:w` requires C consumers at `c17` or newer"),
        "expected the C-side error in: {flat}"
    );
    assert!(
        flat_contains(&stderr, "`app:app` (c++20,")
            && flat_contains(
                &stderr,
                "-> `w:w` requires C++ consumers at `c++23` or newer"
            ),
        "expected the C++-side error in: {flat}"
    );
    // Two errors on the one edge: the code renders once each.
    assert_eq!(
        stderr.matches(VIOLATION_CODE).count(),
        2,
        "expected exactly two errors in: {stderr}"
    );
    assert!(
        flat_contains(&stderr, "2 standard compatibility violations"),
        "expected the gating summary to count both in: {flat}"
    );
}

/// A registry dependency errors on the fresh-resolution build -
/// which still writes the lockfile - gets the pin and override
/// remedies, and errors again on the follow-up build that loads
/// the existing lockfile, now with the staleness note.
#[test]
fn registry_dependency_errors_on_fresh_and_lockfile_paths() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();

    let pkg_root = dir.path().join("pkg");
    assert_fs::fixture::ChildPath::new(pkg_root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "acme/libnone"
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
"acme/libnone" = "0.1.0"

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["acme/libnone"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(app_root.join("src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();

    let build = |build_dir: &str| {
        cabin()
            .args(["build", "--manifest-path"])
            .arg(app_root.join("cabin.toml"))
            .arg("--index-path")
            .arg(&registry)
            .arg("--cache-dir")
            .arg(dir.path().join("cache"))
            .arg("--build-dir")
            .arg(app_root.join(build_dir))
            .assert()
            .failure()
            .code(1)
    };

    // Fresh resolution: no lockfile exists yet, so the error
    // carries no lockfile note.
    assert!(!app_root.join("cabin.lock").exists());
    let first = build("build");
    let first_stderr = String::from_utf8_lossy(&first.get_output().stderr).to_string();
    let first_flat = flatten(&first_stderr);
    assert!(
        first_stderr.contains(VIOLATION_CODE),
        "expected the error on the fresh-resolution path: {first_stderr}"
    );
    assert!(
        flat_contains(
            &first_stderr,
            "C++ consumption was disabled by `interface-cxx-standard = \"none\"`"
        ),
        "expected the disabled-consumption wording in: {first_flat}"
    );
    // The registry dependency gets the pin remedy, then the
    // override as the last resort.
    assert!(
        flat_contains(
            &first_stderr,
            "or pin `acme/libnone` to an older version (currently 0.1.0)"
        ),
        "expected the pin remedy in: {first_flat}"
    );
    assert!(
        flat_contains(
            &first_stderr,
            "as a last resort, `\"acme/libnone\" = { ..., ignore-interface-standard = true }` in \
             the `[dependencies]` table of"
        ),
        "expected the override remedy in: {first_flat}"
    );
    assert!(
        !flat_contains(&first_stderr, "cabin update"),
        "a fresh-resolution error must not mention the lockfile: {first_flat}"
    );

    // The failing first build still resolved and wrote the
    // lockfile; the second run resolves through it, errors
    // identically, and adds the staleness note.
    assert!(app_root.join("cabin.lock").exists());
    let second = build("build2");
    let second_stderr = String::from_utf8_lossy(&second.get_output().stderr).to_string();
    let second_flat = flatten(&second_stderr);
    assert!(
        second_stderr.contains(VIOLATION_CODE),
        "expected the error on the lockfile-load path: {second_stderr}"
    );
    assert!(
        flat_contains(
            &second_stderr,
            "this dependency's resolved version was loaded from cabin.lock, which records version"
        ),
        "expected the lockfile note on the lockfile-load path: {second_flat}"
    );
}

/// A lockfile generated while every standard was compatible keeps
/// its bytes when a manifest later lowers the consumer's standard:
/// the lockfile-loaded `cabin build` and `cabin test` both fail
/// with the staleness explanation and the `cabin update` remedy,
/// and the check never rewrites `cabin.lock` - which records
/// version pins only, no standards and no toolchain information.
#[test]
fn lockfile_loaded_violation_explains_staleness_and_suggests_update() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();

    let pkg_root = dir.path().join("pkg");
    assert_fs::fixture::ChildPath::new(pkg_root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "acme/libiface"
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
"acme/libiface" = "0.1.0"

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["acme/libiface"]

[target.app_test]
type = "test"
sources = ["tests/app_test.cc"]
deps = ["acme/libiface"]
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
            .args([subcommand, "--manifest-path"])
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
        // The standard-compat error gates the command itself now.
        let assertion = assertion.failure().code(1);
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        let flat = flatten(&stderr);
        assert!(
            stderr.contains(VIOLATION_CODE),
            "expected the error from `cabin {subcommand}` on the lockfile path: {stderr}"
        );
        assert!(
            flat_contains(
                &stderr,
                "this dependency's resolved version was loaded from cabin.lock, which \
                 records version pins only"
            ),
            "expected the pins-only explanation from `cabin {subcommand}`: {flat}"
        );
        assert!(
            flat_contains(
                &stderr,
                "if a standard declaration changed in a manifest after the lockfile was \
                 generated, run `cabin update` to re-resolve"
            ),
            "expected the staleness cause and update remedy from `cabin {subcommand}`: {flat}"
        );
    }

    // The check validated a lockfile-loaded graph twice and never
    // rewrote the lockfile.
    assert_eq!(
        std::fs::read(&lock_path).unwrap(),
        locked_bytes,
        "validation must not modify cabin.lock"
    );
}

/// A library whose public header uses a feature C++17 **removed**
/// (the `register` storage-class specifier) declares the honest
/// bounded interface `{ min = "c++11", max = "c++14" }`.  A
/// consumer on c++17 is refused with the above-maximum wording and
/// the lower-the-standard remedy - raising can never help against a
/// cap - and a consumer capped at c++14 really compiles the header,
/// proving the fixture (and the bound) genuine.
#[test]
fn bounded_range_caps_consumers_above_the_maximum() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("legacy/cabin.toml"))
        .write_str(
            r#"[package]
name = "legacy"
version = "0.1.0"

[target.legacy]
type = "library"
sources = ["src/legacy.cc"]
include-dirs = ["include"]
cxx-standard = "c++14"
interface-cxx-standard = { min = "c++11", max = "c++14" }
"#,
        )
        .unwrap();
    // `register` was removed in C++17 - the real-world shape the
    // declared `max = "c++14"` encodes.  Compilers differ in whether
    // they reject or merely warn about removed features, so the
    // `#error` guard makes the cap's compile-time meaning
    // deterministic; the c++14 leg below proves the header genuinely
    // compiles inside the declared range.
    assert_fs::fixture::ChildPath::new(dir.path().join("legacy/include/legacy.h"))
        .write_str(
            "#pragma once
#if defined(__cplusplus) && __cplusplus > 201402L
#error \"legacy's headers do not support C++17 or newer\"
#endif
inline int legacy_value() {
    register int fast = 7;
    return fast;
}
",
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("legacy/src/legacy.cc"))
        .write_str("#include \"legacy.h\"\nint legacy_anchor() { return legacy_value(); }\n")
        .unwrap();
    let write_app = |standard: &str| {
        assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
            .write_str(&format!(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
legacy = {{ path = "../legacy" }}

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["legacy"]
cxx-standard = "{standard}"
"#
            ))
            .unwrap();
    };
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.cc"))
        .write_str("#include \"legacy.h\"\nint main() { return legacy_value() == 7 ? 0 : 1; }\n")
        .unwrap();

    // c++17 violates the cap: refused before any compiler runs.
    write_app("c++17");
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains(VIOLATION_CODE),
        "expected the violation code in: {stderr}"
    );
    assert!(
        flat_contains(
            &stderr,
            "requires C++ consumers within `c++11..c++14` (`interface-cxx-standard`,"
        ),
        "expected the bounded-range sentence in: {}",
        flatten(&stderr)
    );
    assert!(
        flat_contains(&stderr, "lower `app:app`'s C++ standard to at most `c++14`"),
        "expected the lower remedy in: {}",
        flatten(&stderr)
    );
    assert!(
        !flat_contains(&stderr, "raise `app:app`"),
        "an above-maximum violation must not suggest raising: {}",
        flatten(&stderr)
    );

    // The per-edge override cannot silence a range violation: the
    // always-on build-time enforcement rejects it independently.
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
legacy = { path = "../legacy", ignore-interface-standard = true }

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["legacy"]
cxx-standard = "c++17"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "lower `app:app`'s C++ standard to at most `c++14`"),
        "the build-time layer must still reject the cap violation: {}",
        flatten(&stderr)
    );

    // Capped at c++14 the whole graph builds and runs - the
    // removed-feature header genuinely compiles below the cap.
    write_app("c++14");
    cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build2"))
        .assert()
        .success();
}

/// The C twin: a C library whose header hard-errors above C17
/// declares `{ min = "c99", max = "c17" }`; a c23 consumer is
/// refused with the range wording, and a c17 consumer compiles.
#[test]
fn bounded_range_caps_c_consumers_above_the_maximum() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("clib/cabin.toml"))
        .write_str(
            r#"[package]
name = "clib"
version = "0.1.0"

[target.clib]
type = "library"
sources = ["src/clib.c"]
include-dirs = ["include"]
c-standard = "c17"
interface-c-standard = { min = "c99", max = "c17" }
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("clib/include/clib.h"))
        .write_str(
            "#pragma once\n#if defined(__STDC_VERSION__) && __STDC_VERSION__ > 201710L\n#error \"clib's headers do not support standards after C17\"\n#endif\nint clib_value(void);\n",
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("clib/src/clib.c"))
        .write_str("#include \"clib.h\"\nint clib_value(void) { return 7; }\n")
        .unwrap();
    let write_app = |standard: &str| {
        assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
            .write_str(&format!(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
clib = {{ path = "../clib" }}

[target.app]
type = "executable"
sources = ["src/main.c"]
deps = ["clib"]
c-standard = "{standard}"
"#
            ))
            .unwrap();
    };
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("#include \"clib.h\"\nint main(void) { return clib_value() == 7 ? 0 : 1; }\n")
        .unwrap();

    write_app("c23");
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains(VIOLATION_CODE),
        "expected the violation code in: {stderr}"
    );
    assert!(
        flat_contains(
            &stderr,
            "requires C consumers within `c99..c17` (`interface-c-standard`,"
        ),
        "expected the bounded-range sentence in: {}",
        flatten(&stderr)
    );
    assert!(
        flat_contains(&stderr, "lower `app:app`'s C standard to at most `c17`"),
        "expected the lower remedy in: {}",
        flatten(&stderr)
    );

    write_app("c17");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build2"))
        .assert()
        .success();
}

/// Two public requirements that cannot overlap - one floor above
/// the other's cap - forbid the shared consumer outright, and the
/// diagnostic names both origins with their chains.
#[test]
fn empty_intersection_names_both_clashing_origins() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("modern/cabin.toml"))
        .write_str(
            r#"[package]
name = "modern"
version = "0.1.0"

[target.modern]
type = "library"
sources = ["src/modern.cc"]
include-dirs = ["include"]
cxx-standard = "c++20"
interface-cxx-standard = "c++20"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("modern/include/modern.h"))
        .write_str("#pragma once\nint modern_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("modern/src/modern.cc"))
        .write_str("#include \"modern.h\"\nint modern_value() { return 1; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("legacy/cabin.toml"))
        .write_str(
            r#"[package]
name = "legacy"
version = "0.1.0"

[target.legacy]
type = "library"
sources = ["src/legacy.cc"]
include-dirs = ["include"]
cxx-standard = "c++14"
interface-cxx-standard = { min = "c++11", max = "c++14" }
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("legacy/include/legacy.h"))
        .write_str("#pragma once\nint legacy_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("legacy/src/legacy.cc"))
        .write_str("#include \"legacy.h\"\nint legacy_value() { return 2; }\n")
        .unwrap();
    // The header-only aggregator re-exports both libraries: its
    // effective requirement intersects a c++20 floor with a c++14
    // cap - empty, so no consumer standard can satisfy the edge.
    assert_fs::fixture::ChildPath::new(dir.path().join("mid/cabin.toml"))
        .write_str(
            r#"[package]
name = "mid"
version = "0.1.0"

[dependencies]
modern = { path = "../modern" }
legacy = { path = "../legacy" }

[target.mid]
type = "header-only"
include-dirs = ["include"]
interface-cxx-standard = "c++11"
deps = [
    { name = "modern", public = true },
    { name = "legacy", public = true },
]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("mid/include/mid.h"))
        .write_str("#pragma once\n#include \"modern.h\"\n#include \"legacy.h\"\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
mid = { path = "../mid" }

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["mid"]
cxx-standard = "c++17"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();

    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains(VIOLATION_CODE),
        "expected the violation code in: {stderr}"
    );
    assert!(
        flat_contains(
            &stderr,
            "no C++ standard satisfies its combined interface requirements"
        ),
        "expected the empty-intersection sentence in: {}",
        flatten(&stderr)
    );
    assert!(
        flat_contains(
            &stderr,
            "`modern:modern` requires at least `c++20` via public dependency `modern:modern`"
        ),
        "expected the floor origin and chain in: {}",
        flatten(&stderr)
    );
    assert!(
        flat_contains(
            &stderr,
            "while `legacy:legacy` accepts at most `c++14` via public dependency `legacy:legacy`"
        ),
        "expected the cap origin and chain in: {}",
        flatten(&stderr)
    );
    assert!(
        flat_contains(
            &stderr,
            "no C++ standard satisfies both `modern:modern` (at least `c++20`) and              `legacy:legacy` (at most `c++14`)"
        ),
        "expected the conflicting-requirements help in: {}",
        flatten(&stderr)
    );
}
