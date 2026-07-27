//! The upstream-provenance pass: check a `[package.upstream]`-bearing
//! version against the pinned upstream archive the workflow
//! downloaded (`docs/remote-registry.md`, "The verifier's checks").
//!
//! The consistency pass already proved the stored metadata equals
//! what the archived manifest derives, so the stored `upstream`
//! block is the declaration to enforce.  This pass:
//!
//! 1. hashes the downloaded upstream file and requires the pinned
//!    SHA-256 - the workflow's `curl` is untrusted transport, the
//!    digest is the integrity boundary;
//! 2. safely interprets the archive with `cabin-artifact`'s hardened
//!    extractors (the exact code path foundation ports use: bomb
//!    caps, lexical path safety, `strip_prefix` matching, symlinks
//!    skipped) into a scratch directory next to the downloaded file;
//! 3. applies the declared copy steps in declaration order;
//! 4. collects the resulting tree under `cabin-package`'s archive
//!    include / exclude policy - the same walk `cabin package` runs -
//!    and hashes each file;
//! 5. requires the published archive's entries to match the expected
//!    tree byte-for-byte, except the root `cabin.toml` (the manifest
//!    is the publisher's, never upstream's).
//!
//! The upstream archive is publisher-pinned but untrusted; the
//! error-channel split mirrors the crate doctrine.  Archive-caused
//! failures - a digest mismatch, a hostile or malformed archive, a
//! missing copy source, a diverging tree - are verdicts.  Filesystem
//! failures (`ArtifactError::Io`, scratch-directory I/O) are
//! [`VerifyError`]s: the version stays pending.  Unlike the registry
//! archive this pass extracts to disk - the tree comparison needs
//! the exact extraction and collection semantics clients use, and
//! the hardened extractor already bounds every dimension of it.

use std::fs;
use std::path::{Path, PathBuf};

use cabin_artifact::{ArtifactError, SafeExtractOptions, safe_extract_tar_gz, safe_extract_zip};
use cabin_core::{UpstreamFormat, UpstreamProvenance};
use cabin_package::PackageError;

use crate::scan::Contents;
use crate::{Reason, VerifyError};

/// The one entry excluded from the tree comparison, on both sides:
/// the published root manifest is authored by the publisher, and an
/// upstream tree that happens to ship its own root `cabin.toml` is
/// overwritten by it during client materialization.
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
) -> Result<Option<Reason>, VerifyError> {
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
    let outcome = check_extracted(upstream_archive, declared, files, &tree);
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
    tree: &Path,
) -> Result<Option<Reason>, VerifyError> {
    let extract = match declared.format() {
        UpstreamFormat::TarGz => safe_extract_tar_gz,
        UpstreamFormat::Zip => safe_extract_zip,
    };
    let extracted = extract(
        upstream_archive,
        tree,
        SafeExtractOptions {
            strip_prefix: declared.strip_prefix(),
            // Real upstream release archives carry convenience
            // symlinks; skip them exactly as foundation-port
            // preparation does.  Nothing is materialized for a
            // skipped entry, so a symlink can never satisfy a
            // published file - the tree comparison still holds.
            skip_symlinks: true,
        },
    );
    if let Err(err) = extracted {
        return match err {
            // `Io` covers real filesystem faults, a gzip/deflate
            // stream that would not decode (the extractor's copy loop
            // maps a mid-stream read failure to `Io` on the
            // destination file), and an entry name the filesystem
            // cannot materialize (a 256-byte component passes the
            // extractor's 256-byte *path* cap but exceeds `NAME_MAX`).
            // Split on the error kind: a decode failure surfaces as
            // `UnexpectedEof` / `InvalidData` / `InvalidInput` and an
            // unmaterializable name as `InvalidFilename` - none of
            // which a local read or write of a well-named complete
            // file can produce - and both are deterministic given the
            // pinned bytes (the digest already matched), so they must
            // reject rather than leave the version pending forever.
            ArtifactError::Io { path, source } => match source.kind() {
                std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::InvalidData
                | std::io::ErrorKind::InvalidInput => {
                    Ok(Some(Reason::UpstreamArchiveInvalid(Some("stream"))))
                }
                std::io::ErrorKind::InvalidFilename => {
                    Ok(Some(Reason::UpstreamArchiveInvalid(Some("file name"))))
                }
                _ => Err(VerifyError::Io { path, source }),
            },
            ArtifactError::MissingStripPrefix { .. } => {
                Ok(Some(Reason::UpstreamArchiveInvalid(Some("strip prefix"))))
            }
            // Everything else the hardened extractor refuses is
            // caused by the archive bytes (hostile entries, bomb
            // caps, truncation, a corrupt stream): the pinned
            // archive itself cannot be interpreted as declared.
            _ => Ok(Some(Reason::UpstreamArchiveInvalid(None))),
        };
    }

    for step in declared.copies() {
        let from = tree.join(step.from().as_std_path());
        let to = tree.join(step.to().as_std_path());
        if !from.is_file() {
            return Ok(Some(Reason::UpstreamCopyInvalid("missing source")));
        }
        // Pre-flight the destination topology so a copy plan that
        // cannot apply to this tree - `to` already a directory, or an
        // ancestor of `to` occupied by a regular file - rejects
        // deterministically instead of surfacing as a filesystem
        // error the caller would treat as "leave pending forever".
        // The paths are lexically clean (no `.`/`..`; the parse layer
        // enforced it) and the tree holds no symlinks, so ancestry
        // checks are plain component walks.
        if to.exists() && !to.is_file() {
            return Ok(Some(Reason::UpstreamCopyInvalid("destination")));
        }
        let mut ancestor = to.parent();
        while let Some(dir) = ancestor {
            if dir == tree {
                break;
            }
            if dir.exists() && !dir.is_dir() {
                return Ok(Some(Reason::UpstreamCopyInvalid("destination")));
            }
            ancestor = dir.parent();
        }
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|source| VerifyError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::copy(&from, &to).map_err(|source| VerifyError::Io { path: to, source })?;
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

    let mut expected: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for file in collected {
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

    Ok(compare_trees(&expected, files))
}

/// Compare the expected (upstream-derived) tree against the
/// published archive's entries, ignoring the root manifest on the
/// published side.  Both maps iterate in sorted order and missing /
/// diverging files are reported before unexplained extras, so the
/// reported divergence is deterministic.
fn compare_trees(expected: &Contents, published: &Contents) -> Option<Reason> {
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
        if path != ROOT_MANIFEST && !expected.contains_key(path) {
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

    #[test]
    fn identical_trees_match() {
        let expected = map(&[("a.c", "1"), ("src/b.h", "2")]);
        let published = map(&[("a.c", "1"), ("cabin.toml", "m"), ("src/b.h", "2")]);
        assert_eq!(compare_trees(&expected, &published), None);
    }

    #[test]
    fn published_root_manifest_is_always_ignored() {
        // The expected side never records cabin.toml (the collector
        // skips it); the published side always has one.
        let expected = map(&[]);
        let published = map(&[("cabin.toml", "m")]);
        assert_eq!(compare_trees(&expected, &published), None);
    }

    #[test]
    fn missing_file_reports_deterministically() {
        let expected = map(&[("a.c", "1"), ("b.c", "2")]);
        let published = map(&[("a.c", "1"), ("cabin.toml", "m")]);
        assert_eq!(
            compare_trees(&expected, &published),
            Some(Reason::UpstreamTreeMismatch("missing file"))
        );
    }

    #[test]
    fn extra_file_reports_deterministically() {
        let expected = map(&[("a.c", "1")]);
        let published = map(&[("a.c", "1"), ("cabin.toml", "m"), ("smuggled.c", "3")]);
        assert_eq!(
            compare_trees(&expected, &published),
            Some(Reason::UpstreamTreeMismatch("extra file"))
        );
    }

    #[test]
    fn content_divergence_reports_deterministically() {
        let expected = map(&[("a.c", "1")]);
        let published = map(&[("a.c", "tampered"), ("cabin.toml", "m")]);
        assert_eq!(
            compare_trees(&expected, &published),
            Some(Reason::UpstreamTreeMismatch("file contents"))
        );
    }

    #[test]
    fn nested_cabin_toml_is_not_exempt() {
        // Only the root manifest is excluded; a nested one must
        // match like any other file.
        let expected = map(&[("examples/demo/cabin.toml", "1")]);
        let published = map(&[("cabin.toml", "m")]);
        assert_eq!(
            compare_trees(&expected, &published),
            Some(Reason::UpstreamTreeMismatch("missing file"))
        );
    }
}
