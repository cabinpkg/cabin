//! The registry verifier's archive inspection when the recorded checksum
//! is not the archive's: whatever else the bytes get right, they must
//! never verify.
#![no_main]

use cabin_core::Checksum;
use cabin_registry_verify::Verdict;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|archive: &[u8]| {
    // The digest of other bytes: SHA-256 makes it differ from the
    // archive's own.
    let recorded = Checksum::of_bytes(&[archive, b"\0"].concat());
    let verdict = cabin_fuzz::inspect_as_listed(archive, recorded);
    assert!(
        !matches!(verdict, Ok(Verdict::Verified)),
        "an archive verified against another archive's checksum"
    );
});
