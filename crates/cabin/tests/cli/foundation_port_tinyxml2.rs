//! Network-free schema-lock tests for the bundled tinyxml2
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::tinyxml2_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_tinyxml2_matches_published_values() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during tests");
    let port_toml = PathBuf::from(manifest_dir)
        .join("../cabin-port/ports/tinyxml2/11.0.0/port.toml")
        .canonicalize()
        .expect("canonicalize ports/tinyxml2/11.0.0/port.toml");
    let descriptor =
        cabin_port::load_port(&port_toml).expect("ports/tinyxml2/11.0.0/port.toml should parse");
    assert_eq!(descriptor.name.as_str(), "tinyxml2");
    assert_eq!(descriptor.version, semver::Version::new(11, 0, 0));
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
            assert_eq!(strip_prefix.as_deref(), Some("tinyxml2-11.0.0"));
        }
    }
    assert_eq!(
        descriptor.overlay.relative_path,
        PathBuf::from("cabin.toml")
    );
    assert_eq!(descriptor.metadata.license.as_deref(), Some("Zlib"));
}

#[test]
fn tinyxml2_is_bundled_and_parses() {
    let entry = cabin_port::builtin::lookup("tinyxml2", &semver::VersionReq::parse("^11").unwrap())
        .expect("tinyxml2 should be bundled");
    assert_eq!(entry.name, "tinyxml2");
    assert_eq!(entry.version, "11.0.0");
    let descriptor = cabin_port::parse_port_str(
        entry.port_toml,
        std::path::Path::new("<builtin:tinyxml2>/port.toml"),
    )
    .expect("embedded tinyxml2 port.toml parses");
    assert_eq!(descriptor.name.as_str(), "tinyxml2");
    assert_eq!(descriptor.version.to_string(), "11.0.0");
}

#[test]
fn tinyxml2_overlay_declares_single_cxx_library_target() {
    let entry = cabin_port::builtin::lookup("tinyxml2", &semver::VersionReq::parse(">=0").unwrap())
        .unwrap();
    let overlay = entry.overlay_toml;
    assert!(overlay.contains("[target.tinyxml2]"), "overlay: {overlay}");
    assert!(overlay.contains("type = \"library\""), "overlay: {overlay}");
    assert!(
        overlay.contains("sources = [\"tinyxml2.cpp\"]"),
        "overlay: {overlay}"
    );
    // The upstream test harness is intentionally not built.
    assert!(
        !overlay.contains("\"xmltest.cpp\""),
        "overlay should not build the upstream test harness: {overlay}"
    );
}
