//! Network-free schema-lock tests for the bundled nlohmann_json
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::nlohmann_json_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_nlohmann_json_matches_published_values() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during tests");
    let port_toml = PathBuf::from(manifest_dir)
        .join("../cabin-port/ports/nlohmann_json/3.12.0/port.toml")
        .canonicalize()
        .expect("canonicalize ports/nlohmann_json/3.12.0/port.toml");
    let descriptor = cabin_port::load_port(&port_toml)
        .expect("ports/nlohmann_json/3.12.0/port.toml should parse");
    assert_eq!(descriptor.name.as_str(), "nlohmann_json");
    assert_eq!(descriptor.version, semver::Version::new(3, 12, 0));
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
            assert_eq!(strip_prefix.as_deref(), Some("json-3.12.0"));
        }
    }
    assert_eq!(
        descriptor.overlay.relative_path,
        PathBuf::from("cabin.toml")
    );
    assert_eq!(descriptor.metadata.license.as_deref(), Some("MIT"));
}

#[test]
fn nlohmann_json_is_bundled_and_parses() {
    let entry = cabin_port::builtin::lookup(
        "nlohmann_json",
        &semver::VersionReq::parse("^3.12").unwrap(),
    )
    .expect("nlohmann_json should be bundled");
    assert_eq!(entry.name, "nlohmann_json");
    assert_eq!(entry.version, "3.12.0");
    let descriptor = cabin_port::parse_port_str(
        entry.port_toml,
        std::path::Path::new("<builtin:nlohmann_json>/port.toml"),
    )
    .expect("embedded nlohmann_json port.toml parses");
    assert_eq!(descriptor.name.as_str(), "nlohmann_json");
    assert_eq!(descriptor.version.to_string(), "3.12.0");
}

#[test]
fn nlohmann_json_overlay_declares_header_only_target() {
    let entry =
        cabin_port::builtin::lookup("nlohmann_json", &semver::VersionReq::parse(">=0").unwrap())
            .unwrap();
    let overlay = entry.overlay_toml;
    assert!(
        overlay.contains("[target.nlohmann_json]"),
        "overlay: {overlay}"
    );
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
    // Header-only targets ship no sources.
    assert!(
        !overlay.contains("sources"),
        "overlay should not list sources: {overlay}"
    );
}
