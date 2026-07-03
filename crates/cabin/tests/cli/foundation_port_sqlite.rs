//! Network-free schema-lock tests for the bundled sqlite3
//! foundation port.  The end-to-end build/run path (including the
//! `single-threaded` feature) is covered by
//! `cabin_examples.rs::sqlite3_usage_builds_and_runs` and
//! `sqlite3_single_threaded_feature_disables_threadsafety`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_sqlite3_matches_published_values() {
    let descriptor =
        load_real_port_and_assert_schema("sqlite3", &semver::Version::new(3, 53, 2), "blessing");
    assert_tar_gz_source(&descriptor, "sqlite-autoconf-3530200");
}

#[test]
fn sqlite3_is_bundled_and_parses() {
    assert_builtin_port_bundled_and_parses("sqlite3", "^3", "3.53.2");
}

#[test]
fn sqlite3_overlay_declares_amalgamation_features_and_link_libs() {
    let overlay = builtin_overlay("sqlite3");
    // Single amalgamation TU, no CLI shell.
    assert!(
        overlay.contains("sources = [\"sqlite3.c\"]"),
        "overlay: {overlay}"
    );
    assert!(
        !overlay.contains("\"shell.c\""),
        "overlay must not build the CLI shell: {overlay}"
    );
    // Threadsafe-by-default with an opt-in single-threaded feature.
    assert!(overlay.contains("single-threaded"), "overlay: {overlay}");
    assert!(
        overlay.contains("SQLITE_THREADSAFE=0"),
        "overlay: {overlay}"
    );
    // Propagating system link libraries, gated to Unix.
    assert!(
        overlay.contains("link-libs = [\"pthread\", \"dl\", \"m\"]"),
        "overlay: {overlay}"
    );
    assert!(
        overlay.contains("cfg(family = \"unix\")"),
        "overlay: {overlay}"
    );
}

/// The overlay must parse as a real manifest whose `single-threaded`
/// feature maps, via a `cfg(feature = ...)` profile layer, to the
/// `SQLITE_THREADSAFE=0` define - the mechanism the end-to-end test
/// exercises.  Network-free: parses the embedded overlay text only.
#[test]
fn sqlite3_overlay_feature_layer_is_well_formed() {
    let manifest = cabin_manifest::parse_manifest_str(builtin_overlay("sqlite3"))
        .expect("overlay parses as a manifest");
    let package = manifest.package.expect("[package]");
    assert!(
        package.features.features.contains_key("single-threaded"),
        "overlay must declare the single-threaded feature"
    );
    let layer = package
        .build
        .conditional
        .iter()
        .find(|c| c.condition.references_feature())
        .expect("a cfg(feature = ...) profile layer");
    assert!(
        layer
            .flags
            .defines
            .iter()
            .any(|d| d == "SQLITE_THREADSAFE=0"),
        "the feature layer must define SQLITE_THREADSAFE=0; got {:?}",
        layer.flags.defines
    );
}
