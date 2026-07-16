use super::*;

/// Workspace with three members named `alpha`, `beta`, `gamma`,
/// each with one executable and a shared `src/main.cc`.  The
/// caller can request that `default-members` and an `exclude`
/// pattern be added, and gets back the manifest path.
fn write_three_member_workspace(
    root: &Path,
    default_members: Option<&[&str]>,
    exclude: Option<&[&str]>,
) {
    use std::fmt::Write as _;
    let mut manifest = String::from("[workspace]\nmembers = [\"packages/*\"]\n");
    if let Some(dm) = default_members {
        let entries: Vec<String> = dm.iter().map(|n| format!("\"packages/{n}\"")).collect();
        writeln!(manifest, "default-members = [{}]", entries.join(", ")).unwrap();
    }
    if let Some(ex) = exclude {
        let entries: Vec<String> = ex.iter().map(|n| format!("\"packages/{n}\"")).collect();
        writeln!(manifest, "exclude = [{}]", entries.join(", ")).unwrap();
    }
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(&manifest)
        .unwrap();
    for name in ["alpha", "beta", "gamma"] {
        assert_fs::fixture::ChildPath::new(root.join(format!("packages/{name}/cabin.toml")))

                .write_str(&format!(
                    "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\ncxx-standard = \"c++17\"\n\n[target.{name}]\ntype = \"executable\"\nsources = [\"src/main.cc\"]\n"
                ))

                .unwrap();
        assert_fs::fixture::ChildPath::new(root.join(format!("packages/{name}/src/main.cc")))
            .write_str(HELLO_MAIN_CC)
            .unwrap();
    }
}

#[test]
fn metadata_reports_workspace_members_default_excluded_selected() {
    let dir = TempDir::new().unwrap();
    write_three_member_workspace(dir.path(), Some(&["alpha"]), Some(&["gamma"]));
    let value = run_metadata(&dir.path().join("cabin.toml"));
    let ws = &value["workspace"];
    assert!(!ws.is_null());
    let members: Vec<&str> = ws["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(members, vec!["alpha", "beta"]);
    let default_members: Vec<&str> = ws["default_members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(default_members, vec!["alpha"]);
    let excluded: Vec<&str> = ws["excluded_members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let expected_excluded = host_path("packages/gamma");
    assert_eq!(excluded, vec![expected_excluded.as_str()]);
    let selected: Vec<&str> = ws["selected_packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    // No CLI selection flags + default-members declared = the
    // current-package fallback selects default-members.
    assert_eq!(selected, vec!["alpha"]);
}

#[test]
fn metadata_inside_member_directory_finds_root() {
    let dir = TempDir::new().unwrap();
    write_three_member_workspace(dir.path(), None, None);
    let json = run_json(
        cabin()
            .current_dir(dir.path().join("packages/beta"))
            .args(["metadata"]),
    );
    let ws = &json["workspace"];
    assert!(
        !ws.is_null(),
        "workspace section missing — root discovery failed"
    );
    let root = ws["root"].as_str().unwrap();
    assert!(
        root.ends_with(dir.path().file_name().unwrap().to_str().unwrap()),
        "root mismatch: {root}"
    );
}

#[test]
fn metadata_workspace_flag_selects_all_members_minus_exclude() {
    let dir = TempDir::new().unwrap();
    write_three_member_workspace(dir.path(), None, None);
    let out = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--workspace", "--exclude", "beta"])
        .assert()
        .success()
        .get_output()
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let selected: Vec<&str> = json["workspace"]["selected_packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(selected, vec!["alpha", "gamma"]);
}

#[test]
fn metadata_explicit_packages_selects_named_members() {
    let dir = TempDir::new().unwrap();
    write_three_member_workspace(dir.path(), None, None);
    let out = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["-p", "alpha", "-p", "gamma"])
        .assert()
        .success()
        .get_output()
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let selected: Vec<&str> = json["workspace"]["selected_packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(selected, vec!["alpha", "gamma"]);
}

#[test]
fn metadata_unknown_package_fails_clearly() {
    let dir = TempDir::new().unwrap();
    write_three_member_workspace(dir.path(), None, None);
    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["-p", "nope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a member"));
}

#[test]
fn metadata_default_members_mode_errors_when_undeclared() {
    let dir = TempDir::new().unwrap();
    write_three_member_workspace(dir.path(), None, None);
    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--default-members"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("default member")
                .or(predicate::str::contains("default-members")),
        );
}

#[test]
fn metadata_exclude_with_explicit_package_errors() {
    let dir = TempDir::new().unwrap();
    write_three_member_workspace(dir.path(), None, None);
    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["-p", "alpha", "--exclude", "beta"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--exclude"));
}

#[test]
fn workspace_default_member_missing_member_errors() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/*"]
default-members = ["packages/missing"]
"#,
        )
        .unwrap();
    dir.child("packages/keep/cabin.toml")
        .write_str("[package]\nname = \"keep\"\nversion = \"0.1.0\"\n")
        .unwrap();
    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("default member"));
}

#[test]
fn unused_exclude_pattern_errors() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/keep"]
exclude = ["packages/missing"]
"#,
        )
        .unwrap();
    dir.child("packages/keep/cabin.toml")
        .write_str("[package]\nname = \"keep\"\nversion = \"0.1.0\"\n")
        .unwrap();
    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("packages/missing"));
}

#[test]
fn nested_workspace_rejected() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["nested"]
"#,
        )
        .unwrap();
    dir.child("nested/cabin.toml")
        .write_str("[workspace]\nmembers = []\n")
        .unwrap();
    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("nested workspace"));
}

#[test]
fn workspace_dependency_inheritance_resolves_in_metadata() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/app"]

[workspace.dependencies]
fmt = ">=10 <11"
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = { workspace = true }
"#,
        )
        .unwrap();
    let json = run_metadata(&dir.path().join("cabin.toml"));
    let app = json["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "app")
        .unwrap();
    let deps = app["dependencies"].as_array().unwrap();
    assert_eq!(deps.len(), 1);
    // The Workspace marker has been resolved into a Version
    // source by the workspace loader.
    assert_eq!(deps[0]["kind"], "version");
}

#[test]
fn workspace_dependency_unresolved_errors() {
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

[dependencies]
fmt = { workspace = true }
"#,
        )
        .unwrap();
    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("workspace = true"));
}

#[test]
fn build_workspace_flag_builds_every_member() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_three_member_workspace(dir.path(), None, None);
    let build_dir = dir.path().join("build");
    cabin()
        .args(["build", "--workspace", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(&build_dir)
        .assert()
        .success();
    for name in ["alpha", "beta", "gamma"] {
        let exe = build_dir
            .join("dev")
            .join("packages")
            .join(name)
            .join(host_exe(name));
        assert!(exe.is_file(), "missing built binary {}", exe.display());
    }
}

#[test]
fn build_with_explicit_packages_builds_only_those() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_three_member_workspace(dir.path(), None, None);
    let build_dir = dir.path().join("build");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["-p", "beta"])
        .arg("--build-dir")
        .arg(&build_dir)
        .assert()
        .success();
    assert!(
        build_dir
            .join("dev/packages/beta")
            .join(host_exe("beta"))
            .is_file()
    );
    // alpha and gamma must not have been built.
    assert!(
        !build_dir
            .join("dev/packages/alpha")
            .join(host_exe("alpha"))
            .exists()
    );
    assert!(
        !build_dir
            .join("dev/packages/gamma")
            .join(host_exe("gamma"))
            .exists()
    );
}

#[test]
fn build_workspace_with_exclude_skips_member() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_three_member_workspace(dir.path(), None, None);
    let build_dir = dir.path().join("build");
    cabin()
        .args([
            "build",
            "--workspace",
            "--exclude",
            "gamma",
            "--manifest-path",
        ])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(&build_dir)
        .assert()
        .success();
    assert!(
        build_dir
            .join("dev/packages/alpha")
            .join(host_exe("alpha"))
            .is_file()
    );
    assert!(
        build_dir
            .join("dev/packages/beta")
            .join(host_exe("beta"))
            .is_file()
    );
    assert!(
        !build_dir
            .join("dev/packages/gamma")
            .join(host_exe("gamma"))
            .exists()
    );
}

#[test]
fn build_unknown_package_fails_clearly() {
    let dir = TempDir::new().unwrap();
    write_three_member_workspace(dir.path(), None, None);
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["-p", "nope", "--build-dir"])
        .arg(dir.path().join("build"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a member"));
}

#[test]
fn package_in_workspace_requires_explicit_selection() {
    let dir = TempDir::new().unwrap();
    write_three_member_workspace(dir.path(), None, None);
    // Without --package, packaging the workspace root must fail.
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("--package <name>"));

    // With a single --package, packaging the chosen member works.
    let dist = dir.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["-p", "beta", "--output-dir"])
        .arg(&dist)
        .assert()
        .success();
    assert!(dist.join("beta-0.1.0.tar.gz").is_file());
}

#[test]
fn publish_in_workspace_requires_explicit_selection() {
    let dir = TempDir::new().unwrap();
    write_three_member_workspace(dir.path(), None, None);
    // The registry rejects bare names, so the published member must be
    // scoped; its bare siblings stay local-only.
    assert_fs::fixture::ChildPath::new(dir.path().join("packages/alpha/cabin.toml"))
        .write_str(
            "[package]\nname = \"acme/alpha\"\nversion = \"0.1.0\"\ncxx-standard = \"c++17\"\n\n[target.alpha]\ntype = \"executable\"\nsources = [\"src/main.cc\"]\n",
        )
        .unwrap();
    let registry = dir.path().join("registry");

    // Without --package, publishing the workspace root must
    // fail with the workspace-boundary error.
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .failure()
        .stderr(predicate::str::contains("--package <name>"));

    // With a single --package, publishing the chosen member
    // succeeds.
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["-p", "acme/alpha", "--registry-dir"])
        .arg(&registry)
        .assert()
        .success();
    assert!(registry.join("packages/acme/alpha.json").is_file());
}
