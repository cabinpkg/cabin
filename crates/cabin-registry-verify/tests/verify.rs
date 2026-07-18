//! The fixture corpus: a benign archive produced by the in-tree
//! packaging code must verify, and each adversarial archive must be
//! rejected with its specific reason code.
//!
//! Hostile archives are assembled byte by byte (`assemble`) so every
//! field a general-purpose zip writer would fix up - the
//! general-purpose flags, the central/local agreement, the declared
//! sizes and CRC, the external-attribute type bits - can be set to the
//! exact hostile shape the strict profile forbids.

// The builder writes entry counts, name lengths, and sizes into
// fixed-width zip header fields; the fixtures stay far below the
// u16/u32 field widths, so the narrowing casts are intentional.
#![allow(clippy::cast_possible_truncation)]

use std::io::Write as _;
use std::path::PathBuf;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use cabin_registry_verify::{Limits, PendingVersion, Reason, Verdict, inspect};

// Zip record signatures (little-endian on disk).
const LOCAL_SIG: u32 = 0x0403_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const EOCD_SIG: u32 = 0x0605_4b50;

/// External attributes for a regular file with `0644` permissions:
/// `S_IFREG | 0o644` in the high 16 bits, matching what `cabin
/// package` emits through the `zip` crate's `System::Unix`.
const REGULAR_FILE_ATTRS: u32 = 0o100_644 << 16;

/// One entry as it will be written into both the local record and the
/// central directory.  Constructed correct, then mutated field by
/// field to craft a specific violation; `assemble` lays the records
/// out contiguously and computes the offsets.
#[derive(Clone)]
struct Entry {
    name: String,
    method: u16,
    gp: u16,
    crc: u32,
    /// Declared compressed size (written verbatim, so a test can lie).
    csize: u32,
    /// Declared uncompressed size (written verbatim, so a test can lie).
    usize_: u32,
    ext_attrs: u32,
    /// The bytes placed in the local record's data area.
    body: Vec<u8>,
    /// Extra-field bytes, written into both headers (the profile bans
    /// any extra field).
    extra: Vec<u8>,
    /// Central-header comment bytes (the profile bans comments).
    comment: Vec<u8>,
    /// When set, the local header's CRC diverges from the central
    /// header's, forcing a local-vs-central disagreement.
    local_crc: Option<u32>,
}

impl Entry {
    /// A deflated entry whose declared sizes, CRC, and UTF-8 flag all
    /// match its contents - the shape `cabin package` emits.
    fn deflated(name: &str, data: &[u8]) -> Self {
        let body = deflate(data);
        Entry {
            name: name.to_owned(),
            method: 8,
            gp: if name.is_ascii() { 0 } else { 0x0800 },
            crc: crc32(data),
            csize: body.len() as u32,
            usize_: data.len() as u32,
            ext_attrs: REGULAR_FILE_ATTRS,
            body,
            extra: Vec::new(),
            comment: Vec::new(),
            local_crc: None,
        }
    }

    /// A stored (uncompressed) entry.
    fn stored(name: &str, data: &[u8]) -> Self {
        Entry {
            name: name.to_owned(),
            method: 0,
            gp: if name.is_ascii() { 0 } else { 0x0800 },
            crc: crc32(data),
            csize: data.len() as u32,
            usize_: data.len() as u32,
            ext_attrs: REGULAR_FILE_ATTRS,
            body: data.to_vec(),
            extra: Vec::new(),
            comment: Vec::new(),
            local_crc: None,
        }
    }
}

/// Raw deflate (method 8) body for `data`, matching the client's
/// `flate2` deflate the verifier decodes.
fn deflate(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(6));
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn crc32(data: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(data);
    hasher.finalize()
}

/// Lay `entries` out as a strict-profile container: local records
/// (contiguous, in order), then the central directory, then the EOCD.
fn assemble(entries: &[Entry]) -> Vec<u8> {
    let mut local = Vec::new();
    let mut offsets = Vec::with_capacity(entries.len());
    for entry in entries {
        offsets.push(local.len() as u32);
        local.extend(LOCAL_SIG.to_le_bytes());
        local.extend(20u16.to_le_bytes()); // version needed
        local.extend(entry.gp.to_le_bytes());
        local.extend(entry.method.to_le_bytes());
        local.extend(0u16.to_le_bytes()); // mod time
        local.extend(0u16.to_le_bytes()); // mod date
        local.extend(entry.local_crc.unwrap_or(entry.crc).to_le_bytes());
        local.extend(entry.csize.to_le_bytes());
        local.extend(entry.usize_.to_le_bytes());
        local.extend((entry.name.len() as u16).to_le_bytes());
        local.extend((entry.extra.len() as u16).to_le_bytes());
        local.extend(entry.name.as_bytes());
        local.extend(&entry.extra);
        local.extend(&entry.body);
    }
    let cd_offset = local.len() as u32;

    let mut central = Vec::new();
    for (entry, offset) in entries.iter().zip(&offsets) {
        central.extend(CENTRAL_SIG.to_le_bytes());
        central.extend(0x031eu16.to_le_bytes()); // version made by (Unix)
        central.extend(20u16.to_le_bytes()); // version needed
        central.extend(entry.gp.to_le_bytes());
        central.extend(entry.method.to_le_bytes());
        central.extend(0u16.to_le_bytes()); // mod time
        central.extend(0u16.to_le_bytes()); // mod date
        central.extend(entry.crc.to_le_bytes());
        central.extend(entry.csize.to_le_bytes());
        central.extend(entry.usize_.to_le_bytes());
        central.extend((entry.name.len() as u16).to_le_bytes());
        central.extend((entry.extra.len() as u16).to_le_bytes());
        central.extend((entry.comment.len() as u16).to_le_bytes());
        central.extend(0u16.to_le_bytes()); // disk start
        central.extend(0u16.to_le_bytes()); // internal attrs
        central.extend(entry.ext_attrs.to_le_bytes());
        central.extend(offset.to_le_bytes());
        central.extend(entry.name.as_bytes());
        central.extend(&entry.extra);
        central.extend(&entry.comment);
    }
    let cd_size = central.len() as u32;

    let mut out = local;
    out.extend(central);
    out.extend(EOCD_SIG.to_le_bytes());
    out.extend(0u16.to_le_bytes()); // disk number
    out.extend(0u16.to_le_bytes()); // cd start disk
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend(cd_size.to_le_bytes());
    out.extend(cd_offset.to_le_bytes());
    out.extend(0u16.to_le_bytes()); // comment length
    out
}

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
    let archive = dir.child("archive.zip");
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

/// Wrap hostile archive bytes in a listing entry whose checksum
/// matches, exactly like a malicious publish that passed the server's
/// synchronous checks (correct framing and checksum).  The metadata is
/// the generic `demo@1.2.3` document; hostile archives that reject at
/// the structure pass never reach the consistency check that would use
/// it.
fn hostile_pending(dir: &TempDir, bytes: &[u8]) -> (PathBuf, PendingVersion) {
    let archive = dir.child("hostile.zip");
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
                "path": "../artifacts/demo/demo-1.2.3.zip",
                "format": "zip",
            },
        }),
    };
    (archive.to_path_buf(), pending)
}

/// Assemble a benign container by hand and derive its listing entry
/// through the same canonical-metadata seam publish uses, so a
/// hand-built archive can reach - and pass - the consistency check.
/// Used for shapes the real packager cannot emit but that must still
/// verify (a huge flat tree, a UTF-8-flagged non-ASCII name).
fn hand_pending(dir: &TempDir, entries: &[Entry], manifest: &str) -> (PathBuf, PendingVersion) {
    let bytes = assemble(entries);
    let archive = dir.child("hand.zip");
    archive.write_binary(&bytes).unwrap();
    let hex = cabin_core::hash::hash_reader(bytes.as_slice()).unwrap();
    let parsed = cabin_manifest::parse_manifest_str(manifest).unwrap();
    let package = parsed.package.unwrap();
    let metadata = cabin_package::metadata::canonical_metadata(&package, &format!("sha256:{hex}"));
    let pending = PendingVersion {
        name: package.name.as_str().to_owned(),
        version: package.version.to_string(),
        checksum: hex,
        published_at: "2026-07-10T00:00:00.000Z".to_owned(),
        metadata: serde_json::to_value(metadata).unwrap(),
    };
    (archive.to_path_buf(), pending)
}

fn assert_rejected(archive: &std::path::Path, pending: &PendingVersion, reason: Reason) {
    let verdict = inspect(archive, pending, &Limits::default()).unwrap();
    assert_eq!(verdict, Verdict::Rejected(vec![reason]));
}

/// A one-entry hostile archive is common enough to name.
fn hostile_one(dir: &TempDir, entry: Entry) -> (PathBuf, PendingVersion) {
    hostile_pending(dir, &assemble(&[entry]))
}

const MINIMAL_MANIFEST: &str = "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n";

// ---------------------------------------------------------------------------
// Benign round-trips through the real packager.
// ---------------------------------------------------------------------------

#[test]
fn benign_archive_verifies() {
    let dir = TempDir::new().unwrap();
    let (archive, pending) = benign(&dir);
    let verdict = inspect(&archive, &pending, &Limits::default()).unwrap();
    assert_eq!(verdict, Verdict::Verified);
}

/// A hosted package is always scoped: the full `<scope>/<name>` string
/// threads identically through the archive manifest, the canonical
/// metadata document, and the admin listing row, and the three-way
/// consistency check verifies it.
#[test]
fn scoped_archive_verifies() {
    let dir = TempDir::new().unwrap();
    dir.child("pkg/cabin.toml")
        .write_str(
            "[package]\n\
             name = \"acme/demo\"\n\
             version = \"1.2.3\"\n\
             cxx-standard = \"c++20\"\n\
             \n\
             [target.demo]\n\
             type = \"library\"\n\
             sources = [\"src/lib.cc\"]\n",
        )
        .unwrap();
    dir.child("pkg/src/lib.cc").write_str("int f();\n").unwrap();
    let staged = cabin_package::stage_with_project(
        dir.child("pkg/cabin.toml").path(),
        None,
        None,
        &cabin_core::WorkspaceDepRequirements::default(),
    )
    .unwrap();
    let (archive, pending) = write_pending(&dir, &staged.archive_bytes, &staged);
    assert_eq!(pending.name, "acme/demo");
    let verdict = inspect(&archive, &pending, &Limits::default()).unwrap();
    assert_eq!(verdict, Verdict::Verified);

    // A listing row naming a different scope must not bind this
    // archive's verdict.
    let mut mismatched = pending;
    mismatched.name = "intruder/demo".to_owned();
    assert_rejected(&archive, &mismatched, Reason::NameMismatch);
}

#[test]
fn long_paths_within_the_cap_verify() {
    // A 120-byte path is well under the 256-byte default cap; the real
    // packager emits it as an ordinary long name field and the
    // verifier must tolerate it.
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

/// A non-ASCII entry name is legal only with the UTF-8 general-purpose
/// bit (bit 11) set; a hand-built archive that sets it exactly must
/// verify.  The negative direction is
/// `utf8_bit_set_on_ascii_name_is_rejected` /
/// `non_ascii_name_without_utf8_bit_is_rejected`.
#[test]
fn non_ascii_name_with_the_utf8_bit_verifies() {
    let dir = TempDir::new().unwrap();
    let entries = [
        Entry::deflated("cabin.toml", MINIMAL_MANIFEST.as_bytes()),
        Entry::deflated("src/café.h", b"int f();\n"),
    ];
    let (archive, pending) = hand_pending(&dir, &entries, MINIMAL_MANIFEST);
    let verdict = inspect(&archive, &pending, &Limits::default()).unwrap();
    assert_eq!(verdict, Verdict::Verified);
}

#[test]
fn many_tiny_entries_stay_within_the_ratio_floor() {
    // Zip framing for an entry-cap-sized archive of tiny files vastly
    // outweighs its compressed size; the floor derived from the entry
    // cap must keep it verifiable.
    let dir = TempDir::new().unwrap();
    let mut entries = vec![Entry::deflated("cabin.toml", MINIMAL_MANIFEST.as_bytes())];
    entries.extend((0..9000).map(|i| Entry::deflated(&format!("src/f{i}.h"), b"")));
    let (archive, pending) = hand_pending(&dir, &entries, MINIMAL_MANIFEST);
    let verdict = inspect(&archive, &pending, &Limits::default()).unwrap();
    assert_eq!(verdict, Verdict::Verified);
}

/// The profile permits the store method (0) as well as deflate; the
/// real packager only emits deflate, so a hand-built stored entry is
/// the only exercise of that decode branch and its CRC check.
#[test]
fn stored_entry_verifies() {
    let dir = TempDir::new().unwrap();
    let entries = [Entry::stored("cabin.toml", MINIMAL_MANIFEST.as_bytes())];
    let (archive, pending) = hand_pending(&dir, &entries, MINIMAL_MANIFEST);
    let verdict = inspect(&archive, &pending, &Limits::default()).unwrap();
    assert_eq!(verdict, Verdict::Verified);
}

// ---------------------------------------------------------------------------
// Path rules.
// ---------------------------------------------------------------------------

#[test]
fn path_traversal_is_rejected() {
    let dir = TempDir::new().unwrap();
    let (archive, pending) = hostile_one(&dir, Entry::deflated("../evil", b"x"));
    assert_rejected(&archive, &pending, Reason::PathTraversal);
}

#[test]
fn absolute_path_is_rejected() {
    let dir = TempDir::new().unwrap();
    let (archive, pending) = hostile_one(&dir, Entry::deflated("/etc/passwd", b"x"));
    assert_rejected(&archive, &pending, Reason::AbsolutePath);
}

#[test]
fn windows_drive_path_is_rejected() {
    let dir = TempDir::new().unwrap();
    let (archive, pending) = hostile_one(&dir, Entry::deflated("c:/evil", b"x"));
    assert_rejected(&archive, &pending, Reason::AbsolutePath);
}

#[test]
fn backslash_name_is_invalid() {
    let dir = TempDir::new().unwrap();
    let (archive, pending) = hostile_one(&dir, Entry::deflated("a\\b", b"x"));
    assert_rejected(&archive, &pending, Reason::InvalidPath(None));
}

#[test]
fn dot_component_is_invalid() {
    let dir = TempDir::new().unwrap();
    let (archive, pending) = hostile_one(&dir, Entry::deflated("a/./b", b"x"));
    assert_rejected(&archive, &pending, Reason::InvalidPath(None));
}

#[test]
fn trailing_slash_directory_marker_is_invalid() {
    let dir = TempDir::new().unwrap();
    let (archive, pending) = hostile_one(&dir, Entry::deflated("src/", b""));
    assert_rejected(&archive, &pending, Reason::InvalidPath(None));
}

/// A Windows-hostile component surfaces the shared portability rule it
/// violated as the parenthesized detail.
#[test]
fn portability_violation_carries_its_detail() {
    for (name, detail) in [
        ("src/a:b.h", "colon"),
        ("CON", "windows device name"),
        ("file.", "trailing dot"),
    ] {
        let dir = TempDir::new().unwrap();
        let (archive, pending) = hostile_one(&dir, Entry::deflated(name, b"x"));
        assert_rejected(&archive, &pending, Reason::InvalidPath(Some(detail)));
    }
}

#[test]
fn over_long_path_is_rejected() {
    let dir = TempDir::new().unwrap();
    let (archive, pending) = hostile_one(&dir, Entry::deflated(&"a".repeat(64), b"x"));
    let limits = Limits {
        max_path_len: 32,
        ..Limits::default()
    };
    let verdict = inspect(&archive, &pending, &limits).unwrap();
    assert_eq!(verdict, Verdict::Rejected(vec![Reason::PathTooLong]));
}

// ---------------------------------------------------------------------------
// Entry types.
// ---------------------------------------------------------------------------

#[test]
fn symlink_type_is_rejected() {
    // S_IFLNK in the external attributes' high bits.
    let dir = TempDir::new().unwrap();
    let mut entry = Entry::deflated("link", b"cabin.toml");
    entry.ext_attrs = 0o120_777 << 16;
    let (archive, pending) = hostile_one(&dir, entry);
    assert_rejected(&archive, &pending, Reason::ForbiddenEntryType);
}

#[test]
fn dos_directory_attribute_is_rejected() {
    let dir = TempDir::new().unwrap();
    let mut entry = Entry::deflated("adir", b"");
    entry.ext_attrs = 0x10;
    let (archive, pending) = hostile_one(&dir, entry);
    assert_rejected(&archive, &pending, Reason::ForbiddenEntryType);
}

// ---------------------------------------------------------------------------
// Collisions.
// ---------------------------------------------------------------------------

#[test]
fn duplicate_paths_are_rejected() {
    let dir = TempDir::new().unwrap();
    let bytes = assemble(&[
        Entry::deflated("src/a.cc", b"first"),
        Entry::deflated("src/a.cc", b"second"),
    ]);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    assert_rejected(&archive, &pending, Reason::DuplicatePath);
}

#[test]
fn case_folded_duplicate_names_are_rejected() {
    let dir = TempDir::new().unwrap();
    let bytes = assemble(&[
        Entry::deflated("README", b"a"),
        Entry::deflated("readme", b"b"),
    ]);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    assert_rejected(&archive, &pending, Reason::CaseConflict);
}

#[test]
fn file_used_as_directory_is_rejected() {
    // A regular file `src` alongside `src/main.cc` cannot be extracted;
    // both orderings reject the same way.
    for entries in [
        vec![
            Entry::deflated("src", b"i am a file"),
            Entry::deflated("src/main.cc", b"int main(){}"),
        ],
        vec![
            Entry::deflated("src/main.cc", b"int main(){}"),
            Entry::deflated("src", b"i am a file"),
        ],
    ] {
        let dir = TempDir::new().unwrap();
        let (archive, pending) = hostile_pending(&dir, &assemble(&entries));
        assert_rejected(&archive, &pending, Reason::PathConflict);
    }
}

#[test]
fn case_folded_file_used_as_directory_is_rejected() {
    // `a` (a file) versus `A/b` collides only under case folding.
    let dir = TempDir::new().unwrap();
    let bytes = assemble(&[Entry::deflated("a", b"x"), Entry::deflated("A/b", b"y")]);
    let (archive, pending) = hostile_pending(&dir, &bytes);
    assert_rejected(&archive, &pending, Reason::CaseConflict);
}

// ---------------------------------------------------------------------------
// Size discipline.
// ---------------------------------------------------------------------------

#[test]
fn entry_flood_is_rejected() {
    let dir = TempDir::new().unwrap();
    let entries: Vec<Entry> = (0..8)
        .map(|i| Entry::deflated(&format!("f{i}"), b"x"))
        .collect();
    let (archive, pending) = hostile_pending(&dir, &assemble(&entries));
    let limits = Limits {
        max_entries: 4,
        ..Limits::default()
    };
    let verdict = inspect(&archive, &pending, &limits).unwrap();
    assert_eq!(verdict, Verdict::Rejected(vec![Reason::TooManyEntries]));
}

#[test]
fn decompression_bomb_is_rejected() {
    // A tiny deflate body that declares a 64 MiB uncompressed size: the
    // cheap pre-check on the declared sum aborts before anything is
    // inflated, well under the ratio floor (~24 MiB for the default
    // entry cap).
    let dir = TempDir::new().unwrap();
    let mut entry = Entry::deflated("cabin.toml", b"x");
    entry.usize_ = 64 * 1024 * 1024;
    let (archive, pending) = hostile_one(&dir, entry);
    assert_rejected(&archive, &pending, Reason::DecompressedTooLarge);
}

#[test]
fn absolute_cap_bounds_low_ratio_archives() {
    let dir = TempDir::new().unwrap();
    let mut entry = Entry::deflated("cabin.toml", b"x");
    entry.usize_ = 64 * 1024;
    let (archive, pending) = hostile_one(&dir, entry);
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

// ---------------------------------------------------------------------------
// Banned zip features.
// ---------------------------------------------------------------------------

#[test]
fn unsupported_compression_method_is_rejected() {
    let dir = TempDir::new().unwrap();
    let mut entry = Entry::deflated("cabin.toml", b"x");
    entry.method = 99;
    let (archive, pending) = hostile_one(&dir, entry);
    assert_rejected(&archive, &pending, Reason::UnsupportedZipFeature("method"));
}

#[test]
fn data_descriptor_flag_is_rejected() {
    let dir = TempDir::new().unwrap();
    let mut entry = Entry::deflated("cabin.toml", b"x");
    entry.gp |= 0x0008;
    let (archive, pending) = hostile_one(&dir, entry);
    assert_rejected(
        &archive,
        &pending,
        Reason::UnsupportedZipFeature("data descriptor"),
    );
}

#[test]
fn utf8_bit_set_on_ascii_name_is_rejected() {
    let dir = TempDir::new().unwrap();
    let mut entry = Entry::deflated("cabin.toml", b"x");
    entry.gp = 0x0800;
    let (archive, pending) = hostile_one(&dir, entry);
    assert_rejected(&archive, &pending, Reason::UnsupportedZipFeature("gp flag"));
}

#[test]
fn non_ascii_name_without_utf8_bit_is_rejected() {
    let dir = TempDir::new().unwrap();
    let mut entry = Entry::deflated("café.h", b"x");
    entry.gp = 0;
    let (archive, pending) = hostile_one(&dir, entry);
    assert_rejected(&archive, &pending, Reason::UnsupportedZipFeature("gp flag"));
}

#[test]
fn extra_field_is_rejected() {
    let dir = TempDir::new().unwrap();
    let mut entry = Entry::deflated("cabin.toml", b"x");
    entry.extra = vec![0u8; 4];
    let (archive, pending) = hostile_one(&dir, entry);
    assert_rejected(
        &archive,
        &pending,
        Reason::UnsupportedZipFeature("extra field"),
    );
}

#[test]
fn entry_comment_is_rejected() {
    let dir = TempDir::new().unwrap();
    let mut entry = Entry::deflated("cabin.toml", b"x");
    entry.comment = b"hi".to_vec();
    let (archive, pending) = hostile_one(&dir, entry);
    assert_rejected(&archive, &pending, Reason::UnsupportedZipFeature("comment"));
}

#[test]
fn zip64_sentinel_is_rejected() {
    let dir = TempDir::new().unwrap();
    let mut entry = Entry::deflated("cabin.toml", b"x");
    entry.usize_ = 0xFFFF_FFFF;
    let (archive, pending) = hostile_one(&dir, entry);
    assert_rejected(&archive, &pending, Reason::UnsupportedZipFeature("zip64"));
}

#[test]
fn eocd_zip64_total_sentinel_is_rejected() {
    // A `0xFFFF` total-entries field in the EOCD is the zip64 marker; it
    // is reported distinctly (`unsupported_zip_feature (zip64)`), not
    // lumped into the generic `archive_invalid` for a count mismatch.
    let dir = TempDir::new().unwrap();
    let mut bytes = assemble(&[Entry::deflated("cabin.toml", b"x")]);
    let n = bytes.len();
    // EOCD total-entries field: `len - 22 + 10`.
    bytes[n - 12..n - 10].copy_from_slice(&0xFFFFu16.to_le_bytes());
    let (archive, pending) = hostile_pending(&dir, &bytes);
    assert_rejected(&archive, &pending, Reason::UnsupportedZipFeature("zip64"));
}

// ---------------------------------------------------------------------------
// Header integrity.
// ---------------------------------------------------------------------------

#[test]
fn stored_entry_with_size_disagreement_is_rejected() {
    let dir = TempDir::new().unwrap();
    let mut entry = Entry::stored("cabin.toml", b"hello");
    entry.usize_ = 6; // differs from the stored (compressed) size
    let (archive, pending) = hostile_one(&dir, entry);
    assert_rejected(&archive, &pending, Reason::HeaderMismatch("size"));
}

#[test]
fn crc_disagreement_is_rejected() {
    let dir = TempDir::new().unwrap();
    let mut entry = Entry::deflated("cabin.toml", b"hello");
    entry.crc = 0xDEAD_BEEF;
    let (archive, pending) = hostile_one(&dir, entry);
    assert_rejected(&archive, &pending, Reason::HeaderMismatch("crc"));
}

#[test]
fn local_header_disagreeing_with_central_is_rejected() {
    let dir = TempDir::new().unwrap();
    let mut entry = Entry::deflated("cabin.toml", b"hello");
    entry.local_crc = Some(entry.crc ^ 0xFFFF_FFFF);
    let (archive, pending) = hostile_one(&dir, entry);
    assert_rejected(&archive, &pending, Reason::HeaderMismatch("local header"));
}

#[test]
fn truncated_deflate_stream_is_rejected() {
    let dir = TempDir::new().unwrap();
    let mut entry = Entry::deflated("cabin.toml", b"hello world, a longer payload");
    entry.body.truncate(entry.body.len() / 2);
    entry.csize = entry.body.len() as u32;
    let (archive, pending) = hostile_one(&dir, entry);
    assert_rejected(&archive, &pending, Reason::HeaderMismatch("deflate"));
}

#[test]
fn non_final_deflate_block_at_eof_is_rejected() {
    // A single non-final stored deflate block (BFINAL = 0) that produces
    // exactly the declared bytes with a matching CRC. `flate2`'s reader
    // maps the input EOF that follows a non-final block to a silent
    // `Ok(0)`, so only requiring `Status::StreamEnd` keeps this
    // never-terminated stream from verifying.
    let dir = TempDir::new().unwrap();
    let data = b"hello";
    let mut body = vec![0x00]; // BFINAL = 0, BTYPE = 00 (stored), byte-aligned
    body.extend((data.len() as u16).to_le_bytes());
    body.extend((!(data.len() as u16)).to_le_bytes());
    body.extend_from_slice(data);
    let mut entry = Entry::deflated("cabin.toml", data);
    entry.body = body;
    entry.csize = entry.body.len() as u32; // CRC and usize already match `data`
    let (archive, pending) = hostile_one(&dir, entry);
    assert_rejected(&archive, &pending, Reason::HeaderMismatch("deflate"));
}

// ---------------------------------------------------------------------------
// Container framing.
// ---------------------------------------------------------------------------

#[test]
fn non_archive_bytes_are_rejected() {
    let dir = TempDir::new().unwrap();
    let (archive, pending) = hostile_pending(&dir, b"this is not a zip container");
    assert_rejected(&archive, &pending, Reason::ArchiveInvalid);
}

#[test]
fn truncated_container_is_rejected() {
    let dir = TempDir::new().unwrap();
    let bytes = assemble(&[Entry::deflated("cabin.toml", b"x")]);
    let (archive, pending) = hostile_pending(&dir, &bytes[..bytes.len() / 2]);
    assert_rejected(&archive, &pending, Reason::ArchiveInvalid);
}

#[test]
fn trailing_bytes_after_the_eocd_are_rejected() {
    // The EOCD must sit at exactly `len - 22`; appended bytes break the
    // tiling equation.
    let dir = TempDir::new().unwrap();
    let mut bytes = assemble(&[Entry::deflated("cabin.toml", b"x")]);
    bytes.extend_from_slice(b"trailing");
    let (archive, pending) = hostile_pending(&dir, &bytes);
    assert_rejected(&archive, &pending, Reason::ArchiveInvalid);
}

// ---------------------------------------------------------------------------
// Consistency pass (reached only once the container is well-formed).
// ---------------------------------------------------------------------------

#[test]
fn manifest_declaring_an_absent_source_is_rejected() {
    let dir = TempDir::new().unwrap();
    let manifest = "[package]\nname = \"demo\"\nversion = \"1.2.3\"\ncxx-standard = \"c++20\"\n\n\
                    [target.demo]\ntype = \"library\"\nsources = [\"src/lib.cc\"]\n";
    let (archive, pending) = hostile_one(&dir, Entry::deflated("cabin.toml", manifest.as_bytes()));
    assert_rejected(&archive, &pending, Reason::MissingSource);
}

#[test]
fn missing_manifest_is_rejected() {
    let dir = TempDir::new().unwrap();
    let (archive, pending) = hostile_one(&dir, Entry::deflated("src/a.cc", b"x"));
    assert_rejected(&archive, &pending, Reason::ManifestMissing);
}

#[test]
fn unparsable_manifest_is_rejected() {
    let dir = TempDir::new().unwrap();
    let (archive, pending) = hostile_one(&dir, Entry::deflated("cabin.toml", b"not toml ["));
    assert_rejected(&archive, &pending, Reason::ManifestInvalid);
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
        let (archive, pending) =
            hostile_one(&dir, Entry::deflated("cabin.toml", manifest.as_bytes()));
        let verdict = inspect(&archive, &pending, &Limits::default()).unwrap();
        assert_eq!(
            verdict,
            Verdict::Rejected(vec![Reason::ManifestInvalid]),
            "manifest: {manifest}"
        );
    }
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
    // slips through anyway it must be a rejection, not an operational
    // error that would wedge the verification queue.
    let dir = TempDir::new().unwrap();
    let (archive, mut pending) = benign(&dir);
    pending.metadata["schema"] = serde_json::json!(2);
    assert_rejected(&archive, &pending, Reason::MetadataMismatch);
}

#[test]
fn feature_table_mismatch_is_rejected() {
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
    fn rejected_prints_the_reason_code_and_exits_zero() {
        let dir = TempDir::new().unwrap();
        let (archive, pending) = hostile_one(&dir, Entry::deflated("../evil", b"x"));
        let entry = write_entry(&dir, &pending);
        verifier()
            .arg(&archive)
            .arg(&entry)
            .assert()
            .success()
            .stdout("{\"verdict\":\"rejected\",\"reasons\":[\"path_traversal\"]}\n");
    }

    #[test]
    fn rejected_reason_renders_its_parenthesized_detail() {
        // The reason string in the JSON is the code plus its fixed
        // detail, matching what lands in `verification_reason`.
        let dir = TempDir::new().unwrap();
        let (archive, pending) = hostile_one(&dir, Entry::deflated("src/a:b.h", b"x"));
        let entry = write_entry(&dir, &pending);
        verifier()
            .arg(&archive)
            .arg(&entry)
            .assert()
            .success()
            .stdout("{\"verdict\":\"rejected\",\"reasons\":[\"invalid_path (colon)\"]}\n");
    }

    #[test]
    fn operational_failures_exit_two_with_no_verdict() {
        let dir = TempDir::new().unwrap();
        let (_, pending) = benign(&dir);
        let entry = write_entry(&dir, &pending);
        verifier()
            .arg(dir.child("missing.zip").path())
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
