//! Network-free schema-lock tests for the bundled spdlog
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::spdlog_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_spdlog_matches_published_values() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during tests");
    let port_toml = PathBuf::from(manifest_dir)
        .join("../cabin-port/ports/spdlog/1.17.0/port.toml")
        .canonicalize()
        .expect("canonicalize ports/spdlog/1.17.0/port.toml");
    let descriptor =
        cabin_port::load_port(&port_toml).expect("ports/spdlog/1.17.0/port.toml should parse");
    assert_eq!(descriptor.name.as_str(), "spdlog");
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
            assert_eq!(strip_prefix.as_deref(), Some("spdlog-1.17.0"));
        }
    }
    assert_eq!(
        descriptor.overlay.relative_path,
        PathBuf::from("cabin.toml")
    );
    assert_eq!(descriptor.metadata.license.as_deref(), Some("MIT"));
}

#[test]
fn spdlog_is_bundled_and_parses() {
    let entry = cabin_port::builtin::lookup("spdlog", &semver::VersionReq::parse("^1.17").unwrap())
        .expect("spdlog should be bundled");
    assert_eq!(entry.name, "spdlog");
    assert_eq!(entry.version, "1.17.0");
    let descriptor = cabin_port::parse_port_str(
        entry.port_toml,
        std::path::Path::new("<builtin:spdlog>/port.toml"),
    )
    .expect("embedded spdlog port.toml parses");
    assert_eq!(descriptor.name.as_str(), "spdlog");
    assert_eq!(descriptor.version.to_string(), "1.17.0");
}

#[test]
fn spdlog_overlay_declares_header_only_target() {
    let entry =
        cabin_port::builtin::lookup("spdlog", &semver::VersionReq::parse(">=0").unwrap()).unwrap();
    let overlay = entry.overlay_toml;
    assert!(overlay.contains("[target.spdlog]"), "overlay: {overlay}");
    assert!(
        overlay.contains("type = \"header-only\""),
        "overlay: {overlay}"
    );
    assert!(
        overlay.contains("include-dirs = [\"include\"]"),
        "overlay: {overlay}"
    );
    // Upstream's default build is C++11; the declaration keeps
    // Cabin's c++17 fallback from rejecting lower-standard consumers.
    assert!(
        overlay.contains("interface-cxx-standard = \"c++11\""),
        "overlay: {overlay}"
    );
    // The opt-in compiled variant must stay unbuilt: header-only
    // targets ship no sources, and SPDLOG_COMPILED_LIB cannot reach
    // consumer TUs.
    assert!(
        !overlay.contains("sources"),
        "overlay should not list sources: {overlay}"
    );
    // std::thread / std::mutex need pthread on Unix.
    assert!(
        overlay.contains("link-libs = [\"pthread\"]"),
        "overlay: {overlay}"
    );
}
