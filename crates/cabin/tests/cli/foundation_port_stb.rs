//! Network-free schema-lock tests for the bundled stb
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::stb_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_stb_matches_published_values() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during tests");
    let port_toml = PathBuf::from(manifest_dir)
        .join("../cabin-port/ports/stb/2026.4.15/port.toml")
        .canonicalize()
        .expect("canonicalize ports/stb/2026.4.15/port.toml");
    let descriptor =
        cabin_port::load_port(&port_toml).expect("ports/stb/2026.4.15/port.toml should parse");
    assert_eq!(descriptor.name.as_str(), "stb");
    // stb has no upstream releases; the version is the pinned
    // commit's date spelled as SemVer.
    assert_eq!(descriptor.version, semver::Version::new(2026, 4, 15));
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
            // The archive is an immutable commit tarball, pinned by
            // full 40-hex commit id (no floating branch or tag).
            assert!(
                url.path()
                    .ends_with("/31c1ad37456438565541f4919958214b6e762fb4.tar.gz"),
                "expected a commit-pinned tarball, got {url}"
            );
            assert_eq!(url.scheme(), "https");
            assert_eq!(sha256.to_hex().len(), 64);
            assert_eq!(
                strip_prefix.as_deref(),
                Some("stb-31c1ad37456438565541f4919958214b6e762fb4")
            );
        }
    }
    assert_eq!(
        descriptor.overlay.relative_path,
        PathBuf::from("cabin.toml")
    );
    assert_eq!(
        descriptor.metadata.license.as_deref(),
        Some("MIT OR Unlicense")
    );
}

#[test]
fn stb_is_bundled_and_parses() {
    let entry = cabin_port::builtin::lookup("stb", &semver::VersionReq::parse("^2026").unwrap())
        .expect("stb should be bundled");
    assert_eq!(entry.name, "stb");
    assert_eq!(entry.version, "2026.4.15");
    let descriptor = cabin_port::parse_port_str(
        entry.port_toml,
        std::path::Path::new("<builtin:stb>/port.toml"),
    )
    .expect("embedded stb port.toml parses");
    assert_eq!(descriptor.name.as_str(), "stb");
    assert_eq!(descriptor.version.to_string(), "2026.4.15");
}

#[test]
fn stb_overlay_declares_header_only_target() {
    let entry =
        cabin_port::builtin::lookup("stb", &semver::VersionReq::parse(">=0").unwrap()).unwrap();
    let overlay = entry.overlay_toml;
    assert!(overlay.contains("[target.stb]"), "overlay: {overlay}");
    assert!(
        overlay.contains("type = \"header-only\""),
        "overlay: {overlay}"
    );
    assert!(
        overlay.contains("include-dirs = [\".\"]"),
        "overlay: {overlay}"
    );
    // Which implementation TUs exist is the consumer's choice; the
    // port itself compiles nothing.
    assert!(
        !overlay.contains("sources"),
        "overlay should not list sources: {overlay}"
    );
    // The math-heavy stb implementations need libm on Unix; the port
    // propagates it so consumers do not have to know.
    assert!(
        overlay.contains("link-libs = [\"m\"]"),
        "overlay: {overlay}"
    );
}
