use camino::Utf8PathBuf;

use cabin_core::PackageName;
use semver::Version;
use url::Url;

/// Validated `port.toml` document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortDescriptor {
    /// Authoritative package identity.  The overlay manifest's
    /// `[package]` must match these values; mismatches surface
    /// at preparation time.
    pub name: PackageName,
    pub version: Version,
    pub metadata: PortMetadata,
    pub source: PortSource,
    pub overlay: OverlayManifest,
    /// In-tree file placements applied to the extracted source
    /// after `strip_prefix`.  Each step copies an upstream file to
    /// a second in-tree location - used when a project ships a
    /// build-time config under a different name (e.g. libpng's
    /// `scripts/pnglibconf.h.prebuilt` → `pnglibconf.h`).  This is a
    /// static copy, not a build script: foundation ports never run
    /// upstream configure/codegen.
    pub copies: Vec<CopyStep>,
}

/// One declarative file placement: copy `from` to `to`, both
/// relative to the extracted source root.  Validated as
/// non-empty safe relative paths so neither can escape the
/// source directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyStep {
    pub from: Utf8PathBuf,
    pub to: Utf8PathBuf,
}

/// Optional human-facing fields.  Always present in the struct
/// (with `None` defaults) so callers can render metadata
/// uniformly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortMetadata {
    pub description: Option<String>,
    pub license: Option<String>,
    pub homepage: Option<Url>,
    pub upstream: Option<Url>,
}

/// Where the port's upstream bytes come from.  Only the
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

/// SHA-256 digest of a port's source archive.  Stored as the
/// validated 64-character lowercase hex string - every consumer
/// compares digests in hex form, so the byte decode would be
/// dead weight.  Render with [`PortChecksum::to_hex`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortChecksum(String);

impl PortChecksum {
    /// Parse a 64-character lowercase hex digest.  Anything
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
        Some(Self(value.to_owned()))
    }

    /// 64-character lowercase hex digest.
    pub fn to_hex(&self) -> String {
        self.0.clone()
    }
}

/// Overlay manifest pointer.  The path is relative to the port
/// directory; absolute paths and `..` components are rejected by
/// the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayManifest {
    pub relative_path: Utf8PathBuf,
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
