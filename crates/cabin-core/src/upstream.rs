//! Typed `[package.upstream]` provenance: an optional, machine-
//! verifiable claim that a published package's source tree came from
//! a pinned upstream archive.
//!
//! The declaration is inert metadata for consumers - resolving,
//! fetching, and building a package never touches the upstream URL.
//! Only the registry's external verification workflow downloads the
//! archive and checks the published tree against it
//! (`docs/remote-registry.md`, "The verifier's checks").
//!
//! The shape deliberately mirrors a foundation-port recipe's
//! `[source]` + `[[copy]]` tables (`cabin-port`): a pinned HTTPS
//! archive, a SHA-256, an optional single-component strip prefix,
//! and declarative file-to-file copy steps.  Unlike a port recipe
//! this is *published* metadata, so the URL is restricted to
//! credential-free HTTPS and the archive format is declared
//! explicitly instead of inferred from the URL.

use std::fmt;

use camino::Utf8PathBuf;
use serde::de::{Deserializer, Error as _};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

/// Why an upstream declaration was rejected.  The messages are
/// user-facing sentences; `cabin-manifest` surfaces them with the
/// `[package.upstream]` field context attached.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UpstreamError {
    #[error("upstream url {value:?} is not a valid URL: {message}")]
    InvalidUrl { value: String, message: String },
    #[error("upstream url {value:?} must use the `https` scheme")]
    InsecureUrl { value: String },
    #[error("upstream url {value:?} must not embed credentials")]
    UrlWithCredentials { value: String },
    #[error("upstream sha256 {value:?} must be 64 lowercase hexadecimal characters")]
    InvalidChecksum { value: String },
    #[error("unsupported upstream format {value:?}: expected \"tar.gz\" or \"zip\"")]
    UnsupportedFormat { value: String },
    #[error("upstream strip-prefix {value:?} must be a single non-empty relative path component")]
    InvalidStripPrefix { value: String },
    #[error(
        "upstream copy `{field}` path {value:?} must be a plain forward-slash relative path with \
         portable components (no `.` or `..` components, no `\\`)"
    )]
    UnsafeCopyPath { field: &'static str, value: String },
    #[error(
        "upstream copy `from` {from:?} and `to` {to:?} name the same file (paths are compared \
         case-folded, matching the package archive's case-conflict rule); a copy must name two \
         different files"
    )]
    SelfReferentialCopy { from: String, to: String },
    #[error("too many upstream copy steps ({count}): at most {MAX_COPY_STEPS} are supported")]
    TooManyCopies { count: usize },
    #[error("upstream url is {len} bytes; at most {MAX_URL_BYTES} are supported")]
    UrlTooLong { len: usize },
    #[error(
        "upstream copy `from` path {value:?} prefixed with strip-prefix {prefix:?} exceeds the \
         {MAX_COPY_PATH_BYTES}-byte archive entry-path cap"
    )]
    PrefixedCopySourceTooLong { prefix: String, value: String },
    #[error(
        "upstream copy paths {first:?} and {second:?} collide under case folding (the package \
         archive rejects case conflicts); use byte-identical or distinct names"
    )]
    CaseCollidingCopies { first: String, second: String },
}

/// Byte cap on the upstream URL's normalized serialization: a
/// practical bound (browsers and CDNs cap around here) far below
/// anything that could distress the verification workflow's process
/// argument limits.
pub const MAX_URL_BYTES: usize = 2048;

/// Cap on `[[package.upstream.copy]]` steps.  Copies duplicate bytes
/// out of the extracted upstream tree on the verifier's runner, so an
/// unbounded plan would be a disk-amplification lever; real recipes
/// need one or two steps (libpng's prebuilt config header).
pub const MAX_COPY_STEPS: usize = 16;

/// Declared container format of the pinned upstream archive.  The
/// set matches what foundation-port recipes need today; anything
/// else is rejected at parse time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamFormat {
    TarGz,
    Zip,
}

impl UpstreamFormat {
    /// The manifest / metadata string form (`"tar.gz"` / `"zip"`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
            Self::Zip => "zip",
        }
    }

    /// Parse the manifest / metadata string form.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "tar.gz" => Some(Self::TarGz),
            "zip" => Some(Self::Zip),
            _ => None,
        }
    }
}

impl fmt::Display for UpstreamFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One declarative file placement applied to the extracted upstream
/// tree: copy `from` to `to`, both validated non-empty safe relative
/// paths so neither can escape the source root.  Mirrors a port
/// recipe's `[[copy]]` step: a static file-to-file copy, never a
/// build script or codegen hook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpstreamCopy {
    from: Utf8PathBuf,
    to: Utf8PathBuf,
}

impl UpstreamCopy {
    /// Validate one copy step.
    ///
    /// # Errors
    /// Returns [`UpstreamError::UnsafeCopyPath`] when either path is
    /// not a plain portable forward-slash relative path, and
    /// [`UpstreamError::SelfReferentialCopy`] when `from` and `to`
    /// name the same file.
    pub fn new(from: String, to: String) -> Result<Self, UpstreamError> {
        let from = safe_copy_path("from", from)?;
        let to = safe_copy_path("to", to)?;
        // With `.` components and `\` rejected, case-folded string
        // equality is path-aliasing equality (the extracted tree
        // holds no symlinks - the extractor skips them).  An exact
        // self-copy is at best a no-op and `fs::copy(p, p)` actually
        // truncates the file; a case-folded pair materializes a case
        // conflict the packaging walk rejects.  Either way the
        // declaration could never verify, so it is a parse error.
        // Unicode default lowercasing matches the package archive's
        // case-conflict rule.
        if from.as_str().to_lowercase() == to.as_str().to_lowercase() {
            return Err(UpstreamError::SelfReferentialCopy {
                from: from.into_string(),
                to: to.into_string(),
            });
        }
        Ok(Self { from, to })
    }

    #[must_use]
    pub fn from(&self) -> &camino::Utf8Path {
        &self.from
    }

    #[must_use]
    pub fn to(&self) -> &camino::Utf8Path {
        &self.to
    }
}

/// Byte cap on a whole copy path, matching the published-archive
/// entry-path cap; a longer path could never name an archive entry.
const MAX_COPY_PATH_BYTES: usize = 256;

/// Byte cap on one copy-path component: Linux `NAME_MAX`.  A longer
/// component cannot be materialized on the verifier's filesystem, so
/// the copy could only ever fail operationally.
const MAX_COPY_COMPONENT_BYTES: usize = 255;

/// Copy paths are the canonical forward-slash spelling of entries in
/// the verifier's tree comparison, so anything the published-archive
/// path rules would reject - an overlong path, any non-canonical
/// alias (`./a`), a backslash, or a `.`/`..`/empty component - is
/// rejected here too.  A looser rule would only produce declarations
/// that can never verify (the published archive cannot contain the
/// matching entry, or the verifier cannot materialize the copy).
fn safe_copy_path(field: &'static str, value: String) -> Result<Utf8PathBuf, UpstreamError> {
    let structurally_safe = !value.is_empty()
        && value.len() <= MAX_COPY_PATH_BYTES
        && !value.starts_with('/')
        && !value.contains('\\')
        && value.split('/').all(|component| {
            !component.is_empty()
                && component.len() <= MAX_COPY_COMPONENT_BYTES
                && component != "."
                && component != ".."
        });
    if !structurally_safe || cabin_fs::path::relative_path_portability(&value).is_some() {
        return Err(UpstreamError::UnsafeCopyPath { field, value });
    }
    Ok(Utf8PathBuf::from(value))
}

/// Validated `[package.upstream]` declaration.  Constructed only
/// through [`UpstreamProvenance::new`], so a value in hand always
/// satisfies the invariants: a credential-free HTTPS URL, a
/// 64-character lowercase-hex SHA-256, a supported archive format,
/// an optional single-component strip prefix, and safe relative
/// copy paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamProvenance {
    url: Url,
    sha256: String,
    format: UpstreamFormat,
    strip_prefix: Option<String>,
    copies: Vec<UpstreamCopy>,
}

impl UpstreamProvenance {
    /// Validate a raw upstream declaration.
    ///
    /// # Errors
    /// Returns the first failing [`UpstreamError`]: an unparsable,
    /// non-HTTPS, or credential-bearing `url`; a `sha256` that is not
    /// 64 lowercase hex characters; a `format` other than `"tar.gz"`
    /// / `"zip"`; or a `strip_prefix` that is not a single non-empty
    /// relative path component.
    pub fn new(
        url: &str,
        sha256: &str,
        format: &str,
        strip_prefix: Option<String>,
        copies: Vec<UpstreamCopy>,
    ) -> Result<Self, UpstreamError> {
        let parsed = Url::parse(url).map_err(|err| UpstreamError::InvalidUrl {
            value: url.to_owned(),
            message: err.to_string(),
        })?;
        // Checked on the normalized serialization (what metadata
        // carries and the verification workflow passes to curl).
        if parsed.as_str().len() > MAX_URL_BYTES {
            return Err(UpstreamError::UrlTooLong {
                len: parsed.as_str().len(),
            });
        }
        if parsed.scheme() != "https" {
            return Err(UpstreamError::InsecureUrl {
                value: url.to_owned(),
            });
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(UpstreamError::UrlWithCredentials {
                value: url.to_owned(),
            });
        }
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(UpstreamError::InvalidChecksum {
                value: sha256.to_owned(),
            });
        }
        let format =
            UpstreamFormat::parse(format).ok_or_else(|| UpstreamError::UnsupportedFormat {
                value: format.to_owned(),
            })?;
        // The prefix must be a component the extractor could accept
        // on a raw archive path: the extractor's length and
        // portability checks run on the *unstripped* entry path, so
        // an overlong or Windows-hostile prefix (`con`, a trailing
        // dot) could only ever produce `upstream_archive_invalid` at
        // verification time - reject it at declaration time instead.
        // The length bound leaves room for the shortest possible
        // child (`/` plus one byte) under the archive entry-path
        // cap; a longer prefix admits no entry at all.
        if let Some(prefix) = &strip_prefix
            && (!cabin_fs::path::is_safe_single_component(prefix)
                || prefix.len() > MAX_COPY_PATH_BYTES - 2
                || cabin_fs::path::component_portability(prefix).is_some())
        {
            return Err(UpstreamError::InvalidStripPrefix {
                value: prefix.clone(),
            });
        }
        if copies.len() > MAX_COPY_STEPS {
            return Err(UpstreamError::TooManyCopies {
                count: copies.len(),
            });
        }
        // A copy's `from` must exist in the archive, where the
        // extractor caps the *unstripped* entry path - prefix, `/`,
        // and `from` together - at the archive path limit.  A
        // combination over it could only ever reject at
        // verification time.
        if let Some(prefix) = &strip_prefix {
            for step in &copies {
                if prefix.len() + 1 + step.from().as_str().len() > MAX_COPY_PATH_BYTES {
                    return Err(UpstreamError::PrefixedCopySourceTooLong {
                        prefix: prefix.clone(),
                        value: step.from().as_str().to_owned(),
                    });
                }
            }
        }
        // Copies materialize alongside the extracted tree, and the
        // packaging walk rejects case-folded collisions, so two
        // steps whose paths fold together without being
        // byte-identical are a guaranteed dead end: `to` against
        // another `to`, and `to` against another step's `from`.
        // Byte-identical spellings stay legal - duplicate `to`s mean
        // the later step deterministically wins, and an exact
        // `to == from` chain reads the previously placed file.
        for (index, step) in copies.iter().enumerate() {
            let to = step.to().as_str();
            for other in &copies[index + 1..] {
                let other_to = other.to().as_str();
                if to != other_to && to.to_lowercase() == other_to.to_lowercase() {
                    return Err(UpstreamError::CaseCollidingCopies {
                        first: to.to_owned(),
                        second: other_to.to_owned(),
                    });
                }
            }
            for (other_index, other) in copies.iter().enumerate() {
                let other_from = other.from().as_str();
                if other_index != index
                    && to != other_from
                    && to.to_lowercase() == other_from.to_lowercase()
                {
                    return Err(UpstreamError::CaseCollidingCopies {
                        first: to.to_owned(),
                        second: other_from.to_owned(),
                    });
                }
            }
        }
        Ok(Self {
            url: parsed,
            sha256: sha256.to_owned(),
            format,
            strip_prefix,
            copies,
        })
    }

    #[must_use]
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// 64-character lowercase hex digest of the pinned archive bytes.
    #[must_use]
    pub fn sha256_hex(&self) -> &str {
        &self.sha256
    }

    #[must_use]
    pub fn format(&self) -> UpstreamFormat {
        self.format
    }

    /// Single directory component stripped from every archive entry;
    /// `None` means the archive root is the source root.
    #[must_use]
    pub fn strip_prefix(&self) -> Option<&str> {
        self.strip_prefix.as_deref()
    }

    /// Declared copy steps, in declaration order (applied in order,
    /// so a later step deterministically wins a conflicting `to`).
    #[must_use]
    pub fn copies(&self) -> &[UpstreamCopy] {
        &self.copies
    }
}

/// Wire shape shared by the manifest-adjacent JSON surfaces (canonical
/// metadata, index entries).  Field order is serialization order; the
/// key names match the `[package.upstream]` manifest spelling.
#[derive(Serialize)]
struct WireUpstream<'a> {
    url: &'a str,
    sha256: &'a str,
    format: &'a str,
    #[serde(rename = "strip-prefix", skip_serializing_if = "Option::is_none")]
    strip_prefix: Option<&'a str>,
    #[serde(skip_serializing_if = "<[UpstreamCopy]>::is_empty")]
    copy: &'a [UpstreamCopy],
}

impl Serialize for UpstreamProvenance {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        WireUpstream {
            url: self.url.as_str(),
            sha256: &self.sha256,
            format: self.format.as_str(),
            strip_prefix: self.strip_prefix(),
            copy: &self.copies,
        }
        .serialize(serializer)
    }
}

/// Raw deserialization mirror.  `deny_unknown_fields` keeps unknown
/// future syntax a parse error on every read surface (index entries,
/// stored registry metadata) exactly as the manifest parser does.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUpstream {
    url: String,
    sha256: String,
    format: String,
    #[serde(default, rename = "strip-prefix")]
    strip_prefix: Option<String>,
    #[serde(default)]
    copy: Vec<RawCopy>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCopy {
    from: String,
    to: String,
}

impl<'de> Deserialize<'de> for UpstreamProvenance {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawUpstream::deserialize(deserializer)?;
        let copies = raw
            .copy
            .into_iter()
            .map(|step| UpstreamCopy::new(step.from, step.to))
            .collect::<Result<Vec<_>, _>>()
            .map_err(D::Error::custom)?;
        Self::new(&raw.url, &raw.sha256, &raw.format, raw.strip_prefix, copies)
            .map_err(D::Error::custom)
    }
}

impl<'de> Deserialize<'de> for UpstreamCopy {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawCopy::deserialize(deserializer)?;
        Self::new(raw.from, raw.to).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23";
    const URL: &str = "https://example.com/library-1.2.3.tar.gz";

    fn valid() -> UpstreamProvenance {
        UpstreamProvenance::new(URL, SHA, "tar.gz", Some("library-1.2.3".into()), Vec::new())
            .unwrap()
    }

    #[test]
    fn accepts_a_valid_declaration() {
        let upstream = valid();
        assert_eq!(upstream.url().as_str(), URL);
        assert_eq!(upstream.sha256_hex(), SHA);
        assert_eq!(upstream.format(), UpstreamFormat::TarGz);
        assert_eq!(upstream.strip_prefix(), Some("library-1.2.3"));
        assert!(upstream.copies().is_empty());
    }

    #[test]
    fn rejects_non_https_urls() {
        for url in [
            "http://example.com/lib.tar.gz",
            "ftp://example.com/lib.tar.gz",
            "file:///tmp/lib.tar.gz",
        ] {
            let err = UpstreamProvenance::new(url, SHA, "tar.gz", None, Vec::new()).unwrap_err();
            assert!(
                matches!(err, UpstreamError::InsecureUrl { .. }),
                "{url}: {err:?}"
            );
        }
    }

    #[test]
    fn rejects_urls_with_credentials() {
        for url in [
            "https://user@example.com/lib.tar.gz",
            "https://user:secret@example.com/lib.tar.gz",
        ] {
            let err = UpstreamProvenance::new(url, SHA, "tar.gz", None, Vec::new()).unwrap_err();
            assert!(
                matches!(err, UpstreamError::UrlWithCredentials { .. }),
                "{url}: {err:?}"
            );
        }
    }

    #[test]
    fn rejects_unparsable_url() {
        let err =
            UpstreamProvenance::new("::not a url::", SHA, "tar.gz", None, Vec::new()).unwrap_err();
        assert!(matches!(err, UpstreamError::InvalidUrl { .. }), "{err:?}");
    }

    #[test]
    fn rejects_bad_checksums() {
        for sha in [
            "deadbeef",
            &SHA.to_uppercase(),
            &format!("g{}", &SHA[1..]),
            "",
        ] {
            let err = UpstreamProvenance::new(URL, sha, "tar.gz", None, Vec::new()).unwrap_err();
            assert!(
                matches!(err, UpstreamError::InvalidChecksum { .. }),
                "{sha:?}: {err:?}"
            );
        }
    }

    #[test]
    fn rejects_unsupported_formats() {
        for format in ["tar.xz", "tar.bz2", "7z", "rar", ""] {
            let err = UpstreamProvenance::new(URL, SHA, format, None, Vec::new()).unwrap_err();
            assert!(
                matches!(err, UpstreamError::UnsupportedFormat { .. }),
                "{format:?}: {err:?}"
            );
        }
    }

    #[test]
    fn accepts_zip_format() {
        let upstream = UpstreamProvenance::new(URL, SHA, "zip", None, Vec::new()).unwrap();
        assert_eq!(upstream.format(), UpstreamFormat::Zip);
    }

    #[test]
    fn rejects_bad_strip_prefixes() {
        for prefix in [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            // Shapes the extractor's raw-path checks could never
            // accept: a component too long to admit any child entry
            // under the 256-byte raw-path cap, a device name, a
            // trailing dot.
            &"a".repeat(255),
            "con",
            "lib.",
        ] {
            let err = UpstreamProvenance::new(URL, SHA, "tar.gz", Some(prefix.into()), Vec::new())
                .unwrap_err();
            assert!(
                matches!(err, UpstreamError::InvalidStripPrefix { .. }),
                "{prefix:?}: {err:?}"
            );
        }
    }

    #[test]
    fn rejects_unsafe_copy_paths() {
        for (from, to, field) in [
            ("../escape", "conf.h", "from"),
            ("/etc/passwd", "conf.h", "from"),
            ("", "conf.h", "from"),
            // Non-canonical aliases and platform-hostile shapes: the
            // path is a comparison key against published archive
            // entries, which are canonical forward-slash portable
            // paths, so these could never verify.
            ("./scripts/conf.prebuilt", "conf.h", "from"),
            (".", "conf.h", "from"),
            ("a\\b", "conf.h", "from"),
            ("a//b", "conf.h", "from"),
            ("scripts/conf.prebuilt", "../escape", "to"),
            ("scripts/conf.prebuilt", "/abs", "to"),
            ("scripts/conf.prebuilt", "", "to"),
            ("scripts/conf.prebuilt", "conf.h.", "to"),
            ("scripts/conf.prebuilt", "con", "to"),
        ] {
            let err = UpstreamCopy::new(from.into(), to.into()).unwrap_err();
            assert!(
                matches!(err, UpstreamError::UnsafeCopyPath { field: f, .. } if f == field),
                "{from:?} -> {to:?}: {err:?}"
            );
        }
    }

    #[test]
    fn rejects_overlong_copy_paths() {
        // A 256-byte component exceeds `NAME_MAX`; a >256-byte path
        // exceeds the archive entry-path cap.  Both could only ever
        // fail at verification time, so they are parse errors.
        let long_component = "a".repeat(256);
        let long_path = format!("{}/{}", "d".repeat(200), "f".repeat(60));
        for path in [long_component, long_path] {
            let err = UpstreamCopy::new(path.clone(), "conf.h".into()).unwrap_err();
            assert!(
                matches!(err, UpstreamError::UnsafeCopyPath { field: "from", .. }),
                "{path:?}: {err:?}"
            );
        }
        // The bounds themselves are inclusive.
        UpstreamCopy::new("a".repeat(255), "conf.h".into()).unwrap();
    }

    #[test]
    fn rejects_overlong_urls() {
        let url = format!("https://example.com/{}", "a".repeat(MAX_URL_BYTES));
        let err = UpstreamProvenance::new(&url, SHA, "tar.gz", None, Vec::new()).unwrap_err();
        assert!(matches!(err, UpstreamError::UrlTooLong { .. }), "{err:?}");
    }

    #[test]
    fn rejects_copy_source_overlong_under_the_strip_prefix() {
        // 200-byte prefix + `/` + 60-byte `from` = 261 bytes of raw
        // archive path: over the extractor's entry-path cap even
        // though each part is individually valid.
        let copy = UpstreamCopy::new("f".repeat(60), "conf.h".into()).unwrap();
        let err = UpstreamProvenance::new(
            URL,
            SHA,
            "tar.gz",
            Some("p".repeat(200)),
            vec![copy.clone()],
        )
        .unwrap_err();
        assert!(
            matches!(err, UpstreamError::PrefixedCopySourceTooLong { .. }),
            "{err:?}"
        );
        // Without the prefix the same step is fine.
        UpstreamProvenance::new(URL, SHA, "tar.gz", None, vec![copy]).unwrap();
    }

    #[test]
    fn rejects_self_referential_copies() {
        // Exact and case-folded pairs: the latter aliases on a
        // case-insensitive filesystem and materializes a case
        // conflict on a case-sensitive one - never verifiable.
        for (from, to) in [("config.h", "config.h"), ("README", "readme")] {
            let err = UpstreamCopy::new(from.into(), to.into()).unwrap_err();
            assert!(
                matches!(err, UpstreamError::SelfReferentialCopy { .. }),
                "{from:?} -> {to:?}: {err:?}"
            );
        }
    }

    #[test]
    fn rejects_case_colliding_copy_steps() {
        // `to` vs `to`, and `to` vs another step's `from`: both
        // materialize a guaranteed case conflict on the verifier.
        let cases = [
            (("a", "README"), ("b", "readme")),
            (("a", "Scripts/x"), ("scripts/X", "b")),
        ];
        for ((from1, to1), (from2, to2)) in cases {
            let copies = vec![
                UpstreamCopy::new(from1.into(), to1.into()).unwrap(),
                UpstreamCopy::new(from2.into(), to2.into()).unwrap(),
            ];
            let err = UpstreamProvenance::new(URL, SHA, "tar.gz", None, copies).unwrap_err();
            assert!(
                matches!(err, UpstreamError::CaseCollidingCopies { .. }),
                "{from1}->{to1} + {from2}->{to2}: {err:?}"
            );
        }
        // Byte-identical spellings stay legal: duplicate `to`s are
        // last-wins, and an exact `to == from` chain is a valid
        // read-after-place sequence.
        let copies = vec![
            UpstreamCopy::new("a".into(), "config.h".into()).unwrap(),
            UpstreamCopy::new("b".into(), "config.h".into()).unwrap(),
            UpstreamCopy::new("config.h".into(), "other.h".into()).unwrap(),
        ];
        UpstreamProvenance::new(URL, SHA, "tar.gz", None, copies).unwrap();
    }

    #[test]
    fn rejects_too_many_copies() {
        let copies: Vec<UpstreamCopy> = (0..=MAX_COPY_STEPS)
            .map(|i| UpstreamCopy::new(format!("src/{i}.h"), format!("{i}.h")).unwrap())
            .collect();
        let err = UpstreamProvenance::new(URL, SHA, "tar.gz", None, copies).unwrap_err();
        assert!(
            matches!(err, UpstreamError::TooManyCopies { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn serializes_in_manifest_key_spelling_and_omits_absent_fields() {
        let full = UpstreamProvenance::new(
            URL,
            SHA,
            "tar.gz",
            Some("library-1.2.3".into()),
            vec![UpstreamCopy::new("scripts/config.h.prebuilt".into(), "config.h".into()).unwrap()],
        )
        .unwrap();
        let value = serde_json::to_value(&full).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "url": URL,
                "sha256": SHA,
                "format": "tar.gz",
                "strip-prefix": "library-1.2.3",
                "copy": [{"from": "scripts/config.h.prebuilt", "to": "config.h"}],
            })
        );

        let minimal = UpstreamProvenance::new(URL, SHA, "zip", None, Vec::new()).unwrap();
        let value = serde_json::to_value(&minimal).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"url": URL, "sha256": SHA, "format": "zip"})
        );
    }

    #[test]
    fn deserialization_round_trips_and_validates() {
        let full = UpstreamProvenance::new(
            URL,
            SHA,
            "tar.gz",
            Some("library-1.2.3".into()),
            vec![UpstreamCopy::new("a".into(), "b".into()).unwrap()],
        )
        .unwrap();
        let json = serde_json::to_string(&full).unwrap();
        let back: UpstreamProvenance = serde_json::from_str(&json).unwrap();
        assert_eq!(back, full);

        // Validation runs on deserialization too: a stored document
        // with a non-HTTPS URL or an escaping copy path never
        // produces a value.
        let bad_url = json.replace("https://", "http://");
        assert!(serde_json::from_str::<UpstreamProvenance>(&bad_url).is_err());
        let bad_copy = json.replace("\"from\":\"a\"", "\"from\":\"../a\"");
        assert!(serde_json::from_str::<UpstreamProvenance>(&bad_copy).is_err());
    }

    #[test]
    fn deserialization_rejects_unknown_fields() {
        let err = serde_json::from_value::<UpstreamProvenance>(serde_json::json!({
            "url": URL, "sha256": SHA, "format": "zip", "mirror": "https://x.example"
        }))
        .unwrap_err();
        assert!(err.to_string().contains("unknown field"), "{err}");
    }
}
