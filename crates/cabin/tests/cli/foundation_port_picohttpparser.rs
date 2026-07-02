//! Network-free schema-lock tests for the bundled picohttpparser
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::picohttpparser_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_picohttpparser_matches_published_values() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during tests");
    let port_toml = PathBuf::from(manifest_dir)
        .join("../cabin-port/ports/picohttpparser/2026.4.6/port.toml")
        .canonicalize()
        .expect("canonicalize ports/picohttpparser/2026.4.6/port.toml");
    let descriptor = cabin_port::load_port(&port_toml)
        .expect("ports/picohttpparser/2026.4.6/port.toml should parse");
    assert_eq!(descriptor.name.as_str(), "picohttpparser");
    // picohttpparser has no upstream releases; the version is the
    // pinned commit's date spelled as SemVer.
    assert_eq!(descriptor.version, semver::Version::new(2026, 4, 6));
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
                    .ends_with("/f4d94b48b31e0abae029ebeafcfd9ca0680ede58.tar.gz"),
                "expected a commit-pinned tarball, got {url}"
            );
            assert_eq!(url.scheme(), "https");
            assert_eq!(sha256.to_hex().len(), 64);
            assert_eq!(
                strip_prefix.as_deref(),
                Some("picohttpparser-f4d94b48b31e0abae029ebeafcfd9ca0680ede58")
            );
        }
    }
    assert_eq!(
        descriptor.overlay.relative_path,
        PathBuf::from("cabin.toml")
    );
    assert_eq!(
        descriptor.metadata.license.as_deref(),
        Some("MIT OR Artistic-1.0-Perl")
    );
}

#[test]
fn picohttpparser_is_bundled_and_parses() {
    let entry = cabin_port::builtin::lookup(
        "picohttpparser",
        &semver::VersionReq::parse("^2026").unwrap(),
    )
    .expect("picohttpparser should be bundled");
    assert_eq!(entry.name, "picohttpparser");
    assert_eq!(entry.version, "2026.4.6");
    let descriptor = cabin_port::parse_port_str(
        entry.port_toml,
        std::path::Path::new("<builtin:picohttpparser>/port.toml"),
    )
    .expect("embedded picohttpparser port.toml parses");
    assert_eq!(descriptor.name.as_str(), "picohttpparser");
    assert_eq!(descriptor.version.to_string(), "2026.4.6");
}

#[test]
fn picohttpparser_overlay_declares_single_c_library_target() {
    let entry =
        cabin_port::builtin::lookup("picohttpparser", &semver::VersionReq::parse(">=0").unwrap())
            .unwrap();
    let overlay = entry.overlay_toml;
    assert!(
        overlay.contains("[target.picohttpparser]"),
        "overlay: {overlay}"
    );
    assert!(overlay.contains("type = \"library\""), "overlay: {overlay}");
    assert!(
        overlay.contains("sources = [\"picohttpparser.c\"]"),
        "overlay: {overlay}"
    );
    assert!(
        overlay.contains("include-dirs = [\".\"]"),
        "overlay: {overlay}"
    );
    // The upstream test harness stays unbuilt.
    assert!(
        !overlay.contains("\"test.c\"") && !overlay.contains("\"bench.c\""),
        "overlay should not build the upstream test harness: {overlay}"
    );
}
