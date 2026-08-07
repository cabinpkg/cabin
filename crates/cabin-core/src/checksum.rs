//! The shared checksum representation: `sha256:<64 lowercase hex>`.
//!
//! Wherever a digest of package-archive bytes crosses a boundary -
//! manifests, lockfiles, index entries, registry payloads, vendor
//! metadata, diagnostics - it is serialized in this self-describing
//! spelling.  [`Checksum`] parses it strictly, holds it canonically,
//! and renders it back; a checksum boundary being added or changed
//! parses into the type instead of threading raw strings.  `sha256`
//! is the only supported algorithm; a future algorithm changes the
//! accepted prefixes, not the key names.
//!
//! A packaging-revision id (the digest's leading 16 hex characters,
//! see [`crate::registry`]) is a derived *identifier*, not a
//! checksum: it is carried bare, and deliberately never parses as a
//! [`Checksum`].

use std::fmt;

use serde::de::{Deserializer, Error as _};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The one checksum algorithm prefix Cabin supports.
const CHECKSUM_PREFIX: &str = "sha256:";

/// Why a checksum string was rejected.  The message is the normative
/// user-facing sentence; callers with field context wrap it (the
/// manifest parser prefixes `[package.upstream]`).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "checksum {value:?} must be `sha256:` followed by 64 lowercase hexadecimal characters \
     (sha256 is the only supported algorithm)"
)]
pub struct ChecksumError {
    pub value: String,
}

/// A validated `sha256:<64 lowercase hex>` archive checksum, stored
/// in its canonical serialized spelling.  Constructed only by
/// [`Checksum::parse`] or by hashing bytes, so a value in hand is
/// always well-formed; display, serialization, and [`Checksum::as_str`]
/// all yield the identical prefixed string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Checksum(String);

impl Checksum {
    /// Parse the canonical spelling, strictly: the exact `sha256:`
    /// prefix followed by exactly 64 lowercase hexadecimal
    /// characters.  Anything else - a bare digest, uppercase hex, a
    /// truncated body, surrounding whitespace - is rejected; there is
    /// no lenient or legacy form.
    ///
    /// # Errors
    /// Returns [`ChecksumError`] echoing the rejected value.
    pub fn parse(value: &str) -> Result<Self, ChecksumError> {
        let digest_ok = value.strip_prefix(CHECKSUM_PREFIX).is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        });
        if !digest_ok {
            return Err(ChecksumError {
                value: value.to_owned(),
            });
        }
        Ok(Self(value.to_owned()))
    }

    /// Hash `bytes` with SHA-256.
    #[must_use]
    pub fn of_bytes(bytes: &[u8]) -> Self {
        use sha2::Digest as _;
        Self::from_hex(&crate::hash::hex_digest(&sha2::Sha256::digest(bytes)))
    }

    /// Stream `reader` through SHA-256 (the shared
    /// [`crate::hash::hash_reader`] primitive).
    ///
    /// # Errors
    /// Returns the [`std::io::Error`] propagated from reading
    /// `reader`; interrupted reads are retried, not errors.
    pub fn of_reader<R: std::io::Read>(reader: R) -> std::io::Result<Self> {
        crate::hash::hash_reader(reader).map(|hex| Self::from_hex(&hex))
    }

    /// Stream `reader` into `writer` while hashing the bytes in the
    /// same pass (the shared [`crate::hash::hash_copy`] primitive),
    /// so a torn copy surfaces as a checksum mismatch.
    ///
    /// # Errors
    /// Returns the first non-interrupted [`std::io::Error`] from
    /// reading `reader` or writing `writer`.
    pub fn of_copy<R: std::io::Read, W: std::io::Write>(
        reader: R,
        writer: W,
    ) -> std::io::Result<Self> {
        crate::hash::hash_copy(reader, writer).map(|hex| Self::from_hex(&hex))
    }

    /// Wrap a digest freshly rendered by [`crate::hash`], which only
    /// produces 64 lowercase hex characters.
    fn from_hex(hex: &str) -> Self {
        debug_assert_eq!(hex.len(), 64);
        Self(format!("{CHECKSUM_PREFIX}{hex}"))
    }

    /// The canonical `sha256:<hex>` spelling - exactly what
    /// serialization and display emit.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The 64-character lowercase hex digest without the algorithm
    /// prefix - the form hashing comparisons and content-addressed
    /// path leaves consume.
    #[must_use]
    pub fn hex(&self) -> &str {
        &self.0[CHECKSUM_PREFIX.len()..]
    }

    /// The packaging-revision identifier this digest mints: its
    /// leading [`crate::registry::PACKAGING_REVISION_HEX_LEN`] hex
    /// characters, carried bare everywhere revisions appear.
    #[must_use]
    pub fn revision_id(&self) -> &str {
        &self.hex()[..crate::registry::PACKAGING_REVISION_HEX_LEN]
    }
}

impl fmt::Display for Checksum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Checksum {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Checksum {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const SHA: &str = "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23";

    fn checksum() -> Checksum {
        Checksum::parse(&format!("sha256:{SHA}")).unwrap()
    }

    #[test]
    fn parses_the_canonical_spelling() {
        let parsed = checksum();
        assert_eq!(parsed.as_str(), format!("sha256:{SHA}"));
        assert_eq!(parsed.hex(), SHA);
        assert_eq!(parsed.to_string(), format!("sha256:{SHA}"));
    }

    #[test]
    fn rejects_everything_that_is_not_the_canonical_spelling() {
        for value in [
            // A bare digest carries no algorithm.
            SHA,
            "sha256:deadbeef",
            &format!("sha512:{SHA}"),
            // Uppercase is a different spelling of the same bytes;
            // one canonical form keeps string equality equal to
            // digest equality everywhere.
            &format!("sha256:{}", SHA.to_uppercase()),
            &format!("sha256:g{}", &SHA[1..]),
            // The prefix is case-sensitive too: `SHA256:` is not the
            // canonical spelling.
            &format!("SHA256:{SHA}"),
            &format!("sha256:sha256:{SHA}"),
            &format!(" sha256:{SHA}"),
            &format!("sha256:{SHA} "),
            &format!("sha256:{SHA}ff"),
            "sha256:",
            "",
            // A packaging-revision id is an identifier, not a
            // checksum; the grammars must stay disjoint.
            "9a93b2b7dfdac77c",
            "sha256:9a93b2b7dfdac77c",
        ] {
            let err = Checksum::parse(value).unwrap_err();
            assert_eq!(err.value, value, "{value:?}");
            assert!(
                err.to_string().contains("64 lowercase hexadecimal"),
                "{err}"
            );
        }
    }

    #[test]
    fn hashing_constructors_agree_with_the_shared_primitives() {
        // SHA-256 of the empty input, a fixed reference value.
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(Checksum::of_bytes(b"").hex(), empty);
        assert_eq!(Checksum::of_reader(Cursor::new(b"")).unwrap().hex(), empty);

        let expected = Checksum::of_bytes(b"abc");
        assert_eq!(
            expected.as_str(),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        );
        assert_eq!(Checksum::of_reader(Cursor::new(b"abc")).unwrap(), expected);

        let mut copied = Vec::new();
        let copy = Checksum::of_copy(Cursor::new(b"abc"), &mut copied).unwrap();
        assert_eq!(copy, expected);
        assert_eq!(copied, b"abc");
    }

    #[test]
    fn revision_id_is_the_leading_hex_prefix() {
        let parsed = checksum();
        assert_eq!(parsed.revision_id(), "9a93b2b7dfdac77c");
        assert!(crate::registry::is_valid_packaging_revision(
            parsed.revision_id()
        ));
        assert_eq!(
            Some(parsed.revision_id()),
            crate::registry::packaging_revision_from_sha256_hex(parsed.hex()),
        );
    }

    #[test]
    fn serde_round_trips_and_rejects_malformed_values() {
        let json = serde_json::to_string(&checksum()).unwrap();
        assert_eq!(json, format!("\"sha256:{SHA}\""));
        let back: Checksum = serde_json::from_str(&json).unwrap();
        assert_eq!(back, checksum());

        for bad in [
            format!("\"{SHA}\""),
            format!("\"sha256:{}\"", SHA.to_uppercase()),
            "\"sha256:\"".to_owned(),
            "42".to_owned(),
        ] {
            let err = serde_json::from_str::<Checksum>(&bad).unwrap_err();
            assert!(
                err.to_string().contains("64 lowercase hexadecimal")
                    || err.to_string().contains("expected a string"),
                "{bad}: {err}"
            );
        }
    }

    #[test]
    fn ordering_and_equality_follow_the_canonical_string() {
        let a = Checksum::parse(&format!("sha256:{}", "a".repeat(64))).unwrap();
        let b = Checksum::parse(&format!("sha256:{}", "b".repeat(64))).unwrap();
        assert!(a < b);
        assert_eq!(a, a.clone());
    }
}
