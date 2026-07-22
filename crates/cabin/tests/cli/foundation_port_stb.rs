//! Network-free schema-lock tests for the bundled stb
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::stb_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_stb_matches_published_values() {
    // stb has no upstream releases; the version is the pinned
    // commit's date spelled as SemVer.
    let descriptor = load_real_port_and_assert_schema(
        "stb",
        &semver::Version::new(2026, 4, 15),
        "MIT OR Unlicense",
    );
    let url = &descriptor.source.url;
    // The archive is an immutable commit tarball, pinned by
    // full 40-hex commit id (no floating branch or tag).
    assert!(
        url.path()
            .ends_with("/31c1ad37456438565541f4919958214b6e762fb4.tar.gz"),
        "expected a commit-pinned tarball, got {url}"
    );
    assert_tar_gz_source(&descriptor, "stb-31c1ad37456438565541f4919958214b6e762fb4");
}

#[test]
fn stb_is_bundled_and_parses() {
    assert_builtin_port_bundled_and_parses("stb", "^2026", "2026.4.15");
}

#[test]
fn stb_overlay_declares_header_only_target() {
    let overlay = builtin_overlay("stb");
    assert!(overlay.contains("[target.stb]"), "overlay: {overlay}");
    assert!(
        overlay.contains("type = \"header-only\""),
        "overlay: {overlay}"
    );
    assert!(
        overlay.contains("include-dirs = [\".\"]"),
        "overlay: {overlay}"
    );
    // Which implementation TUs exist is the consumer's choice; the
    // port itself compiles nothing.
    assert!(
        !overlay.contains("sources"),
        "overlay should not list sources: {overlay}"
    );
    // The math-heavy stb implementations need libm on Unix; the port
    // propagates it so consumers do not have to know.
    assert!(
        overlay.contains("link-libs = [\"m\"]"),
        "overlay: {overlay}"
    );
}
