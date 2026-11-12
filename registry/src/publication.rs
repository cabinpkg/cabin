//! Placing a verified revision's archive in ARCHIVES: the pure half of
//! the pass in `glue/publication.rs` that turns a verified revision
//! into a served one (`docs/remote-registry.md`, "Verification
//! lifecycle").
//!
//! A verified revision is served only once its archive is confirmed in
//! ARCHIVES under the revision's own key. The key names one byte
//! sequence - the revision id is the leading hex of the archive's
//! SHA-256 - so placing it again converges on the object already there,
//! and an object that holds anything else is never replaced.

use crate::checksum;

/// The `Cache-Control` R2 stores with a placed archive, and answers
/// with when the bucket's own domain serves it: the key never names
/// other bytes.
pub const CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// The `verification_reason` of a verified revision whose archive could
/// not be placed because its upload no longer holds the verified bytes.
/// The revision was never served and may be published again.
pub const SOURCE_LOST: &str = "publication_source_lost";

/// The ARCHIVES key of a revision, which is also its path under the
/// registry's `archives` area.
#[must_use]
pub fn archive_key(scope: &str, name: &str, version: &str, revision: &str) -> String {
    format!("archives/{scope}/{name}/{scope}-{name}-{version}-{revision}.zip")
}

/// The 32 bytes a canonical checksum names, for R2's integrity check
/// of a put. A non-canonical value (a corrupt row) names none.
#[must_use]
pub fn digest(checksum: &str) -> Option<Vec<u8>> {
    let hex = checksum::hex(checksum);
    if hex.is_empty() {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).ok())
        .collect()
}

/// Whether an object R2 reports with this size and SHA-256 holds the
/// archive a revision row records. R2 records a SHA-256 only for an
/// object put with one, which every put of this Worker is; an object
/// without it was not written here and holds nothing known.
#[must_use]
pub fn holds_archive(size: u64, sha256: Option<&[u8]>, archive_size: u64, checksum: &str) -> bool {
    size == archive_size && sha256.is_some() && sha256 == digest(checksum).as_deref()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHECKSUM: &str =
        "sha256:00ff10a0b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72";

    #[test]
    fn digest_decodes_canonical_checksums_only() {
        let bytes = digest(CHECKSUM).expect("canonical");
        assert_eq!(bytes.len(), 32);
        assert_eq!(bytes[..4], [0x00, 0xff, 0x10, 0xa0]);
        for value in ["", "sha256:00ff", &CHECKSUM["sha256:".len()..]] {
            assert_eq!(digest(value), None, "{value:?}");
        }
    }

    #[test]
    fn an_object_holds_the_archive_only_with_its_size_and_recorded_digest() {
        let sha256 = digest(CHECKSUM).expect("canonical");
        assert!(holds_archive(10, Some(&sha256), 10, CHECKSUM));
        assert!(!holds_archive(11, Some(&sha256), 10, CHECKSUM));
        assert!(!holds_archive(10, None, 10, CHECKSUM));
        let mut other = sha256.clone();
        other[31] ^= 1;
        assert!(!holds_archive(10, Some(&other), 10, CHECKSUM));
        // A corrupt row names no digest, so nothing matches it.
        assert!(!holds_archive(10, Some(&sha256), 10, "sha256:00ff"));
        assert!(!holds_archive(10, None, 10, "sha256:00ff"));
    }
}
