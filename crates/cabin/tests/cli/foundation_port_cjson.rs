//! Network-free schema-lock tests for the bundled cJSON
//! foundation port. The end-to-end build/run path is covered by
//! `cabin_examples.rs::cjson_usage_builds_and_runs`; these tests
//! pin the on-disk recipe against the typed parser so an
//! accidental edit (wrong checksum, renamed source, dropped
//! field) is caught without touching the network.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_cjson_matches_published_values() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during tests");
    let port_toml = PathBuf::from(manifest_dir)
        .join("../cabin-port/ports/cJSON/1.7.18/port.toml")
        .canonicalize()
        .expect("canonicalize ports/cJSON/1.7.18/port.toml");
    let descriptor =
        cabin_port::load_port(&port_toml).expect("ports/cJSON/1.7.18/port.toml should parse");
    assert_eq!(descriptor.name.as_str(), "cJSON");
    assert_eq!(descriptor.version, semver::Version::new(1, 7, 18));
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
            assert_eq!(strip_prefix.as_deref(), Some("cJSON-1.7.18"));
        }
    }
    assert_eq!(
        descriptor.overlay.relative_path,
        PathBuf::from("cabin.toml")
    );
    assert_eq!(descriptor.metadata.license.as_deref(), Some("MIT"));
}

#[test]
fn cjson_is_bundled_and_parses() {
    let entry = cabin_port::builtin::lookup("cJSON", &semver::VersionReq::parse("^1.7").unwrap())
        .expect("cJSON should be bundled");
    assert_eq!(entry.name, "cJSON");
    assert_eq!(entry.version, "1.7.18");
    let descriptor = cabin_port::parse_port_str(
        entry.port_toml,
        std::path::Path::new("<builtin:cJSON>/port.toml"),
    )
    .expect("embedded cJSON port.toml parses");
    assert_eq!(descriptor.name.as_str(), "cJSON");
    assert_eq!(descriptor.version.to_string(), "1.7.18");
}

#[test]
fn cjson_overlay_declares_single_library_target() {
    let entry =
        cabin_port::builtin::lookup("cJSON", &semver::VersionReq::parse(">=0").unwrap()).unwrap();
    let overlay = entry.overlay_toml;
    assert!(overlay.contains("[target.cJSON]"), "overlay: {overlay}");
    assert!(overlay.contains("type = \"library\""), "overlay: {overlay}");
    assert!(
        overlay.contains("sources = [\"cJSON.c\"]"),
        "overlay: {overlay}"
    );
    // The optional utilities TU is intentionally excluded from the
    // built source list (a backtick-quoted mention in the recipe
    // comment is fine; a double-quoted source entry is not).
    assert!(
        !overlay.contains("\"cJSON_Utils.c\""),
        "overlay should not build the optional utilities TU: {overlay}"
    );
}
