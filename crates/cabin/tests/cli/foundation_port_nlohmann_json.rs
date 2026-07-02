//! Network-free schema-lock tests for the bundled nlohmann_json
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::nlohmann_json_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_nlohmann_json_matches_published_values() {
    let descriptor =
        load_real_port_and_assert_schema("nlohmann_json", &semver::Version::new(3, 12, 0), "MIT");
    assert_tar_gz_source(&descriptor, "json-3.12.0");
}

#[test]
fn nlohmann_json_is_bundled_and_parses() {
    assert_builtin_port_bundled_and_parses("nlohmann_json", "^3.12", "3.12.0");
}

#[test]
fn nlohmann_json_overlay_declares_header_only_target() {
    let overlay = builtin_overlay("nlohmann_json");
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
