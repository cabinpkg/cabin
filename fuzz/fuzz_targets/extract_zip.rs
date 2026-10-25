//! The upstream materializer's zip extraction over arbitrary bytes:
//! whatever it accepts or refuses, it writes inside its destination,
//! and only directories and singly linked regular files.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|archive: &[u8]| {
    cabin_fuzz::assert_extraction_contained(archive, cabin_artifact::safe_extract_zip);
});
