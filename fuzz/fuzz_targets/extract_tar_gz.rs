//! The upstream materializer's tar.gz extraction over arbitrary tar
//! bytes, gzipped by the harness: whatever it accepts or refuses, it
//! writes inside its destination, and only directories and singly
//! linked regular files.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|tar: &[u8]| {
    cabin_fuzz::assert_extraction_contained(
        &cabin_fuzz::gzip(tar),
        cabin_artifact::safe_extract_tar_gz,
    );
});
