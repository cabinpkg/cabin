//! Shared assertions for the per-port foundation-port
//! schema-lock tests (`cli/foundation_port_*.rs`).
//!
//! Every foundation port keeps its own `#[test]` functions - one
//! failing port must be identifiable from the test name alone,
//! and port-specific details (URL shape, `strip_prefix`,
//! `[[copy]]` steps, overlay contents) stay asserted in the
//! port's own file.  These helpers own only the boilerplate every
//! port repeats verbatim: loading the on-disk `port.toml`,
//! checking the schema invariants all ports share, and the
//! bundled-recipe lookup.

use std::path::{Path, PathBuf};

/// Load `crates/cabin-port/ports/<name>/<version>/port.toml` and
/// assert the schema fields every foundation port shares: the
/// declared identity matches the directory, the source is an
/// `https` archive with a 64-hex sha256, the overlay manifest is
/// `cabin.toml`, and the published license matches.  Returns the
/// descriptor so the caller can assert its port-specific details.
pub fn load_real_port_and_assert_schema(
    name: &str,
    version: &semver::Version,
    license: &str,
) -> cabin_port::PortDescriptor {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during tests");
    let port_toml = PathBuf::from(manifest_dir)
        .join(format!("../cabin-port/ports/{name}/{version}/port.toml"))
        .canonicalize()
        .unwrap_or_else(|err| panic!("canonicalize ports/{name}/{version}/port.toml: {err}"));
    let descriptor = cabin_port::load_port(&port_toml)
        .unwrap_or_else(|err| panic!("ports/{name}/{version}/port.toml should parse: {err:?}"));
    assert_eq!(descriptor.name.as_str(), name);
    assert_eq!(&descriptor.version, version);
    assert_eq!(descriptor.source.url.scheme(), "https");
    assert_eq!(descriptor.source.sha256.to_hex().len(), 64);
    assert_eq!(
        descriptor.overlay.relative_path,
        PathBuf::from("cabin.toml")
    );
    assert_eq!(descriptor.metadata.license.as_deref(), Some(license));
    descriptor
}

/// Assert the descriptor's archive source has the common
/// release-tarball shape: a `.tar.gz` URL with the given
/// `strip_prefix`.  Ports with a non-standard archive shape
/// (miniz's zip, inih's tag tarball) assert their source inline
/// instead.
pub fn assert_tar_gz_source(descriptor: &cabin_port::PortDescriptor, expected_strip_prefix: &str) {
    let url = &descriptor.source.url;
    assert!(
        url.as_str().ends_with(".tar.gz"),
        "expected a .tar.gz URL, got {url}"
    );
    assert_eq!(
        descriptor.source.strip_prefix.as_deref(),
        Some(expected_strip_prefix)
    );
}

/// Assert `name` is bundled in the builtin port registry at
/// exactly `version` (looked up through `req`) and that the
/// embedded `port.toml` parses back to the same identity.
pub fn assert_builtin_port_bundled_and_parses(name: &str, req: &str, version: &str) {
    let entry = cabin_port::builtin::lookup(name, &semver::VersionReq::parse(req).unwrap())
        .unwrap_or_else(|| panic!("{name} should be bundled"));
    assert_eq!(entry.name, name);
    assert_eq!(entry.version, version);
    let descriptor = cabin_port::parse_port_str(
        entry.port_toml,
        Path::new(&format!("<builtin:{name}>/port.toml")),
    )
    .unwrap_or_else(|err| panic!("embedded {name} port.toml should parse: {err:?}"));
    assert_eq!(descriptor.name.as_str(), name);
    assert_eq!(descriptor.version.to_string(), version);
}

/// The bundled overlay manifest for `name` (any bundled version).
pub fn builtin_overlay(name: &str) -> &'static str {
    cabin_port::builtin::lookup(name, &semver::VersionReq::parse(">=0").unwrap())
        .unwrap_or_else(|| panic!("{name} should be bundled"))
        .overlay_toml
}
