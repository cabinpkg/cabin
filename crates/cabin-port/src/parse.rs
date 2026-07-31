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
use crate::model::{
    ArchiveSource, CopyStep, OverlayManifest, PortChecksum, PortDescriptor, PortMetadata,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPort {
    port: RawPortIdentity,
    source: RawSource,
    overlay: RawOverlay,
    #[serde(default)]
    copy: Vec<RawCopy>,
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
    /// `[source].patches` - declared unified-diff files applied to
    /// the assembled source, mirroring the published
    /// `[package.upstream]` table where `patches` sits beside the
    /// pin.
    #[serde(default)]
    patches: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOverlay {
    manifest: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCopy {
    from: String,
    to: String,
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
/// unknown fields.  Returns [`PortError::InvalidField`] for a malformed
/// `[port].name` or `[port].version`, an empty/multi-component
/// `[source].strip_prefix`, or a missing `[source].url`;
/// [`PortError::InvalidUrl`] for an unparsable
/// `[source].url`, `homepage`, or `upstream`.  Returns
/// [`PortError::UnsupportedSourceType`] when `[source].type` is not
/// `archive`, [`PortError::MissingChecksum`] or
/// [`PortError::InvalidChecksum`] for an absent or non-64-hex
/// `[source].sha256`, [`PortError::UnsafeOverlayPath`] when the
/// overlay manifest is not a safe relative path, and
/// [`PortError::UnsafeCopyPath`] when a `[[copy]]` `from`/`to` is not
/// a safe relative path.
pub fn parse_port_str(text: &str, path: &Path) -> Result<PortDescriptor, PortError> {
    let raw: RawPort = toml::from_str(text).map_err(|source| PortError::Toml {
        path: path.to_path_buf(),
        source,
    })?;
    let RawPort {
        port,
        mut source,
        overlay,
        copy,
    } = raw;
    let raw_patches = std::mem::take(&mut source.patches);

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
    let copies = copies_from_raw(path, copy)?;
    let patches = patches_from_raw(path, raw_patches)?;

    // Reject patch/copy and patch/patch collisions at parse time with
    // the same rule the published `[package.upstream]` declaration
    // enforces, so a recipe that would only fail at publish-time
    // conversion fails here instead.
    if !patches.is_empty() {
        let patch_strings: Vec<String> = patches.iter().map(|p| p.as_str().to_owned()).collect();
        let plan_paths: Vec<&str> = copies
            .iter()
            .flat_map(|step| [step.from.as_str(), step.to.as_str()])
            .collect();
        cabin_core::upstream::validate_patch_plan(&patch_strings, &plan_paths).map_err(
            |source| PortError::InvalidPatchPlan {
                path: path.to_path_buf(),
                source,
            },
        )?;
    }

    Ok(PortDescriptor {
        name,
        version,
        metadata,
        source,
        overlay,
        copies,
        patches,
    })
}

fn copies_from_raw(path: &Path, raw: Vec<RawCopy>) -> Result<Vec<CopyStep>, PortError> {
    raw.into_iter()
        .map(|step| {
            let from = safe_copy_path(path, "from", step.from)?;
            let to = safe_copy_path(path, "to", step.to)?;
            Ok(CopyStep { from, to })
        })
        .collect()
}

/// Validate the `[source].patches` list: each entry must be
/// `patches/<file>` - a single file directly under the port
/// directory's `patches/` subdirectory - so the recipe layout matches
/// the published `[package.upstream]` declaration and the bundling
/// build script can embed the files without parsing the descriptor.
/// Entries are capped and duplicates rejected, mirroring the
/// published declaration's rules.
fn patches_from_raw(path: &Path, raw: Vec<String>) -> Result<Vec<Utf8PathBuf>, PortError> {
    if raw.len() > cabin_core::MAX_PATCH_FILES {
        return Err(PortError::InvalidField {
            path: path.to_path_buf(),
            field: "[source].patches",
            message: format!(
                "at most {} patch files are supported",
                cabin_core::MAX_PATCH_FILES
            ),
        });
    }
    let mut patches = Vec::with_capacity(raw.len());
    for value in raw {
        // The full archive-path rule (portability, length caps) runs
        // too: the entry is stamped verbatim into the published
        // `[package.upstream]` declaration, so a spelling the
        // publish-side validation would reject must fail here.
        let well_shaped = value
            .strip_prefix("patches/")
            .is_some_and(is_safe_single_component)
            && cabin_core::upstream::is_safe_archive_path(&value);
        if !well_shaped {
            return Err(PortError::UnsafePatchPath {
                path: path.to_path_buf(),
                value,
            });
        }
        let rel = Utf8PathBuf::from(&value);
        if patches.contains(&rel) {
            return Err(PortError::InvalidField {
                path: path.to_path_buf(),
                field: "[source].patches",
                message: format!("duplicate patch entry `{value}`"),
            });
        }
        patches.push(rel);
    }
    Ok(patches)
}

fn safe_copy_path(
    path: &Path,
    field: &'static str,
    value: String,
) -> Result<Utf8PathBuf, PortError> {
    let rel = Utf8PathBuf::from(&value);
    if !is_non_empty_safe_relative_path(rel.as_std_path()) {
        return Err(PortError::UnsafeCopyPath {
            path: path.to_path_buf(),
            field,
            value,
        });
    }
    Ok(rel)
}

fn source_from_raw(path: &Path, raw: RawSource) -> Result<ArchiveSource, PortError> {
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
    Ok(ArchiveSource {
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
        assert_eq!(
            port.source.url.as_str(),
            "https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz"
        );
        assert_eq!(
            port.source.sha256.to_hex(),
            "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"
        );
        assert_eq!(port.source.strip_prefix.as_deref(), Some("zlib-1.3.1"));
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

    #[test]
    fn no_copy_section_yields_empty_copies() {
        let port = parse(ZLIB_PORT).unwrap();
        assert!(port.copies.is_empty());
    }

    #[test]
    fn parses_copy_steps() {
        let text = format!(
            "{ZLIB_PORT}\n[[copy]]\nfrom = \"scripts/pnglibconf.h.prebuilt\"\nto = \"pnglibconf.h\"\n"
        );
        let port = parse(&text).unwrap();
        assert_eq!(port.copies.len(), 1);
        assert_eq!(
            port.copies[0].from,
            Utf8PathBuf::from("scripts/pnglibconf.h.prebuilt")
        );
        assert_eq!(port.copies[0].to, Utf8PathBuf::from("pnglibconf.h"));
    }

    #[test]
    fn rejects_copy_from_escaping_source() {
        let text = format!("{ZLIB_PORT}\n[[copy]]\nfrom = \"../secret\"\nto = \"pnglibconf.h\"\n");
        let err = parse(&text).unwrap_err();
        assert!(
            matches!(err, PortError::UnsafeCopyPath { field: "from", .. }),
            "{err:?}"
        );
    }

    #[test]
    fn rejects_copy_to_escaping_source() {
        let text = format!(
            "{ZLIB_PORT}\n[[copy]]\nfrom = \"scripts/pnglibconf.h.prebuilt\"\nto = \"/etc/passwd\"\n"
        );
        let err = parse(&text).unwrap_err();
        assert!(
            matches!(err, PortError::UnsafeCopyPath { field: "to", .. }),
            "{err:?}"
        );
    }

    #[test]
    fn rejects_unknown_copy_field() {
        let text = format!("{ZLIB_PORT}\n[[copy]]\nfrom = \"a\"\nto = \"b\"\nmode = \"0644\"\n");
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::Toml { .. }), "{err:?}");
    }

    #[test]
    fn parses_patch_entries_in_order() {
        let text = ZLIB_PORT.replace(
            "strip_prefix = \"zlib-1.3.1\"",
            "strip_prefix = \"zlib-1.3.1\"\npatches = [\"patches/0001-fix-msvc-build.patch\", \
             \"patches/0002-portability.patch\"]",
        );
        let descriptor = parse(&text).unwrap();
        assert_eq!(
            descriptor.patches,
            [
                Utf8PathBuf::from("patches/0001-fix-msvc-build.patch"),
                Utf8PathBuf::from("patches/0002-portability.patch"),
            ]
        );
    }

    #[test]
    fn no_patches_key_yields_empty_patches() {
        assert!(parse(ZLIB_PORT).unwrap().patches.is_empty());
    }

    #[test]
    fn rejects_patch_entries_outside_the_patches_directory() {
        for value in [
            "0001-fix.patch",
            "other/0001-fix.patch",
            "patches/nested/0001-fix.patch",
            "patches/",
            "patches/..",
            "../patches/0001-fix.patch",
            "/patches/0001-fix.patch",
            "patches\\0001-fix.patch",
            "patches/fix.patch.",
        ] {
            let text = ZLIB_PORT.replace(
                "strip_prefix = \"zlib-1.3.1\"",
                &format!(
                    "strip_prefix = \"zlib-1.3.1\"\npatches = [\"{}\"]",
                    value.replace('\\', "\\\\")
                ),
            );
            let err = parse(&text).unwrap_err();
            assert!(
                matches!(err, PortError::UnsafePatchPath { .. }),
                "{value:?}: {err:?}"
            );
        }
    }

    #[test]
    fn rejects_patch_conflicting_with_a_copy_path() {
        let text = ZLIB_PORT.replace(
            "[overlay]",
            "patches = [\"patches/gen.h\"]\n\n[[copy]]\nfrom = \"scripts/gen.h.in\"\n\
             to = \"patches/gen.h\"\n\n[overlay]",
        );
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::InvalidPatchPlan { .. }), "{err:?}");
    }

    #[test]
    fn rejects_case_folded_duplicate_patch_entries() {
        let text = ZLIB_PORT.replace(
            "strip_prefix = \"zlib-1.3.1\"",
            "strip_prefix = \"zlib-1.3.1\"\npatches = [\"patches/A.patch\", \"patches/a.patch\"]",
        );
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::InvalidPatchPlan { .. }), "{err:?}");
    }

    #[test]
    fn rejects_duplicate_patch_entries() {
        let text = ZLIB_PORT.replace(
            "strip_prefix = \"zlib-1.3.1\"",
            "strip_prefix = \"zlib-1.3.1\"\npatches = [\"patches/0001-fix.patch\", \
             \"patches/0001-fix.patch\"]",
        );
        let err = parse(&text).unwrap_err();
        assert!(
            matches!(
                err,
                PortError::InvalidField {
                    field: "[source].patches",
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn rejects_too_many_patch_entries() {
        let entries: Vec<String> = (0..=cabin_core::MAX_PATCH_FILES)
            .map(|i| format!("\"patches/{i}.patch\""))
            .collect();
        let text = ZLIB_PORT.replace(
            "strip_prefix = \"zlib-1.3.1\"",
            &format!(
                "strip_prefix = \"zlib-1.3.1\"\npatches = [{}]",
                entries.join(", ")
            ),
        );
        let err = parse(&text).unwrap_err();
        assert!(
            matches!(
                err,
                PortError::InvalidField {
                    field: "[source].patches",
                    ..
                }
            ),
            "{err:?}"
        );
    }
}
