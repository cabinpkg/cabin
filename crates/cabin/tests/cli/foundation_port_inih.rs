//! Network-free schema-lock tests for the bundled inih
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::inih_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_inih_matches_published_values() {
    // Upstream's r62 tag spelled as SemVer.
    let descriptor =
        load_real_port_and_assert_schema("inih", &semver::Version::new(62, 0, 0), "BSD-3-Clause");
    match &descriptor.source {
        cabin_port::PortSource::Archive {
            url, strip_prefix, ..
        } => {
            assert!(
                url.as_str().ends_with("/r62.tar.gz"),
                "expected the r62 tag tarball, got {url}"
            );
            assert_eq!(strip_prefix.as_deref(), Some("inih-r62"));
        }
    }
}

#[test]
fn inih_is_bundled_and_parses() {
    assert_builtin_port_bundled_and_parses("inih", "^62", "62.0.0");
}

#[test]
fn inih_overlay_declares_single_c_library_target() {
    let overlay = builtin_overlay("inih");
    assert!(overlay.contains("[target.inih]"), "overlay: {overlay}");
    assert!(overlay.contains("type = \"library\""), "overlay: {overlay}");
    assert!(
        overlay.contains("sources = [\"ini.c\"]"),
        "overlay: {overlay}"
    );
    assert!(
        overlay.contains("include-dirs = [\".\"]"),
        "overlay: {overlay}"
    );
    // The optional C++ INIReader stays unbuilt.
    assert!(
        !overlay.contains("\"cpp/INIReader.cpp\""),
        "overlay should not build the optional C++ API: {overlay}"
    );
}
