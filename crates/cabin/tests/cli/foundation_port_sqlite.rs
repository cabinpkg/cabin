//! Network-free schema-lock tests for the bundled sqlite3
//! foundation port.  The end-to-end build/run path (including the
//! `single-threaded` feature) is covered by
//! `cabin_examples.rs::sqlite3_usage_builds_and_runs` and
//! `sqlite3_single_threaded_feature_disables_threadsafety`.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_sqlite3_matches_published_values() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during tests");
    let port_toml = PathBuf::from(manifest_dir)
        .join("../cabin-port/ports/sqlite3/3.53.2/port.toml")
        .canonicalize()
        .expect("canonicalize ports/sqlite3/3.53.2/port.toml");
    let descriptor =
        cabin_port::load_port(&port_toml).expect("ports/sqlite3/3.53.2/port.toml should parse");
    assert_eq!(descriptor.name.as_str(), "sqlite3");
    assert_eq!(descriptor.version, semver::Version::new(3, 53, 2));
    match &descriptor.source {
        cabin_port::PortSource::Archive {
            url,
            sha256,
            strip_prefix,
        } => {
            assert!(
                url.as_str().ends_with(".tar.gz"),
                "expected a .tar.gz URL, got {url}"
            );
            assert_eq!(url.scheme(), "https");
            assert_eq!(sha256.to_hex().len(), 64);
            assert_eq!(strip_prefix.as_deref(), Some("sqlite-autoconf-3530200"));
        }
    }
    assert_eq!(
        descriptor.overlay.relative_path,
        PathBuf::from("cabin.toml")
    );
    assert_eq!(descriptor.metadata.license.as_deref(), Some("blessing"));
}

#[test]
fn sqlite3_is_bundled_and_parses() {
    let entry = cabin_port::builtin::lookup("sqlite3", &semver::VersionReq::parse("^3").unwrap())
        .expect("sqlite3 should be bundled");
    assert_eq!(entry.name, "sqlite3");
    assert_eq!(entry.version, "3.53.2");
    let descriptor = cabin_port::parse_port_str(
        entry.port_toml,
        std::path::Path::new("<builtin:sqlite3>/port.toml"),
    )
    .expect("embedded sqlite3 port.toml parses");
    assert_eq!(descriptor.name.as_str(), "sqlite3");
}

#[test]
fn sqlite3_overlay_declares_amalgamation_features_and_link_libs() {
    let entry =
        cabin_port::builtin::lookup("sqlite3", &semver::VersionReq::parse(">=0").unwrap()).unwrap();
    let overlay = entry.overlay_toml;
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
    let entry =
        cabin_port::builtin::lookup("sqlite3", &semver::VersionReq::parse(">=0").unwrap()).unwrap();
    let manifest = cabin_manifest::parse_manifest_str(entry.overlay_toml)
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
