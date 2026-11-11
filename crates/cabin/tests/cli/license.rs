use super::*;

const GCC_RUNTIME_EXCEPTION: &str = "Apache-2.0 AND (GPL-3.0-or-later WITH GCC-exception-3.1)";

/// The package `write_app_using_scoped_fmt` depends on, declaring
/// `license`.
fn write_licensed_package(root: &Path, license: &str) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "fmtlib/fmt"
version = "10.2.1"
license = "{license}"
c-standard = "c11"

[target.fmt]
type = "library"
sources = ["src/fmt.c"]
"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/fmt.c"))
        .write_str("int fmt_value(void) { return 42; }\n")
        .unwrap();
}

/// `cabin metadata` reports each local package's declared license and
/// omits the key for a package that declares none: unspecified is
/// never rendered as a value.
#[test]
fn metadata_reports_declared_licenses_per_package() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
license = "MIT OR Apache-2.0"

[dependencies]
util = { path = "util" }
"#,
        )
        .unwrap();
    dir.child("util/cabin.toml")
        .write_str(
            r#"[package]
name = "util"
version = "0.1.0"
"#,
        )
        .unwrap();

    let value = run_json(
        cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml")),
    );
    let package = |name: &str| {
        value["packages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|package| package["name"] == name)
            .unwrap_or_else(|| panic!("{name} missing from {value}"))
    };
    assert_eq!(package("app")["license"], "MIT OR Apache-2.0");
    assert!(package("util").get("license").is_none(), "{value}");
}

/// The canonical metadata document `cabin package` and
/// `cabin publish --dry-run` stage carries the expression verbatim.
#[test]
fn package_and_publish_dry_run_carry_the_license() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    write_licensed_package(&pkg_root, GCC_RUNTIME_EXCEPTION);

    for (out, args) in [
        ("packaged", &["package"][..]),
        ("dry-run", &["publish", "--dry-run"][..]),
    ] {
        let dist = dir.path().join(out);
        cabin()
            .args(args)
            .arg("--manifest-path")
            .arg(pkg_root.join("cabin.toml"))
            .arg("--output-dir")
            .arg(&dist)
            .assert()
            .success();
        let body = fs::read_to_string(dist.join("fmtlib-fmt-10.2.1.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["license"], GCC_RUNTIME_EXCEPTION, "{args:?}");
    }
}

/// A file-registry publish writes the license into the package index,
/// where it is readable without the archive, and a consumer resolves
/// and fetches the licensed version exactly as an unlicensed one.
#[test]
fn file_registry_index_carries_the_license() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    write_licensed_package(&pkg_root, GCC_RUNTIME_EXCEPTION);
    let registry = dir.path().join("registry");
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();

    let body = fs::read_to_string(registry.join("packages/fmtlib/fmt.json")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        value["versions"]["10.2.1"]["license"],
        GCC_RUNTIME_EXCEPTION
    );

    write_app_using_scoped_fmt(dir.path(), None);
    let resolved = run_json(
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(&registry)
            .args(["--format", "json"]),
    );
    assert!(
        resolved["packages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|package| package["name"] == "fmtlib/fmt"),
        "{resolved}"
    );
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(&registry)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success();
}

/// An invalid expression fails the manifest load with the offending
/// term and where valid identifiers come from.
#[test]
fn invalid_license_is_rejected_with_an_actionable_diagnostic() {
    let dir = TempDir::new().unwrap();
    write_licensed_package(dir.path(), "MIT OR Apache2");
    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure()
        .stderr(
            predicate::str::contains(
                r#"invalid `[package] license`: "MIT OR Apache2" is not a valid SPDX license expression"#,
            )
            .and(predicate::str::contains(r#"(at "Apache2", byte 7)"#))
            .and(predicate::str::contains("https://spdx.org/licenses/")),
        );
}
