//! Packaging-revision behavior across the client flows: revisions are
//! selected at fetch time, lockfile checksums pin them exactly, and
//! superseded revisions stay reproducible - locally, over sparse HTTP,
//! and through vendoring.  The publish-side rules (no-op on identical
//! bytes, the `--new-revision` opt-in) are covered beside the other
//! file-registry publish tests; this module covers what consumers see
//! after a respin.

use super::*;

/// One consumer package depending on `fmtlib/fmt = "=10.2.1"`.
fn write_consumer(root: &Path) -> std::path::PathBuf {
    let manifest = root.join("cabin.toml");
    assert_fs::fixture::ChildPath::new(&manifest)
        .write_str(
            r#"[package]
name = "consumer"
version = "0.1.0"
cxx-standard = "c++17"

[dependencies]
"fmtlib/fmt" = "=10.2.1"
"#,
        )
        .unwrap();
    manifest
}

/// Publish `fmtlib/fmt 10.2.1` into `registry`, with `marker` baked
/// into the sources so each publish has distinct bytes; respins pass
/// `--new-revision`.
fn publish_fmt(pkg_root: &Path, registry: &Path, marker: &str, new_revision: bool) {
    assert_fs::fixture::ChildPath::new(pkg_root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "fmtlib/fmt"
version = "10.2.1"
cxx-standard = "c++17"

[target.fmt]
type = "library"
sources = ["src/fmt.cc"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join("include/fmt.h"))
        .write_str("#pragma once\nint fmt_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join("src/fmt.cc"))
        .write_str(&format!(
            "#include \"fmt.h\"\nint fmt_value() {{ return {marker}; }}\n"
        ))
        .unwrap();
    let mut cmd = cabin();
    cmd.args(["publish", "--manifest-path"]);
    cmd.arg(pkg_root.join("cabin.toml"));
    cmd.arg("--registry-dir").arg(registry);
    if new_revision {
        cmd.arg("--new-revision");
    }
    cmd.assert().success();
}

/// The registry's recorded revisions for `fmtlib/fmt 10.2.1`:
/// `(current, all-ids)`.
fn fmt_revisions(registry: &Path) -> (String, Vec<String>) {
    let body = fs::read_to_string(registry.join("packages/fmtlib/fmt.json")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    let entry = &value["versions"]["10.2.1"];
    let current = entry["revision"].as_str().unwrap().to_owned();
    let all = entry["revisions"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    (current, all)
}

/// The lockfile's recorded checksum for `fmtlib/fmt`.
fn locked_checksum(lockfile_path: &Path) -> String {
    let lockfile = fs::read_to_string(lockfile_path).unwrap();
    lockfile
        .lines()
        .skip_while(|line| *line != "name = \"fmtlib/fmt\"")
        .find_map(|line| line.strip_prefix("checksum = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .expect("lockfile records the fmt checksum")
        .to_owned()
}

/// A respin never churns an existing lockfile (the pinned revision is
/// kept while it stays published), a fresh consumer locks the new
/// current revision, and `cabin update` moves the old consumer
/// forward.
#[test]
fn respins_keep_existing_pins_and_new_consumers_get_the_latest_revision() {
    let dir = TempDir::new().unwrap();
    let registry = dir.path().join("registry");
    publish_fmt(&dir.path().join("pkg"), &registry, "1", false);
    let (first_rev, _) = fmt_revisions(&registry);

    // Consumer A resolves against the first revision.
    let manifest_a = write_consumer(&dir.path().join("consumer-a"));
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest_a)
        .arg("--index-path")
        .arg(&registry)
        .assert()
        .success();
    let lockfile_a = manifest_a.parent().unwrap().join("cabin.lock");
    let pinned = locked_checksum(&lockfile_a);
    assert_eq!(&pinned["sha256:".len()..][..16], first_rev);

    // A packaging correction lands as a new revision.
    publish_fmt(&dir.path().join("pkg"), &registry, "2", true);
    let (second_rev, all) = fmt_revisions(&registry);
    assert_ne!(first_rev, second_rev);
    assert_eq!(all.len(), 2);

    // Re-resolving consumer A keeps the pin - someone else's respin
    // must never churn this lockfile.
    let before = fs::read_to_string(&lockfile_a).unwrap();
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest_a)
        .arg("--index-path")
        .arg(&registry)
        .assert()
        .success();
    assert_eq!(fs::read_to_string(&lockfile_a).unwrap(), before);

    // A fresh consumer locks the latest verified revision.
    let manifest_b = write_consumer(&dir.path().join("consumer-b"));
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest_b)
        .arg("--index-path")
        .arg(&registry)
        .assert()
        .success();
    let lockfile_b = manifest_b.parent().unwrap().join("cabin.lock");
    assert_eq!(
        &locked_checksum(&lockfile_b)["sha256:".len()..][..16],
        second_rev
    );

    // `cabin update` is the deliberate move to the current revision.
    cabin()
        .args(["update", "--manifest-path"])
        .arg(&manifest_a)
        .arg("--index-path")
        .arg(&registry)
        .assert()
        .success();
    assert_eq!(
        &locked_checksum(&lockfile_a)["sha256:".len()..][..16],
        second_rev
    );
}

/// A lockfile that pins a superseded revision keeps fetching exactly
/// those bytes - locally and over sparse HTTP - and `--frozen`
/// reuses them from the cache.
#[test]
fn pinned_superseded_revisions_stay_fetchable() {
    let dir = TempDir::new().unwrap();
    let registry = dir.path().join("registry");
    publish_fmt(&dir.path().join("pkg"), &registry, "1", false);
    let (first_rev, _) = fmt_revisions(&registry);

    let manifest = write_consumer(&dir.path().join("consumer"));
    let cache = dir.path().join("cache");
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&registry)
        .arg("--cache-dir")
        .arg(&cache)
        .assert()
        .success();
    let pinned = locked_checksum(&manifest.parent().unwrap().join("cabin.lock"));
    let pinned_hex = pinned.strip_prefix("sha256:").unwrap().to_owned();

    // The respin supersedes the pinned revision.
    publish_fmt(&dir.path().join("pkg"), &registry, "2", true);

    // A locked fetch into a fresh cache must materialize the pinned
    // (now superseded) revision's bytes, not the current one's.
    let fresh_cache = dir.path().join("cache-fresh");
    cabin()
        .args(["fetch", "--locked", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&registry)
        .arg("--cache-dir")
        .arg(&fresh_cache)
        .assert()
        .success();
    assert!(
        fresh_cache
            .join(format!("archives/sha256/{pinned_hex}.zip"))
            .is_file(),
        "the pinned revision's archive must land in the cache"
    );
    assert_eq!(&pinned_hex[..16], first_rev);

    // The same pin reproduces over sparse HTTP.
    let server = crate::sparse_http::TestServer::serve(registry.clone());
    let http_cache = dir.path().join("cache-http");
    cabin()
        .args(["fetch", "--locked", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-url")
        .arg(server.url())
        .arg("--cache-dir")
        .arg(&http_cache)
        .assert()
        .success();
    assert!(
        http_cache
            .join(format!("archives/sha256/{pinned_hex}.zip"))
            .is_file(),
        "the pinned revision must download over HTTP too"
    );

    // `--frozen` reuses the warm cache without a network or index
    // artifact source.
    cabin()
        .args(["fetch", "--frozen", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&registry)
        .arg("--cache-dir")
        .arg(&fresh_cache)
        .assert()
        .success();
}

/// `cabin vendor` under a lockfile pinning a superseded revision
/// reproduces exactly that revision: the vendored index keeps only
/// the pinned revision (as its current one) and an offline re-fetch
/// from the vendor directory yields the pinned bytes.
#[test]
fn vendor_reproduces_the_pinned_revision_after_a_respin() {
    let dir = TempDir::new().unwrap();
    let registry = dir.path().join("registry");
    publish_fmt(&dir.path().join("pkg"), &registry, "1", false);
    let (first_rev, _) = fmt_revisions(&registry);

    let manifest = write_consumer(&dir.path().join("consumer"));
    let cache = dir.path().join("cache");
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&registry)
        .arg("--cache-dir")
        .arg(&cache)
        .assert()
        .success();

    publish_fmt(&dir.path().join("pkg"), &registry, "2", true);

    let vendor_dir = dir.path().join("vendor");
    cabin()
        .args(["vendor", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&registry)
        .arg("--cache-dir")
        .arg(&cache)
        .arg("--vendor-dir")
        .arg(&vendor_dir)
        .assert()
        .success();

    // The vendored entry carries exactly the pinned revision.
    let (vendored_current, vendored_all) = fmt_revisions(&vendor_dir);
    assert_eq!(vendored_current, first_rev);
    assert_eq!(vendored_all, std::slice::from_ref(&first_rev));
    assert!(
        vendor_dir
            .join(format!(
                "artifacts/fmtlib/fmt/fmtlib-fmt-10.2.1-{first_rev}.zip"
            ))
            .is_file()
    );

    // Offline reconstruction from the vendor directory alone lands
    // the pinned bytes in a fresh cache.
    let pinned_hex = locked_checksum(&manifest.parent().unwrap().join("cabin.lock"))
        .strip_prefix("sha256:")
        .unwrap()
        .to_owned();
    let offline_cache = dir.path().join("cache-offline");
    cabin()
        .args(["fetch", "--offline", "--locked", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&vendor_dir)
        .arg("--cache-dir")
        .arg(&offline_cache)
        .assert()
        .success();
    assert!(
        offline_cache
            .join(format!("archives/sha256/{pinned_hex}.zip"))
            .is_file()
    );
}
