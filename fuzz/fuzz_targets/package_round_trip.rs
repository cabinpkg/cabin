//! A package the packager stages from arbitrary files: the registry
//! verifier must verify it, and the client must fetch back its files.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|spec: &[u8]| {
    cabin_fuzz::assert_package_round_trips(spec);
});
