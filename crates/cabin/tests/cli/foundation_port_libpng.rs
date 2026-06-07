//! Network-free schema-lock tests for the bundled libpng
//! foundation port. The end-to-end build/run path (the real
//! download, the transitive libpng -> zlib edge, the prebuilt
//! pnglibconf.h placement, and the cold/warm/offline/frozen cache
//! lifecycle) is covered by
//! `cabin_examples.rs::libpng_usage_cache_lifecycle_builds_and_runs`.

use super::*;
use cabin_core::{DependencySource, PortDepSource};

#[test]
fn port_toml_schema_for_real_ports_libpng_matches_published_values() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during tests");
    let port_toml = PathBuf::from(manifest_dir)
        .join("../cabin-port/ports/libpng/1.6.50/port.toml")
        .canonicalize()
        .expect("canonicalize ports/libpng/1.6.50/port.toml");
    let descriptor =
        cabin_port::load_port(&port_toml).expect("ports/libpng/1.6.50/port.toml should parse");
    assert_eq!(descriptor.name.as_str(), "libpng");
    assert_eq!(descriptor.version, semver::Version::new(1, 6, 50));
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
            assert_eq!(strip_prefix.as_deref(), Some("libpng-1.6.50"));
        }
    }
    assert_eq!(
        descriptor.overlay.relative_path,
        PathBuf::from("cabin.toml")
    );
    assert_eq!(descriptor.metadata.license.as_deref(), Some("libpng-2.0"));

    // libpng ships its build config as a prebuilt header; the port
    // declares a single [[copy]] step to place it under its build-time
    // name. This is the only port that exercises the copy mechanism.
    assert_eq!(descriptor.copies.len(), 1, "expected one [[copy]] step");
    assert_eq!(
        descriptor.copies[0].from.as_str(),
        "scripts/pnglibconf.h.prebuilt"
    );
    assert_eq!(descriptor.copies[0].to.as_str(), "pnglibconf.h");
}

#[test]
fn libpng_is_bundled_and_parses() {
    let entry = cabin_port::builtin::lookup("libpng", &semver::VersionReq::parse("^1.6").unwrap())
        .expect("libpng should be bundled");
    assert_eq!(entry.name, "libpng");
    assert_eq!(entry.version, "1.6.50");
    let descriptor = cabin_port::parse_port_str(
        entry.port_toml,
        std::path::Path::new("<builtin:libpng>/port.toml"),
    )
    .expect("embedded libpng port.toml parses");
    assert_eq!(descriptor.name.as_str(), "libpng");
}

#[test]
fn libpng_overlay_declares_zlib_edge_simd_off_and_link_libs() {
    let entry =
        cabin_port::builtin::lookup("libpng", &semver::VersionReq::parse(">=0").unwrap()).unwrap();
    let overlay = entry.overlay_toml;
    // The 15-source portable set, with the CLI/test units excluded.
    assert!(overlay.contains("\"pngread.c\""), "overlay: {overlay}");
    assert!(
        !overlay.contains("\"pngtest.c\""),
        "overlay must not build pngtest.c: {overlay}"
    );
    // SIMD optimizations compiled out so the portable set is
    // self-contained on every architecture.
    assert!(
        overlay.contains("PNG_ARM_NEON_OPT=0") && overlay.contains("PNG_INTEL_SSE_OPT=0"),
        "overlay: {overlay}"
    );
    // libm declared as a propagating link-lib, gated to Unix.
    assert!(
        overlay.contains("link-libs = [\"m\"]") && overlay.contains("cfg(family = \"unix\")"),
        "overlay: {overlay}"
    );
}

/// The overlay must parse as a real manifest whose `zlib` dependency
/// is a bundled (`port = true`) port edge — the transitive edge port
/// discovery follows and the end-to-end test links against.
/// Network-free: parses the embedded overlay text only.
#[test]
fn libpng_overlay_depends_on_bundled_zlib() {
    let entry =
        cabin_port::builtin::lookup("libpng", &semver::VersionReq::parse(">=0").unwrap()).unwrap();
    let manifest = cabin_manifest::parse_manifest_str(entry.overlay_toml)
        .expect("overlay parses as a manifest");
    let package = manifest.package.expect("[package]");
    let zlib = package
        .dependencies
        .iter()
        .find(|d| d.name.as_str() == "zlib")
        .expect("overlay must depend on zlib");
    match &zlib.source {
        DependencySource::Port(PortDepSource::Builtin { name, version_req }) => {
            assert_eq!(name.as_str(), "zlib");
            assert_eq!(version_req.to_string(), "^1.3");
        }
        other => panic!("expected a bundled zlib port edge, got {other:?}"),
    }
}
