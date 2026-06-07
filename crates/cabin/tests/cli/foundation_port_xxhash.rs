//! Network-free schema-lock tests for the bundled xxHash
//! foundation port. The end-to-end build/run path is covered by
//! `cabin_examples.rs::xxhash_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_xxhash_matches_published_values() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during tests");
    let port_toml = PathBuf::from(manifest_dir)
        .join("../cabin-port/ports/xxhash/0.8.3/port.toml")
        .canonicalize()
        .expect("canonicalize ports/xxhash/0.8.3/port.toml");
    let descriptor =
        cabin_port::load_port(&port_toml).expect("ports/xxhash/0.8.3/port.toml should parse");
    assert_eq!(descriptor.name.as_str(), "xxhash");
    assert_eq!(descriptor.version, semver::Version::new(0, 8, 3));
    match &descriptor.source {
        cabin_port::PortSource::Archive {
            url,
            sha256,
            strip_prefix,
        } => {
            assert!(
                url.as_str().ends_with(".tar.gz"),
                "expected a .tar.gz URL, got {url}"
            );
            assert_eq!(url.scheme(), "https");
            assert_eq!(sha256.to_hex().len(), 64);
            assert_eq!(strip_prefix.as_deref(), Some("xxHash-0.8.3"));
        }
    }
    assert_eq!(
        descriptor.overlay.relative_path,
        PathBuf::from("cabin.toml")
    );
    assert_eq!(descriptor.metadata.license.as_deref(), Some("BSD-2-Clause"));
}

#[test]
fn xxhash_is_bundled_and_parses() {
    let entry = cabin_port::builtin::lookup("xxhash", &semver::VersionReq::parse("^0.8").unwrap())
        .expect("xxhash should be bundled");
    assert_eq!(entry.name, "xxhash");
    assert_eq!(entry.version, "0.8.3");
    let descriptor = cabin_port::parse_port_str(
        entry.port_toml,
        std::path::Path::new("<builtin:xxhash>/port.toml"),
    )
    .expect("embedded xxhash port.toml parses");
    assert_eq!(descriptor.name.as_str(), "xxhash");
    assert_eq!(descriptor.version.to_string(), "0.8.3");
}

#[test]
fn xxhash_overlay_declares_single_library_target() {
    let entry =
        cabin_port::builtin::lookup("xxhash", &semver::VersionReq::parse(">=0").unwrap()).unwrap();
    let overlay = entry.overlay_toml;
    assert!(overlay.contains("[target.xxhash]"), "overlay: {overlay}");
    assert!(overlay.contains("type = \"library\""), "overlay: {overlay}");
    assert!(
        overlay.contains("sources = [\"xxhash.c\"]"),
        "overlay: {overlay}"
    );
    // The optional x86 dispatch TU is intentionally not built.
    assert!(
        !overlay.contains("\"xxh_x86dispatch.c\""),
        "overlay should not build the optional x86 dispatch TU: {overlay}"
    );
}
