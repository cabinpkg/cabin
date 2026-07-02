//! Network-free schema-lock tests for the bundled Catch2
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::catch2_usage_runs_tests`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_catch2_matches_published_values() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during tests");
    let port_toml = PathBuf::from(manifest_dir)
        .join("../cabin-port/ports/catch2/3.15.1/port.toml")
        .canonicalize()
        .expect("canonicalize ports/catch2/3.15.1/port.toml");
    let descriptor =
        cabin_port::load_port(&port_toml).expect("ports/catch2/3.15.1/port.toml should parse");
    assert_eq!(descriptor.name.as_str(), "catch2");
    assert_eq!(descriptor.version, semver::Version::new(3, 15, 1));
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
            assert_eq!(strip_prefix.as_deref(), Some("Catch2-3.15.1"));
        }
    }
    assert_eq!(
        descriptor.overlay.relative_path,
        PathBuf::from("cabin.toml")
    );
    assert_eq!(descriptor.metadata.license.as_deref(), Some("BSL-1.0"));
}

#[test]
fn catch2_is_bundled_and_parses() {
    let entry = cabin_port::builtin::lookup("catch2", &semver::VersionReq::parse("^3.15").unwrap())
        .expect("catch2 should be bundled");
    assert_eq!(entry.name, "catch2");
    assert_eq!(entry.version, "3.15.1");
    let descriptor = cabin_port::parse_port_str(
        entry.port_toml,
        std::path::Path::new("<builtin:catch2>/port.toml"),
    )
    .expect("embedded catch2 port.toml parses");
    assert_eq!(descriptor.name.as_str(), "catch2");
    assert_eq!(descriptor.version.to_string(), "3.15.1");
}

#[test]
fn catch2_overlay_declares_amalgamated_library_with_custom_main_feature() {
    let entry =
        cabin_port::builtin::lookup("catch2", &semver::VersionReq::parse(">=0").unwrap()).unwrap();
    let overlay = entry.overlay_toml;
    assert!(overlay.contains("[target.catch2]"), "overlay: {overlay}");
    assert!(overlay.contains("type = \"library\""), "overlay: {overlay}");
    assert!(
        overlay.contains("sources = [\"extras/catch_amalgamated.cpp\"]"),
        "overlay: {overlay}"
    );
    assert!(
        overlay.contains("include-dirs = [\"extras\"]"),
        "overlay: {overlay}"
    );
    // Upstream's public interface requirement is C++14.
    assert!(
        overlay.contains("interface-cxx-standard = \"c++14\""),
        "overlay: {overlay}"
    );
    // The default build ships Catch2's main(); the opt-in
    // `custom-main` feature compiles it out for consumers that
    // define their own.
    assert!(overlay.contains("custom-main = []"), "overlay: {overlay}");
    assert!(
        overlay.contains("defines = [\"CATCH_AMALGAMATED_CUSTOM_MAIN\"]"),
        "overlay: {overlay}"
    );
}
