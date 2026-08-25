//! Byte-level helpers for the smoke run: the publish body framing,
//! the packaging-revision id, the hash retargeting, and the zip
//! tamperer.

use sha2::Digest as _;

/// `frame`: the publish body,
/// `[u32 LE metadata_len][metadata][u32 LE archive_len][archive]` (the
/// shell emitted each length through nested octal `printf`s because
/// the bytes are usually NULs).
#[must_use]
pub fn frame(metadata: &[u8], archive: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + metadata.len() + archive.len());
    out.extend_from_slice(
        &u32::try_from(metadata.len())
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    out.extend_from_slice(metadata);
    out.extend_from_slice(
        &u32::try_from(archive.len())
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    out.extend_from_slice(archive);
    out
}

/// `revision_of`: the packaging revision id, the leading 16 hex of the
/// archive's SHA-256.  The immutable unit is (scope, name, version,
/// revision), and both the artifact route and the canonical source
/// path spell the id out.
#[must_use]
pub fn revision_of(archive: &[u8]) -> String {
    cabin_core::Checksum::of_bytes(archive)
        .revision_id()
        .to_owned()
}

/// The full SHA-256 of a blob, lowercase hex - what the shell spelled
/// `shasum -a 256 | cut -d' ' -f1`.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    cabin_core::hash::hex_digest(&sha2::Sha256::digest(data))
}

/// `retarget_hash`: rewrite a canonical metadata document onto an
/// archive with `new` as its digest.  Two TEXTUAL global
/// substitutions over the raw bytes, never a JSON edit (a re-serialize
/// would change the document's own bytes, hence its sha256, hence the
/// packaging revision and every derived artifact path): `checksum`
/// carries all 64 hex, `source.path` only the leading 16.  The full
/// digest goes first - the prefix rewrite would otherwise corrupt it.
#[must_use]
pub fn retarget_hash(document: &[u8], old: &str, new: &str) -> Vec<u8> {
    let full = replace_all(document, old.as_bytes(), new.as_bytes());
    replace_all(&full, &old.as_bytes()[..16], &new.as_bytes()[..16])
}

/// `sed s/old/new/g` over raw bytes.  The needles here are hex
/// strings, so `sed`'s per-line application and a whole-buffer
/// replacement cannot differ.
#[must_use]
pub fn replace_all(haystack: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return haystack.to_vec();
    }
    let mut out = Vec::with_capacity(haystack.len());
    let mut rest = haystack;
    while let Some(at) = rest
        .windows(needle.len())
        .position(|window| window == needle)
    {
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(replacement);
        rest = &rest[at + needle.len()..];
    }
    out.extend_from_slice(rest);
    out
}

/// `tamper_zip`: one interior byte flipped so the bytes (and thus the
/// checksum) change while the container stays well formed - the
/// worker's fixed-offset sanity check reads only the four-byte
/// local-header prefix and the trailing EOCD, so touching the middle
/// keeps the request on the immutability/verification path.  A
/// distinct `seed` yields distinct bytes; a seed whose low byte is
/// zero flips bit 0, as the Python's `or 1` did.
#[must_use]
pub fn tamper_zip(source: &[u8], seed: u32) -> Vec<u8> {
    let mut data = source.to_vec();
    let mask = match u8::try_from(seed & 0xFF) {
        Ok(0) | Err(_) => 1,
        Ok(byte) => byte,
    };
    let middle = data.len() / 2;
    data[middle] ^= mask;
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_is_length_prefixed_little_endian() {
        let framed = frame(b"meta", b"archive!");
        assert_eq!(&framed[0..4], &[4, 0, 0, 0]);
        assert_eq!(&framed[4..8], b"meta");
        assert_eq!(&framed[8..12], &[8, 0, 0, 0]);
        assert_eq!(&framed[12..], b"archive!");
    }

    /// The full digest is rewritten before its prefix: the reverse
    /// order would corrupt every full occurrence.
    #[test]
    fn retargeting_rewrites_full_then_prefix() {
        let old = "aaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbccccccccccccccccdddddddddddddddd";
        let new = "1111111111111111222222222222222233333333333333334444444444444444";
        let doc = format!("checksum={old} path={}", &old[..16]);
        let out = retarget_hash(doc.as_bytes(), old, new);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("checksum={new} path={}", &new[..16])
        );
    }

    #[test]
    fn tampering_flips_one_interior_byte() {
        let zip = vec![0u8; 10];
        let tampered = tamper_zip(&zip, 7);
        assert_eq!(tampered[5], 7);
        assert_eq!(tampered.iter().filter(|byte| **byte != 0).count(), 1);
        // A zero low byte still flips something.
        assert_eq!(tamper_zip(&zip, 0x100)[5], 1);
    }
}
