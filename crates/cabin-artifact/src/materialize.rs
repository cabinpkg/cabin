//! The one upstream-provenance materialization pipeline: verify the
//! pinned archive's SHA-256, safely extract it (`strip-prefix`
//! applied, symlinks skipped), apply the declared copy steps, then
//! the declared patches - in exactly that order, which
//! `docs/manifest.md` pins as normative.
//!
//! Today the registry verifier replays every declaration through
//! this module.  It is the one shared pipeline for the ports
//! publisher too: every committed port is package-shaped and stages
//! through it, so a byte the publisher assembles is a byte the
//! verifier derives.  The publisher's retained recipe path
//! (`cabin-port`) remains a separate implementation, on its way
//! out.  Only what genuinely differs stays with the caller:
//! where patch bytes come from, and what happens to the assembled
//! tree afterwards.
//!
//! Error channel: defects ([`MaterializeDefect`]) are deterministic
//! consequences of the declared inputs - the pinned bytes, the
//! declaration, the patch contents - and callers may treat them as
//! terminal (the verifier maps each to a rejection reason).
//! [`MaterializeError::Io`] is environmental and must stay
//! retryable (the verifier leaves the version pending).

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cabin_core::UpstreamProvenance;
use camino::Utf8Path;

use crate::error::ArtifactError;
use crate::extract::{SafeExtractOptions, safe_extract_tar_gz, safe_extract_zip};
use crate::patch::{PatchError, PatchInput, apply_unified_patches, create_would_conflict};

/// A deterministic materialization defect: given the same archive
/// bytes, declaration, and patch contents, the same defect fires
/// again.  The `&'static str` details are stable vocabulary the
/// verifier surfaces verbatim in rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializeDefect {
    /// The archive's SHA-256 differs from the declared pin.
    ChecksumMismatch,
    /// The archive cannot be interpreted as declared: hostile or
    /// malformed entries, bomb caps, truncation (`None`), an
    /// undecodable stream (`"stream"`), an entry name the filesystem
    /// cannot materialize (`"file name"`), a root directory that does
    /// not match the declared `strip-prefix` (`"strip prefix"`), or a
    /// file set the packaging walk refuses (`"file set"`, mapped by
    /// the verifier's collection - never produced here).
    ArchiveInvalid(Option<&'static str>),
    /// A declared copy step cannot apply: `"missing source"` when
    /// `from` names no regular file, `"destination"` when `to` (or an
    /// ancestor of it) is occupied incompatibly.
    CopyInvalid(&'static str),
    /// A declared patch cannot apply; the detail is the stable
    /// vocabulary shared with the registry verifier's
    /// `UpstreamPatchInvalid` reason (`"missing file"`, `"too
    /// large"`, `"malformed"`, `"binary"`, `"unsafe path"`,
    /// `"missing target"`, `"target conflict"`, `"context
    /// mismatch"`, `"target too large"`, `"work budget exceeded"`,
    /// `"too many file entries"`, `"shadows tree"`).
    PatchInvalid(&'static str),
}

/// A materialization failure: a deterministic [`MaterializeDefect`]
/// or an environmental I/O fault.
#[derive(Debug)]
pub enum MaterializeError {
    /// Deterministic, input-caused; terminal for the given inputs.
    Defect(MaterializeDefect),
    /// Environmental; retryable.
    Io {
        /// The path the failed operation touched.
        path: PathBuf,
        /// The underlying I/O failure.
        source: io::Error,
    },
}

impl std::fmt::Display for MaterializeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Defect(defect) => {
                let (step, detail) = match defect {
                    MaterializeDefect::ChecksumMismatch => ("checksum mismatch", None),
                    MaterializeDefect::ArchiveInvalid(detail) => ("invalid archive", *detail),
                    MaterializeDefect::CopyInvalid(detail) => ("invalid copy step", Some(*detail)),
                    MaterializeDefect::PatchInvalid(detail) => ("invalid patch", Some(*detail)),
                };
                match detail {
                    Some(detail) => write!(f, "{step} ({detail})"),
                    None => f.write_str(step),
                }
            }
            Self::Io { path, source } => write!(f, "I/O failure at {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for MaterializeError {}

/// How a declared patch's bytes were resolved by the caller.
#[derive(Debug)]
pub enum PatchFetch {
    /// The declared file's bytes.
    Found(Vec<u8>),
    /// The declared file does not exist where this caller reads
    /// patches from.
    Missing,
    /// The declared file exists but exceeds the shared
    /// [`cabin_core::MAX_PATCH_BYTES`] cap (callers that stream from
    /// bounded storage report it without buffering).
    Oversized,
}

/// Materialize `provenance` from the archive at `archive` into the
/// existing directory `dest`: verify the SHA-256 pin, extract with
/// the hardened extractor (`strip-prefix` applied, symlinks
/// skipped), apply the copy steps in declaration order, then the
/// declared patches in declaration order with the bytes
/// `fetch_patch` resolves.  After application, every declared patch
/// path must still be placeable in the assembled tree - a patch path
/// that names (case/normalization-folded) a produced file is the
/// `"shadows tree"` defect, because the published archive could only
/// carry one file there.
///
/// # Errors
///
/// [`MaterializeError::Defect`] for deterministic input defects,
/// [`MaterializeError::Io`] for environmental failures.
pub fn materialize_upstream(
    provenance: &UpstreamProvenance,
    archive: &Path,
    dest: &Path,
    fetch_patch: &mut dyn FnMut(&Utf8Path) -> Result<PatchFetch, MaterializeError>,
) -> Result<(), MaterializeError> {
    let file = fs::File::open(archive).map_err(|source| MaterializeError::Io {
        path: archive.to_path_buf(),
        source,
    })?;
    let actual = cabin_core::hash::hash_reader(file).map_err(|source| MaterializeError::Io {
        path: archive.to_path_buf(),
        source,
    })?;
    if actual != provenance.sha256_hex() {
        return Err(MaterializeError::Defect(
            MaterializeDefect::ChecksumMismatch,
        ));
    }

    extract_archive(provenance, archive, dest)?;

    for step in provenance.copies() {
        let from = dest.join(step.from().as_std_path());
        let to = dest.join(step.to().as_std_path());
        if !from.is_file() {
            return Err(MaterializeError::Defect(MaterializeDefect::CopyInvalid(
                "missing source",
            )));
        }
        // Pre-flight the destination topology so a copy plan that
        // cannot apply to this tree - `to` already a directory, or an
        // ancestor of `to` occupied by a regular file - fails as the
        // deterministic defect it is instead of surfacing as an
        // environmental error.  The paths are lexically clean (no
        // `.`/`..`; validation enforced it) and the tree holds no
        // symlinks, so ancestry checks are plain component walks.
        if to.exists() && !to.is_file() {
            return Err(MaterializeError::Defect(MaterializeDefect::CopyInvalid(
                "destination",
            )));
        }
        let mut ancestor = to.parent();
        while let Some(dir) = ancestor {
            if dir == dest {
                break;
            }
            if dir.exists() && !dir.is_dir() {
                return Err(MaterializeError::Defect(MaterializeDefect::CopyInvalid(
                    "destination",
                )));
            }
            ancestor = dir.parent();
        }
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|source| MaterializeError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::copy(&from, &to).map_err(|source| MaterializeError::Io { path: to, source })?;
    }

    apply_declared_patches(provenance, dest, fetch_patch)?;

    // A declared patch file is a publisher-authored input, never a
    // product of the upstream transformation: if its path names a
    // file the assembled tree produced (under the collision fold -
    // macOS resolves lookups case- and normalization-insensitively),
    // the archive could only carry one file there, so the produced
    // bytes would go unverified.  Checked for every declared path,
    // duplicates deduplicated (the declaration validator already
    // rejects fold-equal repeats).
    let mut checked: BTreeSet<&str> = BTreeSet::new();
    for patch in provenance.patches() {
        if !checked.insert(patch.as_str()) {
            continue;
        }
        let conflicts =
            create_would_conflict(dest, patch.as_str()).map_err(|source| MaterializeError::Io {
                path: dest.join(patch.as_std_path()),
                source,
            })?;
        if conflicts {
            return Err(MaterializeError::Defect(MaterializeDefect::PatchInvalid(
                "shadows tree",
            )));
        }
    }

    Ok(())
}

fn extract_archive(
    provenance: &UpstreamProvenance,
    archive: &Path,
    dest: &Path,
) -> Result<(), MaterializeError> {
    let extract = match provenance.format() {
        cabin_core::UpstreamFormat::TarGz => safe_extract_tar_gz,
        cabin_core::UpstreamFormat::Zip => safe_extract_zip,
    };
    let extracted = extract(
        archive,
        dest,
        SafeExtractOptions {
            strip_prefix: provenance.strip_prefix(),
            // Real upstream release archives carry convenience
            // symlinks; nothing is materialized for a skipped entry,
            // so a symlink can never satisfy a published file.
            skip_symlinks: true,
        },
    );
    let Err(err) = extracted else { return Ok(()) };
    Err(match err {
        // `Io` covers real filesystem faults, a gzip/deflate stream
        // that would not decode (the extractor's copy loop maps a
        // mid-stream read failure to `Io` on the destination file),
        // and an entry name the filesystem cannot materialize (a
        // 256-byte component passes the extractor's 256-byte *path*
        // cap but exceeds `NAME_MAX`).  Split on the error kind: a
        // decode failure surfaces as `UnexpectedEof` / `InvalidData`
        // / `InvalidInput` and an unmaterializable name as
        // `InvalidFilename` - none of which a local read or write of
        // a well-named complete file can produce - and both are
        // deterministic given the pinned bytes (the digest already
        // matched), so they are defects rather than environmental
        // failures.
        ArtifactError::Io { path, source } => match source.kind() {
            io::ErrorKind::UnexpectedEof
            | io::ErrorKind::InvalidData
            | io::ErrorKind::InvalidInput => {
                MaterializeError::Defect(MaterializeDefect::ArchiveInvalid(Some("stream")))
            }
            io::ErrorKind::InvalidFilename => {
                MaterializeError::Defect(MaterializeDefect::ArchiveInvalid(Some("file name")))
            }
            _ => MaterializeError::Io { path, source },
        },
        ArtifactError::MissingStripPrefix { .. } => {
            MaterializeError::Defect(MaterializeDefect::ArchiveInvalid(Some("strip prefix")))
        }
        // Everything else the hardened extractor refuses is caused by
        // the archive bytes (hostile entries, bomb caps, truncation,
        // a corrupt stream): the pinned archive itself cannot be
        // interpreted as declared.
        _ => MaterializeError::Defect(MaterializeDefect::ArchiveInvalid(None)),
    })
}

fn apply_declared_patches(
    provenance: &UpstreamProvenance,
    dest: &Path,
    fetch_patch: &mut dyn FnMut(&Utf8Path) -> Result<PatchFetch, MaterializeError>,
) -> Result<(), MaterializeError> {
    if provenance.patches().is_empty() {
        return Ok(());
    }
    let mut resolved = Vec::with_capacity(provenance.patches().len());
    for path in provenance.patches() {
        let bytes = match fetch_patch(path)? {
            PatchFetch::Found(bytes) => bytes,
            PatchFetch::Missing => {
                return Err(MaterializeError::Defect(MaterializeDefect::PatchInvalid(
                    "missing file",
                )));
            }
            PatchFetch::Oversized => {
                return Err(MaterializeError::Defect(MaterializeDefect::PatchInvalid(
                    "too large",
                )));
            }
        };
        if bytes.len() > cabin_core::MAX_PATCH_BYTES {
            return Err(MaterializeError::Defect(MaterializeDefect::PatchInvalid(
                "too large",
            )));
        }
        resolved.push((path.as_str().to_owned(), bytes));
    }
    let inputs: Vec<PatchInput<'_>> = resolved
        .iter()
        .map(|(name, bytes)| PatchInput { name, bytes })
        .collect();
    match apply_unified_patches(dest, &inputs) {
        Ok(()) => Ok(()),
        Err(PatchError::Io { path, source }) => Err(MaterializeError::Io { path, source }),
        Err(err) => Err(MaterializeError::Defect(MaterializeDefect::PatchInvalid(
            patch_error_detail(&err),
        ))),
    }
}

/// The stable detail vocabulary for engine failures, shared verbatim
/// with the registry verifier's `UpstreamPatchInvalid` reason.
fn patch_error_detail(err: &PatchError) -> &'static str {
    match err {
        PatchError::Io { .. } => unreachable!("Io is mapped before this"),
        PatchError::Malformed { .. } => "malformed",
        PatchError::Binary { .. } => "binary",
        PatchError::UnsafePath { .. } => "unsafe path",
        PatchError::MissingTarget { .. } => "missing target",
        PatchError::TargetConflict { .. } => "target conflict",
        PatchError::ContextMismatch { .. } => "context mismatch",
        PatchError::TargetTooLarge { .. } => "target too large",
        PatchError::WorkBudgetExceeded { .. } => "work budget exceeded",
        PatchError::TooManyFileEntries { .. } => "too many file entries",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write as _;

    /// A one-file tar.gz under `prefix/`, plus its SHA-256.
    fn tar_gz(dir: &Path, prefix: &str, name: &str, body: &str) -> (PathBuf, String) {
        let path = dir.join("upstream.tar.gz");
        let file = fs::File::create(&path).unwrap();
        let mut builder = tar::Builder::new(GzEncoder::new(file, Compression::default()));
        let bytes = body.as_bytes();
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                format!("{prefix}/{name}"),
                &mut std::io::Cursor::new(bytes),
            )
            .unwrap();
        builder
            .into_inner()
            .unwrap()
            .finish()
            .unwrap()
            .flush()
            .unwrap();
        let digest = cabin_core::hash::hash_reader(fs::File::open(&path).unwrap()).unwrap();
        (path, digest)
    }

    fn tar_gz_files(dir: &Path, prefix: &str, files: &[(&str, &[u8])]) -> (PathBuf, String) {
        let path = dir.join("upstream.tar.gz");
        let file = fs::File::create(&path).unwrap();
        let mut builder = tar::Builder::new(GzEncoder::new(file, Compression::default()));
        for (name, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    format!("{prefix}/{name}"),
                    &mut std::io::Cursor::new(*bytes),
                )
                .unwrap();
        }
        builder
            .into_inner()
            .unwrap()
            .finish()
            .unwrap()
            .flush()
            .unwrap();
        let digest = cabin_core::hash::hash_reader(fs::File::open(&path).unwrap()).unwrap();
        (path, digest)
    }

    fn provenance(sha256: &str, patches: &[&str]) -> UpstreamProvenance {
        UpstreamProvenance::new(
            "https://upstream.invalid/lib-1.0.tar.gz",
            sha256,
            "tar.gz",
            Some("lib-1.0".to_owned()),
            Vec::new(),
            patches.iter().map(|p| (*p).to_owned()).collect(),
        )
        .unwrap()
    }

    /// Drive a real patch failure end to end and report its defect.
    fn defect_for_patch(patch: &[u8]) -> MaterializeDefect {
        let dir = assert_fs::TempDir::new().unwrap();
        let (archive, digest) = tar_gz(dir.path(), "lib-1.0", "t.txt", "old\n");
        let dest = dir.path().join("tree");
        fs::create_dir_all(&dest).unwrap();
        let declared = provenance(&digest, &["patches/p.patch"]);
        let bytes = patch.to_vec();
        let mut fetch = |_: &Utf8Path| Ok(PatchFetch::Found(bytes.clone()));
        match materialize_upstream(&declared, &archive, &dest, &mut fetch) {
            Err(MaterializeError::Defect(defect)) => defect,
            other => panic!("expected a defect, got {other:?}"),
        }
    }

    /// Every engine failure must surface its documented stable
    /// detail: the registry verifier renders these strings verbatim
    /// as rejection reasons, so a mistyped arm would silently change
    /// a published verdict's wire text.  Driven through the real
    /// engine, not by constructing the defect.
    #[test]
    fn engine_failures_map_to_their_documented_details() {
        for (patch, expected) in [
            (&b"this is not a diff\n"[..], "malformed"),
            (&b"Binary files a/t.txt and b/t.txt differ\n"[..], "binary"),
            (
                &b"--- a/../escape.txt\n+++ b/../escape.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n"[..],
                "unsafe path",
            ),
            (
                &b"--- a/absent.txt\n+++ b/absent.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n"[..],
                "missing target",
            ),
            (
                &b"--- a/t.txt\n+++ b/t.txt\n@@ -1,1 +1,1 @@\n-nope\n+y\n"[..],
                "context mismatch",
            ),
        ] {
            assert_eq!(
                defect_for_patch(patch),
                MaterializeDefect::PatchInvalid(expected),
                "patch: {}",
                String::from_utf8_lossy(patch)
            );
        }
    }

    /// The caller's patch source decides `missing file` vs `too
    /// large`; both are deterministic defects, never I/O errors.
    #[test]
    fn absent_and_oversized_patch_sources_are_deterministic_defects() {
        let dir = assert_fs::TempDir::new().unwrap();
        let (archive, digest) = tar_gz(dir.path(), "lib-1.0", "t.txt", "old\n");
        let declared = provenance(&digest, &["patches/p.patch"]);
        for (fetched, expected) in [
            (PatchFetch::Missing, "missing file"),
            (PatchFetch::Oversized, "too large"),
        ] {
            let dest = dir.path().join(format!("tree-{expected}"));
            fs::create_dir_all(&dest).unwrap();
            let mut once = Some(fetched);
            let mut fetch = |_: &Utf8Path| Ok(once.take().expect("one patch declared"));
            let err = materialize_upstream(&declared, &archive, &dest, &mut fetch).unwrap_err();
            assert!(
                matches!(err, MaterializeError::Defect(MaterializeDefect::PatchInvalid(d)) if d == expected),
                "{err:?}"
            );
        }
    }

    /// A checksum that does not match the pin is a defect, and
    /// nothing is extracted.
    #[test]
    fn a_checksum_mismatch_refuses_before_extraction() {
        let dir = assert_fs::TempDir::new().unwrap();
        let (archive, _) = tar_gz(dir.path(), "lib-1.0", "t.txt", "old\n");
        let dest = dir.path().join("tree");
        fs::create_dir_all(&dest).unwrap();
        let declared = provenance(&"a".repeat(64), &[]);
        let mut fetch = |_: &Utf8Path| Ok(PatchFetch::Missing);
        let err = materialize_upstream(&declared, &archive, &dest, &mut fetch).unwrap_err();
        assert!(
            matches!(
                err,
                MaterializeError::Defect(MaterializeDefect::ChecksumMismatch)
            ),
            "{err:?}"
        );
        assert!(fs::read_dir(&dest).unwrap().next().is_none());
    }

    /// A declared patch path that names a file the assembled tree
    /// produces is the `shadows tree` defect: the published archive
    /// could only carry one file there, and declared patch entries
    /// are exempt from the verifier's tree comparison.
    /// The remaining engine caps, each driven end to end: their
    /// details are rendered verbatim as rejection reasons too, and
    /// none of them may reclassify as an environment error.
    #[test]
    fn engine_caps_map_to_their_documented_details() {
        // Two entries in one patch whose targets collide under the
        // fold: "target conflict".
        let dir = assert_fs::TempDir::new().unwrap();
        let (archive, digest) = tar_gz_files(
            dir.path(),
            "lib-1.0",
            &[("t.txt", b"old\n"), ("u.txt", b"old\n")],
        );
        let dest = dir.path().join("tree");
        fs::create_dir_all(&dest).unwrap();
        let declared = provenance(&digest, &["patches/p.patch"]);
        let patch = b"--- /dev/null\n+++ b/T.TXT\n@@ -0,0 +1,1 @@\n+new\n".to_vec();
        let mut fetch = |_: &Utf8Path| Ok(PatchFetch::Found(patch.clone()));
        let err = materialize_upstream(&declared, &archive, &dest, &mut fetch).unwrap_err();
        assert!(
            matches!(
                err,
                MaterializeError::Defect(MaterializeDefect::PatchInvalid("target conflict"))
            ),
            "{err:?}"
        );

        // A target one byte over the per-target cap: "target too
        // large".  16 MiB + 1 of zeros stays under the extractor's
        // 64 MiB ratio floor, so the ratio cap never engages.
        let dir = assert_fs::TempDir::new().unwrap();
        let big = vec![b'\n'; usize::try_from(crate::patch::MAX_PATCH_TARGET_BYTES + 1).unwrap()];
        let (archive, digest) = tar_gz_files(dir.path(), "lib-1.0", &[("big.txt", &big)]);
        let dest = dir.path().join("tree");
        fs::create_dir_all(&dest).unwrap();
        let declared = provenance(&digest, &["patches/p.patch"]);
        let patch = b"--- a/big.txt\n+++ b/big.txt\n@@ -1,1 +1,1 @@\n-\n+x\n".to_vec();
        let mut fetch = |_: &Utf8Path| Ok(PatchFetch::Found(patch.clone()));
        let err = materialize_upstream(&declared, &archive, &dest, &mut fetch).unwrap_err();
        assert!(
            matches!(
                err,
                MaterializeError::Defect(MaterializeDefect::PatchInvalid("target too large"))
            ),
            "{err:?}"
        );

        // The work-budget cap alone cannot be driven end to end
        // affordably (crossing 128 MiB of rewrites needs a fixture
        // past the 64 MiB ratio floor, i.e. ~150 MiB incompressible
        // per run); the engine's own tests cover it firing, and the
        // detail mapping it routes through is pinned directly.
        assert_eq!(
            patch_error_detail(&PatchError::WorkBudgetExceeded { total: 2, limit: 1 }),
            "work budget exceeded"
        );

        // One over the declared-entry cap, all in one patch: "too
        // many file entries" (counted before anything applies).
        let dir = assert_fs::TempDir::new().unwrap();
        let (archive, digest) = tar_gz_files(dir.path(), "lib-1.0", &[("t.txt", b"old\n")]);
        let dest = dir.path().join("tree");
        fs::create_dir_all(&dest).unwrap();
        let declared = provenance(&digest, &["patches/p.patch"]);
        let mut many = String::new();
        for i in 0..=crate::patch::MAX_PATCH_FILE_ENTRIES {
            use std::fmt::Write as _;
            writeln!(many, "--- /dev/null\n+++ b/f{i}.txt\n@@ -0,0 +1,1 @@\n+x").unwrap();
        }
        let patch = many.into_bytes();
        let mut fetch = |_: &Utf8Path| Ok(PatchFetch::Found(patch.clone()));
        let err = materialize_upstream(&declared, &archive, &dest, &mut fetch).unwrap_err();
        assert!(
            matches!(
                err,
                MaterializeError::Defect(MaterializeDefect::PatchInvalid("too many file entries"))
            ),
            "{err:?}"
        );
    }

    /// `Found` bytes over the per-patch cap are the same defect as a
    /// caller-declared `Oversized`: the cap must hold whichever side
    /// enforces it.
    #[test]
    fn oversized_fetched_patch_bytes_are_refused() {
        let dir = assert_fs::TempDir::new().unwrap();
        let (archive, digest) = tar_gz(dir.path(), "lib-1.0", "t.txt", "old\n");
        let dest = dir.path().join("tree");
        fs::create_dir_all(&dest).unwrap();
        let declared = provenance(&digest, &["patches/p.patch"]);
        let huge = vec![b' '; cabin_core::MAX_PATCH_BYTES + 1];
        let mut fetch = |_: &Utf8Path| Ok(PatchFetch::Found(huge.clone()));
        let err = materialize_upstream(&declared, &archive, &dest, &mut fetch).unwrap_err();
        assert!(
            matches!(
                err,
                MaterializeError::Defect(MaterializeDefect::PatchInvalid("too large"))
            ),
            "{err:?}"
        );
    }

    /// The shadow check runs under the collision fold - `README`
    /// shadows an upstream `Readme`, because macOS and Windows
    /// resolve the two to one file, so the published archive could
    /// not carry both and the upstream bytes would go unverified.
    /// Deliberately folded where the pre-refactor verifier compared
    /// exact paths: the same input was still rejected there, but
    /// only later, by the published-archive scan as a `CaseConflict`
    /// - this classifies it at its cause, the declared patch path.
    #[test]
    fn a_fold_shadowing_patch_path_is_refused() {
        let dir = assert_fs::TempDir::new().unwrap();
        let (archive, digest) = tar_gz(dir.path(), "lib-1.0", "Readme", "upstream\n");
        let dest = dir.path().join("tree");
        fs::create_dir_all(&dest).unwrap();
        let declared = provenance(&digest, &["README"]);
        let patch = b"--- /dev/null\n+++ b/other.txt\n@@ -0,0 +1,1 @@\n+x\n".to_vec();
        let mut fetch = |_: &Utf8Path| Ok(PatchFetch::Found(patch.clone()));
        let err = materialize_upstream(&declared, &archive, &dest, &mut fetch).unwrap_err();
        assert!(
            matches!(
                err,
                MaterializeError::Defect(MaterializeDefect::PatchInvalid("shadows tree"))
            ),
            "{err:?}"
        );
    }

    #[test]
    fn a_patch_path_shadowing_the_assembled_tree_is_refused() {
        let dir = assert_fs::TempDir::new().unwrap();
        let (archive, digest) = tar_gz(dir.path(), "lib-1.0", "shadow.txt", "upstream\n");
        let dest = dir.path().join("tree");
        fs::create_dir_all(&dest).unwrap();
        let declared = provenance(&digest, &["shadow.txt"]);
        let patch =
            b"--- a/shadow.txt\n+++ b/shadow.txt\n@@ -1,1 +1,1 @@\n-upstream\n+patched\n".to_vec();
        let mut fetch = |_: &Utf8Path| Ok(PatchFetch::Found(patch.clone()));
        let err = materialize_upstream(&declared, &archive, &dest, &mut fetch).unwrap_err();
        assert!(
            matches!(
                err,
                MaterializeError::Defect(MaterializeDefect::PatchInvalid("shadows tree"))
            ),
            "{err:?}"
        );
    }
}
