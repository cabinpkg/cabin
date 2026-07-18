//! Hostile-archive inspection for the registry's verification
//! lifecycle (`docs/remote-registry.md`, "Verification lifecycle").
//!
//! The hosted registry stores every newly published version as
//! `pending`; an external verifier lists pending versions through the
//! admin API (scope `verify`), downloads each archive plus the
//! canonical metadata the registry stored at publish, inspects the
//! archive, and renders a `verified` / `rejected` verdict.  This
//! crate is that verifier: [`inspect`] runs the checks and the
//! `cabin-registry-verify` binary wraps it for the GitHub Actions
//! workflow (`.github/workflows/registry-verify.yml`).  The crate is
//! a client of the registry service and never appears in the `cabin`
//! binary's dependency graph.
//!
//! The inspector assumes the archive is hostile: it never extracts
//! to disk, reads the container into memory once (bounded by the
//! registry's publish size limit) and hand-parses it, decompressing
//! every entry through a capped reader so the bomb caps hold no
//! matter what the deflate layer does.  It bounds every dimension of
//! decompression (total bytes, entry count, path length) with the
//! caps in [`Limits`] so a crafted archive aborts with a rejection
//! reason instead of exhausting the runner.  Checks run in order:
//!
//! 1. structure and size discipline over the strict zip container
//!    (`registry/docs/archive-format.md`): a fixed-offset EOCD, a
//!    contiguously tiled central directory and local records, no
//!    zip64/descriptors/extra fields, methods restricted to
//!    store/deflate, local headers matching central, declared
//!    sizes/CRCs matching the decompressed bytes, safe portable
//!    relative paths, regular files only, and the ratio/absolute/
//!    entry-count/path-length caps;
//! 2. consistency: the embedded manifest, parsed with the real
//!    manifest parser, must agree with the canonical metadata the
//!    registry stored, and the archive bytes must hash to the
//!    checksum the registry recorded.
//!
//! Failures caused by the archive bytes are verdicts
//! ([`Verdict::Rejected`] with a machine-readable [`Reason`]);
//! failures caused by the environment (unreadable files, metadata
//! that is not the shape the registry stores) are [`VerifyError`]s,
//! which the caller must treat as "leave the version pending".

use std::fmt;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

mod consistency;
mod limits;
mod scan;

pub use limits::{Limits, LimitsError, limits_from_env};

/// One entry of the admin listing
/// (`GET /api/v1/admin/versions?status=pending`), as the registry
/// serves it.  Tolerant of extra fields so the verifier keeps
/// working when the listing grows.
#[derive(Debug, Clone, Deserialize)]
pub struct PendingVersion {
    pub name: String,
    pub version: String,
    /// Raw lowercase SHA-256 hex of the archive bytes (no `sha256:`
    /// prefix) - the `versions.checksum` column, echoed back to bind
    /// the verdict.
    pub checksum: String,
    /// The row generation the listing reported, echoed back to bind
    /// the verdict.
    pub published_at: String,
    /// The canonical per-version metadata document stored verbatim
    /// at publish.
    pub metadata: serde_json::Value,
}

/// The verifier's verdict on one pending version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Verified,
    /// Rejected, with the machine-readable reason codes (the first
    /// failing check short-circuits, so today this carries exactly
    /// one code; the shape leaves room for collecting more).
    Rejected(Vec<Reason>),
}

/// Machine-readable rejection reason codes.  Snake-case code strings
/// are a public contract: they land in the registry's
/// `verification_reason` column and in
/// `docs/remote-registry.md`.
///
/// A recorded reason is the [`code`](Reason::code) optionally
/// followed by one parenthesized fixed detail that narrows the cause
/// (`invalid_path (trailing dot)`, `unsupported_zip_feature (zip64)`,
/// `header_mismatch (crc)`); [`Display`](fmt::Display) renders that
/// full string, while `code` stays the machine prefix.  Detail texts
/// are short, lower-case, and never echo archive bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The running decompressed total crossed the cap (see
    /// [`Limits`] for the formula).
    DecompressedTooLarge,
    /// More zip entries than `max_entries`.
    TooManyEntries,
    /// An entry path longer than `max_path_len` bytes.
    PathTooLong,
    /// A non-regular entry: a non-regular Unix type in the external
    /// attributes (symlink, device, ...) or the DOS directory
    /// attribute.
    ForbiddenEntryType,
    /// An absolute entry path (POSIX or Windows-drive form).
    AbsolutePath,
    /// An entry path with a `..` component.
    PathTraversal,
    /// An entry name that is empty, not UTF-8, contains `\`, has an
    /// empty or `.` component, is a directory marker, or violates the
    /// shared portability set.  The optional detail names the violated
    /// portability rule (`trailing dot`, `colon`, ...).
    InvalidPath(Option<&'static str>),
    /// The same name (raw bytes) appears twice.
    DuplicatePath,
    /// Two names collide under Unicode default lowercasing on a
    /// case-insensitive filesystem, including a file used as a
    /// case-folded parent directory (`a` vs `A/b`).
    CaseConflict,
    /// A banned zip feature.  The detail names it: `method`,
    /// `gp flag`, `data descriptor`, `extra field`, `comment`, or
    /// `zip64`.
    UnsupportedZipFeature(&'static str),
    /// A local header disagrees with its central header, a stored
    /// entry's compressed size differs from its uncompressed size, a
    /// deflated entry does not cleanly consume its compressed span,
    /// or a declared size/CRC disagrees with the decompressed bytes.
    /// The detail names which: `local header`, `size`, `deflate`, or
    /// `crc`.
    HeaderMismatch(&'static str),
    /// A regular file is used as another entry's parent directory
    /// (e.g. a file `src` alongside `src/main.cc`): no extractor can
    /// materialize both.
    PathConflict,
    /// The manifest declares a target source that is not present in
    /// the archive - the package would extract but fail to build.
    MissingSource,
    /// No `cabin.toml` at the archive root.
    ManifestMissing,
    /// The embedded manifest does not parse as a publishable single
    /// package.
    ManifestInvalid,
    /// The manifest's package name disagrees with the canonical
    /// metadata or the listing row.
    NameMismatch,
    /// The manifest's version disagrees with the canonical metadata
    /// or the listing row.
    VersionMismatch,
    /// The manifest's dependency tables disagree with the canonical
    /// metadata.
    DependencyMismatch,
    /// The manifest's language-standard fields (package-level
    /// settings or the derived per-target `standards` table)
    /// disagree with the canonical metadata.
    LanguageStandardMismatch,
    /// The archive bytes do not hash to the checksum the registry
    /// recorded.
    ChecksumMismatch,
    /// Any other canonical-metadata field (schema, features,
    /// profiles, toolchain, build, compiler wrapper, yanked flag,
    /// source block) disagrees with what the manifest derives.
    MetadataMismatch,
    /// The bytes are not a well-formed zip container in the strict
    /// profile: a bad or misplaced EOCD, a non-contiguous layout, or
    /// bytes outside the tiled regions.
    ArchiveInvalid,
}

impl Reason {
    /// The stable snake-case code string for this reason: the
    /// machine-readable prefix, without any detail (see
    /// [`Display`](fmt::Display) for the full reason string).
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Reason::DecompressedTooLarge => "decompressed_too_large",
            Reason::TooManyEntries => "too_many_entries",
            Reason::PathTooLong => "path_too_long",
            Reason::ForbiddenEntryType => "forbidden_entry_type",
            Reason::AbsolutePath => "absolute_path",
            Reason::PathTraversal => "path_traversal",
            Reason::InvalidPath(_) => "invalid_path",
            Reason::DuplicatePath => "duplicate_path",
            Reason::CaseConflict => "case_conflict",
            Reason::UnsupportedZipFeature(_) => "unsupported_zip_feature",
            Reason::HeaderMismatch(_) => "header_mismatch",
            Reason::PathConflict => "path_conflict",
            Reason::MissingSource => "missing_source",
            Reason::ManifestMissing => "manifest_missing",
            Reason::ManifestInvalid => "manifest_invalid",
            Reason::NameMismatch => "name_mismatch",
            Reason::VersionMismatch => "version_mismatch",
            Reason::DependencyMismatch => "dependency_mismatch",
            Reason::LanguageStandardMismatch => "language_standard_mismatch",
            Reason::ChecksumMismatch => "checksum_mismatch",
            Reason::MetadataMismatch => "metadata_mismatch",
            Reason::ArchiveInvalid => "archive_invalid",
        }
    }

    /// The fixed detail that narrows this reason, when it carries one.
    fn detail(self) -> Option<&'static str> {
        match self {
            Reason::InvalidPath(detail) => detail,
            Reason::UnsupportedZipFeature(detail) | Reason::HeaderMismatch(detail) => Some(detail),
            _ => None,
        }
    }
}

impl fmt::Display for Reason {
    /// The full reason string stored in `verification_reason`: the
    /// [`code`](Reason::code), optionally followed by one
    /// parenthesized detail.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.detail() {
            Some(detail) => write!(f, "{} ({detail})", self.code()),
            None => f.write_str(self.code()),
        }
    }
}

/// Operational failures: the environment, not the archive, is at
/// fault, so no verdict is rendered and the version stays pending
/// (fail safe).
#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The listing's metadata document is not the shape the registry
    /// stores (the registry validated it at publish, so this is an
    /// infrastructure fault, not a hostile archive).
    #[error("the canonical metadata is not the shape the registry stores: missing {0}")]
    MalformedMetadata(&'static str),
}

/// Inspect `archive` against the listing entry the registry reported
/// and render a verdict.
///
/// # Errors
///
/// Returns [`VerifyError`] for operational failures (see its
/// documentation); the caller must leave the version pending.
pub fn inspect(
    archive: &Path,
    pending: &PendingVersion,
    limits: &Limits,
) -> Result<Verdict, VerifyError> {
    let (manifest, files) = match scan::scan_archive(archive, limits)? {
        scan::ScanOutcome::Manifest { bytes, files } => (bytes, files),
        scan::ScanOutcome::Reject(reason) => return Ok(Verdict::Rejected(vec![reason])),
    };

    let file = File::open(archive).map_err(|source| VerifyError::Io {
        path: archive.to_path_buf(),
        source,
    })?;
    let archive_hex = cabin_core::hash::hash_reader(file).map_err(|source| VerifyError::Io {
        path: archive.to_path_buf(),
        source,
    })?;

    match consistency::check(&manifest, &files, pending, &archive_hex)? {
        Some(reason) => Ok(Verdict::Rejected(vec![reason])),
        None => Ok(Verdict::Verified),
    }
}

#[cfg(test)]
mod tests {
    use super::Reason;

    #[test]
    fn detailless_reason_renders_as_its_code() {
        assert_eq!(Reason::PathTraversal.to_string(), "path_traversal");
        assert_eq!(Reason::InvalidPath(None).to_string(), "invalid_path");
        assert_eq!(Reason::CaseConflict.to_string(), "case_conflict");
    }

    #[test]
    fn detailed_reason_renders_code_and_parenthesized_detail() {
        assert_eq!(
            Reason::InvalidPath(Some("trailing dot")).to_string(),
            "invalid_path (trailing dot)"
        );
        assert_eq!(
            Reason::UnsupportedZipFeature("zip64").to_string(),
            "unsupported_zip_feature (zip64)"
        );
        assert_eq!(
            Reason::HeaderMismatch("crc").to_string(),
            "header_mismatch (crc)"
        );
    }

    #[test]
    fn code_stays_the_bare_machine_prefix() {
        assert_eq!(Reason::InvalidPath(Some("colon")).code(), "invalid_path");
        assert_eq!(
            Reason::UnsupportedZipFeature("method").code(),
            "unsupported_zip_feature"
        );
        assert_eq!(Reason::HeaderMismatch("deflate").code(), "header_mismatch");
    }
}
