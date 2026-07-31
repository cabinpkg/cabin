//! The upstream-provenance pass: check a `[package.upstream]`-bearing
//! version against the pinned upstream archive the workflow
//! downloaded (`docs/remote-registry.md`, "The verifier's checks").
//!
//! The consistency pass already proved the stored metadata equals
//! what the archived manifest derives, so the stored `upstream`
//! block is the declaration to enforce.  This pass:
//!
//! 1. materializes the declaration through
//!    [`cabin_artifact::materialize_upstream`] - the pipeline the
//!    ports publisher adopts as recipes collapse into
//!    provenance-bearing packages: checksum pin, hardened
//!    extraction (`strip_prefix` matching, symlinks skipped), copy
//!    steps, then the declared patches, applied byte-exactly with
//!    the patch bytes retained from the published archive - so
//!    verification stays self-contained: pinned upstream archive
//!    plus published package, nothing else;
//! 2. collects the resulting tree under `cabin-package`'s archive
//!    include / exclude policy - the same walk `cabin package` runs -
//!    and hashes each file;
//! 3. requires the published archive's entries to match the expected
//!    tree byte-for-byte, except the root `cabin.toml` (the manifest
//!    is the publisher's, never upstream's) and the declared patch
//!    files (publisher-authored inputs to the patch step, not
//!    products of the upstream transformation).
//!
//! The upstream archive is publisher-pinned but untrusted; the
//! error-channel split mirrors the crate doctrine.  Deterministic
//! materialization defects - a digest mismatch, a hostile or
//! malformed archive, a missing copy source, a patch that does not
//! apply, a diverging tree - are verdicts.  Filesystem failures
//! ([`MaterializeError::Io`], scratch-directory I/O) are
//! [`VerifyError`]s: the version stays pending.  Unlike the registry
//! archive this pass extracts to disk - the tree comparison needs
//! the exact extraction and collection semantics clients use, and
//! the hardened extractor already bounds every dimension of it.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use cabin_artifact::{MaterializeDefect, MaterializeError, PatchFetch, materialize_upstream};
use cabin_core::UpstreamProvenance;
use cabin_package::PackageError;

use crate::scan::Contents;
use crate::{Reason, VerifyError};

/// The one entry excluded from the tree comparison, on both sides:
/// the published root manifest is authored by the publisher, and an
/// upstream tree that happens to ship its own root `cabin.toml` is
/// overwritten by it during publisher materialization.
const ROOT_MANIFEST: &str = "cabin.toml";

/// Check the published archive's `files` against the pinned upstream
/// archive at `upstream_archive`.  `Ok(Some(reason))` is a
/// rejection; `Ok(None)` means the published tree is exactly the
/// declared transformation of the upstream archive.
///
/// # Errors
///
/// [`VerifyError::Io`] for filesystem failures (reading the
/// downloaded archive, the scratch directory, hashing extracted
/// files); the caller must leave the version pending.
pub(crate) fn check(
    upstream_archive: &Path,
    declared: &UpstreamProvenance,
    files: &Contents,
    patch_bytes: &BTreeMap<String, Vec<u8>>,
) -> Result<Option<Reason>, VerifyError> {
    // Checksum before any scratch work, though the shared pipeline
    // verifies it again: a mismatching archive must classify as the
    // publisher-determined defect even when the scratch directory is
    // unusable, or that corner would flip from a terminal rejection
    // to an Io retry loop that leaves the version pending forever.
    let file = fs::File::open(upstream_archive).map_err(|source| VerifyError::Io {
        path: upstream_archive.to_path_buf(),
        source,
    })?;
    let actual = cabin_core::hash::hash_reader(file).map_err(|source| VerifyError::Io {
        path: upstream_archive.to_path_buf(),
        source,
    })?;
    if actual != declared.sha256_hex() {
        return Ok(Some(Reason::UpstreamChecksumMismatch));
    }

    let tree = scratch_dir(upstream_archive);
    remove_scratch(&tree)?;
    fs::create_dir_all(&tree).map_err(|source| VerifyError::Io {
        path: tree.clone(),
        source,
    })?;
    let outcome = check_extracted(upstream_archive, declared, files, patch_bytes, &tree);
    // Best-effort cleanup; the verdict (or error) already stands.
    let _ = fs::remove_dir_all(&tree);
    outcome
}

/// Scratch directory the upstream archive extracts into: a sibling
/// of the downloaded file, so the workflow's per-version workdir
/// cleanup removes it even if this process dies mid-pass.
fn scratch_dir(upstream_archive: &Path) -> PathBuf {
    let mut name = upstream_archive
        .file_name()
        .unwrap_or_default()
        .to_os_string();
    name.push(".tree");
    upstream_archive.with_file_name(name)
}

fn remove_scratch(tree: &Path) -> Result<(), VerifyError> {
    match fs::remove_dir_all(tree) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(VerifyError::Io {
            path: tree.to_path_buf(),
            source,
        }),
    }
}

fn check_extracted(
    upstream_archive: &Path,
    declared: &UpstreamProvenance,
    files: &Contents,
    patch_bytes: &BTreeMap<String, Vec<u8>>,
    tree: &Path,
) -> Result<Option<Reason>, VerifyError> {
    // Patch bytes come from the published archive: the structure pass
    // retains a declared entry only when its declared size fits the
    // shared cap, so an entry present in `files` but absent from
    // `patch_bytes` is an oversized patch, not a missing one (the
    // tree-comparison exclusion means nothing else checks presence).
    let mut fetch_patch = |path: &camino::Utf8Path| {
        Ok(match patch_bytes.get(path.as_str()) {
            Some(bytes) => PatchFetch::Found(bytes.clone()),
            None if files.contains_key(path.as_str()) => PatchFetch::Oversized,
            None => PatchFetch::Missing,
        })
    };
    if let Err(err) = materialize_upstream(declared, upstream_archive, tree, &mut fetch_patch) {
        return match err {
            MaterializeError::Defect(defect) => Ok(Some(defect_reason(defect))),
            MaterializeError::Io { path, source } => Err(VerifyError::Io { path, source }),
        };
    }

    // The same walk `cabin package` runs, so the expected tree drops
    // exactly what packaging drops (`.git`, `build`, `cabin.lock`,
    // ...) and refuses exactly what packaging refuses.
    let collected = match cabin_package::archive::collect_package_files(tree, None) {
        Ok(collected) => collected,
        // The walk reads directories this process just wrote, so
        // I/O faults are environmental; every content refusal
        // (non-UTF-8 name, case conflict) is the archive's.
        Err(PackageError::Io { path, source }) => return Err(VerifyError::Io { path, source }),
        Err(_) => return Ok(Some(Reason::UpstreamArchiveInvalid(Some("file set")))),
    };

    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for file in collected {
        // Only the root manifest is dropped from the *expected*
        // (upstream-derived) side.  Declared patch paths are NOT
        // dropped here: a patch path that names a file the upstream
        // transformation produces was already rejected as a shadow by
        // the materializer, and excluding it on both sides would let
        // the published archive carry arbitrary bytes at that path
        // unverified.
        if file.rel_path == ROOT_MANIFEST {
            continue;
        }
        let handle = fs::File::open(&file.abs_path).map_err(|source| VerifyError::Io {
            path: file.abs_path.clone(),
            source,
        })?;
        let digest = cabin_core::hash::hash_reader(handle).map_err(|source| VerifyError::Io {
            path: file.abs_path.clone(),
            source,
        })?;
        expected.insert(file.rel_path, digest);
    }

    Ok(compare_trees(declared, &expected, files))
}

/// Map a deterministic materialization defect to its rejection
/// reason.  The detail strings ride through verbatim: the shared
/// pipeline's vocabulary is this verifier's stable reason vocabulary.
fn defect_reason(defect: MaterializeDefect) -> Reason {
    match defect {
        MaterializeDefect::ChecksumMismatch => Reason::UpstreamChecksumMismatch,
        MaterializeDefect::ArchiveInvalid(detail) => Reason::UpstreamArchiveInvalid(detail),
        MaterializeDefect::CopyInvalid(detail) => Reason::UpstreamCopyInvalid(detail),
        MaterializeDefect::PatchInvalid(detail) => Reason::UpstreamPatchInvalid(detail),
    }
}

/// Whether a *published-side* entry is exempt from the extras sweep:
/// the root manifest (the publisher's, never upstream's) and every
/// declared patch file (a publisher-authored application input the
/// published archive legitimately carries, but which the upstream
/// transformation never produces).  This exemption is published-side
/// only - the expected-side loop drops solely the root manifest, and
/// the materializer rejects any patch path that shadows an
/// upstream-produced file, so a patch entry can never launder
/// unverified bytes past the comparison.
fn excluded_from_published_sweep(declared: &UpstreamProvenance, path: &str) -> bool {
    path == ROOT_MANIFEST
        || declared
            .patches()
            .iter()
            .any(|patch| patch.as_str() == path)
}

/// Compare the expected (upstream-derived) tree against the
/// published archive's entries, ignoring the root manifest and the
/// declared patch files on the published side.  Both maps iterate in
/// sorted order and missing / diverging files are reported before
/// unexplained extras, so the reported divergence is deterministic.
fn compare_trees(
    declared: &UpstreamProvenance,
    expected: &Contents,
    published: &Contents,
) -> Option<Reason> {
    for (path, digest) in expected {
        match published.get(path) {
            // The published archive lacks a file the declared
            // upstream transformation produces.
            None => return Some(Reason::UpstreamTreeMismatch("missing file")),
            Some(actual) if actual != digest => {
                return Some(Reason::UpstreamTreeMismatch("file contents"));
            }
            Some(_) => {}
        }
    }
    for path in published.keys() {
        if !excluded_from_published_sweep(declared, path) && !expected.contains_key(path) {
            // The published archive carries a file the upstream
            // transformation does not explain.
            return Some(Reason::UpstreamTreeMismatch("extra file"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(entries: &[(&str, &str)]) -> Contents {
        entries
            .iter()
            .map(|(path, digest)| ((*path).to_owned(), (*digest).to_owned()))
            .collect()
    }

    fn declaration(patches: &[&str]) -> UpstreamProvenance {
        UpstreamProvenance::new(
            "https://upstream.invalid/lib-1.0.tar.gz",
            "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23",
            "tar.gz",
            None,
            Vec::new(),
            patches.iter().map(|p| (*p).to_owned()).collect(),
        )
        .unwrap()
    }

    fn compare(expected: &Contents, published: &Contents) -> Option<Reason> {
        compare_trees(&declaration(&[]), expected, published)
    }

    /// The checksum verdict must outrank scratch-directory failures:
    /// with a mismatching archive AND an unusable scratch path, the
    /// caller needs the terminal `UpstreamChecksumMismatch`, not an
    /// `Io` that leaves the version pending and retried forever.
    /// The scratch here is a directory whose read-only subdirectory
    /// makes `remove_dir_all` fail, so any pre-checksum scratch work
    /// would surface as `Io` and fail this test.
    #[cfg(unix)]
    #[test]
    fn a_checksum_mismatch_outranks_an_unusable_scratch() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = assert_fs::TempDir::new().unwrap();
        let archive = dir.path().join("upstream.tar.gz");
        std::fs::write(&archive, b"not the pinned bytes").unwrap();
        let scratch = scratch_dir(&archive);
        let locked = scratch.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("f"), b"x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();

        let outcome = check(
            &archive,
            &declaration(&[]),
            &map(&[]),
            &std::collections::BTreeMap::new(),
        );
        // Restore before asserting so the TempDir can drop either way.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            matches!(outcome, Ok(Some(Reason::UpstreamChecksumMismatch))),
            "{outcome:?}"
        );
    }

    /// Pin the `MaterializeDefect` → `Reason` mapping: every detail
    /// string the shared pipeline emits surfaces verbatim as the
    /// documented stable reason.  The pipeline's own tests prove
    /// each defect fires; the integration suite proves them
    /// end-to-end through real archives.
    #[test]
    fn materialize_defects_map_to_stable_reasons() {
        assert_eq!(
            defect_reason(MaterializeDefect::ChecksumMismatch),
            Reason::UpstreamChecksumMismatch
        );
        assert_eq!(
            defect_reason(MaterializeDefect::ArchiveInvalid(Some("strip prefix"))),
            Reason::UpstreamArchiveInvalid(Some("strip prefix"))
        );
        assert_eq!(
            defect_reason(MaterializeDefect::CopyInvalid("missing source")),
            Reason::UpstreamCopyInvalid("missing source")
        );
        for detail in [
            "missing file",
            "too large",
            "malformed",
            "binary",
            "unsafe path",
            "missing target",
            "target conflict",
            "context mismatch",
            "target too large",
            "work budget exceeded",
            "too many file entries",
            "shadows tree",
        ] {
            assert_eq!(
                defect_reason(MaterializeDefect::PatchInvalid(detail)),
                Reason::UpstreamPatchInvalid(detail)
            );
        }
    }

    #[test]
    fn identical_trees_match() {
        let expected = map(&[("a.c", "1"), ("src/b.h", "2")]);
        let published = map(&[("a.c", "1"), ("cabin.toml", "m"), ("src/b.h", "2")]);
        assert_eq!(compare(&expected, &published), None);
    }

    #[test]
    fn published_root_manifest_is_always_ignored() {
        // The expected side never records cabin.toml (the collector
        // skips it); the published side always has one.
        let expected = map(&[]);
        let published = map(&[("cabin.toml", "m")]);
        assert_eq!(compare(&expected, &published), None);
    }

    #[test]
    fn missing_file_reports_deterministically() {
        let expected = map(&[("a.c", "1"), ("b.c", "2")]);
        let published = map(&[("a.c", "1"), ("cabin.toml", "m")]);
        assert_eq!(
            compare(&expected, &published),
            Some(Reason::UpstreamTreeMismatch("missing file"))
        );
    }

    #[test]
    fn extra_file_reports_deterministically() {
        let expected = map(&[("a.c", "1")]);
        let published = map(&[("a.c", "1"), ("cabin.toml", "m"), ("smuggled.c", "3")]);
        assert_eq!(
            compare(&expected, &published),
            Some(Reason::UpstreamTreeMismatch("extra file"))
        );
    }

    #[test]
    fn content_divergence_reports_deterministically() {
        let expected = map(&[("a.c", "1")]);
        let published = map(&[("a.c", "tampered"), ("cabin.toml", "m")]);
        assert_eq!(
            compare(&expected, &published),
            Some(Reason::UpstreamTreeMismatch("file contents"))
        );
    }

    #[test]
    fn declared_patch_files_are_exempt_from_the_extras_sweep() {
        // The published archive ships the declared patch file; the
        // expected (upstream-derived) side never contains it.  Only
        // the declared spelling is exempt - an undeclared sibling is
        // still an extra.
        let declared = declaration(&["patches/0001-fix.patch"]);
        let expected = map(&[("a.c", "1")]);
        let published = map(&[
            ("a.c", "1"),
            ("cabin.toml", "m"),
            ("patches/0001-fix.patch", "p"),
        ]);
        assert_eq!(compare_trees(&declared, &expected, &published), None);

        let smuggled = map(&[
            ("a.c", "1"),
            ("cabin.toml", "m"),
            ("patches/0002-smuggled.patch", "p"),
        ]);
        assert_eq!(
            compare_trees(&declared, &expected, &smuggled),
            Some(Reason::UpstreamTreeMismatch("extra file"))
        );
    }

    #[test]
    fn nested_cabin_toml_is_not_exempt() {
        // Only the root manifest is excluded; a nested one must
        // match like any other file.
        let expected = map(&[("examples/demo/cabin.toml", "1")]);
        let published = map(&[("cabin.toml", "m")]);
        assert_eq!(
            compare(&expected, &published),
            Some(Reason::UpstreamTreeMismatch("missing file"))
        );
    }
}
