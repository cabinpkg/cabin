//! Shared harness code for the fuzz targets in `fuzz_targets/`.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use cabin_artifact::{ArtifactError, SafeExtractOptions};
use cabin_core::Checksum;
use cabin_registry_verify::{Limits, PendingVersion, Verdict, VerifyError, inspect_bytes};
use flate2::Compression;
use flate2::write::GzEncoder;
use tempfile::TempDir;

/// The manifest whose listing the harness falls back to when the
/// client's zip reader cannot read an archive's own `cabin.toml`: an
/// archive the client cannot open can then still verify - the
/// divergence the deep target exists to catch - when its manifest, as
/// the verifier reads it, is this one. `seeds/inspect_zip` holds an
/// archive of it.
pub const FALLBACK_MANIFEST: &str = "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n";

/// Inspect `archive` against the listing entry a publish that passed
/// the server's synchronous checks would produce, with `checksum` as
/// the recorded digest. The canonical metadata is derived through the
/// publish seam from the archive's own `cabin.toml` as the client's zip
/// reader sees it, or from [`FALLBACK_MANIFEST`] when it sees none, so
/// a well-formed archive can reach, and pass, the consistency checks.
///
/// # Errors
///
/// The inspector's operational errors, which no fuzz input should turn
/// into a verdict either way.
///
/// # Panics
///
/// If [`FALLBACK_MANIFEST`] stops being publishable.
pub fn inspect_as_listed(archive: &[u8], checksum: Checksum) -> Result<Verdict, VerifyError> {
    let metadata = client_manifest(archive)
        .and_then(|text| metadata_of(&text, &checksum))
        .unwrap_or_else(|| {
            metadata_of(FALLBACK_MANIFEST, &checksum).expect("the fallback manifest is publishable")
        });
    let pending = PendingVersion {
        name: metadata["name"].as_str().unwrap_or_default().to_owned(),
        version: metadata["version"].as_str().unwrap_or_default().to_owned(),
        revision: checksum.revision_id().to_owned(),
        checksum,
        published_at: "2026-07-10T00:00:00.000Z".to_owned(),
        metadata,
    };
    inspect_bytes(archive, &pending, &Limits::default(), None)
}

/// The archive's root `cabin.toml` as the client's zip reader reads it.
fn client_manifest(archive: &[u8]) -> Option<String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive)).ok()?;
    let mut text = String::new();
    zip.by_name("cabin.toml")
        .ok()?
        // The inspector's absolute decompression cap: no manifest it
        // accepts is longer.
        .take(Limits::default().abs_cap_bytes)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// The canonical metadata publish stores for `manifest`.
fn metadata_of(manifest: &str, checksum: &Checksum) -> Option<serde_json::Value> {
    let package = cabin_manifest::parse_manifest_str(manifest).ok()?.package?;
    // `canonical_metadata` takes a publishable package, as publish and
    // the verifier's consistency pass hand it one.
    cabin_package::validate::validate_publishable(&package).ok()?;
    serde_json::to_value(cabin_package::metadata::canonical_metadata(
        &package, checksum,
    ))
    .ok()
}

/// What a verified archive promises its consumers: the client's own zip
/// reader (the one `cabin-artifact` extracts with) opens it and sees
/// regular files only, each named by the exact UTF-8 bytes of its
/// header, under a safe, portable, unique relative path, decompressing
/// to bytes that match its CRC, with `cabin.toml` at the root.
///
/// # Panics
///
/// When any of those promises is broken.
pub fn assert_client_agrees(archive: &[u8]) {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))
        .expect("a verified archive opens in the client's zip reader");
    let mut names = BTreeSet::new();
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .expect("a verified entry opens in the client's zip reader");
        assert!(entry.is_file(), "a verified entry is not a regular file");
        let name = std::str::from_utf8(entry.name_raw())
            .expect("a verified entry name is UTF-8")
            .to_owned();
        assert_eq!(
            entry
                .name()
                .expect("the client decodes a verified entry name"),
            name,
            "the client decodes a verified entry name differently"
        );
        assert!(
            cabin_fs::path::is_safe_relative_path(Path::new(&name))
                && cabin_fs::path::relative_path_portability(&name).is_none(),
            "a verified entry name is unsafe or unportable: {name:?}"
        );
        assert!(names.insert(name), "a verified archive repeats a name");
        io::copy(&mut entry, &mut io::sink())
            .expect("a verified entry decompresses and matches its CRC");
    }
    assert!(
        names.contains("cabin.toml"),
        "a verified archive has no root cabin.toml"
    );
}

/// The top directory `seeds/extract_*` nest every file under, as an
/// upstream release archive does; `cargo fuzz-seeds` writes the same,
/// and the harness's test checks the seeds against it.
pub const SEED_PREFIX: &str = "pkg";

/// An extractor as the upstream materializer calls it.
pub type Extract = fn(&Path, &Path, SafeExtractOptions<'_>) -> Result<(), ArtifactError>;

/// `tar` gzipped, so the tar target's mutations land on the tar
/// structure rather than on a compressed stream that no longer decodes.
///
/// # Panics
///
/// Never: the stream is in memory.
pub fn gzip(tar: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(tar).expect("gzip into memory");
    encoder.finish().expect("gzip into memory")
}

/// Extract `archive` the way the upstream materializer does (symlinks
/// skipped), with [`SEED_PREFIX`] stripped and without, and require
/// that whatever the extractor decides, it leaves its archive as it
/// was, writes nothing beside its destination, and leaves only
/// directories and singly linked regular files in it.  An escape needs a `..` or an absolute
/// component; this sees one that lands a single level up, which is
/// where the shortest such input lands.
///
/// # Panics
///
/// When an extraction breaks any of that.
pub fn assert_extraction_contained(archive: &[u8], extract: Extract) {
    for strip_prefix in [None, Some(SEED_PREFIX)] {
        // Accepted or refused, the verdict is the extractor's to make.
        let (scratch, _) = extract_in_scratch(archive, extract, strip_prefix);
        let mut names: Vec<_> = fs::read_dir(scratch.path())
            .expect("list the scratch directory")
            .map(|entry| entry.expect("list the scratch directory").file_name())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["archive", "dest"],
            "an extraction wrote beside its destination"
        );
        assert_eq!(
            fs::read(scratch.path().join("archive")).expect("read the archive back"),
            archive,
            "an extraction rewrote its archive"
        );
        assert_files_and_dirs(&scratch.path().join("dest"));
    }
}

/// A fresh scratch directory holding `archive` as `archive`, extracted
/// by `extract` into its `dest`.
fn extract_in_scratch(
    archive: &[u8],
    extract: Extract,
    strip_prefix: Option<&str>,
) -> (TempDir, Result<(), ArtifactError>) {
    let scratch = tempfile::tempdir().expect("create a scratch directory");
    let path = scratch.path().join("archive");
    let dest = scratch.path().join("dest");
    fs::write(&path, archive).expect("write the archive");
    fs::create_dir(&dest).expect("create the destination");
    let options = SafeExtractOptions {
        strip_prefix,
        skip_symlinks: true,
    };
    let result = extract(&path, &dest, options);
    (scratch, result)
}

fn assert_files_and_dirs(dir: &Path) {
    for entry in fs::read_dir(dir).expect("list an extracted directory") {
        let path = entry.expect("list an extracted directory").path();
        let metadata = fs::symlink_metadata(&path).expect("stat an extracted entry");
        if metadata.is_dir() {
            assert_files_and_dirs(&path);
        } else {
            assert!(
                metadata.is_file(),
                "an extraction left a link or special file: {path:?}"
            );
            // A hard link is a regular file too, and may share its
            // bytes with one outside the destination.
            assert_eq!(
                metadata.nlink(),
                1,
                "an extraction left a hard link: {path:?}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Containment matters past the extractor's own checks, so every
    /// extraction seed must extract, with its prefix stripped and not.
    #[test]
    fn every_extraction_seed_extracts() {
        assert_seeds_extract(
            "extract_zip",
            cabin_artifact::safe_extract_zip,
            <[u8]>::to_vec,
        );
        assert_seeds_extract("extract_tar_gz", cabin_artifact::safe_extract_tar_gz, gzip);
    }

    /// The oracle's own regression test: `is_file` alone passes a hard
    /// link.
    #[test]
    #[should_panic(expected = "an extraction left a hard link")]
    fn a_hard_linked_file_breaks_containment() {
        fn link_the_archive(
            archive: &Path,
            dest: &Path,
            _: SafeExtractOptions<'_>,
        ) -> Result<(), ArtifactError> {
            fs::hard_link(archive, dest.join("alias")).unwrap();
            Ok(())
        }
        assert_extraction_contained(b"", link_the_archive);
    }

    fn assert_seeds_extract(corpus: &str, extract: Extract, wrap: fn(&[u8]) -> Vec<u8>) {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("seeds")
            .join(corpus);
        let mut seen = 0;
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let archive = wrap(&fs::read(&path).unwrap());
            for strip_prefix in [None, Some(SEED_PREFIX)] {
                let (_scratch, result) = extract_in_scratch(&archive, extract, strip_prefix);
                assert!(
                    result.is_ok(),
                    "{} ({strip_prefix:?}): {result:?}",
                    path.display()
                );
            }
            seen += 1;
        }
        assert!(seen > 0, "no seeds in {}", dir.display());
    }

    /// The deep target's oracle is only exercised on verified archives,
    /// so the harness must verify every seed archive the packager built.
    #[test]
    fn every_seed_archive_verifies_as_listed() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("seeds/inspect_zip");
        let mut seen = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let archive = std::fs::read(&path).unwrap();
            let verdict = inspect_as_listed(&archive, Checksum::of_bytes(&archive));
            assert!(
                matches!(verdict, Ok(Verdict::Verified)),
                "{}: {verdict:?}",
                path.display()
            );
            assert_client_agrees(&archive);
            seen += 1;
        }
        assert!(seen > 0, "no seed archives in {}", dir.display());
    }

    /// Without a seed of the fallback manifest, an archive the client
    /// cannot read verifies only if the fuzzer invents that manifest.
    #[test]
    fn a_seed_archive_carries_the_fallback_manifest() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("seeds/inspect_zip");
        let found = std::fs::read_dir(&dir).unwrap().any(|entry| {
            let archive = std::fs::read(entry.unwrap().path()).unwrap();
            client_manifest(&archive).as_deref() == Some(FALLBACK_MANIFEST)
        });
        assert!(
            found,
            "no seed in {} has the fallback manifest",
            dir.display()
        );
    }
}
