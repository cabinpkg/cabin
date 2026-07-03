//! Network-free schema-lock tests for the bundled GoogleTest
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::googletest_usage_runs_tests`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_googletest_matches_published_values() {
    let descriptor = load_real_port_and_assert_schema(
        "googletest",
        &semver::Version::new(1, 17, 0),
        "BSD-3-Clause",
    );
    assert_tar_gz_source(&descriptor, "googletest-1.17.0");
}

#[test]
fn googletest_is_bundled_and_parses() {
    assert_builtin_port_bundled_and_parses("googletest", "^1.17", "1.17.0");
}

#[test]
fn googletest_overlay_declares_single_umbrella_library_target() {
    let overlay = builtin_overlay("googletest");
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
