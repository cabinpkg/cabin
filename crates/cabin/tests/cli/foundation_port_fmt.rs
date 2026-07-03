//! Network-free schema-lock tests for the bundled {fmt}
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::fmt_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_fmt_matches_published_values() {
    let descriptor =
        load_real_port_and_assert_schema("fmt", &semver::Version::new(12, 2, 0), "MIT");
    assert_tar_gz_source(&descriptor, "fmt-12.2.0");
}

#[test]
fn fmt_is_bundled_and_parses() {
    assert_builtin_port_bundled_and_parses("fmt", "^12", "12.2.0");
}

#[test]
fn fmt_overlay_declares_single_cxx_library_target() {
    let overlay = builtin_overlay("fmt");
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
