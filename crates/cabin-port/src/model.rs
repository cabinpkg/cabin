use std::path::PathBuf;

use cabin_core::PackageName;
use semver::Version;
use url::Url;

/// Validated `port.toml` document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortDescriptor {
    /// Authoritative package identity. The overlay manifest's
    /// `[package]` must match these values; mismatches surface
    /// at preparation time.
    pub name: PackageName,
    pub version: Version,
    pub metadata: PortMetadata,
    pub source: PortSource,
    pub overlay: OverlayManifest,
}

/// Optional human-facing fields. Always present in the struct
/// (with `None` defaults) so callers can render metadata
/// uniformly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortMetadata {
    pub description: Option<String>,
    pub license: Option<String>,
    pub homepage: Option<Url>,
    pub upstream: Option<Url>,
}

/// Where the port's upstream bytes come from. Only the
/// pinned-archive shape is supported; every other form
/// (git, tag-only, branch, `latest`) is rejected by the
/// parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortSource {
    Archive {
        url: Url,
        sha256: PortChecksum,
        /// Single directory component to strip from every
        /// archive entry before joining into the destination.
        /// `None` means the archive root is the destination
        /// root.
        strip_prefix: Option<String>,
    },
}

/// SHA-256 digest of a port's source archive. Stored as 32
/// validated bytes; render with [`PortChecksum::to_hex`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortChecksum([u8; 32]);

impl PortChecksum {
    /// Parse a 64-character lowercase hex digest. Anything
    /// else (wrong length, non-hex characters, upper-case)
    /// is rejected.
    pub fn parse_hex(value: &str) -> Option<Self> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return None;
        }
        let bytes = value.as_bytes();
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            let hi = hex_value(bytes[i * 2])?;
            let lo = hex_value(bytes[i * 2 + 1])?;
            *byte = (hi << 4) | lo;
        }
        Some(Self(out))
    }

    /// 64-character lowercase hex digest.
    pub fn to_hex(self) -> String {
        cabin_core::hash::hex_digest(&self.0)
    }
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(10 + byte - b'a'),
        _ => None,
    }
}

/// Overlay manifest pointer. The path is relative to the port
/// directory; absolute paths and `..` components are rejected by
/// the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayManifest {
    pub relative_path: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_parses_valid_hex() {
        let c = PortChecksum::parse_hex(
            "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23",
        )
        .unwrap();
        assert_eq!(
            c.to_hex(),
            "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"
        );
    }

    #[test]
    fn checksum_rejects_uppercase() {
        assert!(
            PortChecksum::parse_hex(
                "9A93B2B7DFDAC77CEBA5A558A580E74667DD6FEDE4585B91EEFB60F03B72DF23"
            )
            .is_none()
        );
    }

    #[test]
    fn checksum_rejects_wrong_length() {
        assert!(PortChecksum::parse_hex("deadbeef").is_none());
    }

    #[test]
    fn checksum_rejects_non_hex_chars() {
        assert!(
            PortChecksum::parse_hex(
                "g000000000000000000000000000000000000000000000000000000000000000"
            )
            .is_none()
        );
    }
}
