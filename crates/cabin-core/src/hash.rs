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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// SHA-256 of the empty input, a fixed reference value.
    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn hex_digest_encodes_lower_case_hex() {
        assert_eq!(hex_digest(&[]), "");
        assert_eq!(hex_digest(&[0x00]), "00");
        assert_eq!(hex_digest(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
    }

    #[test]
    fn hash_reader_matches_known_sha256_vectors() {
        assert_eq!(hash_reader(Cursor::new(b"")).unwrap(), EMPTY_SHA256);
        assert_eq!(
            hash_reader(Cursor::new(b"abc")).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        );
    }

    #[test]
    fn hash_reader_streams_input_larger_than_one_chunk() {
        // Larger than the 64 KiB streaming buffer, and not a multiple
        // of it, so the loop runs several times with a short tail.
        let data = vec![0xabu8; 3 * 64 * 1024 + 7];
        let expected = hex_digest(&Sha256::digest(&data));
        assert_eq!(hash_reader(Cursor::new(&data)).unwrap(), expected);
    }

    #[test]
    fn hash_reader_propagates_read_errors() {
        struct FailingReader;
        impl Read for FailingReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("read failed"))
            }
        }
        let err = hash_reader(FailingReader).unwrap_err();
        assert_eq!(err.to_string(), "read failed");
    }

    #[test]
    fn hash_copy_writes_bytes_verbatim_and_returns_matching_digest() {
        let data = vec![0x5au8; 64 * 1024 + 3];
        let mut copied = Vec::new();
        let digest = hash_copy(Cursor::new(&data), &mut copied).unwrap();
        assert_eq!(copied, data);
        assert_eq!(digest, hash_reader(Cursor::new(&data)).unwrap());
    }

    #[test]
    fn hash_copy_of_empty_input_writes_nothing() {
        let mut copied = Vec::new();
        let digest = hash_copy(Cursor::new(b""), &mut copied).unwrap();
        assert!(copied.is_empty());
        assert_eq!(digest, EMPTY_SHA256);
    }

    #[test]
    fn hash_copy_propagates_write_errors() {
        struct FailingWriter;
        impl Write for FailingWriter {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("write failed"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let err = hash_copy(Cursor::new(b"payload"), FailingWriter).unwrap_err();
        assert_eq!(err.to_string(), "write failed");
    }
}
