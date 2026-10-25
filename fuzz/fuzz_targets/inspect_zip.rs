//! The registry verifier's archive inspection over arbitrary bytes whose
//! recorded checksum is their own: the deep target. Anything the
//! inspector verifies must hold up in the client's zip reader.
#![no_main]

use cabin_core::Checksum;
use cabin_registry_verify::Verdict;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|archive: &[u8]| {
    if let Ok(Verdict::Verified) =
        cabin_fuzz::inspect_as_listed(archive, Checksum::of_bytes(archive))
    {
        cabin_fuzz::assert_client_agrees(archive);
    }
});
