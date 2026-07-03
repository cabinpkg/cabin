//! Network-free schema-lock and hermetic end-to-end tests for the
//! bundled libpng foundation port.  Real upstream downloads are covered
//! only by ignored smoke tests in `cabin_examples.rs` and the
//! scheduled/manual foundation-port smoke workflow.

use super::*;
use cabin_core::{DependencySource, PortDepSource};

const FAKE_ZLIB_H: &str = include_str!(
    "../../../cabin-port/tests/fixtures/fake-libpng-transitive/archives/fake-zlib-1.3.1/zlib.h"
);
const FAKE_ZLIB_C: &str = include_str!(
    "../../../cabin-port/tests/fixtures/fake-libpng-transitive/archives/fake-zlib-1.3.1/zutil.c"
);
const ZLIB_CABIN_TOML: &str = include_str!("../../../cabin-port/ports/zlib/1.3.1/cabin.toml");
const FAKE_PNG_H: &str = include_str!(
    "../../../cabin-port/tests/fixtures/fake-libpng-transitive/archives/fake-libpng-1.6.50/png.h"
);
const FAKE_PNG_C: &str = include_str!(
    "../../../cabin-port/tests/fixtures/fake-libpng-transitive/archives/fake-libpng-1.6.50/png.c"
);
const FAKE_PNGLIBCONF_H: &str = include_str!(
    "../../../cabin-port/tests/fixtures/fake-libpng-transitive/archives/fake-libpng-1.6.50/scripts/pnglibconf.h.prebuilt"
);
const LIBPNG_CABIN_TOML: &str = include_str!("../../../cabin-port/ports/libpng/1.6.50/cabin.toml");

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../cabin-port/tests/fixtures/fake-libpng-transitive")
}

fn copy_fixture_file(tmp: &Path, relative: &str) {
    let source = fixture_root().join(relative);
    let destination = tmp.join(relative);
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).expect("fixture parent dir");
    }
    let body = fs::read_to_string(&source)
        .unwrap_or_else(|err| panic!("read fixture {}: {err}", source.display()));
    fs::write(&destination, body)
        .unwrap_or_else(|err| panic!("write fixture {}: {err}", destination.display()));
}

fn lay_consumer_fixture(tmp: &Path) -> PathBuf {
    copy_fixture_file(tmp, "consumer/cabin.toml");
    copy_fixture_file(tmp, "consumer/src/main.c");
    tmp.join("consumer/cabin.toml")
}

#[test]
fn port_toml_schema_for_real_ports_libpng_matches_published_values() {
    let descriptor =
        load_real_port_and_assert_schema("libpng", &semver::Version::new(1, 6, 50), "libpng-2.0");
    assert_tar_gz_source(&descriptor, "libpng-1.6.50");

    // libpng ships its build config as a prebuilt header; the port
    // declares a single [[copy]] step to place it under its build-time
    // name.  This is the only port that exercises the copy mechanism.
    assert_eq!(descriptor.copies.len(), 1, "expected one [[copy]] step");
    assert_eq!(
        descriptor.copies[0].from.as_str(),
        "scripts/pnglibconf.h.prebuilt"
    );
    assert_eq!(descriptor.copies[0].to.as_str(), "pnglibconf.h");
}

#[test]
fn libpng_is_bundled_and_parses() {
    assert_builtin_port_bundled_and_parses("libpng", "^1.6", "1.6.50");
}

#[test]
fn libpng_overlay_declares_zlib_edge_simd_off_and_link_libs() {
    let overlay = builtin_overlay("libpng");
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
/// is a bundled (`port = true`) port edge - the transitive edge port
/// discovery follows and the end-to-end test links against.
/// Network-free: parses the embedded overlay text only.
#[test]
fn libpng_overlay_depends_on_bundled_zlib() {
    let manifest = cabin_manifest::parse_manifest_str(builtin_overlay("libpng"))
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

#[test]
fn fake_libpng_cache_lifecycle() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let repo = FakePortRepo::new(tmp.path());
    let zlib = repo
        .port("zlib", "1.3.1")
        .archive_prefix("fake-zlib-1.3.1")
        .file("zlib.h", FAKE_ZLIB_H)
        .stub_declared_sources_except(ZLIB_CABIN_TOML, "zlib", &["zutil.c"])
        .file("zutil.c", FAKE_ZLIB_C)
        .overlay_manifest(ZLIB_CABIN_TOML)
        .build();
    let libpng = repo
        .port("libpng", "1.6.50")
        .archive_prefix("fake-libpng-1.6.50")
        .file("png.h", FAKE_PNG_H)
        .stub_declared_sources_except(LIBPNG_CABIN_TOML, "libpng", &["png.c"])
        .file("png.c", FAKE_PNG_C)
        .file("scripts/pnglibconf.h.prebuilt", FAKE_PNGLIBCONF_H)
        .copy("scripts/pnglibconf.h.prebuilt", "pnglibconf.h")
        .depends_on_builtin_or_path_port("zlib", &zlib)
        .overlay_manifest(LIBPNG_CABIN_TOML)
        .build();
    let server = FakeArchiveServer::new()
        .serve(&zlib.archive)
        .serve(&libpng.archive)
        .start();
    let manifest = lay_consumer_fixture(tmp.path());
    let cache_dir = tmp.path().join("cache");

    run_port_cache_lifecycle(&PortCacheLifecycle {
        label: "fake libpng transitive cache lifecycle",
        manifest,
        build_root: tmp.path().join("build"),
        warm_cache: cache_dir,
        pristine_cache: tmp.path().join("cache-pristine"),
        expected_stdout: &[
            "fake libpng version: 10650",
            "fake zlib via libpng: fake-zlib/1.3.1",
        ],
        expected_downloads: &["libpng", "zlib"],
        frozen_port: "libpng",
    });
    assert_eq!(
        server.requests_for(libpng.archive.name()),
        1,
        "cold run should download fake libpng exactly once"
    );
    assert_eq!(
        server.requests_for(zlib.archive.name()),
        1,
        "cold run should download transitive fake zlib exactly once"
    );
    assert_eq!(
        server.total_requests(),
        2,
        "only the cold phase should request the two served archives"
    );
}
