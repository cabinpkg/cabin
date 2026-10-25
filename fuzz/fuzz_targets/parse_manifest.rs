//! `cabin.toml` parsing, then the publishability check and canonical
//! metadata derivation that publish and the registry verifier run on a
//! parsed package.
#![no_main]

use cabin_core::Checksum;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    let Ok(parsed) = cabin_manifest::parse_manifest_str(text) else {
        return;
    };
    if let Some(package) = parsed.package
        && cabin_package::validate::validate_publishable(&package).is_ok()
    {
        let metadata =
            cabin_package::metadata::canonical_metadata(&package, &Checksum::of_bytes(b""));
        serde_json::to_vec(&metadata).expect("canonical metadata serializes");
    }
});
