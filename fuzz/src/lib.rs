//! Shared harness code for the fuzz targets in `fuzz_targets/`.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use cabin_artifact::{
    ArtifactCache, ArtifactError, FetchEntry, FetchOptions, FetchPlan, FetchSource, PatchInput,
    SafeExtractOptions,
};
use cabin_core::Checksum;
use cabin_registry_verify::{Limits, PendingVersion, Reason, Verdict, VerifyError, inspect_bytes};
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
    inspect_listed_with(archive, checksum, metadata)
}

/// Inspect `archive` as the registry lists it with `metadata`.
fn inspect_listed_with(
    archive: &[u8],
    checksum: Checksum,
    metadata: serde_json::Value,
) -> Result<Verdict, VerifyError> {
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
/// Returns the entry names.
///
/// # Panics
///
/// When any of those promises is broken.
pub fn assert_client_agrees(archive: &[u8]) -> BTreeSet<String> {
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
    names
}

/// Stage the package `spec` describes through the packager and, when it
/// stages within the extraction caps, check that the registry verifier
/// verifies it as publish lists it and the client fetches back exactly
/// the archive's files, each the package's own.
/// `spec` is the package's `cabin.toml`, then `\0`-separated files, each
/// a safe relative path, a newline, and its contents. Returns whether
/// the checks ran.
///
/// # Panics
///
/// When a staged package fails any of those checks, or a scratch
/// directory cannot be created.
pub fn assert_package_round_trips(spec: &[u8]) -> bool {
    let Some((scratch, root)) = write_package(spec) else {
        return false;
    };
    let Ok(staged) = cabin_package::stage_with_project(
        &root.join("cabin.toml"),
        None,
        None,
        &cabin_core::WorkspaceDepRequirements::default(),
    ) else {
        return false;
    };
    // The verifier also needs the pinned upstream archive, which a spec
    // cannot carry.
    if staged.package.upstream.is_some() {
        return false;
    }
    let archive = staged.archive_bytes;
    // Listed with the metadata publish uploads.
    let metadata = serde_json::to_value(&staged.metadata).expect("staged metadata is JSON");
    let verdict = inspect_listed_with(&archive, staged.checksum.clone(), metadata)
        .expect("the verifier judges a package");
    // `cabin package` leaves the extraction caps to their consumers
    // (docs/package-format.md), so a package past one is out of scope.
    if let Verdict::Rejected(reasons) = &verdict
        && let [Reason::PathTooLong | Reason::TooManyEntries | Reason::DecompressedTooLarge] =
            reasons.as_slice()
    {
        return false;
    }
    assert_eq!(
        verdict,
        Verdict::Verified,
        "the verifier rejects a package the packager staged"
    );
    let names = assert_client_agrees(&archive);
    let fetched = cabin_artifact::fetch(
        &FetchPlan {
            entries: vec![FetchEntry {
                name: staged.name,
                version: staged.version,
                checksum: staged.checksum,
                source: FetchSource::InMemoryArchive(archive),
            }],
        },
        &ArtifactCache::new(scratch.path().join("cache")),
        FetchOptions::default(),
    )
    .expect("the client fetches a package the verifier verified");
    let source = &fetched.packages[0].source_dir;
    let mut extracted = BTreeSet::new();
    list_files(source, "", &mut extracted);
    assert_eq!(
        extracted, names,
        "the client extracts other files than the archive holds"
    );
    for name in &names {
        // Staging rewrites only workspace markers, which a standalone
        // package cannot carry, so the manifest compares too.
        let packed = fs::read(source.join(name)).expect("read an extracted file");
        assert!(
            fs::read(root.join(name)).is_ok_and(|own| own == packed),
            "a fetched file is not the package's own: {name:?}"
        );
    }
    true
}

/// The package `spec` describes, under `pkg/` in a scratch directory;
/// `None` for a spec the harness cannot write as files.
fn write_package(spec: &[u8]) -> Option<(TempDir, PathBuf)> {
    let scratch = tempfile::tempdir().expect("create a scratch directory");
    let root = scratch.path().join("pkg");
    fs::create_dir(&root).expect("create a scratch directory");
    let mut records = spec.split(|byte| *byte == 0);
    fs::write(root.join("cabin.toml"), records.next()?).expect("write a scratch file");
    for record in records {
        let split = record
            .iter()
            .position(|byte| *byte == b'\n')
            .unwrap_or(record.len());
        let path = std::str::from_utf8(&record[..split]).ok()?;
        if path.is_empty() || !cabin_fs::path::is_safe_relative_path(Path::new(path)) {
            return None;
        }
        let file = root.join(path);
        fs::create_dir_all(file.parent()?).ok()?;
        fs::write(&file, record.get(split + 1..).unwrap_or_default()).ok()?;
    }
    Some((scratch, root))
}

/// The files under `dir`, by path relative to it.
fn list_files(dir: &Path, prefix: &str, out: &mut BTreeSet<String>) {
    for entry in fs::read_dir(dir).expect("list an extracted directory") {
        let entry = entry.expect("list an extracted directory");
        let name = entry.file_name().into_string().expect("a UTF-8 file name");
        let rel = format!("{prefix}{name}");
        if entry.file_type().expect("stat an extracted entry").is_dir() {
            list_files(&entry.path(), &format!("{rel}/"), out);
        } else {
            out.insert(rel);
        }
    }
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
    for entry in fs::read_dir(dir).expect("list a written directory") {
        let path = entry.expect("list a written directory").path();
        let metadata = fs::symlink_metadata(&path).expect("stat a written entry");
        if metadata.is_dir() {
            assert_files_and_dirs(&path);
        } else {
            assert!(
                metadata.is_file(),
                "a tree holds a link or special file: {path:?}"
            );
            // A hard link is a regular file too, and may share its
            // bytes with one outside the tree.
            assert_eq!(metadata.nlink(), 1, "a tree holds a hard link: {path:?}");
        }
    }
}

/// The tree the patch target applies its input to: a C and C++
/// package, one file without a trailing newline.  `cargo fuzz-seeds`
/// writes its seed patches against a copy, and the harness's test
/// checks that every one applies here.
pub const PATCH_TREE: &[(&str, &str)] = &[
    (
        "cabin.toml",
        "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n",
    ),
    ("include/demo.h", "#pragma once\nint demo(void);"),
    (
        "src/demo.c",
        "#include \"demo.h\"\n\nint demo(void) {\n    return 0;\n}\n",
    ),
    (
        "src/main.cpp",
        "#include \"demo.h\"\n\nint main() {\n    return demo();\n}\n",
    ),
];

/// Apply `patch` to a fresh [`PATCH_TREE`], as the upstream
/// materializer applies a declared patch, and require that whatever
/// the applier decides, the tree stays directories and singly linked
/// regular files and a file beside it stays as it was.  An accepted
/// patch must also keep the grammar `cabin_artifact`'s patch module
/// relies on: nothing before its first `@@` line opens a C or C++
/// comment or raw string, splices lines, or starts a preprocessor
/// directive, so nothing lexical hides `@@` from a compiler.
///
/// # Panics
///
/// When an application breaks any of that.
pub fn assert_patch_contained(patch: &[u8]) {
    let (scratch, result) = patch_in_scratch(patch);
    let mut names: Vec<_> = fs::read_dir(scratch.path())
        .expect("list the scratch directory")
        .map(|entry| entry.expect("list the scratch directory").file_name())
        .collect();
    names.sort();
    assert_eq!(names, ["root", "sentinel"], "a patch wrote beside its tree");
    assert_eq!(
        fs::read(scratch.path().join("sentinel")).expect("read the sentinel back"),
        SENTINEL,
        "a patch rewrote a file beside its tree"
    );
    assert_files_and_dirs(&scratch.path().join("root"));
    if result.is_ok() {
        assert_lexable_head(patch);
    }
}

/// The file beside the patched tree.
const SENTINEL: &[u8] = b"int sentinel;\n";

/// A fresh scratch directory holding [`PATCH_TREE`] as `root` and
/// [`SENTINEL`] beside it, with `patch` applied to `root`.
fn patch_in_scratch(patch: &[u8]) -> (TempDir, Result<(), cabin_artifact::PatchError>) {
    let scratch = tempfile::tempdir().expect("create a scratch directory");
    let root = scratch.path().join("root");
    for (path, text) in PATCH_TREE {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("a tree path has a parent"))
            .expect("create the tree");
        fs::write(path, text).expect("write the tree");
    }
    fs::write(scratch.path().join("sentinel"), SENTINEL).expect("write the sentinel");
    let input = PatchInput {
        name: "fuzz.patch",
        bytes: patch,
    };
    let result = cabin_artifact::apply_unified_patches(&root, &[input]);
    (scratch, result)
}

/// What the patch module's grammar argument needs of an accepted patch:
/// it has a hunk, and no line before the first `@@` opens a comment or
/// raw string, splices lines, spells a trigraph, or starts a directive,
/// so nothing lexical hides `@@` from a compiler; a construct that
/// discards tokens needs code crafted around the diff, which this does
/// not check. The checks are on bytes, inside string and character
/// literals too, which spares lexing them: no other literal can span
/// lines, and accepted header paths hold none of these bytes, quoted or
/// not.
fn assert_lexable_head(patch: &[u8]) {
    // C's white space within a line, vertical tab included.
    const C_BLANKS: &[u8] = b" \t\x0b\x0c";
    // Compilers skip a leading byte order mark and end a line at a lone
    // carriage return as well as at a newline.
    let patch = patch.strip_prefix(b"\xef\xbb\xbf").unwrap_or(patch);
    for line in patch.split(|byte| matches!(byte, b'\n' | b'\r')) {
        if line.starts_with(b"@@") {
            return;
        }
        let start = line
            .iter()
            .position(|byte| !C_BLANKS.contains(byte))
            .unwrap_or(line.len());
        let code = &line[start..];
        assert!(
            !code.starts_with(b"#") && !code.starts_with(b"%:"),
            "an accepted patch can start a directive before its first hunk"
        );
        // A trigraph (`??/` is a backslash) is replaced before lexing.
        assert!(
            !line.contains(&b'?'),
            "an accepted patch can spell a trigraph before its first hunk"
        );
        assert!(
            !line.windows(2).any(|pair| pair == b"/*" || pair == b"//"),
            "an accepted patch can open a comment before its first hunk"
        );
        // C++23 and GCC splice across white space after the backslash.
        let end = line
            .iter()
            .rposition(|byte| !C_BLANKS.contains(byte))
            .map_or(0, |at| at + 1);
        assert!(
            !line[..end].ends_with(b"\\"),
            "an accepted patch can splice a line before its first hunk"
        );
        assert!(
            !opens_raw_string(line),
            "an accepted patch can open a raw string literal before its first hunk"
        );
    }
    panic!("an accepted patch has no hunk");
}

/// Whether `line` holds a C++ raw string's opening `R"delim(`, which can
/// span lines: at most 16 delimiter bytes, none a space, a control byte,
/// a parenthesis, or a backslash.
fn opens_raw_string(line: &[u8]) -> bool {
    (0..line.len()).any(|at| {
        let Some(rest) = line[at..].strip_prefix(b"R\"") else {
            return false;
        };
        let delimiter = rest
            .iter()
            .take_while(|byte| byte.is_ascii_graphic() && !b"()\\".contains(byte))
            .count();
        delimiter <= 16 && rest.get(delimiter) == Some(&b'(')
    })
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
    #[should_panic(expected = "a tree holds a hard link")]
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

    /// The grammar half of the patch oracle runs only on accepted
    /// patches, so every patch seed must apply.
    #[test]
    fn every_patch_seed_applies() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("seeds/apply_patch");
        let mut seen = 0;
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let (_scratch, result) = patch_in_scratch(&fs::read(&path).unwrap());
            assert!(result.is_ok(), "{}: {result:?}", path.display());
            seen += 1;
        }
        assert!(seen > 0, "no seeds in {}", dir.display());
    }

    /// The oracle's own regression tests: each input it must flag hides
    /// the first `@@` from a compiler, and the paths it must pass are
    /// ones `git diff` writes.
    #[test]
    #[should_panic(expected = "an accepted patch can start a directive")]
    fn a_directive_before_the_first_hunk_breaks_the_grammar() {
        assert_lexable_head(b"#if 0\n@@ -1 +1 @@\n");
    }

    #[test]
    fn quoted_and_apostrophe_paths_keep_the_grammar() {
        assert_lexable_head(b"--- \"a/caf\\303\\251.c\"\n+++ \"b/caf\\303\\251.c\"\n@@ -1 +1 @@\n");
        assert_lexable_head(b"--- a/it's.c\r\n+++ b/it's.c\r\n@@ -1 +1 @@\r\n");
        assert_lexable_head(b"diff --git \"a/\\303\\251R\" \"b/\\303\\251R\"\n@@ -1 +1 @@\n");
    }

    #[test]
    #[should_panic(expected = "an accepted patch can open a comment")]
    fn a_quote_in_a_literal_hides_no_comment() {
        assert_lexable_head(b"int x = 1'0; int c = '\"'; /* \"\n@@ -1 +1 @@\n");
    }

    #[test]
    #[should_panic(expected = "an accepted patch can splice a line")]
    fn white_space_after_a_backslash_still_splices() {
        assert_lexable_head(b"char *s = \"\\ \x0b\n@@ -1 +1 @@\n\";\n");
    }

    #[test]
    #[should_panic(expected = "an accepted patch can start a directive")]
    fn a_lone_carriage_return_starts_a_line() {
        assert_lexable_head(b"int x;\r#if 0\n@@ -1 +1 @@\n");
    }

    #[test]
    #[should_panic(expected = "an accepted patch can start a directive")]
    fn a_byte_order_mark_hides_no_directive() {
        assert_lexable_head(b"\xef\xbb\xbf#if 0\n@@ -1 +1 @@\n");
    }

    #[test]
    #[should_panic(expected = "an accepted patch can open a raw string literal")]
    fn a_raw_string_prefix_breaks_the_grammar() {
        assert_lexable_head(b"--- R\"(a/x\n@@ -1 +1 @@\n");
    }

    /// The round-trip checks run only on a package that stages, so every
    /// package seed must.
    #[test]
    fn every_package_seed_round_trips() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("seeds/package_round_trip");
        let mut seen = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            assert!(
                assert_package_round_trips(&std::fs::read(&path).unwrap()),
                "{} does not stage",
                path.display()
            );
            seen += 1;
        }
        assert!(seen > 0, "no package seeds in {}", dir.display());
    }

    /// A path past the extraction cap stages but is out of the round
    /// trip's scope, not a crash.
    #[test]
    fn a_package_past_an_extraction_cap_is_skipped() {
        let mut spec = b"[package]\nname = \"demo\"\nversion = \"1.2.3\"\n\0".to_vec();
        spec.extend(format!("{}/{}.h\nx", "a".repeat(130), "b".repeat(130)).bytes());
        assert!(!assert_package_round_trips(&spec));
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
