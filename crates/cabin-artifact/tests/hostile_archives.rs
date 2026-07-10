//! The adversarial corpus for `cabin-artifact`'s extractor.
//!
//! Every archive here is generated programmatically - no opaque
//! binary fixtures - and every case asserts the same three
//! invariants that make extraction safe against a hostile or
//! compromised registry:
//!
//! 1. the extraction fails with a *typed* error that names the
//!    offending entry (or the cap it crossed);
//! 2. nothing is written outside the extraction target - a canary
//!    file sits beside the target, and the whole tempdir is checked
//!    for stray entries;
//! 3. the bytes written stay bounded by the caps, and the fetch
//!    layer leaves no partial state behind.
//!
//! The client cannot assume a well-behaved registry, so these rules
//! hold for archives from any source: the hosted registry, a
//! third-party one, or a local file registry.

use std::fs::{self, File};
use std::io::Write as _;
use std::path::Path;

use assert_fs::TempDir;
use assert_fs::fixture::ChildPath;
use assert_fs::prelude::*;
use cabin_artifact::{ArtifactError, SafeExtractOptions, safe_extract_tar_gz};
use flate2::Compression;
use flate2::write::GzEncoder;
use predicates::prelude::*;

/// `canary.txt` sits next to the extraction target `out/`.  An entry
/// that escapes the target shows up either as clobbered canary
/// contents or as a stray file in the tempdir.
struct Corpus {
    dir: TempDir,
}

impl Corpus {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        dir.child("canary.txt").write_str("untouched").unwrap();
        dir.child("out").create_dir_all().unwrap();
        Corpus { dir }
    }

    fn archive(&self) -> ChildPath {
        self.dir.child("hostile.tar.gz")
    }

    fn dest(&self) -> ChildPath {
        self.dir.child("out")
    }

    /// Assert nothing escaped: the canary is intact and the tempdir
    /// still holds exactly the entries it started with.
    fn assert_contained(&self) {
        self.dir.child("canary.txt").assert("untouched");
        let mut names: Vec<String> = fs::read_dir(self.dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["canary.txt", "hostile.tar.gz", "out"],
            "an entry escaped the extraction target"
        );
    }

    /// Total bytes written under the extraction target.
    fn extracted_bytes(&self) -> u64 {
        fn walk(dir: &Path) -> u64 {
            fs::read_dir(dir)
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    let meta = entry.metadata().unwrap();
                    if meta.is_dir() {
                        walk(&entry.path())
                    } else {
                        meta.len()
                    }
                })
                .sum()
        }
        walk(self.dest().path())
    }

    fn extract(&self) -> Result<(), ArtifactError> {
        safe_extract_tar_gz(
            self.archive().path(),
            self.dest().path(),
            SafeExtractOptions::default(),
        )
    }

    /// Extract, expecting a rejection, and check containment.
    fn expect_rejected(&self) -> ArtifactError {
        let err = self.extract().expect_err("hostile archive was extracted");
        self.assert_contained();
        err
    }
}

/// A `tar::Builder` over a gzip stream, finished by [`finish`].
fn builder(path: &Path) -> tar::Builder<GzEncoder<File>> {
    let f = File::create(path).unwrap();
    tar::Builder::new(GzEncoder::new(f, Compression::default()))
}

fn finish(builder: tar::Builder<GzEncoder<File>>) {
    builder
        .into_inner()
        .unwrap()
        .finish()
        .unwrap()
        .flush()
        .unwrap();
}

/// Append an entry whose `name` / `linkname` bytes are written into
/// the header directly, bypassing the builder's validation (which
/// would refuse `..` and absolute paths).
fn append_raw<W: std::io::Write>(
    builder: &mut tar::Builder<W>,
    raw_name: &str,
    entry_type: tar::EntryType,
    link_name: Option<&str>,
    body: &[u8],
) {
    let mut header = tar::Header::new_old();
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
    header.set_entry_type(entry_type);
    {
        let old = header.as_old_mut();
        if let Some(target) = link_name {
            let bytes = target.as_bytes();
            let n = bytes.len().min(old.linkname.len());
            old.linkname[..n].copy_from_slice(&bytes[..n]);
        }
        let bytes = raw_name.as_bytes();
        let n = bytes.len().min(old.name.len());
        old.name[..n].copy_from_slice(&bytes[..n]);
    }
    header.set_cksum();
    builder.append(&header, body).unwrap();
}

/// Append an ordinary regular-file entry (GNU header, long paths
/// ride a `GNULongName` record automatically).
fn append_file<W: std::io::Write>(builder: &mut tar::Builder<W>, path: &str, body: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    builder
        .append_data(&mut header, path, std::io::Cursor::new(body))
        .unwrap();
}

#[test]
fn path_traversal_via_parent_component_is_rejected() {
    let corpus = Corpus::new();
    let mut b = builder(corpus.archive().path());
    append_raw(
        &mut b,
        "../canary.txt",
        tar::EntryType::Regular,
        None,
        b"pwned",
    );
    finish(b);

    match corpus.expect_rejected() {
        ArtifactError::UnsafeArchiveEntry(path) => assert!(path.contains("..")),
        other => panic!("expected UnsafeArchiveEntry, got {other:?}"),
    }
}

#[test]
fn deep_traversal_below_a_normal_component_is_rejected() {
    // `pkg/../../canary.txt` only escapes after the first component
    // is consumed; the check is lexical, so it fires up front.
    let corpus = Corpus::new();
    let mut b = builder(corpus.archive().path());
    append_raw(
        &mut b,
        "pkg/../../canary.txt",
        tar::EntryType::Regular,
        None,
        b"pwned",
    );
    finish(b);
    assert!(matches!(
        corpus.expect_rejected(),
        ArtifactError::UnsafeArchiveEntry(_)
    ));
}

#[test]
fn absolute_path_is_rejected() {
    let corpus = Corpus::new();
    let mut b = builder(corpus.archive().path());
    append_raw(
        &mut b,
        "/etc/passwd",
        tar::EntryType::Regular,
        None,
        b"pwned",
    );
    finish(b);
    assert!(matches!(
        corpus.expect_rejected(),
        ArtifactError::UnsafeArchiveEntry(_)
    ));
}

#[test]
fn symlink_escape_followed_by_write_through_is_rejected() {
    // The classic two-step: a symlink entry pointing outside the
    // target, then a regular file written *through* it.  The archive
    // is refused at the symlink, so the second entry never runs.
    let corpus = Corpus::new();
    let mut b = builder(corpus.archive().path());
    append_raw(&mut b, "escape", tar::EntryType::Symlink, Some(".."), b"");
    append_raw(
        &mut b,
        "escape/canary.txt",
        tar::EntryType::Regular,
        None,
        b"pwned",
    );
    finish(b);

    match corpus.expect_rejected() {
        ArtifactError::UnsupportedArchiveEntry(path) => assert_eq!(path, "escape"),
        other => panic!("expected UnsupportedArchiveEntry, got {other:?}"),
    }
    corpus
        .dest()
        .child("escape")
        .assert(predicate::path::missing());
}

#[test]
fn skipped_symlink_is_not_a_write_through_hole() {
    // Under the foundation-ports `skip_symlinks` opt-in the symlink
    // is skipped, never materialized.  The follow-up entry therefore
    // lands in a real directory *inside* the target rather than
    // writing through a link that points outside it.
    let corpus = Corpus::new();
    let mut b = builder(corpus.archive().path());
    append_raw(&mut b, "escape", tar::EntryType::Symlink, Some(".."), b"");
    append_file(&mut b, "escape/canary.txt", b"contained");
    finish(b);

    safe_extract_tar_gz(
        corpus.archive().path(),
        corpus.dest().path(),
        SafeExtractOptions {
            skip_symlinks: true,
            ..SafeExtractOptions::default()
        },
    )
    .unwrap();
    corpus.assert_contained();
    corpus.dest().child("escape/canary.txt").assert("contained");
}

#[test]
fn hard_link_to_an_outside_path_is_rejected() {
    let corpus = Corpus::new();
    let mut b = builder(corpus.archive().path());
    append_raw(
        &mut b,
        "alias",
        tar::EntryType::Link,
        Some("../canary.txt"),
        b"",
    );
    finish(b);
    match corpus.expect_rejected() {
        ArtifactError::UnsupportedArchiveEntry(path) => assert_eq!(path, "alias"),
        other => panic!("expected UnsupportedArchiveEntry, got {other:?}"),
    }
}

#[test]
fn device_and_fifo_entries_are_rejected() {
    for entry_type in [
        tar::EntryType::Char,
        tar::EntryType::Block,
        tar::EntryType::Fifo,
    ] {
        let corpus = Corpus::new();
        let mut b = builder(corpus.archive().path());
        append_raw(&mut b, "special", entry_type, None, b"");
        finish(b);
        match corpus.expect_rejected() {
            ArtifactError::UnsupportedArchiveEntry(path) => assert_eq!(path, "special"),
            other => panic!("expected UnsupportedArchiveEntry for {entry_type:?}, got {other:?}"),
        }
        corpus
            .dest()
            .child("special")
            .assert(predicate::path::missing());
    }
}

#[test]
fn duplicate_paths_are_rejected_instead_of_last_wins() {
    // Two entries for the same path: an extractor that lets the last
    // one win hands the build different bytes than the ones a
    // reviewer read in the first entry.
    let corpus = Corpus::new();
    let mut b = builder(corpus.archive().path());
    append_file(&mut b, "cabin.toml", b"[package]\nname = \"honest\"\n");
    append_file(&mut b, "cabin.toml", b"[package]\nname = \"evil\"\n");
    finish(b);

    match corpus.expect_rejected() {
        ArtifactError::DuplicateArchiveEntry(path) => assert_eq!(path, "cabin.toml"),
        other => panic!("expected DuplicateArchiveEntry, got {other:?}"),
    }
}

#[test]
fn a_regular_file_used_as_a_parent_directory_is_rejected() {
    // `src` as a file plus `src/main.cc` has no consistent
    // extraction; without the typed check it surfaces as a bare
    // filesystem error (or, in the other order, silently clobbers).
    for (first, second) in [("src", "src/main.cc"), ("src/main.cc", "src")] {
        let corpus = Corpus::new();
        let mut b = builder(corpus.archive().path());
        append_file(&mut b, first, b"x");
        append_file(&mut b, second, b"y");
        finish(b);
        match corpus.expect_rejected() {
            ArtifactError::ConflictingArchiveEntry { conflict, .. } => assert_eq!(conflict, "src"),
            other => panic!("expected ConflictingArchiveEntry, got {other:?}"),
        }
    }
}

#[test]
fn the_worst_legitimate_framing_still_fits_the_metadata_budget() {
    // The metadata budget is charged for tar headers, padding, and
    // the GNU long-name records the reader buffers.  The worst an
    // *honest* archive can spend is the entry cap's worth of
    // max-length paths, each of which needs a long-name record.  If
    // the budget were too tight, this archive - which a registry
    // could legitimately serve - would be rejected as a bomb.
    let corpus = Corpus::new();
    let mut b = builder(corpus.archive().path());
    for index in 0..10_000 {
        // 250 bytes: just under the path cap, so every entry rides a
        // long-name record (the plain header's name field holds 100).
        let path = format!("{:0>243}/{index:0>6}", "d");
        assert!(path.len() <= 256);
        append_file(&mut b, &path, b"x");
    }
    finish(b);

    corpus
        .extract()
        .expect("the worst legitimate framing was rejected");
    corpus.assert_contained();
    assert_eq!(corpus.extracted_bytes(), 10_000);
}

#[test]
fn windows_aliasing_and_device_paths_are_rejected() {
    // Shapes that stay lexically inside the target but that a Windows
    // filesystem aliases to a different destination (so two entries
    // collide) or routes to a device.  Rejected on every platform so
    // an archive built on Linux cannot smuggle them to a Windows
    // client.
    // `\` is deliberately not in this tar list: the tar reader
    // normalizes `\` to a path separator (harmless nesting) on every
    // platform, so it never reaches the portability check here.  (The
    // zip path, which sees the raw name, is covered in the unit
    // tests.)  Every case below is rejected everywhere.
    for name in [
        "cabin.toml.",       // trailing dot: Win32 strips it
        "cabin.toml ",       // trailing space: same
        " cabin.toml",       // leading space: Win32 strips it too
        "cabin.toml::$DATA", // NTFS default data stream
        "sub/evil:stream",   // NTFS alternate data stream
        "NUL",               // reserved device name
        "sub/COM1.txt",      // reserved device name with extension
        "aux.c",             // reserved stem, ordinary extension
        "COM\u{b9}",         // reserved superscript device form
    ] {
        let corpus = Corpus::new();
        let mut b = builder(corpus.archive().path());
        append_file(&mut b, name, b"x");
        finish(b);
        assert!(
            matches!(
                corpus.expect_rejected(),
                ArtifactError::UnsafeArchiveEntry(_)
            ),
            "{name:?} was not rejected"
        );
    }
}

#[test]
fn an_over_long_entry_path_is_rejected() {
    let corpus = Corpus::new();
    let mut b = builder(corpus.archive().path());
    // Rides a GNU long-name record, well past the 256-byte cap.
    let long = format!("{}.h", "a".repeat(400));
    append_file(&mut b, &long, b"x");
    finish(b);

    match corpus.expect_rejected() {
        ArtifactError::ArchiveEntryPathTooLong { path, limit } => {
            assert_eq!(limit, 256);
            // The diagnostic must not copy the whole hostile path.
            assert!(path.len() <= 70, "diagnostic path not truncated: {path}");
        }
        other => panic!("expected ArchiveEntryPathTooLong, got {other:?}"),
    }
}

#[test]
fn deep_nesting_is_bounded_by_the_path_cap() {
    let corpus = Corpus::new();
    let mut b = builder(corpus.archive().path());
    let deep = vec!["d"; 200].join("/");
    append_file(&mut b, &format!("{deep}/leaf"), b"x");
    finish(b);
    assert!(matches!(
        corpus.expect_rejected(),
        ArtifactError::ArchiveEntryPathTooLong { .. }
    ));
}

#[test]
fn an_entry_count_flood_is_rejected_at_the_cap() {
    // 10_001 empty entries: cheap to ship, expensive to materialize
    // as inodes.  Uses the real production cap.
    let corpus = Corpus::new();
    let mut b = builder(corpus.archive().path());
    for index in 0..10_001 {
        append_file(&mut b, &format!("f{index}"), b"");
    }
    finish(b);

    match corpus.expect_rejected() {
        ArtifactError::ArchiveTooManyEntries { limit } => assert_eq!(limit, 10_000),
        other => panic!("expected ArchiveTooManyEntries, got {other:?}"),
    }
    assert_eq!(corpus.extracted_bytes(), 0, "flood wrote file bytes");
}

#[test]
fn a_high_ratio_decompression_bomb_is_rejected() {
    // ~256 MiB of zeros in a handful of compressed KiB.  The stream
    // cap - a multiple of the compressed size, floored - stops it
    // long before either the per-entry or the aggregate cap could,
    // and disk stays bounded by that same cap.
    let corpus = Corpus::new();
    let mut b = builder(corpus.archive().path());
    let bomb = vec![0u8; 256 * 1024 * 1024];
    append_file(&mut b, "bomb.bin", &bomb);
    drop(bomb);
    finish(b);

    let compressed = fs::metadata(corpus.archive().path()).unwrap().len();
    let err = corpus.expect_rejected();
    assert!(
        matches!(err, ArtifactError::ArchiveStreamTooLarge { .. }),
        "{err:?}"
    );
    // The floor (64 MiB) is what a tiny archive is allowed; nothing
    // near the 256 MiB the bomb wanted reached the disk.
    let written = corpus.extracted_bytes();
    assert!(
        written <= 64 * 1024 * 1024,
        "bomb wrote {written} bytes from a {compressed}-byte archive"
    );
}

#[test]
fn a_metadata_bomb_is_rejected_without_buffering_it() {
    // A GNU long-name record whose payload is enormous.  The tar
    // reader buffers such a record in memory *before* any path or
    // type check can see it, so the metadata budget - not the
    // whole-stream cap - is what bounds the allocation.
    let corpus = Corpus::new();
    let mut b = builder(corpus.archive().path());
    let payload = vec![b'a'; 128 * 1024 * 1024];
    let mut header = tar::Header::new_gnu();
    header.set_size(payload.len() as u64);
    header.set_mode(0o644);
    header.set_entry_type(tar::EntryType::GNULongName);
    header.set_cksum();
    b.append(&header, &payload[..]).unwrap();
    drop(payload);
    append_file(&mut b, "cabin.toml", b"[package]\n");
    finish(b);

    let err = corpus.expect_rejected();
    match err {
        // 10_000 permitted entries x 4096 bytes of framing each.
        ArtifactError::ArchiveMetadataTooLarge { limit } => assert_eq!(limit, 10_000 * 4096),
        other => panic!("expected ArchiveMetadataTooLarge, got {other:?}"),
    }
    assert_eq!(corpus.extracted_bytes(), 0);
}

#[test]
fn a_header_size_larger_than_the_content_is_rejected() {
    // The header claims more bytes than the archive holds.  A naive
    // extractor writes a silently truncated source file; Cabin names
    // the entry and refuses, leaving nothing behind.
    let corpus = Corpus::new();
    let f = File::create(corpus.archive().path()).unwrap();
    let enc = GzEncoder::new(f, Compression::default());
    let mut b = tar::Builder::new(enc);
    let mut header = tar::Header::new_gnu();
    header.set_path("truncated.bin").unwrap();
    header.set_size(4096);
    header.set_mode(0o644);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    // `append` writes only the bytes it is given; the header's
    // `size` says 4096, so the stream ends mid-entry.
    b.append(&header, &b"short"[..]).unwrap();
    finish(b);

    match corpus.expect_rejected() {
        ArtifactError::ArchiveEntryTruncated {
            expected, actual, ..
        } => {
            assert_eq!(expected, 4096);
            assert!(actual < expected, "actual {actual} not short of {expected}");
        }
        other => panic!("expected ArchiveEntryTruncated, got {other:?}"),
    }
    assert_eq!(corpus.extracted_bytes(), 0, "truncated entry left bytes");
}

#[test]
fn a_header_size_smaller_than_the_content_is_rejected() {
    // The inverse: the declared size stops short, so the tar reader
    // parses the middle of the content as the next header.  Whatever
    // it makes of those bytes, the archive must not extract.
    let corpus = Corpus::new();
    let f = File::create(corpus.archive().path()).unwrap();
    let enc = GzEncoder::new(f, Compression::default());
    let mut b = tar::Builder::new(enc);
    let mut header = tar::Header::new_gnu();
    header.set_path("understated.bin").unwrap();
    header.set_size(2);
    header.set_mode(0o644);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    b.append(&header, &vec![b'x'; 4096][..]).unwrap();
    finish(b);

    let err = corpus.extract().unwrap_err();
    corpus.assert_contained();
    assert!(
        matches!(
            err,
            ArtifactError::Extract { .. } | ArtifactError::ArchiveEntryTruncated { .. }
        ),
        "{err:?}"
    );
}

/// Write a 512-byte tar header block followed by `body`, padded to
/// the next 512-byte boundary.
fn push_block(out: &mut Vec<u8>, header: &tar::Header, body: &[u8]) {
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(body);
    let pad = (512 - body.len() % 512) % 512;
    out.extend(std::iter::repeat_n(0u8, pad));
}

#[test]
fn a_pax_size_record_overrides_the_header_field() {
    // A PAX extended header carries the entry's real size, and the
    // regular header's `size` field says something else.  The
    // truncation check has to compare against the *effective* size
    // the tar reader uses, or every legitimate PAX archive - the
    // shape `bsdtar` and `git archive` emit - is rejected.
    let corpus = Corpus::new();
    let body = b"hello";

    let record = {
        // A PAX record is `<len> <key>=<value>\n`, where `<len>`
        // counts its own decimal digits too.
        let mut len = 1;
        loop {
            let candidate = format!("{len} size={}\n", body.len());
            if candidate.len() == len {
                break candidate;
            }
            len = candidate.len();
        }
    };

    let mut raw = Vec::new();
    let mut pax = tar::Header::new_ustar();
    pax.set_path("PaxHeaders/pax.bin").unwrap();
    pax.set_size(record.len() as u64);
    pax.set_mode(0o644);
    pax.set_entry_type(tar::EntryType::XHeader);
    pax.set_cksum();
    push_block(&mut raw, &pax, record.as_bytes());

    let mut header = tar::Header::new_ustar();
    header.set_path("pax.bin").unwrap();
    // Deliberately wrong: the PAX record above is authoritative.
    header.set_size(0);
    header.set_mode(0o644);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    push_block(&mut raw, &header, body);
    raw.extend(std::iter::repeat_n(0u8, 1024)); // tar terminator

    let f = File::create(corpus.archive().path()).unwrap();
    let mut enc = GzEncoder::new(f, Compression::default());
    enc.write_all(&raw).unwrap();
    enc.finish().unwrap().flush().unwrap();

    corpus
        .extract()
        .expect("PAX-decorated archive was rejected");
    corpus.assert_contained();
    assert_eq!(
        fs::read(corpus.dest().child("pax.bin").path()).unwrap(),
        body,
        "the PAX size did not drive the extracted bytes"
    );
}

#[test]
fn a_benign_archive_still_extracts() {
    // Positive control: the same harness, nothing hostile.
    let corpus = Corpus::new();
    let mut b = builder(corpus.archive().path());
    append_file(&mut b, "cabin.toml", b"[package]\nname = \"fmt\"\n");
    append_file(&mut b, "src/main.cc", b"int main() { return 0; }\n");
    // A path just under the cap, riding a GNU long-name record.
    append_file(
        &mut b,
        &format!("include/{}.h", "n".repeat(240)),
        b"#pragma once\n",
    );
    finish(b);

    corpus.extract().unwrap();
    corpus.assert_contained();
    corpus
        .dest()
        .child("cabin.toml")
        .assert(predicate::path::is_file());
    corpus
        .dest()
        .child("src/main.cc")
        .assert(predicate::path::is_file());
}
