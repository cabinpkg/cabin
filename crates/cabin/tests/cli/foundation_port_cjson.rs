//! Network-free schema-lock tests for the bundled cJSON
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::cjson_usage_builds_and_runs`; these tests
//! pin the on-disk recipe against the typed parser so an
//! accidental edit (wrong checksum, renamed source, dropped
//! field) is caught without touching the network.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_cjson_matches_published_values() {
    let descriptor =
        load_real_port_and_assert_schema("cJSON", &semver::Version::new(1, 7, 18), "MIT");
    assert_tar_gz_source(&descriptor, "cJSON-1.7.18");
}

#[test]
fn cjson_is_bundled_and_parses() {
    assert_builtin_port_bundled_and_parses("cJSON", "^1.7", "1.7.18");
}

#[test]
fn cjson_overlay_declares_single_library_target() {
    let overlay = builtin_overlay("cJSON");
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
