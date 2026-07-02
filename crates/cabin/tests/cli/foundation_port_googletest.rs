//! Network-free schema-lock tests for the bundled GoogleTest
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::googletest_usage_runs_tests`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_googletest_matches_published_values() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during tests");
    let port_toml = PathBuf::from(manifest_dir)
        .join("../cabin-port/ports/googletest/1.17.0/port.toml")
        .canonicalize()
        .expect("canonicalize ports/googletest/1.17.0/port.toml");
    let descriptor =
        cabin_port::load_port(&port_toml).expect("ports/googletest/1.17.0/port.toml should parse");
    assert_eq!(descriptor.name.as_str(), "googletest");
    assert_eq!(descriptor.version, semver::Version::new(1, 17, 0));
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
            assert_eq!(strip_prefix.as_deref(), Some("googletest-1.17.0"));
        }
    }
    assert_eq!(
        descriptor.overlay.relative_path,
        PathBuf::from("cabin.toml")
    );
    assert_eq!(descriptor.metadata.license.as_deref(), Some("BSD-3-Clause"));
}

#[test]
fn googletest_is_bundled_and_parses() {
    let entry =
        cabin_port::builtin::lookup("googletest", &semver::VersionReq::parse("^1.17").unwrap())
            .expect("googletest should be bundled");
    assert_eq!(entry.name, "googletest");
    assert_eq!(entry.version, "1.17.0");
    let descriptor = cabin_port::parse_port_str(
        entry.port_toml,
        std::path::Path::new("<builtin:googletest>/port.toml"),
    )
    .expect("embedded googletest port.toml parses");
    assert_eq!(descriptor.name.as_str(), "googletest");
    assert_eq!(descriptor.version.to_string(), "1.17.0");
}

#[test]
fn googletest_overlay_declares_single_umbrella_library_target() {
    let entry =
        cabin_port::builtin::lookup("googletest", &semver::VersionReq::parse(">=0").unwrap())
            .unwrap();
    let overlay = entry.overlay_toml;
    assert!(
        overlay.contains("[target.googletest]"),
        "overlay: {overlay}"
    );
    assert!(overlay.contains("type = \"library\""), "overlay: {overlay}");
    assert!(
        overlay.contains("sources = [\"googletest/src/gtest-all.cc\"]"),
        "overlay: {overlay}"
    );
    assert!(
        overlay.contains("include-dirs = [\"googletest/include\", \"googletest\"]"),
        "overlay: {overlay}"
    );
    // Upstream's hard C++17 floor is declared, not inherited from
    // Cabin's default.
    assert!(
        overlay.contains("cxx-standard = \"c++17\""),
        "overlay: {overlay}"
    );
    // gtest_main.cc stays unbuilt (consumers supply main), and
    // GoogleMock stays out of the port.
    assert!(
        !overlay.contains("\"googletest/src/gtest_main.cc\""),
        "overlay should not build gtest_main: {overlay}"
    );
    assert!(
        !overlay.contains("\"googlemock/"),
        "overlay should not build GoogleMock sources: {overlay}"
    );
}
