//! Shared hash-encoding helpers used across the workspace.

use std::fmt::Write as _;

/// Lower-case hex encoding of a digest (or any byte slice).
pub fn hex_digest(digest: &[u8]) -> String {
    let mut hex = String::with_capacity(2 * digest.len());
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}
