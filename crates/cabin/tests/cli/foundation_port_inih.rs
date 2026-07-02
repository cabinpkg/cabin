//! Network-free schema-lock tests for the bundled inih
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::inih_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_inih_matches_published_values() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during tests");
    let port_toml = PathBuf::from(manifest_dir)
        .join("../cabin-port/ports/inih/62.0.0/port.toml")
        .canonicalize()
        .expect("canonicalize ports/inih/62.0.0/port.toml");
    let descriptor =
        cabin_port::load_port(&port_toml).expect("ports/inih/62.0.0/port.toml should parse");
    assert_eq!(descriptor.name.as_str(), "inih");
    // Upstream's r62 tag spelled as SemVer.
    assert_eq!(descriptor.version, semver::Version::new(62, 0, 0));
    match &descriptor.source {
        cabin_port::PortSource::Archive {
            url,
            sha256,
            strip_prefix,
        } => {
            assert!(
                url.as_str().ends_with("/r62.tar.gz"),
                "expected the r62 tag tarball, got {url}"
            );
            assert_eq!(url.scheme(), "https");
            assert_eq!(sha256.to_hex().len(), 64);
            assert_eq!(strip_prefix.as_deref(), Some("inih-r62"));
        }
    }
    assert_eq!(
        descriptor.overlay.relative_path,
        PathBuf::from("cabin.toml")
    );
    assert_eq!(descriptor.metadata.license.as_deref(), Some("BSD-3-Clause"));
}

#[test]
fn inih_is_bundled_and_parses() {
    let entry = cabin_port::builtin::lookup("inih", &semver::VersionReq::parse("^62").unwrap())
        .expect("inih should be bundled");
    assert_eq!(entry.name, "inih");
    assert_eq!(entry.version, "62.0.0");
    let descriptor = cabin_port::parse_port_str(
        entry.port_toml,
        std::path::Path::new("<builtin:inih>/port.toml"),
    )
    .expect("embedded inih port.toml parses");
    assert_eq!(descriptor.name.as_str(), "inih");
    assert_eq!(descriptor.version.to_string(), "62.0.0");
}

#[test]
fn inih_overlay_declares_single_c_library_target() {
    let entry =
        cabin_port::builtin::lookup("inih", &semver::VersionReq::parse(">=0").unwrap()).unwrap();
    let overlay = entry.overlay_toml;
    assert!(overlay.contains("[target.inih]"), "overlay: {overlay}");
    assert!(overlay.contains("type = \"library\""), "overlay: {overlay}");
    assert!(
        overlay.contains("sources = [\"ini.c\"]"),
        "overlay: {overlay}"
    );
    assert!(
        overlay.contains("include-dirs = [\".\"]"),
        "overlay: {overlay}"
    );
    // The optional C++ INIReader stays unbuilt.
    assert!(
        !overlay.contains("\"cpp/INIReader.cpp\""),
        "overlay should not build the optional C++ API: {overlay}"
    );
}
