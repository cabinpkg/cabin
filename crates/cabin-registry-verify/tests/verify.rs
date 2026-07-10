//! The fixture corpus: a benign archive produced by the in-tree
//! packaging code must verify, and each adversarial archive must be
//! rejected with its specific reason code.

use std::io::Write as _;
use std::path::PathBuf;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use cabin_registry_verify::{Limits, PendingVersion, Reason, Verdict, inspect};

/// A benign package staged through the real `cabin package` code
/// path, so the verifier is pinned to what publish actually emits.
/// Exercises rich dependency shapes (optional, cfg-conditioned,
/// `default-features = false`, a `"1.2"`-style requirement that
/// normalizes), a library target with an interface standard (a
/// non-empty derived `standards` table), and declared features.
fn benign(dir: &TempDir) -> (PathBuf, PendingVersion) {
    let staged = benign_staged(dir);
    write_pending(dir, &staged.archive_bytes, &staged)
}

fn benign_staged(dir: &TempDir) -> cabin_package::StagedPackage {
    dir.child("pkg/cabin.toml")
        .write_str(
            "[package]\n\
             name = \"demo\"\n\
             version = \"1.2.3\"\n\
             cxx-standard = \"c++20\"\n\
             \n\
             [dependencies]\n\
             fmt = \"1.2\"\n\
             ssl = { version = \"^3\", optional = true, default-features = false }\n\
             \n\
             [target.'cfg(os = \"windows\")'.dependencies]\n\
             winonly = \"^1\"\n\
             \n\
             [dev-dependencies]\n\
             catch2 = { version = \"^3\", features = [\"main\"] }\n\
             \n\
             [features]\n\
             default = []\n\
             tls = [\"dep:ssl\"]\n\
             \n\
             [target.demo]\n\
             type = \"library\"\n\
             sources = [\"src/lib.cc\"]\n\
             interface-cxx-standard = \"c++17\"\n",
        )
        .unwrap();
    dir.child("pkg/src/lib.cc").write_str("int f();\n").unwrap();
    cabin_package::stage_with_project(
        dir.child("pkg/cabin.toml").path(),
        None,
        None,
        &cabin_core::WorkspaceDepRequirements::default(),
    )
    .unwrap()
}

fn write_pending(
    dir: &TempDir,
    archive_bytes: &[u8],
    staged: &cabin_package::StagedPackage,
) -> (PathBuf, PendingVersion) {
    let archive = dir.child("archive.tar.gz");
    archive.write_binary(archive_bytes).unwrap();
    let hex = staged.checksum.strip_prefix("sha256:").unwrap().to_owned();
    let pending = PendingVersion {
        name: staged.name.as_str().to_owned(),
        version: staged.version.to_string(),
        checksum: hex,
        published_at: "2026-07-10T00:00:00.000Z".to_owned(),
        metadata: serde_json::to_value(&staged.metadata).unwrap(),
    };
    (archive.to_path_buf(), pending)
}

/// One hostile tar entry.  `raw_path` bypasses the tar writer's
/// path handling so unsafe names (absolute, `..`) land verbatim in
/// the header, exactly as an attacker would craft them.
struct HostileEntry<'a> {
    path: &'a str,
    kind: tar::EntryType,
    data: &'a [u8],
    link: Option<&'a str>,
}

impl<'a> HostileEntry<'a> {
    fn file(path: &'a str, data: &'a [u8]) -> Self {
        HostileEntry {
            path,
            kind: tar::EntryType::Regular,
            data,
            link: None,
        }
    }
}

fn hostile_tar_gz(entries: &[HostileEntry<'_>]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let gz = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
        let mut builder = tar::Builder::new(gz);
        for entry in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(entry.data.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(entry.kind);
            {
                let name = &mut header.as_old_mut().name;
                name[..entry.path.len()].copy_from_slice(entry.path.as_bytes());
            }
            if let Some(link) = entry.link {
                header.set_link_name(link).unwrap();
            }
            header.set_cksum();
            builder.append(&header, entry.data).unwrap();
        }
        builder
            .into_inner()
            .unwrap()
            .finish()
            .unwrap()
            .flush()
            .unwrap();
    }
    buf
}

/// Wrap hostile archive bytes in a listing entry whose checksum
/// matches, exactly like a malicious publish that passed the
/// server's synchronous checks (correct framing and checksum).
fn hostile_pending(dir: &TempDir, bytes: &[u8]) -> (PathBuf, PendingVersion) {
    let archive = dir.child("hostile.tar.gz");
    archive.write_binary(bytes).unwrap();
    let hex = cabin_core::hash::hash_reader(bytes).unwrap();
    let pending = PendingVersion {
        name: "demo".to_owned(),
        version: "1.2.3".to_owned(),
        checksum: hex.clone(),
        published_at: "2026-07-10T00:00:00.000Z".to_owned(),
        metadata: serde_json::json!({
            "schema": 1,
            "name": "demo",
            "version": "1.2.3",
            "dependencies": {},
            "yanked": false,
            "checksum": format!("sha256:{hex}"),
            "source": {
                "type": "archive",
                "path": "../artifacts/demo/demo-1.2.3.tar.gz",
                "format": "tar.gz",
            },
        }),
    };
    (archive.to_path_buf(), pending)
}

fn assert_rejected(archive: &std::path::Path, pending: &PendingVersion, reason: Reason) {
    let verdict = inspect(archive, pending, &Limits::default()).unwrap();
    assert_eq!(verdict, Verdict::Rejected(vec![reason]));
}

#[test]
fn benign_archive_verifies() {
    let dir = TempDir::new().unwrap();
    let (archive, pending) = benign(&dir);
    let verdict = inspect(&archive, &pending, &Limits::default()).unwrap();
    assert_eq!(verdict, Verdict::Verified);
}

#[test]
fn long_paths_within_the_cap_verify() {
    // Paths over the 100-byte tar header field ride on GNU
    // long-name records; the real packager emits them and the
    // verifier must tolerate them.
    let dir = TempDir::new().unwrap();
    let long = format!("src/{}.cc", "a".repeat(120));
    dir.child("pkg/cabin.toml")
        .write_str("[package]\nname = \"demo\"\nversion = \"1.2.3\"\n")
        .unwrap();
    dir.child(format!("pkg/{long}"))
        .write_str("int f();\n")
        .unwrap();
    let staged = cabin_package::stage_with_project(
        dir.child("pkg/cabin.toml").path(),
        None,
        None,
        &cabin_core::WorkspaceDepRequirements::default(),
    )
    .unwrap();
    let (archive, pending) = write_pending(&dir, &staged.archive_bytes, &staged);
    let verdict = inspect(&archive, &pending, &Limits::default()).unwrap();
    assert_eq!(verdict, Verdict::Verified);
}

#[test]
fn path_traversal_is_rejected() {
    let dir = TempDir::new().unwrap();
    let bytes = hostile_tar_gz(&[HostileEntry::file("../evil", b"x")]);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    assert_rejected(&archive, &pending, Reason::PathTraversal);
}

#[test]
fn absolute_path_is_rejected() {
    let dir = TempDir::new().unwrap();
    let bytes = hostile_tar_gz(&[HostileEntry::file("/etc/passwd", b"x")]);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    assert_rejected(&archive, &pending, Reason::AbsolutePath);
}

#[test]
fn symlink_entry_is_rejected() {
    let dir = TempDir::new().unwrap();
    let bytes = hostile_tar_gz(&[HostileEntry {
        path: "link",
        kind: tar::EntryType::Symlink,
        data: b"",
        link: Some("cabin.toml"),
    }]);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    assert_rejected(&archive, &pending, Reason::ForbiddenEntryType);
}

#[test]
fn hardlink_entry_is_rejected() {
    let dir = TempDir::new().unwrap();
    let bytes = hostile_tar_gz(&[HostileEntry {
        path: "link",
        kind: tar::EntryType::Link,
        data: b"",
        link: Some("cabin.toml"),
    }]);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    assert_rejected(&archive, &pending, Reason::ForbiddenEntryType);
}

#[test]
fn pax_decorated_entry_is_rejected() {
    // The tar reader consumes the PAX record itself and applies its
    // `path` override to the entry that follows; `cabin package`
    // never emits PAX records, so a decorated entry is rejected
    // outright before its rewritten path can matter.
    let dir = TempDir::new().unwrap();
    let bytes = hostile_tar_gz(&[
        HostileEntry {
            path: "pax",
            kind: tar::EntryType::XHeader,
            data: b"27 path=../overridden/path\n",
            link: None,
        },
        HostileEntry::file("harmless", b"x"),
    ]);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    assert_rejected(&archive, &pending, Reason::ForbiddenEntryType);
}

#[test]
fn decompression_bomb_is_rejected() {
    // 64 MiB of zeros gzips to a few dozen KiB; with the default
    // 10x ratio cap the stream aborts at the ratio floor (~24 MiB
    // for the default entry cap), long before 64 MiB decompress.
    let dir = TempDir::new().unwrap();
    let zeros = vec![0u8; 64 * 1024 * 1024];
    let bytes = hostile_tar_gz(&[HostileEntry::file("cabin.toml", &zeros)]);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    assert_rejected(&archive, &pending, Reason::DecompressedTooLarge);
}

#[test]
fn absolute_cap_bounds_low_ratio_archives() {
    // Even a stream within the ratio cap aborts at the absolute
    // cap.
    let dir = TempDir::new().unwrap();
    let body = vec![b'x'; 64 * 1024];
    let bytes = hostile_tar_gz(&[HostileEntry::file("cabin.toml", &body)]);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    let limits = Limits {
        abs_cap_bytes: 1024,
        ..Limits::default()
    };
    let verdict = inspect(&archive, &pending, &limits).unwrap();
    assert_eq!(
        verdict,
        Verdict::Rejected(vec![Reason::DecompressedTooLarge])
    );
}

#[test]
fn entry_flood_is_rejected() {
    let dir = TempDir::new().unwrap();
    let entries: Vec<String> = (0..8).map(|i| format!("f{i}")).collect();
    let hostile: Vec<HostileEntry<'_>> = entries
        .iter()
        .map(|path| HostileEntry::file(path, b"x"))
        .collect();
    let bytes = hostile_tar_gz(&hostile);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    let limits = Limits {
        max_entries: 4,
        ..Limits::default()
    };
    let verdict = inspect(&archive, &pending, &limits).unwrap();
    assert_eq!(verdict, Verdict::Rejected(vec![Reason::TooManyEntries]));
}

#[test]
fn over_long_path_is_rejected() {
    let dir = TempDir::new().unwrap();
    let long = "a".repeat(64);
    let bytes = hostile_tar_gz(&[HostileEntry::file(&long, b"x")]);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    let limits = Limits {
        max_path_len: 32,
        ..Limits::default()
    };
    let verdict = inspect(&archive, &pending, &limits).unwrap();
    assert_eq!(verdict, Verdict::Rejected(vec![Reason::PathTooLong]));
}

#[test]
fn duplicate_paths_are_rejected() {
    let dir = TempDir::new().unwrap();
    let bytes = hostile_tar_gz(&[
        HostileEntry::file("src/a.cc", b"first"),
        HostileEntry::file("src/a.cc", b"second"),
    ]);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    assert_rejected(&archive, &pending, Reason::DuplicatePath);
}

#[test]
fn file_used_as_directory_is_rejected() {
    // A regular file `src` alongside `src/main.cc` passes the
    // duplicate check but cannot be extracted; both orderings reject.
    for entries in [
        vec![
            HostileEntry::file("cabin.toml", b"x"),
            HostileEntry::file("src", b"i am a file"),
            HostileEntry::file("src/main.cc", b"int main(){}"),
        ],
        vec![
            HostileEntry::file("cabin.toml", b"x"),
            HostileEntry::file("src/main.cc", b"int main(){}"),
            HostileEntry::file("src", b"i am a file"),
        ],
    ] {
        let dir = TempDir::new().unwrap();
        let bytes = hostile_tar_gz(&entries);
        let (archive, pending) = hostile_pending(&dir, &bytes);
        assert_rejected(&archive, &pending, Reason::PathConflict);
    }
}

#[test]
fn directory_entry_sharing_a_name_is_not_a_conflict() {
    // An explicit directory entry `src/` is the legitimate parent of
    // `src/main.cc` - not a file/dir collision. (`cabin package`
    // never emits directory entries, but tolerating them must not
    // read as a conflict.)
    let dir = TempDir::new().unwrap();
    let bytes = hostile_tar_gz(&[
        HostileEntry::file(
            "cabin.toml",
            b"[package]\nname = \"demo\"\nversion = \"1.2.3\"\n",
        ),
        HostileEntry {
            path: "src/",
            kind: tar::EntryType::Directory,
            data: b"",
            link: None,
        },
        HostileEntry::file("src/main.cc", b"int main(){}"),
    ]);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    let verdict = inspect(&archive, &pending, &Limits::default()).unwrap();
    assert_eq!(verdict, Verdict::Verified);
}

#[test]
fn source_with_an_internal_parent_component_verifies() {
    // `cabin package` accepts a within-root source that spells an
    // internal `..` and archives the resolved file; the verifier must
    // normalize the same way, not reject it as missing.
    let dir = TempDir::new().unwrap();
    dir.child("pkg/cabin.toml")
        .write_str(
            "[package]\nname = \"demo\"\nversion = \"1.2.3\"\ncxx-standard = \"c++20\"\n\n\
             [target.demo]\ntype = \"library\"\nsources = [\"src/../lib.cc\"]\n",
        )
        .unwrap();
    dir.child("pkg/lib.cc").write_str("int f();\n").unwrap();
    let staged = cabin_package::stage_with_project(
        dir.child("pkg/cabin.toml").path(),
        None,
        None,
        &cabin_core::WorkspaceDepRequirements::default(),
    )
    .unwrap();
    let (archive, pending) = write_pending(&dir, &staged.archive_bytes, &staged);
    let verdict = inspect(&archive, &pending, &Limits::default()).unwrap();
    assert_eq!(verdict, Verdict::Verified);
}

#[test]
fn manifest_declaring_an_absent_source_is_rejected() {
    // The manifest declares a target source, but the archive carries
    // only cabin.toml - the package extracts but cannot build.
    let dir = TempDir::new().unwrap();
    let manifest = "[package]\nname = \"demo\"\nversion = \"1.2.3\"\ncxx-standard = \"c++20\"\n\n\
                    [target.demo]\ntype = \"library\"\nsources = [\"src/lib.cc\"]\n";
    let bytes = hostile_tar_gz(&[HostileEntry::file("cabin.toml", manifest.as_bytes())]);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    assert_rejected(&archive, &pending, Reason::MissingSource);
}

#[test]
fn missing_manifest_is_rejected() {
    let dir = TempDir::new().unwrap();
    let bytes = hostile_tar_gz(&[HostileEntry::file("src/a.cc", b"x")]);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    assert_rejected(&archive, &pending, Reason::ManifestMissing);
}

#[test]
fn unparsable_manifest_is_rejected() {
    let dir = TempDir::new().unwrap();
    let bytes = hostile_tar_gz(&[HostileEntry::file("cabin.toml", b"not toml [")]);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    assert_rejected(&archive, &pending, Reason::ManifestInvalid);
}

#[test]
fn non_archive_bytes_are_rejected() {
    let dir = TempDir::new().unwrap();
    let (archive, pending) = hostile_pending(&dir, b"this is not a gzip stream");
    assert_rejected(&archive, &pending, Reason::ArchiveInvalid);
}

#[test]
fn truncated_archive_is_rejected() {
    let dir = TempDir::new().unwrap();
    let bytes = hostile_tar_gz(&[HostileEntry::file("cabin.toml", b"x")]);
    let (archive, pending) = hostile_pending(&dir, &bytes[..bytes.len() / 2]);
    assert_rejected(&archive, &pending, Reason::ArchiveInvalid);
}

#[test]
fn manifest_name_mismatch_is_rejected() {
    let dir = TempDir::new().unwrap();
    let (archive, mut pending) = benign(&dir);
    pending.name = "other".to_owned();
    pending.metadata["name"] = serde_json::json!("other");
    assert_rejected(&archive, &pending, Reason::NameMismatch);
}

#[test]
fn listing_name_mismatch_is_rejected() {
    // The row the verdict would bind to disagrees with the manifest
    // even though the metadata document agrees.
    let dir = TempDir::new().unwrap();
    let (archive, mut pending) = benign(&dir);
    pending.name = "other".to_owned();
    assert_rejected(&archive, &pending, Reason::NameMismatch);
}

#[test]
fn version_mismatch_is_rejected() {
    let dir = TempDir::new().unwrap();
    let (archive, mut pending) = benign(&dir);
    pending.version = "9.9.9".to_owned();
    pending.metadata["version"] = serde_json::json!("9.9.9");
    assert_rejected(&archive, &pending, Reason::VersionMismatch);
}

#[test]
fn dependency_set_mismatch_is_rejected() {
    let dir = TempDir::new().unwrap();
    let (archive, mut pending) = benign(&dir);
    // The stored metadata claims no dependencies; the manifest
    // declares one.
    pending.metadata["dependencies"] = serde_json::json!({});
    assert_rejected(&archive, &pending, Reason::DependencyMismatch);
}

#[test]
fn dependency_requirement_mismatch_is_rejected() {
    let dir = TempDir::new().unwrap();
    let (archive, mut pending) = benign(&dir);
    pending.metadata["dependencies"] = serde_json::json!({ "fmt": "^11" });
    assert_rejected(&archive, &pending, Reason::DependencyMismatch);
}

#[test]
fn dev_dependency_mismatch_is_rejected() {
    let dir = TempDir::new().unwrap();
    let (archive, mut pending) = benign(&dir);
    pending
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("dev-dependencies");
    assert_rejected(&archive, &pending, Reason::DependencyMismatch);
}

#[test]
fn language_standard_mismatch_is_rejected() {
    let dir = TempDir::new().unwrap();
    let (archive, mut pending) = benign(&dir);
    pending.metadata["language"] = serde_json::json!({ "cxx_standard": "c++17" });
    assert_rejected(&archive, &pending, Reason::LanguageStandardMismatch);
}

#[test]
fn checksum_mismatch_is_rejected() {
    let dir = TempDir::new().unwrap();
    let (archive, mut pending) = benign(&dir);
    let wrong = "0".repeat(64);
    pending.checksum = wrong.clone();
    pending.metadata["checksum"] = serde_json::json!(format!("sha256:{wrong}"));
    assert_rejected(&archive, &pending, Reason::ChecksumMismatch);
}

#[test]
fn metadata_checksum_mismatch_alone_is_rejected() {
    // Defense in depth: the listing checksum is right (the row would
    // accept the verdict binding) but the stored document disagrees
    // with the bytes.
    let dir = TempDir::new().unwrap();
    let (archive, mut pending) = benign(&dir);
    pending.metadata["checksum"] = serde_json::json!(format!("sha256:{}", "0".repeat(64)));
    assert_rejected(&archive, &pending, Reason::ChecksumMismatch);
}

#[test]
fn unsupported_metadata_schema_is_rejected() {
    // The registry refuses schemas other than 1 at publish; if one
    // slips through anyway it must be a rejection, not an
    // operational error that would wedge the verification queue.
    let dir = TempDir::new().unwrap();
    let (archive, mut pending) = benign(&dir);
    pending.metadata["schema"] = serde_json::json!(2);
    assert_rejected(&archive, &pending, Reason::MetadataMismatch);
}

#[test]
fn feature_table_mismatch_is_rejected() {
    // Metadata advertising features the manifest does not declare
    // must not verify: the checksum binds the bytes, not the honesty
    // of the derived document.
    let dir = TempDir::new().unwrap();
    let (archive, mut pending) = benign(&dir);
    pending.metadata["features"] = serde_json::json!({
        "default": ["tls"],
        "features": { "tls": ["dep:ssl"] },
    });
    assert_rejected(&archive, &pending, Reason::MetadataMismatch);
}

#[test]
fn standards_table_mismatch_is_rejected() {
    let dir = TempDir::new().unwrap();
    let (archive, mut pending) = benign(&dir);
    pending.metadata["standards"] = serde_json::json!({
        "targets": { "demo": { "interface": { "c++": { "min": "c++23" } } } },
    });
    assert_rejected(&archive, &pending, Reason::LanguageStandardMismatch);
}

#[test]
fn unpublishable_manifests_are_rejected() {
    // The real client cannot archive these manifests
    // (`cabin_package::validate::validate_publishable`); an archive
    // carrying one was hand-crafted.
    let dir = TempDir::new().unwrap();
    for manifest in [
        // A [patch] table leaks local override state.
        "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n\n[patch]\nfmt = { path = \"../fmt\" }\n",
        // A target source escaping the package root.
        "[package]\nname = \"demo\"\nversion = \"1.2.3\"\ncxx-standard = \"c++17\"\n\n[target.demo]\ntype = \"library\"\nsources = [\"../outside.cc\"]\n",
        // A path dependency is not publishable.
        "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n\n[dependencies]\nlocal = { path = \"../local\" }\n",
    ] {
        let bytes = hostile_tar_gz(&[HostileEntry::file("cabin.toml", manifest.as_bytes())]);
        let (archive, pending) = hostile_pending(&dir, &bytes);
        let verdict = inspect(&archive, &pending, &Limits::default()).unwrap();
        assert_eq!(
            verdict,
            Verdict::Rejected(vec![Reason::ManifestInvalid]),
            "manifest: {manifest}"
        );
    }
}

#[test]
fn corrupted_gzip_trailer_is_rejected() {
    // The stream is drained to EOF after the tar terminator, so the
    // gzip CRC/length trailer is always validated.
    let dir = TempDir::new().unwrap();
    let mut bytes = hostile_tar_gz(&[HostileEntry::file("cabin.toml", b"x")]);
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    let (archive, pending) = hostile_pending(&dir, &bytes);
    assert_rejected(&archive, &pending, Reason::ArchiveInvalid);
}

#[test]
fn content_smuggled_behind_the_terminator_is_rejected() {
    // A second gzip member carrying tar content decodes into the
    // post-terminator drain, where any nonzero byte is refused.
    let dir = TempDir::new().unwrap();
    let mut bytes = hostile_tar_gz(&[HostileEntry::file("cabin.toml", b"x")]);
    bytes.extend_from_slice(&hostile_tar_gz(&[HostileEntry::file(
        "smuggled", b"payload",
    )]));
    let (archive, pending) = hostile_pending(&dir, &bytes);
    assert_rejected(&archive, &pending, Reason::ArchiveInvalid);
}

#[test]
fn empty_trailing_member_is_tolerated() {
    // The deliberate boundary of the drain check: an extra gzip
    // member that decompresses to nothing carries no content, so the
    // post-terminator drain sees only zeros and the archive verifies
    // - the same tolerance the verifier grants mode/mtime differences
    // (`cabin package` never emits either, but neither can smuggle a
    // materialized file). A member with content is rejected
    // (`content_smuggled_behind_the_terminator_is_rejected`).
    let dir = TempDir::new().unwrap();
    let mut bytes = hostile_tar_gz(&[HostileEntry::file(
        "cabin.toml",
        b"[package]\nname = \"demo\"\nversion = \"1.2.3\"\n",
    )]);
    bytes.extend_from_slice(&hostile_tar_gz(&[]));
    let (archive, pending) = hostile_pending(&dir, &bytes);
    let verdict = inspect(&archive, &pending, &Limits::default()).unwrap();
    assert_eq!(verdict, Verdict::Verified);
}

#[test]
fn sparse_entry_is_rejected() {
    let dir = TempDir::new().unwrap();
    let bytes = hostile_tar_gz(&[HostileEntry {
        path: "sparse",
        kind: tar::EntryType::GNUSparse,
        data: b"",
        link: None,
    }]);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    let verdict = inspect(&archive, &pending, &Limits::default()).unwrap();
    // The tar layer may refuse the malformed sparse framing before
    // the type gate sees it; either way the archive never verifies.
    assert!(
        matches!(
            &verdict,
            Verdict::Rejected(reasons)
                if reasons == &vec![Reason::ForbiddenEntryType]
                    || reasons == &vec![Reason::ArchiveInvalid]
        ),
        "got {verdict:?}"
    );
}

/// The workflow scripts against the binary's stdout JSON and exit
/// codes; pin that contract.
mod binary {
    use super::*;
    use assert_cmd::Command;

    fn write_entry(dir: &TempDir, pending: &PendingVersion) -> PathBuf {
        let entry = dir.child("entry.json");
        entry
            .write_str(
                &serde_json::json!({
                    "name": pending.name,
                    "version": pending.version,
                    "checksum": pending.checksum,
                    "published_by": 1,
                    "published_at": pending.published_at,
                    "metadata": pending.metadata,
                })
                .to_string(),
            )
            .unwrap();
        entry.to_path_buf()
    }

    fn verifier() -> Command {
        Command::cargo_bin("cabin-registry-verify").unwrap()
    }

    #[test]
    fn verified_prints_the_verdict_and_exits_zero() {
        let dir = TempDir::new().unwrap();
        let (archive, pending) = benign(&dir);
        let entry = write_entry(&dir, &pending);
        verifier()
            .arg(&archive)
            .arg(&entry)
            .assert()
            .success()
            .stdout("{\"verdict\":\"verified\"}\n");
    }

    #[test]
    fn rejected_prints_the_reasons_and_exits_zero() {
        let dir = TempDir::new().unwrap();
        let bytes = hostile_tar_gz(&[HostileEntry::file("../evil", b"x")]);
        let (archive, pending) = hostile_pending(&dir, &bytes);
        let entry = write_entry(&dir, &pending);
        verifier()
            .arg(&archive)
            .arg(&entry)
            .assert()
            .success()
            .stdout("{\"verdict\":\"rejected\",\"reasons\":[\"path_traversal\"]}\n");
    }

    #[test]
    fn operational_failures_exit_two_with_no_verdict() {
        let dir = TempDir::new().unwrap();
        let (_, pending) = benign(&dir);
        let entry = write_entry(&dir, &pending);
        verifier()
            .arg(dir.child("missing.tar.gz").path())
            .arg(&entry)
            .assert()
            .code(2)
            .stdout("");
    }

    #[test]
    fn invalid_limit_values_exit_two() {
        let dir = TempDir::new().unwrap();
        let (archive, pending) = benign(&dir);
        let entry = write_entry(&dir, &pending);
        verifier()
            .env("VERIFY_RATIO_CAP", "banana")
            .arg(&archive)
            .arg(&entry)
            .assert()
            .code(2)
            .stdout("");
    }
}

#[test]
fn many_tiny_entries_stay_within_the_ratio_floor() {
    // Tar framing for an entry-cap-sized archive of tiny files
    // vastly outweighs its compressed size; the floor derived from
    // the entry cap must keep it verifiable.
    let dir = TempDir::new().unwrap();
    let mut entries = vec![HostileEntry::file(
        "cabin.toml",
        b"[package]\nname = \"demo\"\nversion = \"1.2.3\"\n",
    )];
    let paths: Vec<String> = (0..9000).map(|i| format!("src/f{i}.h")).collect();
    entries.extend(paths.iter().map(|path| HostileEntry::file(path, b"")));
    let bytes = hostile_tar_gz(&entries);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    let verdict = inspect(&archive, &pending, &Limits::default()).unwrap();
    assert_eq!(verdict, Verdict::Verified);
}
