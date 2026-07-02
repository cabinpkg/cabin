//! Network-free schema-lock tests for the bundled CLI11
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::cli11_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_cli11_matches_published_values() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during tests");
    let port_toml = PathBuf::from(manifest_dir)
        .join("../cabin-port/ports/CLI11/2.6.2/port.toml")
        .canonicalize()
        .expect("canonicalize ports/CLI11/2.6.2/port.toml");
    let descriptor =
        cabin_port::load_port(&port_toml).expect("ports/CLI11/2.6.2/port.toml should parse");
    assert_eq!(descriptor.name.as_str(), "CLI11");
    assert_eq!(descriptor.version, semver::Version::new(2, 6, 2));
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
            assert_eq!(strip_prefix.as_deref(), Some("CLI11-2.6.2"));
        }
    }
    assert_eq!(
        descriptor.overlay.relative_path,
        PathBuf::from("cabin.toml")
    );
    assert_eq!(descriptor.metadata.license.as_deref(), Some("BSD-3-Clause"));
}

#[test]
fn cli11_is_bundled_and_parses() {
    let entry = cabin_port::builtin::lookup("CLI11", &semver::VersionReq::parse("^2.6").unwrap())
        .expect("CLI11 should be bundled");
    assert_eq!(entry.name, "CLI11");
    assert_eq!(entry.version, "2.6.2");
    let descriptor = cabin_port::parse_port_str(
        entry.port_toml,
        std::path::Path::new("<builtin:CLI11>/port.toml"),
    )
    .expect("embedded CLI11 port.toml parses");
    assert_eq!(descriptor.name.as_str(), "CLI11");
    assert_eq!(descriptor.version.to_string(), "2.6.2");
}

#[test]
fn cli11_overlay_declares_header_only_target() {
    let entry =
        cabin_port::builtin::lookup("CLI11", &semver::VersionReq::parse(">=0").unwrap()).unwrap();
    let overlay = entry.overlay_toml;
    assert!(overlay.contains("[target.CLI11]"), "overlay: {overlay}");
    assert!(
        overlay.contains("type = \"header-only\""),
        "overlay: {overlay}"
    );
    assert!(
        overlay.contains("include-dirs = [\"include\"]"),
        "overlay: {overlay}"
    );
    // Upstream's public interface requirement is C++11.
    assert!(
        overlay.contains("interface-cxx-standard = \"c++11\""),
        "overlay: {overlay}"
    );
    // The opt-in CLI11_COMPILE precompiled variant stays unbuilt.
    assert!(
        !overlay.contains("sources"),
        "overlay should not list sources: {overlay}"
    );
}
