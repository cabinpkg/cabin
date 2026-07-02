//! Network-free schema-lock tests for the bundled CLI11
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::cli11_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_cli11_matches_published_values() {
    let descriptor =
        load_real_port_and_assert_schema("CLI11", &semver::Version::new(2, 6, 2), "BSD-3-Clause");
    assert_tar_gz_source(&descriptor, "CLI11-2.6.2");
}

#[test]
fn cli11_is_bundled_and_parses() {
    assert_builtin_port_bundled_and_parses("CLI11", "^2.6", "2.6.2");
}

#[test]
fn cli11_overlay_declares_header_only_target() {
    let overlay = builtin_overlay("CLI11");
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
