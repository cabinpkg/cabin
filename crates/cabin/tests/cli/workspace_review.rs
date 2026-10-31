use super::*;

fn write_three_member_workspace_no_default(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[workspace]
members = ["packages/*"]
"#,
        )
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
fn member_with_parent_dir_rejected_at_cli() {
    let dir = TempDir::new().unwrap();
    let workspace_dir = dir.path().join("ws");
    let outside_dir = dir.path().join("outside");
    assert_fs::fixture::ChildPath::new(&workspace_dir)
        .create_dir_all()
        .unwrap();
    assert_fs::fixture::ChildPath::new(&outside_dir)
        .create_dir_all()
        .unwrap();
    assert_fs::fixture::ChildPath::new(workspace_dir.join("cabin.toml"))
        .write_str(
            r#"[workspace]
members = ["../outside"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(outside_dir.join("cabin.toml"))
        .write_str("[package]\nname = \"sneaky\"\nversion = \"0.1.0\"\n")
        .unwrap();
    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(workspace_dir.join("cabin.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("workspace.members"))
        .stderr(predicate::str::contains("../outside"));
}

#[test]
fn exclude_absolute_path_rejected_at_cli() {
    let dir = TempDir::new().unwrap();
    // Absoluteness is platform-specific: `/tmp/outside` is not
    // absolute on Windows (no drive), so use a host-absolute path
    // (with TOML backslash escaping) to exercise the same rejection
    // branch on every host.
    let absolute_exclude = if cfg!(windows) {
        r"C:\\tmp\\outside"
    } else {
        "/tmp/outside"
    };
    dir.child("cabin.toml")
        .write_str(&format!(
            "[workspace]\nmembers = [\"packages/keep\"]\nexclude = [\"{absolute_exclude}\"]\n"
        ))
        .unwrap();
    dir.child("packages/keep/cabin.toml")
        .write_str("[package]\nname = \"keep\"\nversion = \"0.1.0\"\n")
        .unwrap();
    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("workspace.exclude"));
}

/// Building a package whose dep tree has no C/C++ targets must not
/// build every other package.
#[test]
fn select_package_without_cpp_target_errors_clearly() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/*"]
"#,
        )
        .unwrap();
    // empty: declares no targets at all.
    dir.child("packages/empty/cabin.toml")
        .write_str("[package]\nname = \"empty\"\nversion = \"0.1.0\"\n")
        .unwrap();
    dir.child("packages/peer/cabin.toml")
        .write_str(
            r#"[package]
name = "peer"
version = "0.1.0"
cxx-standard = "c++17"

[target.peer]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    dir.child("packages/peer/src/main.cc")
        .write_str(HELLO_MAIN_CC)
        .unwrap();
    let build_dir = dir.path().join("build");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["-p", "empty", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .failure()
        .stderr(predicate::str::contains("no C/C++ targets"));
    assert!(
        !build_dir.join("dev/packages/peer/peer").exists(),
        "selecting `empty` must not have built `peer`"
    );
}

/// `cabin fetch -p missing` must fail at the selection-validation
/// step even when the workspace has no versioned dependencies (and
/// thus no fetch happens).
#[test]
fn fetch_unknown_package_errors_without_versioned_deps() {
    let dir = TempDir::new().unwrap();
    write_three_member_workspace_no_default(dir.path());
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["-p", "missing"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a member"));
}

/// A pure workspace resolves under the synthetic `cabin/workspace`
/// root, so a dependency on that name - direct or from an index
/// package - is refused rather than resolved to an index package
/// the lockfile could not tell apart from the root.
#[test]
fn resolve_pure_workspace_refuses_an_index_naming_the_synthetic_root() {
    // (member dependency, index package, that package's dependencies)
    let cases = [
        ("cabin/workspace", "cabin/workspace", "{}"),
        ("acme/a", "acme/a", r#"{ "cabin/workspace": "*" }"#),
    ];
    for (dependency, indexed, indexed_deps) in cases {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str("[workspace]\nmembers = [\"packages/app\"]\n")
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(&format!(
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
                 [dependencies]\n\"{dependency}\" = \"^1\"\n"
            ))
            .unwrap();
        dir.child(format!("index/{indexed}.json"))
            .write_str(&format!(
                r#"{{
                    "schema": 1,
                    "name": "{indexed}",
                    "versions": {{
                        "1.0.0": {{
                            "dependencies": {indexed_deps},
                            "yanked": false,
                            "revision": "0000000000000000",
                            "revisions": {{
                                "0000000000000000": {{
                                    "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                                    "published-at": "2026-01-01T00:00:00Z",
                                    "source": {{ "type": "archive", "path": "unfetched.zip", "format": "zip" }}
                                }}
                            }}
                        }}
                    }}
                }}"#
            ))
            .unwrap();
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "a dependency names \"cabin/workspace\", the name Cabin reserves",
            ));
    }
}

/// `cabin resolve` over a pure-workspace root (no `[package]`)
/// collects member versioned deps and writes a lockfile.
#[test]
fn resolve_pure_workspace_root_with_member_versioned_deps() {
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
fmt = ">=10.0.0 <11.0.0"
"#,
        )
        .unwrap();
    // A minimal local index with a single fmt version that
    // satisfies the requirement.
    dir.child("index/fmt.json")

            .write_str(r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": {
                        "dependencies": {},
                        "yanked": false,
                        "revision": "0000000000000000",
                        "revisions": {
                            "0000000000000000": {
                                "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                                "published-at": "2026-01-01T00:00:00Z",
                                "source": { "type": "archive", "path": "unfetched.zip", "format": "zip" }
                            }
                        }
                    }
                }
            }"#)

            .unwrap();
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .assert()
        .success();
    let lockfile = fs::read_to_string(dir.path().join("cabin.lock")).unwrap();
    assert!(
        lockfile.contains(r#"name = "fmt""#),
        "lockfile missing fmt: {lockfile}"
    );
}

/// `cabin resolve -p app` selects exactly one member's deps.  With
/// only `app` selected, sibling members' requirements do not
/// contribute.
#[test]
fn resolve_explicit_package_selection() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/app", "packages/sibling"]
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
    // `unknown` is absent from the index, so resolution fails if
    // `sibling`'s requirements contribute.
    dir.child("packages/sibling/cabin.toml")
        .write_str(
            r#"[package]
name = "sibling"
version = "0.1.0"

[dependencies]
unknown = ">=1"
"#,
        )
        .unwrap();
    dir.child("index/fmt.json")

            .write_str(r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": {
                        "dependencies": {},
                        "yanked": false,
                        "revision": "0000000000000000",
                        "revisions": {
                            "0000000000000000": {
                                "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                                "published-at": "2026-01-01T00:00:00Z",
                                "source": { "type": "archive", "path": "unfetched.zip", "format": "zip" }
                            }
                        }
                    }
                }
            }"#)

            .unwrap();
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["-p", "app", "--index-path"])
        .arg(dir.path().join("index"))
        .assert()
        .success();
    let lockfile = fs::read_to_string(dir.path().join("cabin.lock")).unwrap();
    assert!(lockfile.contains(r#"name = "fmt""#));
    assert!(!lockfile.contains("unknown"));
}

#[test]
fn update_split_flag_names() {
    // On `cabin update`, `--package <name>` refreshes a direct
    // versioned dependency; member scoping uses `--workspace`,
    // `--default-members`, and `--exclude`, not `-p`.  The
    // workspace here declares no versioned deps, so any
    // `--package` value is rejected as "not a direct versioned
    // dependency" rather than "not a member".
    let dir = TempDir::new().unwrap();
    write_three_member_workspace_no_default(dir.path());
    cabin()
        .args(["update", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--package", "anything"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "not a direct versioned dependency",
        ));
}

/// `--manifest-path cabin.toml` from inside a workspace member
/// must load the *member* manifest, not the workspace root.
#[test]
fn explicit_manifest_path_overrides_root_discovery() {
    let dir = TempDir::new().unwrap();
    write_three_member_workspace_no_default(dir.path());
    let json = run_json(cabin().current_dir(dir.path().join("packages/beta")).args([
        "metadata",
        "--manifest-path",
        "cabin.toml",
    ]));
    // The metadata document for the *member* manifest has no
    // workspace section.
    assert!(
        json["workspace"].is_null(),
        "expected member-scoped metadata, got: {json}"
    );
    let pkgs = json["packages"].as_array().unwrap();
    assert_eq!(pkgs.len(), 1);
    assert_eq!(pkgs[0]["name"], "beta");
}

/// With no `--manifest-path`, root discovery still finds the
/// workspace root from a member directory.
#[test]
fn default_manifest_path_walks_up_to_workspace_root() {
    let dir = TempDir::new().unwrap();
    write_three_member_workspace_no_default(dir.path());
    let json = run_json(
        cabin()
            .current_dir(dir.path().join("packages/beta"))
            .args(["metadata"]),
    );
    assert!(
        !json["workspace"].is_null(),
        "expected workspace section, got: {json}"
    );
}
