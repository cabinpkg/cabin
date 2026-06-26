use super::*;

/// When the user is sandwiched between two `[workspace]`
/// roots - and the outer does NOT list the nested directory
/// as a member - discovery still errors rather than silently
/// picking one.  The strict rule names both roots, so the
/// user can disambiguate by passing `--manifest-path`
/// explicitly; an earlier rule only rejected the nested case
/// via the loader and only when the outer claimed the nested
/// as a member.
#[test]
fn metadata_inside_nested_workspace_with_unrelated_outer_errors() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = []
"#,
        )
        .unwrap();
    dir.child("nested/cabin.toml")
        .write_str(
            r#"[workspace]
members = []
"#,
        )
        .unwrap();
    cabin()
        .current_dir(dir.path().join("nested"))
        .args(["metadata"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("nested workspace detected"));
}

/// Selection-aware materialization.  With workspace
/// `app + b`, where `b` (unrelated to `app`) declares a
/// versioned dep `spdlog` that is *not* in the registry, and
/// the registry only carries `fmt` (which `app` uses),
/// `cabin resolve -p app` must not error on the missing
/// `spdlog` because `b` is outside the selected closure.
#[test]
fn resolve_p_app_does_not_require_unrelated_dep_in_registry() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/*"]
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        )
        .unwrap();
    // `b` declares a dep on `spdlog` that the registry does
    // not carry.  Selection-aware materialization must skip it.
    dir.child("packages/b/cabin.toml")
        .write_str(
            r#"[package]
name = "b"
version = "0.1.0"

[dependencies]
spdlog = "^1"
"#,
        )
        .unwrap();
    dir.child("index/fmt.json")

            .write_str(r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": { "dependencies": {}, "yanked": false, "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000" }
                }
            }"#)

            .unwrap();
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--package", "app", "--index-path"])
        .arg(dir.path().join("index"))
        .assert()
        .success();
}

/// `cabin update --package <name>` only refreshes direct
/// versioned deps.  Even if a transitive locked package would
/// otherwise be reachable via the lockfile, the CLI rejects
/// it explicitly.
#[test]
fn update_package_rejects_transitive() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        )
        .unwrap();
    dir.child("index/fmt.json")

            .write_str(r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": { "dependencies": {}, "yanked": false, "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000" }
                }
            }"#)

            .unwrap();
    cabin()
        .args(["update", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--package", "spdlog", "--index-path"])
        .arg(dir.path().join("index"))
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "only refreshes direct dependencies",
        ));
}
