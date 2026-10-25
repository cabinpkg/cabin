//! Shared harness code for the fuzz targets in `fuzz_targets/`.

use std::collections::BTreeSet;
use std::io::{self, Cursor, Read};
use std::path::Path;

use cabin_core::Checksum;
use cabin_registry_verify::{Limits, PendingVersion, Verdict, VerifyError, inspect_bytes};

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

#[cfg(test)]
mod tests {
    use super::*;

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
