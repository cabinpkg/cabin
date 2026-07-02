//! Network-free schema-lock tests for the bundled Catch2
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::catch2_usage_runs_tests`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_catch2_matches_published_values() {
    let descriptor =
        load_real_port_and_assert_schema("catch2", &semver::Version::new(3, 15, 1), "BSL-1.0");
    assert_tar_gz_source(&descriptor, "Catch2-3.15.1");
}

#[test]
fn catch2_is_bundled_and_parses() {
    assert_builtin_port_bundled_and_parses("catch2", "^3.15", "3.15.1");
}

#[test]
fn catch2_overlay_declares_amalgamated_library_with_custom_main_feature() {
    let overlay = builtin_overlay("catch2");
    assert!(overlay.contains("[target.catch2]"), "overlay: {overlay}");
    assert!(overlay.contains("type = \"library\""), "overlay: {overlay}");
    assert!(
        overlay.contains("sources = [\"extras/catch_amalgamated.cpp\"]"),
        "overlay: {overlay}"
    );
    assert!(
        overlay.contains("include-dirs = [\"extras\"]"),
        "overlay: {overlay}"
    );
    // Upstream's public interface requirement is C++14.
    assert!(
        overlay.contains("interface-cxx-standard = \"c++14\""),
        "overlay: {overlay}"
    );
    // The default build ships Catch2's main(); the opt-in
    // `custom-main` feature compiles it out for consumers that
    // define their own.
    assert!(overlay.contains("custom-main = []"), "overlay: {overlay}");
    assert!(
        overlay.contains("defines = [\"CATCH_AMALGAMATED_CUSTOM_MAIN\"]"),
        "overlay: {overlay}"
    );
}
