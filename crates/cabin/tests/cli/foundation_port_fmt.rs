//! Network-free schema-lock tests for the bundled {fmt}
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::fmt_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_fmt_matches_published_values() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during tests");
    let port_toml = PathBuf::from(manifest_dir)
        .join("../cabin-port/ports/fmt/12.2.0/port.toml")
        .canonicalize()
        .expect("canonicalize ports/fmt/12.2.0/port.toml");
    let descriptor =
        cabin_port::load_port(&port_toml).expect("ports/fmt/12.2.0/port.toml should parse");
    assert_eq!(descriptor.name.as_str(), "fmt");
    assert_eq!(descriptor.version, semver::Version::new(12, 2, 0));
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
            assert_eq!(strip_prefix.as_deref(), Some("fmt-12.2.0"));
        }
    }
    assert_eq!(
        descriptor.overlay.relative_path,
        PathBuf::from("cabin.toml")
    );
    assert_eq!(descriptor.metadata.license.as_deref(), Some("MIT"));
}

#[test]
fn fmt_is_bundled_and_parses() {
    let entry = cabin_port::builtin::lookup("fmt", &semver::VersionReq::parse("^12").unwrap())
        .expect("fmt should be bundled");
    assert_eq!(entry.name, "fmt");
    assert_eq!(entry.version, "12.2.0");
    let descriptor = cabin_port::parse_port_str(
        entry.port_toml,
        std::path::Path::new("<builtin:fmt>/port.toml"),
    )
    .expect("embedded fmt port.toml parses");
    assert_eq!(descriptor.name.as_str(), "fmt");
    assert_eq!(descriptor.version.to_string(), "12.2.0");
}

#[test]
fn fmt_overlay_declares_single_cxx_library_target() {
    let entry =
        cabin_port::builtin::lookup("fmt", &semver::VersionReq::parse(">=0").unwrap()).unwrap();
    let overlay = entry.overlay_toml;
    assert!(overlay.contains("[target.fmt]"), "overlay: {overlay}");
    assert!(overlay.contains("type = \"library\""), "overlay: {overlay}");
    assert!(
        overlay.contains("sources = [\"src/format.cc\", \"src/os.cc\"]"),
        "overlay: {overlay}"
    );
    // Upstream's public interface requirement is C++11; without the
    // explicit declaration Cabin would impose the c++17 default on
    // consumers.
    assert!(
        overlay.contains("interface-cxx-standard = \"c++11\""),
        "overlay: {overlay}"
    );
    // The C++20 module unit and the optional C API are intentionally
    // not built.
    assert!(
        !overlay.contains("\"src/fmt.cc\""),
        "overlay should not build the C++20 module unit: {overlay}"
    );
    assert!(
        !overlay.contains("\"src/fmt-c.cc\""),
        "overlay should not build the optional C API: {overlay}"
    );
}
