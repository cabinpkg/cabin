//! Schema lock for the zlib recipe still carried in
//! `crates/cabin-port/ports/`.  zlib is the last port that has not
//! been migrated to a provenance-bearing package, so its `port.toml`
//! is still the publisher's input and still has to parse back to the
//! published values.

use super::*;

#[test]
fn port_toml_schema_for_real_ports_zlib_matches_published_values() {
    // Regression test that locks the on-disk port.toml in
    // crates/cabin-port/ports/zlib/1.3.1/ against the typed parser.
    // Catches accidental edits without requiring any network.
    let descriptor =
        load_real_port_and_assert_schema("zlib", &semver::Version::new(1, 3, 1), "Zlib");
    assert_tar_gz_source(&descriptor, "zlib-1.3.1");
}
