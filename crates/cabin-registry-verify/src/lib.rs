//! Hostile-archive inspection for the registry's verification
//! lifecycle (`docs/remote-registry.md`, "Verification lifecycle").
//!
//! The hosted registry stores every newly published version as
//! `pending`; an external verifier lists pending versions through the
//! admin API (scope `verify`), downloads each archive plus the
//! canonical metadata the registry stored at publish, inspects the
//! archive, and renders a `verified` / `rejected` verdict.  This
//! crate is that verifier: [`inspect`] runs the checks and the
//! `cabin-registry-verify` binary wraps it for `cargo registry-verify`
//! (`crates/xtask-registry-admin`), which the `registry-verify` GitHub
//! Actions workflow runs on its cron.  The crate is
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
//!    checksum the registry recorded;
//! 3. upstream provenance, when the metadata declares it: the
//!    workflow-downloaded upstream archive must hash to the pinned
//!    SHA-256, interpret safely under `cabin-artifact`'s hardened
//!    extraction (this pass extracts to a scratch directory - the
//!    never-extract rule binds the hostile *registry* archive, and
//!    the upstream bytes only reach extraction after their digest
//!    pin holds), and reproduce the published tree exactly, except
//!    the root `cabin.toml` (see `upstream`).  The binary still
//!    performs no HTTP: the workflow downloads the upstream archive
//!    (with no bearer token - the URL is publisher-controlled) and
//!    passes it as `--upstream <file>`.
//!
//! Failures caused by the archive bytes are verdicts
//! ([`Verdict::Rejected`] with a machine-readable [`Reason`]);
//! failures caused by the environment (unreadable files, metadata
//! that is not the shape the registry stores) are [`VerifyError`]s,
//! which the caller must treat as "leave the version pending".

use std::collections::BTreeSet;
use std::fmt;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

mod consistency;
mod limits;
pub mod names;
mod scan;
mod upstream;

pub use limits::{Limits, LimitsError, limits_from_env};

/// One entry of the admin listing
/// (`GET /api/v1/admin/versions?status=pending`), as the registry
/// serves it.  Tolerant of extra fields so the verifier keeps
/// working when the listing grows.
#[derive(Debug, Clone, Deserialize)]
pub struct PendingVersion {
    pub name: String,
    pub version: String,
    /// The packaging-revision id of the row: the checksum's leading
    /// [`cabin_core::registry::PACKAGING_REVISION_HEX_LEN`] hex
    /// characters, and the last segment of the artifact filename the
    /// workflow downloads.
    pub revision: String,
    /// Canonical `sha256:<64 lowercase hex>` digest of the archive
    /// bytes - the `revisions.checksum` column, echoed back verbatim
    /// to bind the verdict.  Parsed strictly at this boundary.
    pub checksum: cabin_core::Checksum,
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
    /// The metadata's `upstream` block disagrees with the archived
    /// manifest's `[package.upstream]` declaration.
    UpstreamMismatch,
    /// The downloaded upstream archive does not hash to the declared
    /// pinned SHA-256.
    UpstreamChecksumMismatch,
    /// The pinned upstream archive cannot be interpreted as
    /// declared: the hardened extractor refused it (hostile entries,
    /// bomb caps), its compressed stream would not decode
    /// (`stream`), an entry name cannot be materialized on the
    /// verifier's filesystem (`file name`), the declared strip
    /// prefix is absent (`strip prefix`), or the extracted file set
    /// violates packaging rules (`file set`).
    UpstreamArchiveInvalid(Option<&'static str>),
    /// A declared copy step cannot be applied to the extracted
    /// upstream tree: its `from` file is absent (`missing source`),
    /// or its `to` cannot name a regular file there (`destination` -
    /// the destination is a directory, or one of its ancestors is a
    /// regular file).
    UpstreamCopyInvalid(&'static str),
    /// A declared patch file cannot be applied to the assembled
    /// upstream tree.  The detail names the deterministic cause:
    /// `missing file` (the published archive lacks the declared patch
    /// entry), `too large` (the patch exceeds the per-file byte cap),
    /// `shadows tree` (the patch path also names a file the upstream
    /// transformation produces, so its bytes would go unverified),
    /// `malformed` (not a valid unified diff), `binary` (binary
    /// content in the patch), `unsafe path` (a diff header path that
    /// cannot address the tree), `missing target` (the patched file
    /// is absent), `target conflict` (the patched or created path
    /// cannot name a regular file), `target too large` (the patched
    /// file exceeds the per-file size cap), or `context mismatch`
    /// (the tree's bytes do not match the patch context exactly).
    UpstreamPatchInvalid(&'static str),
    /// The published source tree is not the declared transformation
    /// of the pinned upstream archive.  The detail names the first
    /// divergence in sorted-path order - missing and diverging files
    /// are reported before unexplained extras: `missing file` (the
    /// archive lacks a file the upstream tree produces),
    /// `file contents`, or `extra file` (the archive carries a file
    /// upstream does not explain).
    UpstreamTreeMismatch(&'static str),
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
            Reason::UpstreamMismatch => "upstream_mismatch",
            Reason::UpstreamChecksumMismatch => "upstream_checksum_mismatch",
            Reason::UpstreamArchiveInvalid(_) => "upstream_archive_invalid",
            Reason::UpstreamCopyInvalid(_) => "upstream_copy_invalid",
            Reason::UpstreamPatchInvalid(_) => "upstream_patch_invalid",
            Reason::UpstreamTreeMismatch(_) => "upstream_tree_mismatch",
        }
    }

    /// The fixed detail that narrows this reason, when it carries one.
    fn detail(self) -> Option<&'static str> {
        match self {
            Reason::InvalidPath(detail) | Reason::UpstreamArchiveInvalid(detail) => detail,
            Reason::UnsupportedZipFeature(detail)
            | Reason::HeaderMismatch(detail)
            | Reason::UpstreamCopyInvalid(detail)
            | Reason::UpstreamPatchInvalid(detail)
            | Reason::UpstreamTreeMismatch(detail) => Some(detail),
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
    /// The listing entry's revision id is not its checksum's leading
    /// hex prefix.  The registry derives one from the other, so a row
    /// that disagrees with itself is corrupt registry state and no
    /// verdict could name which bytes it judged.
    #[error("listing revision {revision} is not the leading hex of checksum {checksum}")]
    RevisionMismatch { revision: String, checksum: String },
    /// The metadata declares upstream provenance but the caller
    /// supplied no downloaded upstream archive - the workflow's
    /// download step failed or is out of date.  No verdict can be
    /// rendered without the pinned bytes.
    #[error(
        "the metadata declares upstream provenance but no upstream archive was supplied; \
         pass --upstream <file>"
    )]
    MissingUpstreamArchive,
    /// An upstream archive was supplied for a version whose metadata
    /// declares no provenance - a workflow orchestration fault.
    #[error("an upstream archive was supplied but the metadata declares no upstream provenance")]
    UnexpectedUpstreamArchive,
}

/// Inspect `archive` against the listing entry the registry reported
/// and render a verdict.
///
/// `upstream_archive` is the pinned upstream archive the workflow
/// downloaded from the metadata's `upstream.url`.  It must be
/// supplied exactly when the stored metadata declares an `upstream`
/// block; a mismatch either way is an operational error, not a
/// verdict - the archive bytes were never judged.
///
/// # Errors
///
/// Returns [`VerifyError`] for operational failures (see its
/// documentation); the caller must leave the version pending.
pub fn inspect(
    archive: &Path,
    pending: &PendingVersion,
    limits: &Limits,
    upstream_archive: Option<&Path>,
) -> Result<Verdict, VerifyError> {
    // The registry mints the revision id from the checksum, so a row
    // where the two disagree is corrupt state, not a hostile archive:
    // refuse before inspecting anything (the version stays pending).
    if pending.checksum.revision_id() != pending.revision {
        return Err(VerifyError::RevisionMismatch {
            revision: pending.revision.clone(),
            checksum: pending.checksum.as_str().to_owned(),
        });
    }

    // The scan retains the bytes of every entry the stored metadata
    // declares as an upstream patch file (the upstream pass applies
    // them; nothing else in the archive is kept in memory).  The list
    // is peeked tolerantly: the declaration is only *enforced* after
    // the consistency pass proves the stored document equals what the
    // manifest derives, and a garbage `patches` value simply retains
    // nothing and rejects downstream.
    let declared_patches: BTreeSet<String> = pending
        .metadata
        .get("upstream")
        .and_then(|upstream| upstream.get("patches"))
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(serde_json::Value::as_str)
                .take(cabin_core::MAX_PATCH_FILES)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let (manifest, files, patch_bytes) =
        match scan::scan_archive(archive, limits, &declared_patches)? {
            scan::ScanOutcome::Manifest {
                bytes,
                files,
                retained,
            } => (bytes, files, retained),
            scan::ScanOutcome::Reject(reason) => return Ok(Verdict::Rejected(vec![reason])),
        };

    let file = File::open(archive).map_err(|source| VerifyError::Io {
        path: archive.to_path_buf(),
        source,
    })?;
    let archive_checksum =
        cabin_core::Checksum::of_reader(file).map_err(|source| VerifyError::Io {
            path: archive.to_path_buf(),
            source,
        })?;

    if let Some(reason) = consistency::check(&manifest, &files, pending, &archive_checksum)? {
        return Ok(Verdict::Rejected(vec![reason]));
    }

    // The consistency pass just proved the stored document equals
    // what the archived manifest derives, so the stored `upstream`
    // block (already validated at publish) is the declaration to
    // enforce against the downloaded bytes.
    let declared = pending.metadata.get("upstream");
    match (declared, upstream_archive) {
        (None, None) => Ok(Verdict::Verified),
        (Some(value), Some(path)) => {
            let declared: cabin_core::UpstreamProvenance = serde_json::from_value(value.clone())
                .map_err(|_| VerifyError::MalformedMetadata("upstream"))?;
            match upstream::check(path, &declared, &files, &patch_bytes)? {
                Some(reason) => Ok(Verdict::Rejected(vec![reason])),
                None => Ok(Verdict::Verified),
            }
        }
        (Some(_), None) => Err(VerifyError::MissingUpstreamArchive),
        (None, Some(_)) => Err(VerifyError::UnexpectedUpstreamArchive),
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
