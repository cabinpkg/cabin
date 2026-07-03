//! Network-free schema-lock tests for the bundled spdlog
//! foundation port.  The end-to-end build/run path is covered by
//! `cabin_examples.rs::spdlog_usage_builds_and_runs`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_spdlog_matches_published_values() {
    let descriptor =
        load_real_port_and_assert_schema("spdlog", &semver::Version::new(1, 17, 0), "MIT");
    assert_tar_gz_source(&descriptor, "spdlog-1.17.0");
}

#[test]
fn spdlog_is_bundled_and_parses() {
    assert_builtin_port_bundled_and_parses("spdlog", "^1.17", "1.17.0");
}

#[test]
fn spdlog_overlay_declares_header_only_target() {
    let overlay = builtin_overlay("spdlog");
    assert!(overlay.contains("[target.spdlog]"), "overlay: {overlay}");
    assert!(
        overlay.contains("type = \"header-only\""),
        "overlay: {overlay}"
    );
    assert!(
        overlay.contains("include-dirs = [\"include\"]"),
        "overlay: {overlay}"
    );
    // Upstream's default build is C++11; the declaration keeps
    // Cabin's c++17 fallback from rejecting lower-standard consumers.
    assert!(
        overlay.contains("interface-cxx-standard = \"c++11\""),
        "overlay: {overlay}"
    );
    // The opt-in compiled variant must stay unbuilt: header-only
    // targets ship no sources, and SPDLOG_COMPILED_LIB cannot reach
    // consumer TUs.
    assert!(
        !overlay.contains("sources"),
        "overlay should not list sources: {overlay}"
    );
    // std::thread / std::mutex need pthread on Unix.
    assert!(
        overlay.contains("link-libs = [\"pthread\"]"),
        "overlay: {overlay}"
    );
}
