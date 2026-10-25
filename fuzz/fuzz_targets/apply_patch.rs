//! Declared-patch application over a fixed C and C++ tree: whatever
//! the applier accepts or refuses, the tree stays inside its root as
//! directories and singly linked regular files, and an accepted patch
//! keeps the lexical grammar that stops it doubling as C or C++ source.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|patch: &[u8]| cabin_fuzz::assert_patch_contained(patch));
