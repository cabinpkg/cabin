//! Network-free schema-lock tests for the bundled xxHash
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::xxhash_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_xxhash_matches_published_values() {
    let descriptor =
        load_real_port_and_assert_schema("xxhash", &semver::Version::new(0, 8, 3), "BSD-2-Clause");
    assert_tar_gz_source(&descriptor, "xxHash-0.8.3");
}

#[test]
fn xxhash_is_bundled_and_parses() {
    assert_builtin_port_bundled_and_parses("xxhash", "^0.8", "0.8.3");
}

#[test]
fn xxhash_overlay_declares_single_library_target() {
    let overlay = builtin_overlay("xxhash");
    assert!(overlay.contains("[target.xxhash]"), "overlay: {overlay}");
    assert!(overlay.contains("type = \"library\""), "overlay: {overlay}");
    assert!(
        overlay.contains("sources = [\"xxhash.c\"]"),
        "overlay: {overlay}"
    );
    // The optional x86 dispatch TU is intentionally not built.
    assert!(
        !overlay.contains("\"xxh_x86dispatch.c\""),
        "overlay should not build the optional x86 dispatch TU: {overlay}"
    );
}
