//! Schema-lock tests for the bundled uthash foundation port, plus a
//! hermetic end-to-end build of a fake port whose tarball carries a
//! symlink entry (the uthash `include -> src` shape) to prove port
//! extraction skips it.  The real-upstream build/run path is covered
//! by `cabin_examples.rs::uthash_usage_builds_and_runs`.

use std::io::Write as _;

use sha2::{Digest, Sha256};

use super::*;

#[test]
fn port_toml_schema_for_real_ports_uthash_matches_published_values() {
    let descriptor =
        load_real_port_and_assert_schema("uthash", &semver::Version::new(2, 4, 0), "BSD-1-Clause");
    assert_tar_gz_source(&descriptor, "uthash-2.4.0");
}

#[test]
fn uthash_is_bundled_and_parses() {
    assert_builtin_port_bundled_and_parses("uthash", "^2.4", "2.4.0");
}

#[test]
fn uthash_overlay_declares_header_only_target() {
    let overlay = builtin_overlay("uthash");
    assert!(overlay.contains("[target.uthash]"), "overlay: {overlay}");
    assert!(
        overlay.contains("type = \"header-only\""),
        "overlay: {overlay}"
    );
    // The headers live under src/; the tarball's `include` symlink is
    // skipped at extraction and never referenced.
    assert!(
        overlay.contains("include-dirs = [\"src\"]"),
        "overlay: {overlay}"
    );
    assert!(
        !overlay.contains("sources"),
        "overlay should not list sources: {overlay}"
    );
}

/// Hermetic end-to-end proof that a port tarball carrying a symlink
/// entry prepares and builds: the symlink is skipped (nothing
/// materialized), the regular header extracts, and a consumer links
/// against the port.  Mirrors the real uthash archive's
/// `include -> src` shape over a local `file://` URL - no network.
#[test]
fn fake_symlinked_port_builds_downstream_consumer() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();

    // Lay the fake upstream tarball: one real header under src/ plus
    // a root-level `include -> src` symlink entry.
    let downloads = tmp.path().join("downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    let tar_path = downloads.join("fakehash-1.0.0.tar.gz");
    {
        let f = std::fs::File::create(&tar_path).unwrap();
        let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        let mut builder = tar::Builder::new(enc);
        let body = b"#define FAKEHASH_ANSWER 42\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                "fakehash-1.0.0/src/fakehash.h",
                &mut std::io::Cursor::new(&body[..]),
            )
            .unwrap();
        let mut link = tar::Header::new_gnu();
        link.set_size(0);
        link.set_mode(0o777);
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_link_name("src").unwrap();
        link.set_cksum();
        builder
            .append_data(
                &mut link,
                "fakehash-1.0.0/include",
                &mut std::io::Cursor::new(b""),
            )
            .unwrap();
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap().flush().unwrap();
    }
    let hex = {
        let bytes = fs::read(&tar_path).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        cabin_core::hash::hex_digest(&hasher.finalize())
    };
    let archive_url = url::Url::from_file_path(&tar_path).unwrap();

    let port_dir = tmp.path().join("ports/fakehash/1.0.0");
    assert_fs::fixture::ChildPath::new(port_dir.join("port.toml"))
        .write_str(&format!(
            "[port]\nname = \"fakehash\"\nversion = \"1.0.0\"\n\n[source]\ntype = \"archive\"\nurl = \"{archive_url}\"\nsha256 = \"{hex}\"\nstrip_prefix = \"fakehash-1.0.0\"\n\n[overlay]\nmanifest = \"cabin.toml\"\n"
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(port_dir.join("cabin.toml"))
        .write_str(
            "[package]\nname = \"fakehash\"\nversion = \"1.0.0\"\ninterface-c-standard = \"c11\"\n\n[target.fakehash]\ntype = \"header-only\"\ninclude-dirs = [\"src\"]\n",
        )
        .unwrap();

    let consumer_manifest = tmp.path().join("consumer/cabin.toml");
    assert_fs::fixture::ChildPath::new(&consumer_manifest)
        .write_str(
            "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\nc-standard = \"c11\"\n\n[dependencies]\nfakehash = { port-path = \"../ports/fakehash/1.0.0\" }\n\n[target.consumer]\ntype = \"executable\"\nsources = [\"src/main.c\"]\ndeps = [\"fakehash\"]\n",
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(tmp.path().join("consumer/src/main.c"))
        .write_str(
            "#include <fakehash.h>\n#include <stdio.h>\n\nint main(void) {\n    printf(\"%d\\n\", FAKEHASH_ANSWER);\n    return 0;\n}\n",
        )
        .unwrap();

    let build_dir = tmp.path().join("build");
    let cache_dir = tmp.path().join("cache");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(&consumer_manifest)
        .arg("--build-dir")
        .arg(&build_dir)
        .arg("--cache-dir")
        .arg(&cache_dir)
        .assert()
        .success();

    // The symlink entry must not have been materialized in the
    // prepared source tree.
    let source_root = cache_dir
        .join("ports/sources/fakehash/1.0.0/sha256")
        .join(&hex);
    assert!(
        source_root.join("src/fakehash.h").is_file(),
        "expected extracted header under {}",
        source_root.display()
    );
    assert!(
        std::fs::symlink_metadata(source_root.join("include")).is_err(),
        "symlink entry must be skipped, not materialized"
    );

    let exe_name = format!("consumer{}", std::env::consts::EXE_SUFFIX);
    let exe = build_dir.join("dev/packages/consumer").join(&exe_name);
    let output = std::process::Command::new(&exe)
        .output()
        .expect("run consumer");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("42"), "stdout = {stdout:?}");
}
