//! Network-free schema-lock tests for the bundled picohttpparser
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::picohttpparser_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_picohttpparser_matches_published_values() {
    // picohttpparser has no upstream releases; the version is the
    // pinned commit's date spelled as SemVer.
    let descriptor = load_real_port_and_assert_schema(
        "picohttpparser",
        &semver::Version::new(2026, 4, 6),
        "MIT OR Artistic-1.0-Perl",
    );
    let url = &descriptor.source.url;
    // The archive is an immutable commit tarball, pinned by
    // full 40-hex commit id (no floating branch or tag).
    assert!(
        url.path()
            .ends_with("/f4d94b48b31e0abae029ebeafcfd9ca0680ede58.tar.gz"),
        "expected a commit-pinned tarball, got {url}"
    );
    assert_tar_gz_source(
        &descriptor,
        "picohttpparser-f4d94b48b31e0abae029ebeafcfd9ca0680ede58",
    );
}

#[test]
fn picohttpparser_is_bundled_and_parses() {
    assert_builtin_port_bundled_and_parses("picohttpparser", "^2026", "2026.4.6");
}

#[test]
fn picohttpparser_overlay_declares_single_c_library_target() {
    let overlay = builtin_overlay("picohttpparser");
    assert!(
        overlay.contains("[target.picohttpparser]"),
        "overlay: {overlay}"
    );
    assert!(overlay.contains("type = \"library\""), "overlay: {overlay}");
    assert!(
        overlay.contains("sources = [\"picohttpparser.c\"]"),
        "overlay: {overlay}"
    );
    assert!(
        overlay.contains("include-dirs = [\".\"]"),
        "overlay: {overlay}"
    );
    // The upstream test harness stays unbuilt.
    assert!(
        !overlay.contains("\"test.c\"") && !overlay.contains("\"bench.c\""),
        "overlay should not build the upstream test harness: {overlay}"
    );
}
