use super::*;

/// Workspace fixture: an `app` (executable) that depends on
/// a path-local `lib` (library).  Used to exercise tree
/// rendering and explain queries that need at least one
/// dependency edge.
fn write_app_with_path_dep(dir: &Path) {
    assert_fs::fixture::ChildPath::new(dir.join("cabin.toml"))
        .write_str(
            r#"[workspace]
members = ["app", "lib"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[dependencies]
lib = { path = "../lib" }

[target.app]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("app/src/main.cc"))
        .write_str(HELLO_MAIN_CC)
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("lib/cabin.toml"))
        .write_str(
            r#"[package]
name = "lib"
version = "0.1.0"
cxx-standard = "c++17"

[target.lib]
type = "library"
sources = ["src/lib.cc"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("lib/src/lib.cc"))
        .write_str("int lib_value() { return 1; }\n")
        .unwrap();
}

#[test]
fn tree_human_format_default_emits_box_drawing() {
    let dir = TempDir::new().unwrap();
    write_app_with_path_dep(dir.path());
    let output = cabin()
        .args(["tree", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    // Both packages must appear, app as a workspace root with
    // its workspace label, lib as a normal-kind child of app.
    assert!(stdout.contains("app v0.1.0"), "got: {stdout}");
    assert!(stdout.contains("lib v0.1.0"), "got: {stdout}");
    assert!(stdout.contains("[normal]"), "got: {stdout}");
    // Box-drawing must be emitted for the child edge.
    assert!(
        stdout.contains("└── lib") || stdout.contains("├── lib"),
        "got: {stdout}"
    );
}

#[test]
fn tree_json_format_is_valid_structured_document() {
    let dir = TempDir::new().unwrap();
    write_app_with_path_dep(dir.path());
    let value = run_json(
        cabin()
            .args(["tree", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--format", "json"]),
    );
    let arr = value.as_array().expect("forest must be a JSON array");
    let app = arr
        .iter()
        .find(|n| n["name"] == "app")
        .expect("app must be a root in tree forest");
    assert_eq!(app["version"], "0.1.0");
    // Source provenance is a tagged enum; the workspace
    // member case has no extra fields.
    assert_eq!(app["source"]["kind"], "workspace-member");
    let children = app["children"].as_array().expect("children must be array");
    assert_eq!(children.len(), 1, "app should have exactly one child");
    assert_eq!(children[0]["name"], "lib");
    assert_eq!(children[0]["edge_kind"], "normal");
}

#[test]
fn tree_default_roots_honor_workspace_default_members() {
    let dir = TempDir::new().unwrap();
    write_app_with_path_dep(dir.path());
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["app", "lib"]
default-members = ["app"]
"#,
        )
        .unwrap();

    let output = cabin()
        .args(["tree", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let roots = value.as_array().expect("forest must be a JSON array");
    let root_names: Vec<&str> = roots
        .iter()
        .map(|node| node["name"].as_str().unwrap())
        .collect();

    assert_eq!(
        root_names,
        vec!["app"],
        "implicit tree selection should use default-members, got: {stdout}"
    );
    assert_eq!(roots[0]["children"][0]["name"], "lib");
}

#[test]
fn tree_kind_filter_restricts_to_normal_edges() {
    let dir = TempDir::new().unwrap();
    write_app_with_path_dep(dir.path());
    let value = run_json(
        cabin()
            .args(["tree", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--kind", "normal"])
            .args(["--format", "json"]),
    );
    let app = value
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "app")
        .expect("app must be present");
    let children = app["children"].as_array().unwrap();
    assert!(
        children.iter().all(|c| c["edge_kind"] == "normal"),
        "--kind normal should restrict to normal edges, got: {children:?}"
    );
}

#[test]
fn explain_package_marks_selected_root() {
    let dir = TempDir::new().unwrap();
    write_app_with_path_dep(dir.path());
    let value = run_json(
        cabin()
            .args(["explain", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--format", "json", "package", "app"]),
    );
    assert_eq!(value["kind"], "package");
    assert_eq!(value["name"], "app");
    assert_eq!(value["is_selected_root"], true);
    assert_eq!(value["source"]["kind"], "workspace-member");
}

#[test]
fn explain_package_traces_dep_path_from_root() {
    let dir = TempDir::new().unwrap();
    write_app_with_path_dep(dir.path());
    // Constrain selection to `app` so the only reachable
    // path to lib is via `app -> lib`.  Without this the
    // workspace's other primary package (lib itself) would
    // contribute a length-1 self-path that sorts first.
    let value = run_json(
        cabin()
            .args(["explain", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--package", "app"])
            .args(["--format", "json", "package", "lib"]),
    );
    let paths = value["paths"].as_array().unwrap();
    assert!(!paths.is_empty(), "lib must be reachable from a root");
    let first = paths[0].as_array().unwrap();
    assert_eq!(first[0]["name"], "app");
    assert_eq!(first[1]["name"], "lib");
    assert_eq!(first[1]["edge_kind"], "normal");
}

#[test]
fn explain_unknown_package_returns_diagnostic() {
    let dir = TempDir::new().unwrap();
    write_app_with_path_dep(dir.path());
    let assertion = cabin()
        .args(["explain", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["package", "missing"])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("`missing`") && stderr.contains("not found"),
        "expected package-not-found diagnostic, got: {stderr}"
    );
    assert!(
        stderr.contains("cabin::explain::error"),
        "the typed `ExplainError` must reach the diagnostic dispatcher so the stable code is emitted, got: {stderr}",
    );
}

#[test]
fn explain_target_reports_languages_and_kind() {
    let dir = TempDir::new().unwrap();
    write_app_with_path_dep(dir.path());
    let value = run_json(
        cabin()
            .args(["explain", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--format", "json", "target", "lib"]),
    );
    // Outer tag (the Explanation discriminator).
    assert_eq!(value["kind"], "target");
    assert_eq!(value["package"], "lib");
    assert_eq!(value["target"], "lib");
    // Inner target_kind field carries the Cabin TargetKind
    // string.  Renamed from `kind` so it does not collide
    // with the outer discriminator.
    assert_eq!(value["target_kind"], "library");
    assert!(value["is_buildable"].as_bool().unwrap());
    let langs = value["languages"].as_array().unwrap();
    assert!(
        langs.iter().any(|v| v == "cxx"),
        "expected cxx in languages, got: {langs:?}"
    );
}

#[test]
fn explain_source_reports_workspace_member_provenance() {
    let dir = TempDir::new().unwrap();
    write_app_with_path_dep(dir.path());
    let value = run_json(
        cabin()
            .args(["explain", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--format", "json", "source", "app"]),
    );
    assert_eq!(value["kind"], "source");
    assert_eq!(value["name"], "app");
    assert_eq!(value["source"]["kind"], "workspace-member");
}

#[test]
fn explain_feature_query_without_separator_errors() {
    let dir = TempDir::new().unwrap();
    write_app_with_path_dep(dir.path());
    let assertion = cabin()
        .args(["explain", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["feature", "no-separator"])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("`package/feature`"),
        "expected feature query diagnostic, got: {stderr}"
    );
}

#[test]
fn explain_build_config_emits_fingerprint_field() {
    let dir = TempDir::new().unwrap();
    write_app_with_path_dep(dir.path());
    let value = run_json(
        cabin()
            .args(["explain", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--format", "json", "build-config", "app"]),
    );
    assert_eq!(value["kind"], "build-config");
    assert_eq!(value["package"], "app");
    let cfg = &value["configuration"];
    assert!(
        cfg["fingerprint"].is_string(),
        "fingerprint must be present, got: {cfg}"
    );
    assert!(cfg["profile"].is_object());
}

#[test]
fn tree_renders_deterministically_across_runs() {
    let dir = TempDir::new().unwrap();
    write_app_with_path_dep(dir.path());
    let manifest = dir.path().join("cabin.toml");
    let first = cabin()
        .args(["tree", "--manifest-path"])
        .arg(&manifest)
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let second = cabin()
        .args(["tree", "--manifest-path"])
        .arg(&manifest)
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert_eq!(
        first.stdout, second.stdout,
        "tree output must be byte-stable"
    );
}
