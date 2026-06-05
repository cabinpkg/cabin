//! `port.toml` parser.
//!
//! Raw serde structs are private; the public surface returns
//! the typed [`PortDescriptor`] value built in [`crate::model`].

use std::path::Path;

use cabin_core::PackageName;
use cabin_fs::path::{is_non_empty_safe_relative_path, is_safe_single_component};
use camino::Utf8PathBuf;
use semver::Version;
use serde::Deserialize;
use url::Url;

use crate::error::PortError;
use crate::model::{OverlayManifest, PortChecksum, PortDescriptor, PortMetadata, PortSource};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPort {
    port: RawPortIdentity,
    source: RawSource,
    overlay: RawOverlay,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPortIdentity {
    name: String,
    version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    upstream: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSource {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    strip_prefix: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOverlay {
    manifest: String,
}

/// Read and parse `port.toml` from `path`.
///
/// # Errors
/// Returns [`PortError::Io`] when `path` cannot be read; otherwise
/// propagates any parse or validation error from [`parse_port_str`].
pub fn load_port(path: impl AsRef<Path>) -> Result<PortDescriptor, PortError> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|source| PortError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse_port_str(&text, path)
}

/// Parse the contents of a `port.toml`. `path` is used for diagnostics only.
///
/// # Errors
/// Returns [`PortError::Toml`] when `text` is not valid TOML or has
/// unknown fields. Returns [`PortError::InvalidField`] for a malformed
/// `[port].name` or `[port].version`, an empty/multi-component
/// `[source].strip_prefix`, or a missing `[source].url`;
/// [`PortError::InvalidUrl`] for an unparsable
/// `[source].url`, `homepage`, or `upstream`. Returns
/// [`PortError::UnsupportedSourceType`] when `[source].type` is not
/// `archive`, [`PortError::MissingChecksum`] or
/// [`PortError::InvalidChecksum`] for an absent or non-64-hex
/// `[source].sha256`, and [`PortError::UnsafeOverlayPath`] when the
/// overlay manifest is not a safe relative path.
pub fn parse_port_str(text: &str, path: &Path) -> Result<PortDescriptor, PortError> {
    let raw: RawPort = toml::from_str(text).map_err(|source| PortError::Toml {
        path: path.to_path_buf(),
        source,
    })?;
    let RawPort {
        port,
        source,
        overlay,
    } = raw;

    let name = PackageName::new(port.name.clone()).map_err(|err| PortError::InvalidField {
        path: path.to_path_buf(),
        field: "[port].name",
        message: err.to_string(),
    })?;
    let version = Version::parse(&port.version).map_err(|err| PortError::InvalidField {
        path: path.to_path_buf(),
        field: "[port].version",
        message: err.to_string(),
    })?;

    let metadata = PortMetadata {
        description: port.description,
        license: port.license,
        homepage: parse_optional_url(path, "homepage", port.homepage.as_deref())?,
        upstream: parse_optional_url(path, "upstream", port.upstream.as_deref())?,
    };

    let source = source_from_raw(path, source)?;
    let overlay = overlay_from_raw(path, overlay)?;

    Ok(PortDescriptor {
        name,
        version,
        metadata,
        source,
        overlay,
    })
}

fn source_from_raw(path: &Path, raw: RawSource) -> Result<PortSource, PortError> {
    if raw.kind != "archive" {
        return Err(PortError::UnsupportedSourceType {
            path: path.to_path_buf(),
            kind: raw.kind,
        });
    }
    let url_str = raw.url.ok_or_else(|| PortError::InvalidField {
        path: path.to_path_buf(),
        field: "[source].url",
        message: "expected a non-empty URL".to_owned(),
    })?;
    let url = Url::parse(&url_str).map_err(|err| PortError::InvalidUrl {
        path: path.to_path_buf(),
        field: "url",
        value: url_str,
        message: err.to_string(),
    })?;
    let raw_checksum = raw.sha256.ok_or_else(|| PortError::MissingChecksum {
        path: path.to_path_buf(),
    })?;
    let sha256 =
        PortChecksum::parse_hex(&raw_checksum).ok_or_else(|| PortError::InvalidChecksum {
            path: path.to_path_buf(),
            value: raw_checksum,
        })?;
    let strip_prefix = raw
        .strip_prefix
        .map(|s| {
            if s.is_empty() {
                return Err(PortError::InvalidField {
                    path: path.to_path_buf(),
                    field: "[source].strip_prefix",
                    message: "expected a non-empty prefix".to_owned(),
                });
            }
            if !is_safe_single_component(&s) {
                return Err(PortError::InvalidField {
                    path: path.to_path_buf(),
                    field: "[source].strip_prefix",
                    message: "expected a single non-empty relative path component".to_owned(),
                });
            }
            Ok(s)
        })
        .transpose()?;
    Ok(PortSource::Archive {
        url,
        sha256,
        strip_prefix,
    })
}

fn overlay_from_raw(path: &Path, raw: RawOverlay) -> Result<OverlayManifest, PortError> {
    let rel = Utf8PathBuf::from(&raw.manifest);
    if !is_non_empty_safe_relative_path(rel.as_std_path()) {
        return Err(PortError::UnsafeOverlayPath {
            path: path.to_path_buf(),
            value: raw.manifest,
        });
    }
    Ok(OverlayManifest { relative_path: rel })
}

fn parse_optional_url(
    path: &Path,
    field: &'static str,
    raw: Option<&str>,
) -> Result<Option<Url>, PortError> {
    match raw {
        None => Ok(None),
        Some(value) => Url::parse(value)
            .map(Some)
            .map_err(|err| PortError::InvalidUrl {
                path: path.to_path_buf(),
                field,
                value: value.to_owned(),
                message: err.to_string(),
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    const ZLIB_PORT: &str = r#"
[port]
name = "zlib"
version = "1.3.1"
description = "Compression library"
license = "Zlib"
homepage = "https://zlib.net/"
upstream = "https://github.com/madler/zlib"

[source]
type = "archive"
url = "https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz"
sha256 = "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"
strip_prefix = "zlib-1.3.1"

[overlay]
manifest = "cabin.toml"
"#;

    fn parse(text: &str) -> Result<PortDescriptor, PortError> {
        parse_port_str(text, Path::new("port.toml"))
    }

    #[test]
    fn parses_zlib_port() {
        let port = parse(ZLIB_PORT).unwrap();
        assert_eq!(port.name.as_str(), "zlib");
        assert_eq!(port.version, Version::new(1, 3, 1));
        match &port.source {
            PortSource::Archive {
                url,
                sha256,
                strip_prefix,
            } => {
                assert_eq!(
                    url.as_str(),
                    "https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz"
                );
                assert_eq!(
                    sha256.to_hex(),
                    "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"
                );
                assert_eq!(strip_prefix.as_deref(), Some("zlib-1.3.1"));
            }
        }
        assert_eq!(port.overlay.relative_path, Utf8PathBuf::from("cabin.toml"));
        assert_eq!(
            port.metadata.description.as_deref(),
            Some("Compression library")
        );
        assert_eq!(port.metadata.license.as_deref(), Some("Zlib"));
        assert_eq!(
            port.metadata.homepage.as_ref().map(Url::as_str),
            Some("https://zlib.net/")
        );
        assert_eq!(
            port.metadata.upstream.as_ref().map(Url::as_str),
            Some("https://github.com/madler/zlib")
        );
    }

    #[test]
    fn rejects_missing_sha256() {
        let text = ZLIB_PORT.replace(
            "sha256 = \"9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23\"\n",
            "",
        );
        let err = parse(&text).unwrap_err();
        assert!(
            matches!(err, PortError::MissingChecksum { .. }),
            "expected MissingChecksum, got {err:?}"
        );
    }

    #[test]
    fn rejects_invalid_sha256_length() {
        let text = ZLIB_PORT.replace(
            "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23",
            "deadbeef",
        );
        let err = parse(&text).unwrap_err();
        assert!(
            matches!(err, PortError::InvalidChecksum { .. }),
            "expected InvalidChecksum, got {err:?}"
        );
    }

    #[test]
    fn rejects_uppercase_sha256() {
        let text = ZLIB_PORT.replace(
            "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23",
            "9A93B2B7DFDAC77CEBA5A558A580E74667DD6FEDE4585B91EEFB60F03B72DF23",
        );
        let err = parse(&text).unwrap_err();
        assert!(
            matches!(err, PortError::InvalidChecksum { .. }),
            "expected InvalidChecksum, got {err:?}"
        );
    }

    #[test]
    fn rejects_unsupported_source_type_git() {
        let text = ZLIB_PORT.replace("type = \"archive\"", "type = \"git\"");
        let err = parse(&text).unwrap_err();
        match err {
            PortError::UnsupportedSourceType { kind, .. } => assert_eq!(kind, "git"),
            other => panic!("expected UnsupportedSourceType, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unsupported_source_type_branch() {
        let text = ZLIB_PORT.replace("type = \"archive\"", "type = \"branch\"");
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::UnsupportedSourceType { .. }));
    }

    #[test]
    fn rejects_unsupported_source_type_latest() {
        let text = ZLIB_PORT.replace("type = \"archive\"", "type = \"latest\"");
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::UnsupportedSourceType { .. }));
    }

    #[test]
    fn rejects_unsupported_source_type_tag() {
        // Tags without a SHA-256 are not allowed: the source
        // type must be `archive` plus a pinned SHA-256.
        let text = ZLIB_PORT.replace("type = \"archive\"", "type = \"tag\"");
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::UnsupportedSourceType { .. }));
    }

    #[test]
    fn rejects_absolute_overlay_path() {
        let text = ZLIB_PORT.replace("manifest = \"cabin.toml\"", "manifest = \"/etc/passwd\"");
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::UnsafeOverlayPath { .. }));
    }

    #[test]
    fn rejects_parent_dir_overlay_path() {
        let text = ZLIB_PORT.replace("manifest = \"cabin.toml\"", "manifest = \"../cabin.toml\"");
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::UnsafeOverlayPath { .. }));
    }

    #[test]
    fn rejects_empty_overlay_path() {
        let text = ZLIB_PORT.replace("manifest = \"cabin.toml\"", "manifest = \"\"");
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::UnsafeOverlayPath { .. }));
    }

    #[test]
    fn accepts_nested_overlay_path() {
        let text = ZLIB_PORT.replace(
            "manifest = \"cabin.toml\"",
            "manifest = \"overlay/cabin.toml\"",
        );
        let port = parse(&text).unwrap();
        assert_eq!(
            port.overlay.relative_path,
            Utf8PathBuf::from("overlay/cabin.toml")
        );
    }

    #[test]
    fn rejects_invalid_url() {
        let text = ZLIB_PORT.replace(
            "url = \"https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz\"",
            "url = \"::not a url::\"",
        );
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::InvalidUrl { field: "url", .. }));
    }

    #[test]
    fn rejects_invalid_homepage_url() {
        let text = ZLIB_PORT.replace("homepage = \"https://zlib.net/\"", "homepage = \"::bad::\"");
        let err = parse(&text).unwrap_err();
        assert!(matches!(
            err,
            PortError::InvalidUrl {
                field: "homepage",
                ..
            }
        ));
    }

    #[test]
    fn rejects_unknown_top_level_table() {
        let text = format!("{ZLIB_PORT}\n[extras]\nsomething = true\n");
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::Toml { .. }), "{err:?}");
    }

    #[test]
    fn rejects_unknown_source_field() {
        let text = ZLIB_PORT.replace(
            "type = \"archive\"",
            "type = \"archive\"\nextra_field = \"x\"",
        );
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::Toml { .. }), "{err:?}");
    }

    #[test]
    fn rejects_strip_prefix_with_path_separator() {
        let text = ZLIB_PORT.replace("strip_prefix = \"zlib-1.3.1\"", "strip_prefix = \"a/b\"");
        let err = parse(&text).unwrap_err();
        assert!(matches!(
            err,
            PortError::InvalidField {
                field: "[source].strip_prefix",
                ..
            }
        ));
    }

    #[test]
    fn rejects_strip_prefix_dotdot() {
        let text = ZLIB_PORT.replace("strip_prefix = \"zlib-1.3.1\"", "strip_prefix = \"..\"");
        let err = parse(&text).unwrap_err();
        assert!(matches!(
            err,
            PortError::InvalidField {
                field: "[source].strip_prefix",
                ..
            }
        ));
    }

    #[test]
    fn rejects_strip_prefix_curdir() {
        let text = ZLIB_PORT.replace("strip_prefix = \"zlib-1.3.1\"", "strip_prefix = \".\"");
        let err = parse(&text).unwrap_err();
        assert!(matches!(
            err,
            PortError::InvalidField {
                field: "[source].strip_prefix",
                ..
            }
        ));
    }

    #[test]
    fn rejects_strip_prefix_with_backslash() {
        let text = ZLIB_PORT.replace("strip_prefix = \"zlib-1.3.1\"", r#"strip_prefix = "a\\b""#);
        let err = parse(&text).unwrap_err();
        assert!(matches!(
            err,
            PortError::InvalidField {
                field: "[source].strip_prefix",
                ..
            }
        ));
    }
}
