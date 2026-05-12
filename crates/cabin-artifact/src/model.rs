/// Prefix every Cabin checksum string carries.
pub const CHECKSUM_PREFIX: &str = "sha256:";

/// A parsed `sha256:<hex>` digest, lower-cased and validated.
///
/// Strings are accepted via [`ChecksumDigest::parse`]; anything that does
/// not match `sha256:` followed by exactly 64 hex characters is rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksumDigest {
    hex: String,
}

impl ChecksumDigest {
    /// Parse a `sha256:<hex>` checksum. Returns `None` if the prefix or
    /// the hex body is malformed.
    pub fn parse(value: &str) -> Option<Self> {
        let rest = value.strip_prefix(CHECKSUM_PREFIX)?;
        if rest.len() != 64 || !rest.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        Some(Self {
            hex: rest.to_ascii_lowercase(),
        })
    }

    /// The 64-character lower-case hex body.
    pub fn hex(&self) -> &str {
        &self.hex
    }

    /// Re-render as `sha256:<hex>`.
    pub fn full(&self) -> String {
        format!("{CHECKSUM_PREFIX}{}", self.hex)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_checksum() {
        let body = "a".repeat(64);
        let value = format!("sha256:{body}");
        let parsed = ChecksumDigest::parse(&value).unwrap();
        assert_eq!(parsed.hex(), body);
        assert_eq!(parsed.full(), value);
    }

    #[test]
    fn lowercases_uppercase_hex() {
        let value = format!("sha256:{}", "A".repeat(64));
        let parsed = ChecksumDigest::parse(&value).unwrap();
        assert_eq!(parsed.hex(), "a".repeat(64));
    }

    #[test]
    fn rejects_wrong_prefix() {
        assert!(ChecksumDigest::parse(&format!("md5:{}", "a".repeat(64))).is_none());
    }

    #[test]
    fn rejects_wrong_length() {
        assert!(ChecksumDigest::parse("sha256:abc").is_none());
    }

    #[test]
    fn rejects_non_hex() {
        let body = "z".repeat(64);
        assert!(ChecksumDigest::parse(&format!("sha256:{body}")).is_none());
    }
}
