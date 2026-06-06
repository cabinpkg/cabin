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
                    "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[target.{name}]\ntype = \"executable\"\nsources = [\"src/main.cc\"]\n"
                ))

                .unwrap();
        assert_fs::fixture::ChildPath::new(root.join(format!("packages/{name}/src/main.cc")))
            .write_str(HELLO_MAIN_CC)
            .unwrap();
    }
}

/// Blocking 1: workspace.members with `..` must be rejected.
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

/// Blocking 1: workspace.exclude with absolute path must be
/// rejected.
#[test]
fn exclude_absolute_path_rejected_at_cli() {
    let dir = TempDir::new().unwrap();
    // An absolute exclude path must be rejected. Absoluteness is
    // platform-specific: `/tmp/outside` is not absolute on
    // Windows (no drive), so use a host-absolute path (with TOML
    // backslash escaping) to exercise the same rejection branch
    // on every host.
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

/// Blocking 3: building a package whose dep tree has no C/C++
/// targets must not silently build every other package.
#[test]
fn select_package_without_cpp_target_errors_clearly() {
    skip_if!(
        !build_tools_available(),
        "workspace_semantics review empty selection",
        "ninja or C++ compiler missing"
    );
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
    // peer: a real C++ executable that should NOT be built when
    // the user selects only `empty`.
    dir.child("packages/peer/cabin.toml")
        .write_str(
            r#"[package]
name = "peer"
version = "0.1.0"

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

/// Blocking 2: `cabin fetch -p missing` must fail at the
/// selection-validation step even when the workspace has no
/// versioned dependencies (and thus no fetch happens).
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

/// Blocking 2: `cabin resolve` over a pure-workspace root
/// (no `[package]`) collects member versioned deps and writes a
/// lockfile. An earlier baseline failed with "pure-workspace
/// roots are not supported".
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
                        "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
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

/// Blocking 2: `cabin resolve -p app` selects exactly one
/// member's deps. With only `app` selected, sibling members'
/// requirements do not contribute.
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
    // sibling depends on a name that is NOT in the index. If
    // the resolver sees both, it would error out. With
    // `-p app`, only fmt should be considered.
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
                        "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
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

/// Blocking 2: `cabin update --update-package <name>` is the
/// renamed dep-update flag that used to be `cabin update
/// --package <name>`. The new `--package` is the workspace
/// selector and is validated against the graph.
#[test]
fn update_split_flag_names() {
    // `cabin update --package <name>` is the dep-targeted
    // refresh flag. Workspace member scoping on
    // `cabin update` uses `--workspace`, `--default-members`,
    // and `--exclude` — not `-p`. The workspace here declares
    // no versioned deps, so any `--package` value reports
    // "not a versioned dependency" rather than "not a member";
    // the test asserts that the back-compat flag spelling
    // stays accepted.
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

/// Non-blocking 4: `--manifest-path cabin.toml` from inside a
/// workspace member must load the *member* manifest, not the
/// workspace root. The default-no-flag invocation still walks
/// up to the workspace root (covered by another upward-walk
/// test in this file).
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

/// Non-blocking 4 corollary: with no `--manifest-path`, root
/// discovery still finds the workspace root from a member
/// directory.
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
