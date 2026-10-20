use super::*;

const FMT_PKG_MANIFEST: &str = r#"[package]
name = "fmt"
version = "10.2.1"
cxx-standard = "c++17"

[target.fmt]
type = "library"
sources = ["src/fmt.cc"]
include-dirs = ["include"]
"#;
const FMT_HEADER: &str = "#pragma once\nvoid say_hello();\n";
const FMT_SRC: &str =
    "#include <iostream>\n#include \"fmt.h\"\nvoid say_hello() { std::cout << \"hello\\n\"; }\n";
const APP_MAIN_USING_FMT: &str = "#include \"fmt.h\"\nint main() { say_hello(); return 0; }\n";

/// Selection-aware fixture: `app` (which declares a versioned
/// dep on `fmt`) plus an unrelated workspace member `b` which
/// declares a versioned dep on `spdlog` that the index does
/// *not* cover.  The fixture builds a real `fmt-10.2.1.zip`
/// archive and writes a matching index entry pointing at it,
/// so `cabin fetch -p app` and `cabin build -p app` can
/// succeed end-to-end without ever consulting `spdlog`.
///
/// Returns the sha256 hex of the produced archive so callers
/// can assert against the cache layout.
fn write_workspace_with_real_fmt_archive(root: &Path) -> String {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[workspace]
members = ["packages/*"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["fmt"]

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/app/src/main.cc"))
        .write_str(APP_MAIN_USING_FMT)
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/b/cabin.toml"))
        .write_str(
            r#"[package]
name = "b"
version = "0.1.0"

[dependencies]
spdlog = "^1"
"#,
        )
        .unwrap();
    let archive_path = root.join("artifacts/fmt-10.2.1.zip");
    let hex = make_archive(
        &archive_path,
        &[
            ("cabin.toml", FMT_PKG_MANIFEST),
            ("include/fmt.h", FMT_HEADER),
            ("src/fmt.cc", FMT_SRC),
        ],
    );
    let rev = &hex[..16];
    let index_body = format!(
        r#"{{
  "schema": 1,
  "name": "fmt",
  "versions": {{
    "10.2.1": {{
      "dependencies": {{}},
      "yanked": false,
      "revision": "{rev}",
      "revisions": {{
        "{rev}": {{
          "checksum": "sha256:{hex}",
          "published-at": "2026-01-01T00:00:00Z",
          "source": {{ "type": "archive", "path": "../artifacts/fmt-10.2.1.zip", "format": "zip" }}
        }}
      }}
    }}
  }}
}}"#
    );
    assert_fs::fixture::ChildPath::new(root.join("index/fmt.json"))
        .write_str(&index_body)
        .unwrap();
    hex
}

/// `cabin resolve -p app` must succeed when only `app`'s
/// versioned deps are covered by the index, even if an
/// unrelated workspace member declares a versioned dep that
/// the index does not know about.
#[test]
fn resolve_p_app_succeeds_when_unrelated_dep_missing_from_index() {
    let dir = TempDir::new().unwrap();
    write_workspace_with_real_fmt_archive(dir.path());
    let assertion = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--package", "app", "--index-path"])
        .arg(dir.path().join("index"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(
        stdout.contains("fmt"),
        "resolve -p app should report `fmt` in its output: {stdout}"
    );
}

/// `cabin fetch -p app` against the same fixture must fully
/// succeed: the `fmt` archive is in the index, has a real
/// checksum, and selection-aware loading must skip the
/// unrelated `spdlog` dep declared by `b`.  We verify both
/// cache state (the archive lands in `archives/sha256/<hex>`)
/// and lockfile state (the lockfile pins `fmt` at the
/// archive's checksum).
#[test]
fn fetch_p_app_extracts_fmt_and_skips_unrelated_dep() {
    let dir = TempDir::new().unwrap();
    let hex = write_workspace_with_real_fmt_archive(dir.path());
    let cache = dir.path().join("cache");
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--package", "app", "--index-path"])
        .arg(dir.path().join("index"))
        .args(["--cache-dir"])
        .arg(&cache)
        .assert()
        .success();
    let archive_in_cache = cache.join("archives/sha256").join(format!("{hex}.zip"));
    assert!(
        archive_in_cache.is_file(),
        "fmt archive must be cached at {archive_in_cache:?}"
    );
    let source_in_cache = cache.join("sources/sha256").join(&hex);
    assert!(
        source_in_cache.join("cabin.toml").is_file(),
        "fmt source must be extracted with cabin.toml at root"
    );
    let lock_path = dir.path().join("cabin.lock");
    assert!(lock_path.is_file(), "workspace lockfile should be written");
    let lock_body = fs::read_to_string(&lock_path).unwrap();
    assert!(
        lock_body.contains(r#"name = "fmt""#),
        "lockfile must pin fmt: {lock_body}"
    );
    assert!(
        lock_body.contains(&format!("checksum = \"sha256:{hex}\"")),
        "lockfile must record fmt's archive checksum: {lock_body}"
    );
    assert!(
        !lock_body.contains("spdlog"),
        "selection-aware fetch must not pin spdlog: {lock_body}"
    );
}

/// `cabin build -p app` against the same fixture must succeed
/// end-to-end when the host toolchain is available: the
/// `fmt` archive is fetched and extracted, the C++ link picks
/// up its `library` target, and the resulting `app`
/// executable lands under the build directory. `b` and its
/// unindexed `spdlog` dep never enter the build graph.
#[test]
fn build_p_app_links_against_real_fmt_archive() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_workspace_with_real_fmt_archive(dir.path());
    let build_dir = dir.path().join("build");
    let cache = dir.path().join("cache");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--package", "app", "--build-dir"])
        .arg(&build_dir)
        .args(["--cache-dir"])
        .arg(&cache)
        .args(["--index-path"])
        .arg(dir.path().join("index"))
        .assert()
        .success();
    let app_exe = build_dir.join("dev/packages/app").join(host_exe("app"));
    assert!(
        app_exe.is_file(),
        "app executable must be produced at {app_exe:?}"
    );
}

/// An unsafe package name in a workspace member manifest must
/// fail at manifest parsing time, *before* any sparse-HTTP
/// URL is constructed.  This pins the rule that
/// `PackageName::new` is the structural gate.
#[test]
fn unsafe_package_name_in_manifest_rejected_before_http_url() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "foo?bar"
version = "0.1.0"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("\"foo?bar\""),
        "error must echo the offending name: {stderr}"
    );
    assert!(
        stderr.contains("ASCII letters") && stderr.contains("ASCII digits"),
        "error must describe the allowed alphabet: {stderr}"
    );
}

/// The manifest dependency *name* is also validated up-front.
/// A direct dep named `foo#bar` (a URL-reserved character) is
/// rejected at parse time so a later `--index-url` flow
/// cannot expand it into a hostile URL.
#[test]
fn unsafe_dep_name_in_manifest_rejected_before_http_url() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")

            .write_str("[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\n\"foo#bar\" = \"1.0.0\"\n")

            .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("foo#bar"),
        "expected error mentioning unsafe dep name foo#bar; stderr was: {stderr}"
    );
}

/// Two members with distinct indexed versioned deps: `app` -> `fmt`
/// (real archive, so `fetch -p app` succeeds end-to-end) and
/// `other` -> `spdlog` (index entry only).  Writes the workspace-wide
/// lockfile with `resolve --workspace` and returns its bytes.
fn write_two_member_workspace_and_lock(root: &Path) -> Vec<u8> {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[workspace]
members = ["app", "other"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("other/cabin.toml"))
        .write_str(
            r#"[package]
name = "other"
version = "0.1.0"

[dependencies]
spdlog = "^1"
"#,
        )
        .unwrap();
    let hex = make_archive(
        &root.join("artifacts/fmt-10.2.1.zip"),
        &[
            ("cabin.toml", FMT_PKG_MANIFEST),
            ("include/fmt.h", FMT_HEADER),
            ("src/fmt.cc", FMT_SRC),
        ],
    );
    write_index_entry(
        &root.join("index"),
        "fmt",
        "10.2.1",
        "{}",
        &hex,
        "../artifacts/fmt-10.2.1.zip",
    );
    write_index_entry_no_source(&root.join("index"), "spdlog", "1.13.0", &"1".repeat(64));
    cabin()
        .args(["resolve", "--workspace", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .arg("--index-path")
        .arg(root.join("index"))
        .assert()
        .success();
    let lock = fs::read(root.join("cabin.lock")).unwrap();
    assert!(
        std::str::from_utf8(&lock).unwrap().contains("spdlog"),
        "the workspace-wide lockfile must pin other's dep"
    );
    lock
}

/// A patched name another member reaches through a registry edge is
/// still a registry entry: a member-scoped run carries its pin like
/// any other, so the next whole-workspace `--locked` run still passes.
#[test]
fn member_scoped_resolve_keeps_a_patched_name_an_unselected_member_pins() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("ws");
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            "[workspace]\nmembers = [\"app\", \"other\"]\n\n[patch]\nfmt = { path = \"../fmt\" }\n",
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("app/cabin.toml"))
        .write_str(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nextra = \"^1\"\n",
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("other/cabin.toml"))
        .write_str(
            "[package]\nname = \"other\"\nversion = \"0.1.0\"\n\n[dependencies]\nalpha = \"^1\"\n",
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(parent.path().join("fmt/cabin.toml"))
        .write_str("[package]\nname = \"fmt\"\nversion = \"10.2.0\"\n")
        .unwrap();
    let index = root.join("index");
    write_index_entry_no_source(&index, "extra", "1.0.0", &"a".repeat(64));
    write_index_entry_no_source(&index, "fmt", "10.2.0", &"c".repeat(64));
    assert_fs::fixture::ChildPath::new(index.join("alpha.json"))
        .write_str(
            r#"{"schema":1,"name":"alpha","versions":{"1.0.0":{"dependencies":{"fmt":">=10.0.0 <11.0.0"},"yanked":false,"revision":"bbbbbbbbbbbbbbbb","revisions":{"bbbbbbbbbbbbbbbb":{"checksum":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","published-at":"2026-01-01T00:00:00Z","source":{"type":"archive","path":"unfetched.zip","format":"zip"}}}}}}"#,
        )
        .unwrap();
    let resolve = |args: &[&str]| {
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(root.join("cabin.toml"))
            .arg("--index-path")
            .arg(&index)
            .args(args)
            .assert()
            .success();
    };
    resolve(&["--workspace"]);
    let before = fs::read(root.join("cabin.lock")).unwrap();
    assert!(
        std::str::from_utf8(&before)
            .unwrap()
            .contains("name = \"fmt\""),
        "other's transitive edge must pin the patched name"
    );
    resolve(&["-p", "app"]);
    assert_eq!(fs::read(root.join("cabin.lock")).unwrap(), before);
    resolve(&["--workspace", "--locked"]);
}

/// The lockfile is workspace-wide once produced: a run that selects
/// only `app` neither drops `other`'s entry nor sees it as stale.
#[test]
fn member_scoped_resolve_keeps_unselected_members_entries() {
    let dir = TempDir::new().unwrap();
    let before = write_two_member_workspace_and_lock(dir.path());
    for locked in [&[][..], &["--locked"][..]] {
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["-p", "app", "--index-path"])
            .arg(dir.path().join("index"))
            .args(locked)
            .assert()
            .success();
        assert_eq!(
            fs::read(dir.path().join("cabin.lock")).unwrap(),
            before,
            "resolve -p app {locked:?} must leave the workspace lockfile as it was"
        );
    }
}

/// Same contract on the artifact pipeline (`fetch` / `build` / `vendor`
/// share it): `fetch -p app` materializes `fmt` only and keeps
/// `other`'s `spdlog` entry, so a whole-workspace `--locked` run still
/// finds it.
#[test]
fn member_scoped_fetch_keeps_unselected_members_entries() {
    let dir = TempDir::new().unwrap();
    let before = write_two_member_workspace_and_lock(dir.path());
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["-p", "app", "--index-path"])
        .arg(dir.path().join("index"))
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success();
    assert_eq!(
        fs::read(dir.path().join("cabin.lock")).unwrap(),
        before,
        "fetch -p app must leave the workspace lockfile as it was"
    );
    cabin()
        .args(["resolve", "--workspace", "--locked", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .assert()
        .success();
}

/// Only a selection covering every member is authoritative for the
/// whole file: once `other` no longer needs `spdlog`, a
/// whole-workspace run prunes the entry.
#[test]
fn whole_workspace_resolve_prunes_entries_no_member_needs() {
    let dir = TempDir::new().unwrap();
    write_two_member_workspace_and_lock(dir.path());
    assert_fs::fixture::ChildPath::new(dir.path().join("other/cabin.toml"))
        .write_str(
            r#"[package]
name = "other"
version = "0.1.0"
"#,
        )
        .unwrap();
    cabin()
        .args(["resolve", "--workspace", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .assert()
        .success();
    let lock = fs::read_to_string(dir.path().join("cabin.lock")).unwrap();
    assert!(
        !lock.contains("spdlog"),
        "obsolete entry must be pruned: {lock}"
    );
}
