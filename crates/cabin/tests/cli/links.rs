//! End-to-end coverage for the per-target `links` native-library
//! identity: the post-resolution uniqueness check across local and
//! registry claimants, its exact feature-resolution scoping, the
//! published index round trip, and the building commands'
//! final-graph check for the claimants resolution cannot see
//! (feature-enabled optional deps of transitively-activated forks).
//! Resolve-level tests need no toolchain; the build-level tests
//! guard on build tools.

use super::*;

use crate::standard_compat::flat_contains;

const COLLISION_PHRASE: &str = "is claimed by multiple packages in the dependency graph";

/// A local C library package claiming `links`.
fn write_claiming_lib(dir: &Path, rel: &str, name: &str, links: &str) {
    assert_fs::fixture::ChildPath::new(dir.join(rel).join("cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "{name}"
version = "0.1.0"

[target.{name}]
type = "library"
sources = ["src/{name}.c"]
c-standard = "c11"
links = "{links}"
"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join(rel).join(format!("src/{name}.c")))
        .write_str("void f(void) {}\n")
        .unwrap();
}

fn resolve_assert(manifest: &Path) -> assert_cmd::assert::Assert {
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(manifest)
        .assert()
}

/// Two direct path dependencies claiming the same identity collide,
/// and the diagnostic names the identity and both claimants with
/// package, version, and target.
#[test]
fn direct_local_collision_names_both_claimants() {
    let dir = TempDir::new().unwrap();
    write_claiming_lib(dir.path(), "zlib", "zlib", "z");
    write_claiming_lib(dir.path(), "miniz", "miniz", "z");
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
zlib = { path = "../zlib" }
miniz = { path = "../miniz" }

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();

    let output = resolve_assert(&dir.path().join("app/cabin.toml"))
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    for phrase in [
        COLLISION_PHRASE,
        "miniz v0.1.0 (target `miniz`)",
        "zlib v0.1.0 (target `zlib`)",
    ] {
        assert!(
            flat_contains(&stderr, phrase),
            "expected {phrase:?} in: {stderr}"
        );
    }
}

/// A collision fires through a transitive, private dependency edge:
/// `app -> mid -> leaf` where `leaf` and app's direct dep both claim
/// the same identity.  Native symbol collisions ignore dependency
/// visibility, so the check must too.
#[test]
fn transitive_collision_through_private_dependency_errors() {
    let dir = TempDir::new().unwrap();
    write_claiming_lib(dir.path(), "leaf", "leaf", "sqlite3");
    write_claiming_lib(dir.path(), "direct", "direct", "sqlite3");
    assert_fs::fixture::ChildPath::new(dir.path().join("mid/cabin.toml"))
        .write_str(
            r#"[package]
name = "mid"
version = "0.1.0"

[dependencies]
leaf = { path = "../leaf" }

[target.mid]
type = "library"
sources = ["src/mid.c"]
c-standard = "c11"
deps = ["leaf"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("mid/src/mid.c"))
        .write_str("void mid(void) {}\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
mid = { path = "../mid" }
direct = { path = "../direct" }

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();

    let output = resolve_assert(&dir.path().join("app/cabin.toml"))
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    for phrase in [COLLISION_PHRASE, "leaf v0.1.0", "direct v0.1.0"] {
        assert!(
            flat_contains(&stderr, phrase),
            "expected {phrase:?} in: {stderr}"
        );
    }
}

/// Two registry packages whose index entries claim the same identity
/// collide at resolution time - no archive download involved (the
/// entries carry no `source` block at all).
#[test]
fn registry_collision_fires_from_index_metadata_alone() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    for (name, target) in [("zlib", "z"), ("zlib-ng", "zng")] {
        assert_fs::fixture::ChildPath::new(index.join(format!("{name}.json")))
            .write_str(&format!(
                r#"{{
  "schema": 1,
  "name": "{name}",
  "versions": {{
    "1.0.0": {{
      "dependencies": {{}},
      "yanked": false,
      "standards": {{ "targets": {{ "{target}": {{}} }} }},
      "links": {{ "{target}": "z" }}
    }}
  }}
}}"#
            ))
            .unwrap();
    }
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
zlib = ">=1"
zlib-ng = ">=1"

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();

    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(&index)
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    for phrase in [
        COLLISION_PHRASE,
        "zlib v1.0.0 (target `z`)",
        "zlib-ng v1.0.0 (target `zng`)",
    ] {
        assert!(
            flat_contains(&stderr, phrase),
            "expected {phrase:?} in: {stderr}"
        );
    }
}

/// A local claimant collides with a registry claimant: the identity
/// space is one graph-wide namespace, not per-origin.
#[test]
fn local_claim_collides_with_registry_claim() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    assert_fs::fixture::ChildPath::new(index.join("zlib.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "zlib",
  "versions": {
    "1.0.0": {
      "dependencies": {},
      "yanked": false,
      "standards": { "targets": { "z": {} } },
      "links": { "z": "z" }
    }
  }
}"#,
        )
        .unwrap();
    write_claiming_lib(dir.path(), "vendored", "vendored", "z");
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
zlib = ">=1"
vendored = { path = "../vendored" }

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();

    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(&index)
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    for phrase in [COLLISION_PHRASE, "vendored v0.1.0", "zlib v1.0.0"] {
        assert!(
            flat_contains(&stderr, phrase),
            "expected {phrase:?} in: {stderr}"
        );
    }
}

/// A workspace with no versioned dependencies still rejects two
/// members claiming the same identity: the check runs before the
/// no-versioned-deps fast path can skip resolution.
#[test]
fn local_only_workspace_collision_fires_without_an_index() {
    let dir = TempDir::new().unwrap();
    write_claiming_lib(dir.path(), "ws/one", "one", "z");
    write_claiming_lib(dir.path(), "ws/two", "two", "z");
    assert_fs::fixture::ChildPath::new(dir.path().join("ws/cabin.toml"))
        .write_str(
            r#"[workspace]
members = ["one", "two"]
"#,
        )
        .unwrap();

    let output = resolve_assert(&dir.path().join("ws/cabin.toml"))
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        flat_contains(&stderr, COLLISION_PHRASE),
        "expected the collision diagnostic in: {stderr}"
    );
}

/// Distinct identities, claim-free packages, and a lone claimant
/// resolve cleanly - no false positives from links metadata being
/// present.
#[test]
fn distinct_identities_resolve_cleanly() {
    let dir = TempDir::new().unwrap();
    write_claiming_lib(dir.path(), "zlib", "zlib", "z");
    write_claiming_lib(dir.path(), "libpng", "libpng", "png");
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
zlib = { path = "../zlib" }
libpng = { path = "../libpng" }

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();

    resolve_assert(&dir.path().join("app/cabin.toml")).success();
}

/// A disabled optional path dependency contributes no claim: the
/// check scopes to the feature resolver's exact reachable set, so
/// the collision appears only when the feature enables the edge.
#[test]
fn disabled_optional_dependency_does_not_claim() {
    let dir = TempDir::new().unwrap();
    write_claiming_lib(dir.path(), "vendored", "vendored", "z");
    write_claiming_lib(dir.path(), "zlib", "zlib", "z");
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
zlib = { path = "../zlib" }
vendored = { path = "../vendored", optional = true }

[features]
bundled = ["dep:vendored"]

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    let manifest = dir.path().join("app/cabin.toml");

    resolve_assert(&manifest).success();

    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .args(["--features", "bundled"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        flat_contains(&stderr, COLLISION_PHRASE),
        "expected the collision diagnostic in: {stderr}"
    );
}

/// The local-only enforcement seam is shared by every command with
/// a no-versioned-deps bypass: `cabin build`, `cabin vendor`, and
/// `cabin fetch` refuse the same local-only collision
/// `cabin resolve` does.  The build path errors during workspace
/// preparation, before any toolchain is consulted, so no build
/// tools are required.
#[test]
fn local_only_collision_fires_on_build_vendor_and_fetch() {
    let dir = TempDir::new().unwrap();
    write_claiming_lib(dir.path(), "ws/one", "one", "z");
    write_claiming_lib(dir.path(), "ws/two", "two", "z");
    assert_fs::fixture::ChildPath::new(dir.path().join("ws/cabin.toml"))
        .write_str(
            r#"[workspace]
members = ["one", "two"]
"#,
        )
        .unwrap();
    let manifest = dir.path().join("ws/cabin.toml");

    let vendor = cabin()
        .args(["vendor", "--manifest-path"])
        .arg(&manifest)
        .arg("--vendor-dir")
        .arg(dir.path().join("vendor"))
        .assert()
        .failure()
        .get_output()
        .clone();
    assert!(
        flat_contains(&String::from_utf8_lossy(&vendor.stderr), COLLISION_PHRASE),
        "vendor must refuse the local-only collision"
    );

    let fetch = cabin()
        .args(["fetch", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .failure()
        .get_output()
        .clone();
    assert!(
        flat_contains(&String::from_utf8_lossy(&fetch.stderr), COLLISION_PHRASE),
        "fetch must refuse the local-only collision"
    );

    let build = cabin()
        .args(["build", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .failure()
        .get_output()
        .clone();
    assert!(
        flat_contains(&String::from_utf8_lossy(&build.stderr), COLLISION_PHRASE),
        "build must refuse the local-only collision"
    );
}

/// A `[patch]` entry for a package this invocation never depends on
/// is dormant: its fork's own registry dependencies must not join
/// resolution, so a claim they would carry cannot collide.  The same
/// patch referenced by a real dependency folds its deps in and the
/// collision surfaces.
#[test]
fn dormant_patch_registry_deps_do_not_claim() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    for (name, target) in [("zlib", "z"), ("zlib-ng", "zng")] {
        assert_fs::fixture::ChildPath::new(index.join(format!("{name}.json")))
            .write_str(&format!(
                r#"{{
  "schema": 1,
  "name": "{name}",
  "versions": {{
    "1.0.0": {{
      "dependencies": {{}},
      "yanked": false,
      "standards": {{ "targets": {{ "{target}": {{}} }} }},
      "links": {{ "{target}": "z" }}
    }}
  }}
}}"#
            ))
            .unwrap();
    }
    // The patched fork of `spare` (a package nothing depends on)
    // declares a registry dep whose index claim collides with zlib's.
    assert_fs::fixture::ChildPath::new(dir.path().join("spare-fork/cabin.toml"))
        .write_str(
            r#"[package]
name = "spare"
version = "1.0.0"

[dependencies]
zlib-ng = ">=1"
"#,
        )
        .unwrap();
    let app_manifest = |deps: &str| {
        format!(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
zlib = ">=1"
{deps}

[patch]
spare = {{ path = "../spare-fork" }}

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#
        )
    };
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(&app_manifest(""))
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    let manifest = dir.path().join("app/cabin.toml");

    // Dormant: nothing depends on `spare`, so its fork's zlib-ng dep
    // stays out of resolution and nothing collides.
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .success();

    // Referenced: the same patch now backs a real dependency, its
    // fork's deps fold in, and the collision surfaces.
    assert_fs::fixture::ChildPath::new(manifest.clone())
        .write_str(&app_manifest("spare = \">=1\""))
        .unwrap();
    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        flat_contains(&stderr, COLLISION_PHRASE),
        "a referenced patch's registry deps must still claim: {stderr}"
    );
}

/// A claim reachable only through a patched-away upstream's index
/// edges never links - the fork's real dependencies replace those
/// edges - so it must not collide with a live claimant.  Disabling
/// the patch makes the upstream edge real again and the same fixture
/// collides.
#[test]
fn upstream_only_deps_of_patched_packages_do_not_claim() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    // app -> mid -> inner (patched); upstream inner -> zlib, which
    // shares an identity with the live zlib-ng.
    for (name, deps, links) in [
        ("mid", r#""inner": ">=1""#, ""),
        ("inner", r#""zlib": ">=1""#, ""),
        (
            "zlib",
            "",
            r#", "standards": { "targets": { "z": {} } }, "links": { "z": "z" }"#,
        ),
        (
            "zlib-ng",
            "",
            r#", "standards": { "targets": { "zng": {} } }, "links": { "zng": "z" }"#,
        ),
    ] {
        assert_fs::fixture::ChildPath::new(index.join(format!("{name}.json")))
            .write_str(&format!(
                r#"{{
  "schema": 1,
  "name": "{name}",
  "versions": {{
    "1.0.0": {{ "dependencies": {{ {deps} }}, "yanked": false{links} }}
  }}
}}"#
            ))
            .unwrap();
    }
    // Claim-free fork of `inner` without the zlib dependency.
    assert_fs::fixture::ChildPath::new(dir.path().join("inner-fork/cabin.toml"))
        .write_str(
            r#"[package]
name = "inner"
version = "1.0.0"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
mid = ">=1"
zlib-ng = ">=1"

[patch]
inner = { path = "../inner-fork" }

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    let manifest = dir.path().join("app/cabin.toml");

    // Patched: the fork drops the zlib edge, so zlib's claim never
    // links and nothing collides.
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .success();

    // Unpatched, the upstream edge is real and the collision fires.
    let output = cabin()
        .args(["resolve", "--no-patches", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        flat_contains(&stderr, COLLISION_PHRASE),
        "the unpatched upstream claim must still collide: {stderr}"
    );
}

/// A disabled optional path dependency contributes nothing to
/// resolution at all: its own registry dependencies (and their index
/// claims) stay out of the graph, so a claim conflict materializes
/// only when the feature enables the edge.
#[test]
fn disabled_optional_path_deps_registry_deps_do_not_claim() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    for (name, target) in [("zlib", "z"), ("zlib-ng", "zng")] {
        assert_fs::fixture::ChildPath::new(index.join(format!("{name}.json")))
            .write_str(&format!(
                r#"{{
  "schema": 1,
  "name": "{name}",
  "versions": {{
    "1.0.0": {{
      "dependencies": {{}},
      "yanked": false,
      "standards": {{ "targets": {{ "{target}": {{}} }} }},
      "links": {{ "{target}": "z" }}
    }}
  }}
}}"#
            ))
            .unwrap();
    }
    assert_fs::fixture::ChildPath::new(dir.path().join("extras/cabin.toml"))
        .write_str(
            r#"[package]
name = "extras"
version = "0.1.0"

[dependencies]
zlib-ng = ">=1"

[target.extras]
type = "library"
sources = ["src/extras.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("extras/src/extras.c"))
        .write_str("void e(void) {}\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
zlib = ">=1"
extras = { path = "../extras", optional = true }

[features]
bundled = ["dep:extras"]

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    let manifest = dir.path().join("app/cabin.toml");

    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .success();

    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index)
        .args(["--features", "bundled"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        flat_contains(&stderr, COLLISION_PHRASE),
        "enabling the feature must surface the collision: {stderr}"
    );
}

/// A patched-away upstream reached through a transitive registry
/// edge contributes no index claims: the build links the local
/// replacement, so the upstream's identity must not collide on its
/// behalf.  Without the patch the same graph collides - the
/// sensitivity control.
#[test]
fn patched_away_upstream_index_claims_do_not_collide() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    assert_fs::fixture::ChildPath::new(index.join("foo.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "foo",
  "versions": {
    "1.0.0": {
      "dependencies": {},
      "yanked": false,
      "standards": { "targets": { "foo": {} } },
      "links": { "foo": "z" }
    }
  }
}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(index.join("bar.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "bar",
  "versions": {
    "1.0.0": {
      "dependencies": { "foo": ">=1" },
      "yanked": false,
      "standards": { "targets": { "bar": {} } },
      "links": { "bar": "z" }
    }
  }
}"#,
        )
        .unwrap();
    // The patched fork of `foo` claims nothing.
    assert_fs::fixture::ChildPath::new(dir.path().join("foo-fork/cabin.toml"))
        .write_str(
            r#"[package]
name = "foo"
version = "1.0.0"
"#,
        )
        .unwrap();
    let app_manifest = |patch: &str| {
        format!(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
bar = ">=1"
{patch}

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#
        )
    };
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(&app_manifest(""))
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    let manifest = dir.path().join("app/cabin.toml");

    // Sensitivity control: without the patch, upstream foo and bar
    // both claim `z` and resolution refuses.
    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .failure()
        .get_output()
        .clone();
    assert!(
        flat_contains(&String::from_utf8_lossy(&output.stderr), COLLISION_PHRASE),
        "the unpatched graph must collide"
    );

    // With foo patched to a claim-free fork, only bar claims `z`.
    assert_fs::fixture::ChildPath::new(manifest.clone())
        .write_str(&app_manifest("[patch]\nfoo = { path = \"../foo-fork\" }"))
        .unwrap();
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .success();
}

/// A patch reached only through a transitive registry edge still
/// activates: its fork's own registry dependencies join resolution
/// (here the fork adds `extra`, which upstream `foo` never had), and
/// the fork's claims participate - a claiming fork collides exactly
/// like any local package.
#[test]
fn transitively_reached_patch_activates_deps_and_claims() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    for (name, deps, links) in [
        ("foo", r#"{}"#, r#"{ "foo": "z" }"#),
        ("bar", r#"{ "foo": ">=1" }"#, r#"{}"#),
        ("extra", r#"{}"#, r#"{}"#),
    ] {
        let links_field = if links == "{}" {
            String::new()
        } else {
            // These fixtures claim under a target named after the
            // package; the loader requires a `standards` row per
            // claiming target.
            format!(
                ",\n      \"standards\": {{ \"targets\": {{ \"{name}\": {{}} }} }},\n      \"links\": {links}"
            )
        };
        assert_fs::fixture::ChildPath::new(index.join(format!("{name}.json")))
            .write_str(&format!(
                r#"{{
  "schema": 1,
  "name": "{name}",
  "versions": {{
    "1.0.0": {{
      "dependencies": {deps},
      "yanked": false{links_field}
    }}
  }}
}}"#
            ))
            .unwrap();
    }
    // The fork adds a dependency upstream foo never had, and claims
    // an identity of its own.
    assert_fs::fixture::ChildPath::new(dir.path().join("foo-fork/cabin.toml"))
        .write_str(
            r#"[package]
name = "foo"
version = "1.0.0"

[dependencies]
extra = ">=1"

[target.foo]
type = "library"
sources = ["src/foo.c"]
c-standard = "c11"
links = "foo-fork"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("foo-fork/src/foo.c"))
        .write_str("void f(void) {}\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
bar = ">=1"

[patch]
foo = { path = "../foo-fork" }

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    let manifest = dir.path().join("app/cabin.toml");

    // The fork's added dep resolves even though nothing local
    // references `foo` - activation discovered it transitively.
    let value = run_json(
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(&manifest)
            .arg("--index-path")
            .arg(&index)
            .args(["--format", "json"]),
    );
    let names: Vec<&str> = value["packages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert!(
        names.contains(&"extra"),
        "the fork's added dependency must resolve: {names:?}"
    );

    // And the fork's own claim participates: a direct dep claiming
    // the fork's identity collides with it.
    write_claiming_lib(dir.path(), "clash", "clash", "foo-fork");
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
bar = ">=1"
clash = { path = "../clash" }

[patch]
foo = { path = "../foo-fork" }

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    for phrase in [COLLISION_PHRASE, "foo v1.0.0", "clash v0.1.0"] {
        assert!(
            flat_contains(&stderr, phrase),
            "expected {phrase:?} in: {stderr}"
        );
    }
}

/// A patch chained through another patch's fork still claims: with
/// `app -> indexed A -> patched B` and fork `B` depending on patched
/// `C`, fork `C`'s identity participates in the graph-wide check
/// even though no solution ever surfaces `C`.
#[test]
fn chained_patch_fork_claims_participate() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    assert_fs::fixture::ChildPath::new(index.join("aaa.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "aaa",
  "versions": {
    "1.0.0": { "dependencies": { "bbb": ">=1" }, "yanked": false }
  }
}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(index.join("bbb.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "bbb",
  "versions": {
    "1.0.0": { "dependencies": {}, "yanked": false }
  }
}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("bbb-fork/cabin.toml"))
        .write_str(
            r#"[package]
name = "bbb"
version = "1.0.0"

[dependencies]
ccc = ">=0.1"
"#,
        )
        .unwrap();
    // The chained fork claims the contested identity.
    write_claiming_lib(dir.path(), "ccc-fork", "ccc", "z");
    write_claiming_lib(dir.path(), "clash", "clash", "z");
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
aaa = ">=1"
clash = { path = "../clash" }

[patch]
bbb = { path = "../bbb-fork" }
ccc = { path = "../ccc-fork" }

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();

    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(&index)
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    for phrase in [COLLISION_PHRASE, "ccc v0.1.0", "clash v0.1.0"] {
        assert!(
            flat_contains(&stderr, phrase),
            "expected {phrase:?} in: {stderr}"
        );
    }
}

/// `cabin metadata --format json` surfaces a target's declared
/// `links` claim, and targets without one carry no `links` key.
#[test]
fn metadata_json_carries_target_links() {
    let dir = TempDir::new().unwrap();
    write_claiming_lib(dir.path(), "zlib", "zlib", "z");

    let value = run_json(
        cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("zlib/cabin.toml"))
            .args(["--format", "json"]),
    );
    let target = &value["packages"][0]["targets"][0];
    assert_eq!(target["name"], "zlib");
    assert_eq!(target["links"], "z");

    write_claiming_lib(dir.path(), "plain", "plain", "p");
    let manifest = dir.path().join("plain/cabin.toml");
    let body = std::fs::read_to_string(&manifest).unwrap();
    assert_fs::fixture::ChildPath::new(manifest.clone())
        .write_str(&body.replace("links = \"p\"\n", ""))
        .unwrap();
    let value = run_json(
        cabin()
            .args(["metadata", "--manifest-path"])
            .arg(&manifest)
            .args(["--format", "json"]),
    );
    assert!(
        value["packages"][0]["targets"][0].get("links").is_none(),
        "a claim-free target must not serialize a links key"
    );
}

/// An activation the re-solve walks back must stop claiming: the
/// fork deps injected for a transitively-discovered patch can flip
/// its parent to a version that never reaches the patched name, and
/// a fork absent from the final graph never links.
#[test]
fn deactivated_transitive_patch_claims_are_pruned() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    // aaa 2.0.0 reaches patched bbb but pins ccc to 1.x; the bbb
    // fork's own `ccc >= 2` requirement therefore flips aaa back to
    // 1.0.0, which drops bbb from the solution entirely.
    assert_fs::fixture::ChildPath::new(index.join("aaa.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "aaa",
  "versions": {
    "1.0.0": { "dependencies": {}, "yanked": false },
    "2.0.0": {
      "dependencies": { "bbb": ">=1", "ccc": "=1.0.0" },
      "yanked": false
    }
  }
}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(index.join("bbb.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "bbb",
  "versions": { "1.0.0": { "dependencies": {}, "yanked": false } }
}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(index.join("ccc.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "ccc",
  "versions": {
    "1.0.0": { "dependencies": {}, "yanked": false },
    "2.0.0": { "dependencies": {}, "yanked": false }
  }
}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(index.join("zzz.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "zzz",
  "versions": {
    "1.0.0": {
      "dependencies": {},
      "yanked": false,
      "standards": { "targets": { "zzz": {} } },
      "links": { "zzz": "z" }
    }
  }
}"#,
        )
        .unwrap();
    // The fork claims the same identity as zzz - a collision only if
    // the fork actually links.
    assert_fs::fixture::ChildPath::new(dir.path().join("bbb-fork/cabin.toml"))
        .write_str(
            r#"[package]
name = "bbb"
version = "1.0.0"

[dependencies]
ccc = ">=2"

[target.bbb]
type = "library"
sources = ["src/bbb.c"]
c-standard = "c11"
links = "z"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("bbb-fork/src/bbb.c"))
        .write_str("void b(void) {}\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
aaa = ">=1"
zzz = ">=1"

[patch]
bbb = { path = "../bbb-fork" }

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();

    let value = run_json(
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(&index)
            .args(["--format", "json"]),
    );
    let packages: Vec<(String, String)> = value["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["name"].as_str().unwrap().to_owned(),
                p["version"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert!(
        packages.contains(&("aaa".to_owned(), "1.0.0".to_owned())),
        "the fork's dep must flip aaa to 1.0.0: {packages:?}"
    );
    assert!(
        packages.iter().all(|(name, _)| name != "bbb"),
        "nothing reaches the patched name after the flip: {packages:?}"
    );
    // The injected fork dep stays resolved - withdrawing it would
    // re-select aaa 2.0.0 and oscillate; the residue is an unused
    // package, not a claim.
    assert!(
        packages.contains(&("ccc".to_owned(), "2.0.0".to_owned())),
        "the injected fork dep remains in the solution: {packages:?}"
    );
}

/// The root deps injected for a pruned activation stay in the solve
/// (withdrawing them oscillates), but the orphaned selections they
/// pull in never link: their index claims must not collide with a
/// live package's.  Reachable claimants keep colliding - the
/// suppression is residue-only.
#[test]
fn pruned_activation_residue_claims_are_suppressed() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    // Same flip as the pruning test: aaa 2.0.0 reaches patched bbb
    // but pins ccc to 1.x, so the fork's `ccc >= 2` flips aaa back
    // to 1.0.0 and orphans ccc.  Here the orphan itself claims the
    // identity zzz holds.
    assert_fs::fixture::ChildPath::new(index.join("aaa.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "aaa",
  "versions": {
    "1.0.0": { "dependencies": {}, "yanked": false },
    "2.0.0": {
      "dependencies": { "bbb": ">=1", "ccc": "=1.0.0" },
      "yanked": false
    }
  }
}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(index.join("bbb.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "bbb",
  "versions": { "1.0.0": { "dependencies": {}, "yanked": false } }
}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(index.join("ccc.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "ccc",
  "versions": {
    "1.0.0": { "dependencies": {}, "yanked": false },
    "2.0.0": {
      "dependencies": {},
      "yanked": false,
      "standards": { "targets": { "ccc": {} } },
      "links": { "ccc": "z" }
    }
  }
}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(index.join("zzz.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "zzz",
  "versions": {
    "1.0.0": {
      "dependencies": {},
      "yanked": false,
      "standards": { "targets": { "zzz": {} } },
      "links": { "zzz": "z" }
    }
  }
}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("bbb-fork/cabin.toml"))
        .write_str(
            r#"[package]
name = "bbb"
version = "1.0.0"

[dependencies]
ccc = ">=2"

[target.bbb]
type = "library"
sources = ["src/bbb.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("bbb-fork/src/bbb.c"))
        .write_str("void b(void) {}\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
aaa = ">=1"
zzz = ">=1"

[patch]
bbb = { path = "../bbb-fork" }

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();

    // The orphaned ccc 2.0.0 is selected (residue) but never links,
    // so its claim must not collide with zzz's.
    let value = run_json(
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(&index)
            .args(["--format", "json"]),
    );
    let packages: Vec<(String, String)> = value["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["name"].as_str().unwrap().to_owned(),
                p["version"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert!(
        packages.contains(&("ccc".to_owned(), "2.0.0".to_owned())),
        "the residue stays resolved: {packages:?}"
    );

    // Sensitivity control: the same ccc 2.0.0 reached by a real root
    // dep still claims, so the identical identity pair collides.
    assert_fs::fixture::ChildPath::new(dir.path().join("app2/cabin.toml"))
        .write_str(
            r#"[package]
name = "app2"
version = "0.1.0"

[dependencies]
ccc = ">=2"
zzz = ">=1"

[target.app2]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app2/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app2/cabin.toml"))
        .arg("--index-path")
        .arg(&index)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    for needle in ["\"z\"", "ccc v2.0.0", "zzz v1.0.0"] {
        assert!(stderr.contains(needle), "expected {needle:?} in: {stderr}");
    }
}

/// Residue can select a *patched* name: after the flip prunes bbb,
/// the orphaned ccc 2.0.0 still back-edges onto patched ddd.  A
/// fork reached only through residue never links, so ddd must stay
/// dormant - keeping it activated would let its fork's claim
/// falsely collide with zzz's.  Liveness is reachability, not
/// probe membership.
#[test]
fn residue_reached_patch_stays_dormant() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    assert_fs::fixture::ChildPath::new(index.join("aaa.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "aaa",
  "versions": {
    "1.0.0": { "dependencies": {}, "yanked": false },
    "2.0.0": {
      "dependencies": { "bbb": ">=1", "ccc": "=1.0.0" },
      "yanked": false
    }
  }
}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(index.join("bbb.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "bbb",
  "versions": { "1.0.0": { "dependencies": {}, "yanked": false } }
}"#,
        )
        .unwrap();
    // ccc 2.0.0 - reachable only as the pruned bbb fork's injected
    // dep - is what drags patched ddd into the solution.
    assert_fs::fixture::ChildPath::new(index.join("ccc.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "ccc",
  "versions": {
    "1.0.0": { "dependencies": {}, "yanked": false },
    "2.0.0": { "dependencies": { "ddd": ">=1" }, "yanked": false }
  }
}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(index.join("ddd.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "ddd",
  "versions": { "1.0.0": { "dependencies": {}, "yanked": false } }
}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(index.join("zzz.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "zzz",
  "versions": {
    "1.0.0": {
      "dependencies": {},
      "yanked": false,
      "standards": { "targets": { "zzz": {} } },
      "links": { "zzz": "z" }
    }
  }
}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("bbb-fork/cabin.toml"))
        .write_str(
            r#"[package]
name = "bbb"
version = "1.0.0"

[dependencies]
ccc = ">=2"

[target.bbb]
type = "library"
sources = ["src/bbb.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("bbb-fork/src/bbb.c"))
        .write_str("void b(void) {}\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("ddd-fork/cabin.toml"))
        .write_str(
            r#"[package]
name = "ddd"
version = "1.0.0"

[target.ddd]
type = "library"
sources = ["src/ddd.c"]
c-standard = "c11"
links = "z"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("ddd-fork/src/ddd.c"))
        .write_str("void d(void) {}\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
aaa = ">=1"
zzz = ">=1"

[patch]
bbb = { path = "../bbb-fork" }
ddd = { path = "../ddd-fork" }

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();

    let value = run_json(
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(&index)
            .args(["--format", "json"]),
    );
    let packages: Vec<(String, String)> = value["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["name"].as_str().unwrap().to_owned(),
                p["version"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert!(
        packages.contains(&("aaa".to_owned(), "1.0.0".to_owned())),
        "the fork's dep must flip aaa to 1.0.0: {packages:?}"
    );
    assert!(
        packages.iter().all(|(name, _)| name != "bbb"),
        "nothing reaches the patched name after the flip: {packages:?}"
    );
    assert!(
        packages.contains(&("ddd".to_owned(), "1.0.0".to_owned())),
        "residue-reached ddd stays a plain index selection: {packages:?}"
    );
}

/// A transitively-activated fork's *path dependencies* link too:
/// the reload follows the fork's path edges, so a claim declared by
/// a package the fork pulls in must collide with a registry
/// package's claim exactly like the fork's own.
#[test]
fn activated_fork_path_dep_claims_participate() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    for (name, deps, links) in [
        ("aaa", r#"{ "bbb": ">=1" }"#, r#"{}"#),
        ("bbb", r#"{}"#, r#"{}"#),
        ("zzz", r#"{}"#, r#"{ "zzz": "z" }"#),
    ] {
        let links_field = if links == "{}" {
            String::new()
        } else {
            // These fixtures claim under a target named after the
            // package; the loader requires a `standards` row per
            // claiming target.
            format!(
                ",\n      \"standards\": {{ \"targets\": {{ \"{name}\": {{}} }} }},\n      \"links\": {links}"
            )
        };
        assert_fs::fixture::ChildPath::new(index.join(format!("{name}.json")))
            .write_str(&format!(
                r#"{{
  "schema": 1,
  "name": "{name}",
  "versions": {{
    "1.0.0": {{
      "dependencies": {deps},
      "yanked": false{links_field}
    }}
  }}
}}"#
            ))
            .unwrap();
    }
    // The fork itself claims nothing; its path dependency does.
    write_claiming_lib(dir.path(), "zlocal", "zlocal", "z");
    assert_fs::fixture::ChildPath::new(dir.path().join("bbb-fork/cabin.toml"))
        .write_str(
            r#"[package]
name = "bbb"
version = "1.0.0"

[dependencies]
zlocal = { path = "../zlocal" }

[target.bbb]
type = "library"
sources = ["src/bbb.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("bbb-fork/src/bbb.c"))
        .write_str("void b(void) {}\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
aaa = ">=1"
zzz = ">=1"

[patch]
bbb = { path = "../bbb-fork" }

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();

    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(&index)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    for needle in ["\"z\"", "zlocal", "zzz v1.0.0"] {
        assert!(stderr.contains(needle), "expected {needle:?} in: {stderr}");
    }
}

/// Shared fixture for the optional-path-dep fork claim tests: `app`
/// reaches patched `bbb` through indexed `aaa`, and the fork
/// declares its claiming path dependency `zlocal` behind a feature.
/// `default_enables` decides whether the fork's `default` feature
/// turns that dependency on.
fn write_fork_optional_claim_fixture(root: &std::path::Path, default_enables: bool) {
    let index = root.join("index");
    for (name, deps, links) in [
        ("aaa", r#"{ "bbb": ">=1" }"#, r#"{}"#),
        ("bbb", r#"{}"#, r#"{}"#),
        ("zzz", r#"{}"#, r#"{ "zzz": "z" }"#),
    ] {
        let links_field = if links == "{}" {
            String::new()
        } else {
            // These fixtures claim under a target named after the
            // package; the loader requires a `standards` row per
            // claiming target.
            format!(
                ",\n      \"standards\": {{ \"targets\": {{ \"{name}\": {{}} }} }},\n      \"links\": {links}"
            )
        };
        assert_fs::fixture::ChildPath::new(index.join(format!("{name}.json")))
            .write_str(&format!(
                r#"{{
  "schema": 1,
  "name": "{name}",
  "versions": {{
    "1.0.0": {{
      "dependencies": {deps},
      "yanked": false{links_field}
    }}
  }}
}}"#
            ))
            .unwrap();
    }
    write_claiming_lib(root, "zlocal", "zlocal", "z");
    let default = if default_enables {
        r#"["withz"]"#
    } else {
        "[]"
    };
    assert_fs::fixture::ChildPath::new(root.join("bbb-fork/cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "bbb"
version = "1.0.0"

[features]
default = {default}
withz = ["dep:zlocal"]

[dependencies]
zlocal = {{ path = "../zlocal", optional = true }}

[target.bbb]
type = "library"
sources = ["src/bbb.c"]
c-standard = "c11"
"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("bbb-fork/src/bbb.c"))
        .write_str("void b(void) {}\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
aaa = ">=1"
zzz = ">=1"

[patch]
bbb = { path = "../bbb-fork" }

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
}

/// A transitively-activated fork's *feature-disabled* optional path
/// dependency is loaded into the graph but never linked, so its
/// claims must not participate - the same feature scoping
/// `local_links_claims` applies to the selected closure.
#[test]
fn fork_disabled_optional_path_dep_claims_do_not_participate() {
    let dir = TempDir::new().unwrap();
    write_fork_optional_claim_fixture(dir.path(), false);

    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .assert()
        .success();
}

/// A transitively-activated fork's *feature-enabled* optional path
/// dependency is out of resolution's reach: the fork's real feature
/// set is decided by its dependents' edges, which the resolution
/// layer deliberately does not interpret, so resolve and update
/// stay conservative (mandatory island claims only) and the
/// collision defers to the building commands' final-graph check
/// (`fork_default_enabled_optional_dep_collides_at_build`).
#[test]
fn fork_default_enabled_optional_path_dep_defers_to_build() {
    let dir = TempDir::new().unwrap();
    write_fork_optional_claim_fixture(dir.path(), true);

    for command in ["resolve", "update"] {
        cabin()
            .args([command, "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .assert()
            .success();
    }
}

/// Write a single-C-library package at `root/src-<short>` and
/// publish it into the fixture's `root/registry` file registry.
/// The stub symbol is derived from `short` so several published
/// libraries can link into one binary.
fn publish_pkg(root: &Path, short: &str, manifest: &str) {
    let pkg_root = root.join(format!("src-{short}"));
    assert_fs::fixture::ChildPath::new(pkg_root.join("cabin.toml"))
        .write_str(manifest)
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join("src/lib.c"))
        .write_str(&format!("int {short}_stub(void) {{ return 0; }}\n"))
        .unwrap();
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(root.join("registry"))
        .assert()
        .success();
}

/// Publish-staged fixture for the build-level fork claim tests:
/// `app -> acme/aaa (registry) -> acme/bbb (patched locally)`, plus
/// registry claimant `acme/zzz` (`links = "z"`).  The fork declares
/// the claiming path dependency `zlocal` optional behind feature
/// `withz`; `bbb_edge` is aaa's dependency declaration for the
/// patched name - the real per-edge feature request the final
/// reload applies - and `default_enables` decides whether the
/// fork's `default` feature turns `withz` on.  Returns the app
/// manifest path.
fn write_fork_build_fixture(root: &Path, bbb_edge: &str, default_enables: bool) -> PathBuf {
    publish_pkg(
        root,
        "bbb",
        r#"[package]
name = "acme/bbb"
version = "1.0.0"

[target.bbb]
type = "library"
sources = ["src/lib.c"]
c-standard = "c11"
"#,
    );
    publish_pkg(
        root,
        "aaa",
        &format!(
            r#"[package]
name = "acme/aaa"
version = "1.0.0"

[dependencies]
{bbb_edge}

[target.aaa]
type = "library"
sources = ["src/lib.c"]
c-standard = "c11"
"#
        ),
    );
    publish_pkg(
        root,
        "zzz",
        r#"[package]
name = "acme/zzz"
version = "1.0.0"

[target.zzz]
type = "library"
sources = ["src/lib.c"]
c-standard = "c11"
links = "z"
"#,
    );
    write_claiming_lib(root, "zlocal", "zlocal", "z");
    let default = if default_enables {
        r#"["withz"]"#
    } else {
        "[]"
    };
    assert_fs::fixture::ChildPath::new(root.join("bbb-fork/cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "acme/bbb"
version = "1.0.0"

[features]
default = {default}
withz = ["dep:zlocal"]

[dependencies]
zlocal = {{ path = "../zlocal", optional = true }}

[target.bbb]
type = "library"
sources = ["src/bbb.c"]
c-standard = "c11"
"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("bbb-fork/src/bbb.c"))
        .write_str("void b(void) {}\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
"acme/aaa" = ">=1"
"acme/zzz" = ">=1"

[patch]
"acme/bbb" = { path = "../bbb-fork" }

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    root.join("app/cabin.toml")
}

fn build_assert(root: &Path, manifest: &Path) -> assert_cmd::assert::Assert {
    cabin()
        .args(["build", "--manifest-path"])
        .arg(manifest)
        .arg("--index-path")
        .arg(root.join("registry"))
        .arg("--cache-dir")
        .arg(root.join("cache"))
        .arg("--build-dir")
        .arg(root.join("build"))
        .assert()
}

fn resolve_with_index_assert(root: &Path, manifest: &Path) -> assert_cmd::assert::Assert {
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(manifest)
        .arg("--index-path")
        .arg(root.join("registry"))
        .assert()
}

/// The build-level control for
/// `fork_default_enabled_optional_path_dep_defers_to_build`: the
/// final reload applies aaa's real edge request (a plain `>=1`
/// keeps the fork's default features on), the enabled `withz`
/// links `zlocal`, and the final-graph check refuses the collision
/// with the registry claimant.
#[test]
fn fork_default_enabled_optional_dep_collides_at_build() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let manifest = write_fork_build_fixture(dir.path(), r#""acme/bbb" = ">=1""#, true);

    resolve_with_index_assert(dir.path(), &manifest).success();
    let output = build_assert(dir.path(), &manifest).failure();
    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    for needle in [COLLISION_PHRASE, "\"z\"", "zlocal", "acme/zzz"] {
        assert!(stderr.contains(needle), "expected {needle:?} in: {stderr}");
    }
}

/// The false-reject control: aaa's real edge disables the fork's
/// default features, so `withz` stays off and `zlocal` never links.
/// Resolution must not assume any particular edge request (it would
/// refuse this graph if it assumed defaults), and the final-graph
/// check sees the disabled dependency excluded - both accept.
#[test]
fn fork_edge_default_features_off_builds_clean() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let manifest = write_fork_build_fixture(
        dir.path(),
        r#""acme/bbb" = { version = ">=1", default-features = false }"#,
        true,
    );

    resolve_with_index_assert(dir.path(), &manifest).success();
    build_assert(dir.path(), &manifest).success();
}

/// An explicitly-requested edge feature enables the claiming
/// optional dependency even though the fork's own `default` never
/// would: invisible to resolution's conservative pass, refused by
/// the final-graph check.
#[test]
fn fork_edge_explicit_feature_collides_at_build() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let manifest = write_fork_build_fixture(
        dir.path(),
        r#""acme/bbb" = { version = ">=1", default-features = false, features = ["withz"] }"#,
        false,
    );

    resolve_with_index_assert(dir.path(), &manifest).success();
    let output = build_assert(dir.path(), &manifest).failure();
    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    for needle in [COLLISION_PHRASE, "\"z\"", "zlocal", "acme/zzz"] {
        assert!(stderr.contains(needle), "expected {needle:?} in: {stderr}");
    }
}
