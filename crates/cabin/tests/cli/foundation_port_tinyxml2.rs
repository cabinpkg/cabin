//! Network-free schema-lock tests for the bundled tinyxml2
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::tinyxml2_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_tinyxml2_matches_published_values() {
    let descriptor =
        load_real_port_and_assert_schema("tinyxml2", &semver::Version::new(11, 0, 0), "Zlib");
    assert_tar_gz_source(&descriptor, "tinyxml2-11.0.0");
}

#[test]
fn tinyxml2_is_bundled_and_parses() {
    assert_builtin_port_bundled_and_parses("tinyxml2", "^11", "11.0.0");
}

#[test]
fn tinyxml2_overlay_declares_single_cxx_library_target() {
    let overlay = builtin_overlay("tinyxml2");
    assert!(overlay.contains("[target.tinyxml2]"), "overlay: {overlay}");
    assert!(overlay.contains("type = \"library\""), "overlay: {overlay}");
    assert!(
        overlay.contains("sources = [\"tinyxml2.cpp\"]"),
        "overlay: {overlay}"
    );
    // The upstream test harness is intentionally not built.
    assert!(
        !overlay.contains("\"xmltest.cpp\""),
        "overlay should not build the upstream test harness: {overlay}"
    );
}
