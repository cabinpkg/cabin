//! Shared hash-encoding helpers used across the workspace.

use std::fmt::Write as _;
use std::io::{Read, Write};

use sha2::{Digest, Sha256};

/// Lower-case hex encoding of a digest (or any byte slice).
pub fn hex_digest(digest: &[u8]) -> String {
    let mut hex = String::with_capacity(2 * digest.len());
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Stream `reader` through SHA-256 in 64 KiB chunks and return the
/// lower-case hex digest.  This is the shared primitive behind every
/// Cabin file / archive integrity check; callers own opening the
/// file and mapping any [`std::io::Error`] into their crate's own
/// error type (and re-attaching path context).
///
/// # Errors
/// Returns the [`std::io::Error`] propagated from reading `reader`.
pub fn hash_reader<R: Read>(mut reader: R) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_digest(&hasher.finalize()))
}

/// Stream `reader` into `writer` in 64 KiB chunks, hashing the bytes
/// with SHA-256 as they pass through, and return the lower-case hex
/// digest.  This is the shared primitive behind Cabin's
/// stream-to-temp-and-verify archive staging (download, local copy,
/// vendor): the bytes are written exactly once while the digest is
/// computed in the same pass, so a torn copy surfaces as a checksum
/// mismatch rather than a silently bad archive.
///
/// Like [`hash_reader`], callers own opening and creating the handles
/// and mapping any [`std::io::Error`] into their crate's own error
/// type with path context.
///
/// # Errors
/// Returns the first [`std::io::Error`] encountered while reading
/// `reader` or writing `writer`.
pub fn hash_copy<R: Read, W: Write>(mut reader: R, mut writer: W) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        writer.write_all(&buf[..n])?;
    }
    Ok(hex_digest(&hasher.finalize()))
}
