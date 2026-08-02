//! Byte-exact unified-diff application for declared upstream patch
//! files.
//!
//! Every application of a patch declaration goes through this module,
//! so the transformation is identical on every platform and between
//! the producer and the verifier.  The ports publisher and the
//! registry's external verifier (`cabin-registry-verify`) both reach
//! it through [`materialize_upstream`](crate::materialize_upstream).
//! That symmetry is the whole point:
//! application is deliberately strict so two runs over the same bytes
//! can never disagree.
//!
//! - Text unified diffs only.  Binary hunks (`GIT binary patch`,
//!   `Binary files ... differ`) and NUL bytes are rejected.
//! - Application is byte-exact: context and removed lines must match
//!   the target file byte for byte, including line endings and the
//!   presence of a trailing newline.  There is no fuzz, no offset
//!   search, and no newline normalization.
//! - The strip level is fixed at the `-p1` equivalent: every path in
//!   a `---` / `+++` header must carry exactly one leading component
//!   (git's `a/` / `b/`) which is discarded, and the remainder must
//!   be a safe portable relative path inside the tree.
//! - Git's textual preamble (`diff --git`, `index`, mode lines) is
//!   accepted only in its exact shape; rename and copy headers are
//!   rejected because their effect is not representable as pure hunk
//!   application.  All other free text - a `format-patch` commit
//!   message, a signature trailer, arbitrary bytes around the diff -
//!   is rejected.  Declared patch files are exempt from the
//!   verifier's tree comparison, so this grammar is the only
//!   constraint on their bytes: free text would allow a single file
//!   to be both a valid patch and valid C/C++ source (the diff
//!   wrapped in comments), laundering unverified code past
//!   verification whenever the file is also reachable as a build
//!   input.  With every line held to the grammar, nothing before the
//!   first mandatory `@@` hunk header can open a comment or string
//!   literal: safe paths ban `"`, `*`, `?`, and `\` and admit no empty
//!   component (so no `//` either), the `-p1` prefixes are exactly `a`
//!   and `b`, and the header timestamp alphabet excludes those bytes
//!   too.  A naked `@@` is unlexable in C and C++, so the first hunk
//!   header always lands as raw code.  The hunk heading after `@@`
//!   stays free text (git puts enclosing-function context there) - it
//!   sits *after* that unlexable token, so it can never hide it.
//!
//! Every failure except a real filesystem fault is deterministic
//! given the patch bytes and the tree, which is what lets the
//! verifier map [`PatchError`] variants to rejection verdicts while
//! keeping [`PatchError::Io`] an operational (stay-pending) error.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Byte cap on any single file a patch reads from or creates in the
/// tree.  16 MiB comfortably exceeds any real source a build-system
/// or portability patch touches (the largest bundled amalgamation is
/// ~9 MiB).
pub const MAX_PATCH_TARGET_BYTES: u64 = 16 * 1024 * 1024;

/// Line-count cap on a patched target.  The byte cap alone does not
/// bound peak memory: a many-newline file splits into one ~24-byte
/// line record per line (and `assemble` holds a second such vector),
/// so a 16 MiB all-newline target would allocate ~800 MiB before any
/// verdict.  Capping the line count keeps that peak bounded while
/// still dwarfing any real source (the largest bundled amalgamation
/// is ~250k lines).
pub const MAX_PATCH_TARGET_LINES: usize = 4 * 1024 * 1024;

/// Cumulative cap on the bytes one declaration's patches may rewrite -
/// the sum of every patched file's assembled size across the whole
/// `patches` slice.  The per-target caps alone bound only one file at
/// a time: 16 declared patches each touching every file of a tree at
/// the extraction cap would rewrite gigabytes from a few hundred KiB
/// of patch input.  128 MiB still admits several full rewrites of the
/// largest realistic target (a ~9 MiB bundled amalgamation) and dwarfs
/// the handful of small files a build-system or portability
/// correction touches.
pub const MAX_PATCH_WORK_BYTES: u64 = 128 * 1024 * 1024;

/// Cumulative cap on the file entries one declaration's patches may
/// carry.  The byte budget does not bound the entry *count*: minimal
/// one-byte creations are ~40 bytes of diff each, so a 1 MiB patch
/// holds tens of thousands and the 16-patch limit hundreds of
/// thousands - each one a directory scan in
/// [`create_would_conflict`], which makes same-directory creation
/// quadratic.  1024 entries is orders of magnitude beyond any real
/// build-system or portability correction and keeps that scan cost
/// negligible.
pub const MAX_PATCH_FILE_ENTRIES: usize = 1024;

/// One declared patch: its declaration-order name (used in every
/// error) and its raw bytes.
#[derive(Debug, Clone, Copy)]
pub struct PatchInput<'a> {
    pub name: &'a str,
    pub bytes: &'a [u8],
}

/// Why a patch could not be applied.  Every variant except [`Io`] is
/// deterministic given the patch bytes and the tree contents.
///
/// [`Io`]: PatchError::Io
#[derive(Debug, Error)]
pub enum PatchError {
    #[error("patch `{patch}` is not a valid unified diff: {detail}")]
    Malformed { patch: String, detail: &'static str },
    #[error("patch `{patch}` contains binary content; only text unified diffs are supported")]
    Binary { patch: String },
    #[error(
        "patch `{patch}` names an unsafe file path {value:?}; diff headers must carry exactly \
         one strippable leading component followed by a portable relative path inside the tree"
    )]
    UnsafePath { patch: String, value: String },
    #[error("patch `{patch}` modifies `{file}`, which does not exist in the tree")]
    MissingTarget { patch: String, file: String },
    #[error(
        "patch `{patch}` cannot materialize `{file}`: the path or one of its ancestors is \
         already occupied"
    )]
    TargetConflict { patch: String, file: String },
    #[error(
        "patch `{patch}` does not apply to `{file}`: the tree's bytes do not match the patch \
         context exactly"
    )]
    ContextMismatch { patch: String, file: String },
    #[error(
        "patch `{patch}` target `{file}` is {size} {unit}; at most {limit} {unit} are supported"
    )]
    TargetTooLarge {
        patch: String,
        file: String,
        size: u64,
        limit: u64,
        unit: &'static str,
    },
    #[error(
        "applying the declared patches would rewrite {total} bytes of the tree; at most \
         {limit} bytes are supported"
    )]
    WorkBudgetExceeded { total: u64, limit: u64 },
    #[error("the declared patches carry {total} file entries; at most {limit} are supported")]
    TooManyFileEntries { total: usize, limit: usize },
    #[error("filesystem error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Apply `patches` to the tree rooted at `root`, in slice order.
///
/// Files created or modified by an earlier patch are visible to later
/// ones; a patch that touches several files applies its file entries
/// in the order they appear.
///
/// # Errors
/// The first failing [`PatchError`].  A failure can leave earlier
/// patches (and earlier file entries of the failing patch) applied;
/// callers treat the tree as scratch and discard it on error.
pub fn apply_unified_patches(root: &Path, patches: &[PatchInput<'_>]) -> Result<(), PatchError> {
    // Aggregate accounting across the whole slice: the per-patch
    // one-entry-per-path rule bounds repetition inside a patch, but
    // 16 declarations may each touch every file of the tree.
    apply_with_budget(root, patches, &mut 0)
}

/// [`apply_unified_patches`] with the work accumulator supplied, so
/// tests can exercise the budget at its boundary without performing
/// 128 MiB of real I/O.
fn apply_with_budget(
    root: &Path,
    patches: &[PatchInput<'_>],
    work: &mut u64,
) -> Result<(), PatchError> {
    let mut entry_count = 0usize;
    for patch in patches {
        let entries = parse_patch(patch.name, patch.bytes)?;
        // Counted before anything is applied, so an over-cap
        // declaration costs one parse rather than a tree full of
        // files (and the directory scans that come with them).
        entry_count = entry_count.saturating_add(entries.len());
        if entry_count > MAX_PATCH_FILE_ENTRIES {
            return Err(PatchError::TooManyFileEntries {
                total: entry_count,
                limit: MAX_PATCH_FILE_ENTRIES,
            });
        }
        for entry in &entries {
            apply_file_entry(root, patch.name, entry, work)?;
        }
    }
    Ok(())
}

/// One line of a text file or hunk: its bytes without the `\n`
/// terminator, plus whether the terminator was present.  The flag is
/// part of byte-exact matching - a final line without a trailing
/// newline only matches a hunk line marked `\ No newline at end of
/// file`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TextLine<'a> {
    bytes: &'a [u8],
    newline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HunkLineKind {
    Context,
    Remove,
    Add,
}

#[derive(Debug, Clone, Copy)]
struct HunkLine<'a> {
    kind: HunkLineKind,
    line: TextLine<'a>,
}

#[derive(Debug)]
struct Hunk<'a> {
    old_start: usize,
    old_count: usize,
    lines: Vec<HunkLine<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileOp {
    Create,
    Delete,
    Modify,
}

/// One `---` / `+++` file entry of a patch.
#[derive(Debug)]
struct FileEntry<'a> {
    /// Tree-relative forward-slash path, after the `-p1` strip.
    path: String,
    op: FileOp,
    hunks: Vec<Hunk<'a>>,
}

/// Split `bytes` after every `\n`.  A trailing byte run without a
/// terminator becomes a final line with `newline: false`; an empty
/// input yields no lines.
fn split_lines(bytes: &[u8]) -> Vec<TextLine<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            lines.push(TextLine {
                bytes: &bytes[start..index],
                newline: true,
            });
            start = index + 1;
        }
    }
    if start < bytes.len() {
        lines.push(TextLine {
            bytes: &bytes[start..],
            newline: false,
        });
    }
    lines
}

fn malformed(patch: &str, detail: &'static str) -> PatchError {
    PatchError::Malformed {
        patch: patch.to_owned(),
        detail,
    }
}

/// Parse one patch file into its file entries.
fn parse_patch<'a>(name: &str, bytes: &'a [u8]) -> Result<Vec<FileEntry<'a>>, PatchError> {
    // Unified diffs of text files never contain NUL; one anywhere
    // means binary content ended up in the patch.
    if bytes.contains(&0) {
        return Err(PatchError::Binary {
            patch: name.to_owned(),
        });
    }
    // In a well-formed unified diff every line is `\n`-terminated in
    // the patch file; the only exception is a trailing
    // `\ No newline at end of file` marker.  A patch whose final byte
    // is not `\n` and whose last line is not that marker is truncated
    // or malformed.  Rejecting it (git reports the same as "corrupt
    // patch") keeps `split_lines`' unterminated-final-line flag - which
    // encodes the *target's* missing trailing newline - from being
    // forged by the patch file's own missing terminator.
    if !bytes.is_empty()
        && !bytes.ends_with(b"\n")
        && !bytes
            .rsplit(|byte| *byte == b'\n')
            .next()
            .is_some_and(is_no_newline_marker)
    {
        return Err(malformed(name, "patch does not end with a newline"));
    }
    let lines = split_lines(bytes);
    let mut entries = Vec::new();
    let mut index = 0;
    // Track `diff --git` sections so a section that carries only mode /
    // `index` lines and no `---`/`+++` hunk - git's shape for an
    // empty-file add or delete or a mode-only change - is rejected as
    // unsupported instead of silently skipped (which would half-apply
    // a multi-file patch).
    let mut in_git_section = false;
    let mut section_had_entry = false;
    while index < lines.len() {
        let line = lines[index].bytes;
        let trimmed = strip_cr(line);
        if line.starts_with(b"Binary files ") || trimmed == b"GIT binary patch" {
            return Err(PatchError::Binary {
                patch: name.to_owned(),
            });
        }
        if [
            b"rename from ".as_slice(),
            b"rename to ",
            b"copy from ",
            b"copy to ",
        ]
        .iter()
        .any(|prefix| line.starts_with(prefix))
        {
            return Err(malformed(
                name,
                "git rename and copy headers are not supported",
            ));
        }
        if let Some(rest) = trimmed.strip_prefix(b"diff --git ") {
            if in_git_section && !section_had_entry {
                return Err(malformed(
                    name,
                    "git section has no hunks; empty-file or mode-only changes are unsupported",
                ));
            }
            validate_diff_git_paths(name, rest)?;
            in_git_section = true;
            section_had_entry = false;
            index += 1;
            continue;
        }
        // A git mode line (`new file mode 120000`, `index a..b 120000`,
        // ...) naming a non-regular file - a symlink, submodule, or
        // directory - has no text-hunk representation; git emits a
        // `/dev/null` content hunk that would otherwise materialize a
        // plain file with the link target as its bytes.  Reject it.
        if let Some(mode) = git_file_mode(trimmed)
            && !mode.starts_with(b"100")
        {
            return Err(malformed(
                name,
                "non-regular git file mode is unsupported (symlink, submodule, or directory)",
            ));
        }
        if !line.starts_with(b"--- ") {
            // Outside file entries only git's own preamble may
            // appear, and only in its exact shape (see the module
            // doc: any free-text line could open a C comment or
            // string literal that hides the first `@@` from a
            // compiler, turning the patch file into a C/diff
            // polyglot).  This also rejects hunk-shaped lines the
            // hunk framing lost track of, which would otherwise
            // half-apply a corrupted patch.
            validate_git_preamble_line(name, trimmed)?;
            index += 1;
            continue;
        }
        section_had_entry = true;
        let (entry, next) = parse_file_entry(name, &lines, index)?;
        index = next;
        entries.push(entry);
    }
    // The final git section must also have produced a file entry.
    if in_git_section && !section_had_entry {
        return Err(malformed(
            name,
            "git section has no hunks; empty-file or mode-only changes are unsupported",
        ));
    }
    if entries.is_empty() {
        return Err(malformed(name, "no file entries"));
    }
    // A faithful diff contains at most one entry per file - `git diff`
    // never repeats a path.  Beyond fidelity this bounds the apply
    // work: every modification re-reads and rewrites its whole target,
    // so repeated entries would let a 1 MiB patch amplify into
    // hundreds of gigabytes of I/O against a near-cap target
    // (thousands of alternating one-line modifications of the same
    // file), multiplied across the 16 declared patches.
    let mut seen = std::collections::HashSet::new();
    for entry in &entries {
        if !seen.insert(entry.path.as_str()) {
            return Err(malformed(name, "duplicate file entry for one path"));
        }
    }
    Ok(entries)
}

/// Parse one `---` / `+++` file entry starting at `lines[start]` (which
/// begins with `--- `), returning the entry and the index of the first
/// line after its hunks.
fn parse_file_entry<'a>(
    name: &str,
    lines: &[TextLine<'a>],
    start: usize,
) -> Result<(FileEntry<'a>, usize), PatchError> {
    let old_path = header_path(name, &lines[start].bytes[4..], "a")?;
    let mut index = start + 1;
    let new_line = lines
        .get(index)
        .ok_or_else(|| malformed(name, "expected a `+++` header after `---`"))?;
    if !new_line.bytes.starts_with(b"+++ ") {
        return Err(malformed(name, "expected a `+++` header after `---`"));
    }
    let new_path = header_path(name, &new_line.bytes[4..], "b")?;
    index += 1;

    let (path, op) = match (old_path, new_path) {
        (None, None) => {
            return Err(malformed(name, "both sides of a file entry are /dev/null"));
        }
        (None, Some(path)) => (path, FileOp::Create),
        (Some(path), None) => (path, FileOp::Delete),
        (Some(old), Some(new)) => {
            if old != new {
                return Err(malformed(
                    name,
                    "old and new file names disagree; renames are not supported",
                ));
            }
            (old, FileOp::Modify)
        }
    };

    let mut hunks = Vec::new();
    // A faithful diff's new-side start is fully determined: the
    // old-side start shifted by the net lines every earlier hunk
    // added or removed (a count-0 side anchors one line early).
    // `diff` and git always emit it that way, and the new side is
    // otherwise unused here - so without this check a corrupted or
    // hand-edited `+start` would apply silently.
    let mut delta: i128 = 0;
    while index < lines.len() && lines[index].bytes.starts_with(b"@@ -") {
        let (hunk, new_start, next) = parse_hunk(name, lines, index)?;
        index = next;
        let new_count = hunk
            .lines
            .iter()
            .filter(|line| line.kind != HunkLineKind::Remove)
            .count();
        let mut expected = hunk.old_start as i128 + delta;
        if hunk.old_count == 0 {
            expected += 1;
        }
        if new_count == 0 {
            expected -= 1;
        }
        if new_start as i128 != expected {
            return Err(malformed(
                name,
                "hunk new-side start disagrees with the cumulative line delta",
            ));
        }
        delta += new_count as i128 - hunk.old_count as i128;
        hunks.push(hunk);
    }
    validate_entry_shape(name, op, &hunks)?;
    Ok((FileEntry { path, op, hunks }, index))
}

/// The file mode a git preamble line declares, if any: the trailing
/// octal on `new file mode` / `deleted file mode` / `old mode` /
/// `new mode`, or on an `index <a>..<b> <mode>` line.  A regular file
/// is `100644` / `100755`; anything else (`120000` symlink, `160000`
/// submodule, `040000` directory) has no text-hunk representation.
fn git_file_mode(line: &[u8]) -> Option<&[u8]> {
    for prefix in [
        b"new file mode ".as_slice(),
        b"deleted file mode ",
        b"old mode ",
        b"new mode ",
    ] {
        if let Some(mode) = line.strip_prefix(prefix) {
            return Some(mode);
        }
    }
    // `index <old>..<new> <mode>`: the mode is the last space-separated
    // field, present only when the blob mode is unchanged.
    if line.starts_with(b"index ") {
        let mode = line.rsplit(|byte| *byte == b' ').next()?;
        if mode.len() == 6 && mode.iter().all(u8::is_ascii_digit) {
            return Some(mode);
        }
    }
    None
}

/// Validate a non-entry line against git's diff preamble grammar:
/// `index <hex>..<hex>[ <mode>]` or one of the four mode lines, each
/// in its exact shape.  Anything else is free text, which
/// [`parse_patch`] rejects outright.
fn validate_git_preamble_line(patch: &str, line: &[u8]) -> Result<(), PatchError> {
    if let Some(rest) = line.strip_prefix(b"index ") {
        return validate_index_line(patch, rest);
    }
    for prefix in [
        b"old mode ".as_slice(),
        b"new mode ",
        b"new file mode ",
        b"deleted file mode ",
    ] {
        if let Some(mode) = line.strip_prefix(prefix) {
            return if is_octal_mode(mode) {
                Ok(())
            } else {
                Err(malformed(patch, "malformed git mode line"))
            };
        }
    }
    Err(malformed(patch, "content outside the unified diff grammar"))
}

/// Validate the remainder of an `index ` line: two non-empty hex blob
/// names joined by `..`, optionally followed by one space and a
/// six-digit octal mode.
fn validate_index_line(patch: &str, rest: &[u8]) -> Result<(), PatchError> {
    let err = || malformed(patch, "malformed git index line");
    let (hashes, mode) = match rest.iter().position(|byte| *byte == b' ') {
        Some(space) => (&rest[..space], Some(&rest[space + 1..])),
        None => (rest, None),
    };
    if mode.is_some_and(|mode| !is_octal_mode(mode)) {
        return Err(err());
    }
    let dots = hashes
        .windows(2)
        .position(|pair| pair == b"..")
        .ok_or_else(err)?;
    for hash in [&hashes[..dots], &hashes[dots + 2..]] {
        if hash.is_empty() || hash.len() > 64 || !hash.iter().all(u8::is_ascii_hexdigit) {
            return Err(err());
        }
    }
    Ok(())
}

fn is_octal_mode(mode: &[u8]) -> bool {
    mode.len() == 6 && mode.iter().all(|byte| (b'0'..=b'7').contains(byte))
}

/// Validate the remainder of a `diff --git` line: exactly git's
/// `a/<path> b/<path>` for one identical safe path, in the plain or
/// the quoted spelling.  The authoritative paths come from the
/// `---`/`+++` headers; this line is validated only so it cannot
/// carry free text (see the module doc).
fn validate_diff_git_paths(patch: &str, rest: &[u8]) -> Result<(), PatchError> {
    let err = || malformed(patch, "malformed diff --git paths");
    let (old, new) = if rest.first() == Some(&b'"') {
        let (old, after) = decode_git_quoted_path(rest).ok_or_else(err)?;
        let second = after.strip_prefix(b" ").ok_or_else(err)?;
        if second.first() != Some(&b'"') {
            return Err(err());
        }
        let (new, tail) = decode_git_quoted_path(second).ok_or_else(err)?;
        if !tail.is_empty() {
            return Err(err());
        }
        (old, new)
    } else {
        // Unquoted, both sides name the same path (renames are
        // rejected), so the split point is fixed by length even when
        // the path contains spaces: `a/P b/P`.
        if rest.len() < 5 || !(rest.len() - 5).is_multiple_of(2) {
            return Err(err());
        }
        let path_len = (rest.len() - 5) / 2;
        if rest[2 + path_len] != b' ' {
            return Err(err());
        }
        (rest[..2 + path_len].to_vec(), rest[3 + path_len..].to_vec())
    };
    let stripped = |side: &[u8], prefix: &[u8]| -> Option<String> {
        let stripped = side.strip_prefix(prefix)?;
        let text = std::str::from_utf8(stripped).ok()?;
        cabin_core::upstream::is_safe_archive_path(text).then(|| text.to_owned())
    };
    match (stripped(&old, b"a/"), stripped(&new, b"b/")) {
        (Some(old), Some(new)) if old == new => Ok(()),
        _ => Err(err()),
    }
}

/// GNU diff appends a tab plus a timestamp to `---`/`+++` headers.
/// That trailer is the one free-text field the grammar would
/// otherwise admit before the first hunk, so it is held to the
/// timestamp alphabet: alphanumerics and `: . , + -` and space -
/// nothing that can open or extend a C/C++ comment, string, char
/// literal, line splice, or trigraph (`/`, `*`, `"`, `'`, `\`, `?`).
fn validate_header_trailer(patch: &str, trailer: &[u8]) -> Result<(), PatchError> {
    if trailer.is_empty() {
        return Ok(());
    }
    let timestamp = trailer
        .strip_prefix(b"\t")
        .ok_or_else(|| malformed(patch, "invalid header timestamp"))?;
    if timestamp.iter().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b':' | b'.' | b',' | b'+' | b'-')
    }) {
        Ok(())
    } else {
        Err(malformed(patch, "invalid header timestamp"))
    }
}

/// A file entry must carry hunks, and a creation or deletion must be
/// the one shape `diff` emits for it: a single hunk against the
/// empty side.  Anything else is not a faithful unified diff of the
/// change.
fn validate_entry_shape(name: &str, op: FileOp, hunks: &[Hunk<'_>]) -> Result<(), PatchError> {
    let Some(first) = hunks.first() else {
        return Err(malformed(name, "file entry has no hunks"));
    };
    match op {
        FileOp::Create => {
            if hunks.len() != 1 || first.old_count != 0 {
                return Err(malformed(
                    name,
                    "a file creation must be a single hunk against an empty file",
                ));
            }
        }
        FileOp::Delete => {
            let new_count = first
                .lines
                .iter()
                .filter(|line| line.kind != HunkLineKind::Remove)
                .count();
            if hunks.len() != 1 || new_count != 0 {
                return Err(malformed(
                    name,
                    "a file deletion must be a single hunk removing every line",
                ));
            }
        }
        FileOp::Modify => {}
    }
    Ok(())
}

/// Strip one trailing `\r`, so headers of a CRLF-encoded patch file
/// parse.  Hunk *content* lines keep their `\r` bytes - byte-exact
/// matching means a CRLF patch applies to CRLF content.
fn strip_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// Parse a `---` / `+++` header path: everything up to the first tab
/// (GNU diff and git append a tab plus a timestamp there), `-p1`
/// stripped.  `expected_prefix` is the exact component the `-p1`
/// strip discards - `a` on the old side, `b` on the new.  `None` is
/// `/dev/null`.
fn header_path(
    patch: &str,
    raw: &[u8],
    expected_prefix: &str,
) -> Result<Option<String>, PatchError> {
    let raw = strip_cr(raw);
    let unsafe_path = |value: &[u8]| PatchError::UnsafePath {
        patch: patch.to_owned(),
        value: String::from_utf8_lossy(value).into_owned(),
    };
    // A path git had to quote (`core.quotePath`, on by default) is
    // wrapped in double quotes with C-style escapes; a plain path is
    // taken verbatim up to the first tab.  Decoding the quoted form
    // means ordinary `git diff` output for a non-ASCII filename
    // (`--- "a/caf\303\251.c"`) applies without the user changing
    // their Git configuration.
    let decoded = if raw.first() == Some(&b'"') {
        let (decoded, trailer) = decode_git_quoted_path(raw).ok_or_else(|| unsafe_path(raw))?;
        validate_header_trailer(patch, trailer)?;
        decoded
    } else {
        let end = raw
            .iter()
            .position(|byte| *byte == b'\t')
            .unwrap_or(raw.len());
        validate_header_trailer(patch, &raw[end..])?;
        raw[..end].to_vec()
    };
    if decoded == b"/dev/null" {
        return Ok(None);
    }
    let text = std::str::from_utf8(&decoded).map_err(|_| unsafe_path(&decoded))?;
    // Fixed `-p1`: the discarded component must be git's exact `a` /
    // `b`, the only prefixes `git diff` emits at this strip level (the
    // `diff --git` line is held to the same pair).  Accepting an
    // arbitrary non-empty component would leave a free-text field in
    // the one file the verifier's tree comparison exempts - the
    // C/diff-polyglot hole the module doc's grammar argument closes.
    let (prefix, stripped) = text.split_once('/').ok_or_else(|| unsafe_path(&decoded))?;
    if prefix != expected_prefix || !cabin_core::upstream::is_safe_archive_path(stripped) {
        return Err(unsafe_path(&decoded));
    }
    Ok(Some(stripped.to_owned()))
}

/// Decode git's quoted-path form: a `"`-delimited string with C-style
/// escapes (`\NNN` octal, `\a \b \t \n \v \f \r`, `\"`, `\\`).  The
/// decoded bytes are the raw filename; the caller validates them and
/// whatever follows the closing quote (a header timestamp, the second
/// path of a `diff --git` line).  Returns `None` for anything not a
/// well-formed quoted path (unterminated, or a stray escape).
fn decode_git_quoted_path(raw: &[u8]) -> Option<(Vec<u8>, &[u8])> {
    if raw.first() != Some(&b'"') {
        return None;
    }
    let mut out = Vec::new();
    let mut index = 1;
    loop {
        let byte = *raw.get(index)?;
        index += 1;
        match byte {
            b'"' => break,
            b'\\' => {
                let escaped = *raw.get(index)?;
                index += 1;
                match escaped {
                    b'a' => out.push(0x07),
                    b'b' => out.push(0x08),
                    b't' => out.push(b'\t'),
                    b'n' => out.push(b'\n'),
                    b'v' => out.push(0x0b),
                    b'f' => out.push(0x0c),
                    b'r' => out.push(b'\r'),
                    b'"' => out.push(b'"'),
                    b'\\' => out.push(b'\\'),
                    d0 @ b'0'..=b'7' => {
                        // Exactly three octal digits, as git emits.
                        let d1 = *raw.get(index)?;
                        let d2 = *raw.get(index + 1)?;
                        index += 2;
                        if !d1.is_ascii_digit() || d1 > b'7' || !d2.is_ascii_digit() || d2 > b'7' {
                            return None;
                        }
                        let value = (u32::from(d0 - b'0') << 6)
                            | (u32::from(d1 - b'0') << 3)
                            | u32::from(d2 - b'0');
                        out.push(u8::try_from(value).ok()?);
                    }
                    _ => return None,
                }
            }
            byte => out.push(byte),
        }
    }
    Some((out, &raw[index..]))
}

/// Parse the hunk starting at `lines[start]` (which begins with
/// `@@ -`).  Returns the hunk, the header's new-side start (the
/// caller validates it against the cumulative line delta), and the
/// index of the first line after the hunk.
fn parse_hunk<'a>(
    patch: &str,
    lines: &[TextLine<'a>],
    start: usize,
) -> Result<(Hunk<'a>, usize, usize), PatchError> {
    let header = strip_cr(lines[start].bytes);
    let (old_start, old_count, new_start, new_count) =
        parse_hunk_header(header).ok_or_else(|| malformed(patch, "invalid hunk header"))?;

    let mut body = Vec::new();
    let mut old_seen = 0usize;
    let mut new_seen = 0usize;
    let mut index = start + 1;
    while old_seen < old_count || new_seen < new_count {
        let Some(line) = lines.get(index) else {
            return Err(malformed(patch, "truncated hunk"));
        };
        index += 1;
        let (marker, content) = match line.bytes.split_first() {
            Some((marker, content)) => (*marker, content),
            // `diff` always writes an explicit space marker on an
            // empty context line; strictness rejects the bare-line
            // spelling some tools tolerate.
            None => {
                return Err(malformed(
                    patch,
                    "hunk line must start with ' ', '+', '-', or '\\'",
                ));
            }
        };
        let kind = match marker {
            b' ' if old_seen < old_count && new_seen < new_count => HunkLineKind::Context,
            b'-' if old_seen < old_count => HunkLineKind::Remove,
            b'+' if new_seen < new_count => HunkLineKind::Add,
            b' ' | b'-' | b'+' => {
                return Err(malformed(
                    patch,
                    "hunk line counts disagree with the header",
                ));
            }
            b'\\' => {
                // The only legal `\`-line is the exact no-newline
                // marker; a `\ garbage` line is malformed, not a
                // signal that the previous line lacks a terminator.
                if !is_no_newline_marker(line.bytes) {
                    return Err(malformed(patch, "invalid no-newline marker"));
                }
                mark_no_newline(patch, &mut body)?;
                continue;
            }
            _ => {
                return Err(malformed(
                    patch,
                    "hunk line must start with ' ', '+', '-', or '\\'",
                ));
            }
        };
        match kind {
            HunkLineKind::Context => {
                old_seen += 1;
                new_seen += 1;
            }
            HunkLineKind::Remove => old_seen += 1,
            HunkLineKind::Add => new_seen += 1,
        }
        body.push(HunkLine {
            kind,
            line: TextLine {
                bytes: content,
                newline: line.newline,
            },
        });
    }
    // A trailing marker applies to the hunk's final line.  A
    // `\`-prefixed line here must be the exact no-newline marker; a
    // `\ garbage` line is malformed.
    if let Some(line) = lines.get(index)
        && line.bytes.starts_with(b"\\")
    {
        if !is_no_newline_marker(line.bytes) {
            return Err(malformed(patch, "invalid no-newline marker"));
        }
        mark_no_newline(patch, &mut body)?;
        index += 1;
    }
    Ok((
        Hunk {
            old_start,
            old_count,
            lines: body,
        },
        new_start,
        index,
    ))
}

/// The exact `\ No newline at end of file` marker (a trailing `\r`
/// from a CRLF-encoded patch is tolerated).  Anything else beginning
/// with `\` is malformed.
fn is_no_newline_marker(line: &[u8]) -> bool {
    strip_cr(line) == b"\\ No newline at end of file"
}

fn mark_no_newline(patch: &str, body: &mut [HunkLine<'_>]) -> Result<(), PatchError> {
    let Some(last) = body.last_mut() else {
        return Err(malformed(patch, "misplaced no-newline marker"));
    };
    // A line takes at most one no-newline marker; a second (the line's
    // terminator is already cleared) is a malformed duplicate.
    if !last.line.newline {
        return Err(malformed(patch, "duplicate no-newline marker"));
    }
    last.line.newline = false;
    Ok(())
}

/// Parse `@@ -<start>[,<count>] +<start>[,<count>] @@...` into
/// `(old_start, old_count, new_start, new_count)`.  Absent counts
/// default to 1; a zero start is only meaningful with a zero count
/// (pure insertion / deletion anchor).
fn parse_hunk_header(header: &[u8]) -> Option<(usize, usize, usize, usize)> {
    let text = std::str::from_utf8(header).ok()?;
    let rest = text.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, rest) = rest.split_once(" @@")?;
    // Anything after the closing `@@` is a section heading; it must
    // be empty or begin with a space.
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    let (old_start, old_count) = parse_range(old)?;
    let (new_start, new_count) = parse_range(new)?;
    if (old_count > 0 && old_start == 0) || (new_count > 0 && new_start == 0) {
        return None;
    }
    // A hunk that neither removes nor adds anything (`@@ -0,0 +0,0 @@`)
    // is a no-op git never emits; accepting it would let a fake
    // zero-length hunk create or delete an empty file, slipping past
    // the hunkless-section guard.
    if old_count == 0 && new_count == 0 {
        return None;
    }
    Some((old_start, old_count, new_start, new_count))
}

fn parse_range(range: &str) -> Option<(usize, usize)> {
    let (start, count) = match range.split_once(',') {
        Some((start, count)) => (start, count.parse().ok()?),
        None => (range, 1),
    };
    // Reject empty and non-digit spellings (`usize::parse` handles
    // both) so garbage headers never parse.
    Some((start.parse().ok()?, count))
}

/// Apply one file entry to the tree.
fn apply_file_entry(
    root: &Path,
    patch: &str,
    entry: &FileEntry<'_>,
    work: &mut u64,
) -> Result<(), PatchError> {
    let target = root.join(Path::new(&entry.path));
    match entry.op {
        FileOp::Create => {
            // The target must be materializable: nothing already at the
            // path, no ancestor occupied by a regular file, and - so
            // creation matches the case-sensitive Linux verifier - no
            // component that only case-collides with an existing entry
            // (on a case-insensitive host `Src/new.c` would otherwise
            // reuse an existing `src/`, producing `src/new.c`).  The
            // tree holds no symlinks (extraction skips them), so this
            // is a plain per-component scan.
            let conflict =
                create_would_conflict(root, &entry.path).map_err(|source| PatchError::Io {
                    path: target.clone(),
                    source,
                })?;
            if conflict {
                return Err(PatchError::TargetConflict {
                    patch: patch.to_owned(),
                    file: entry.path.clone(),
                });
            }
            let content = assemble(patch, entry, &[])?;
            check_output_size(patch, entry, &content, work)?;
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|source| PatchError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            fs::write(&target, content).map_err(|source| PatchError::Io {
                path: target,
                source,
            })
        }
        FileOp::Modify | FileOp::Delete => {
            // Case-exact existence: the Linux verifier looks the target
            // up case-sensitively, so a case-mismatched header (`Src/Foo.c`
            // for `src/foo.c`) that a case-insensitive host would match
            // must be a `missing target` here too - otherwise
            // preparation succeeds locally for a package the verifier
            // rejects.
            let exists =
                path_is_case_exact(root, &entry.path).map_err(|source| PatchError::Io {
                    path: target.clone(),
                    source,
                })?;
            if !exists {
                return Err(PatchError::MissingTarget {
                    patch: patch.to_owned(),
                    file: entry.path.clone(),
                });
            }
            // One explicit stat, not `is_file()`: a failed metadata
            // lookup must stay an operational `Io` (the verifier
            // leaves the version pending), never collapse into the
            // deterministic `TargetConflict` rejection.
            let metadata = target.metadata().map_err(|source| PatchError::Io {
                path: target.clone(),
                source,
            })?;
            if !metadata.is_file() {
                return Err(PatchError::TargetConflict {
                    patch: patch.to_owned(),
                    file: entry.path.clone(),
                });
            }
            // Bound the target before reading it: `split_lines`
            // allocates a line record per line, so a many-newline
            // file within the extraction caps would amplify into
            // gigabytes of metadata.
            let size = metadata.len();
            if size > MAX_PATCH_TARGET_BYTES {
                return Err(PatchError::TargetTooLarge {
                    patch: patch.to_owned(),
                    file: entry.path.clone(),
                    size,
                    limit: MAX_PATCH_TARGET_BYTES,
                    unit: "bytes",
                });
            }
            // Charge the read before performing it: re-reading a
            // near-cap target once per declared patch is the other half
            // of the amplification the budget bounds.
            charge_work(work, size)?;
            let old_bytes = fs::read(&target).map_err(|source| PatchError::Io {
                path: target.clone(),
                source,
            })?;
            // A plain scan, not the `bytecount` crate: this runs once
            // per patched file over an already-capped (≤16 MiB) buffer,
            // not worth a dependency.
            #[allow(clippy::naive_bytecount)]
            let line_count = old_bytes.iter().filter(|byte| **byte == b'\n').count() + 1;
            if line_count > MAX_PATCH_TARGET_LINES {
                return Err(PatchError::TargetTooLarge {
                    patch: patch.to_owned(),
                    file: entry.path.clone(),
                    size: line_count as u64,
                    limit: MAX_PATCH_TARGET_LINES as u64,
                    unit: "lines",
                });
            }
            let old_lines = split_lines(&old_bytes);
            let content = assemble(patch, entry, &old_lines)?;
            if entry.op == FileOp::Delete {
                // The single deletion hunk must have consumed the
                // whole file; leftover bytes mean the patch was
                // diffed against something else.
                if !content.is_empty() {
                    return Err(PatchError::ContextMismatch {
                        patch: patch.to_owned(),
                        file: entry.path.clone(),
                    });
                }
                return fs::remove_file(&target).map_err(|source| PatchError::Io {
                    path: target,
                    source,
                });
            }
            // The output cap holds too: a patch that grows a
            // near-cap target past the limit (or many patches each
            // adding to the same file) would otherwise materialize a
            // file the next patch cannot even read.
            check_output_size(patch, entry, &content, work)?;
            fs::write(&target, content).map_err(|source| PatchError::Io {
                path: target,
                source,
            })
        }
    }
}

/// Reject a patched or created file whose assembled size exceeds the
/// per-target byte or line cap, keeping every materialized file
/// within both bounds the read side relies on - a later patch (or a
/// re-run) must never find a target this application produced over
/// either cap.
/// Charge `bytes` against the slice-wide work budget.  Every byte the
/// application reads from or writes to the tree is charged, so a
/// declaration cannot multiply a small patch input into gigabytes of
/// scratch I/O by having each of its patches touch every large file.
fn charge_work(work: &mut u64, bytes: u64) -> Result<(), PatchError> {
    *work = work.saturating_add(bytes);
    if *work > MAX_PATCH_WORK_BYTES {
        return Err(PatchError::WorkBudgetExceeded {
            total: *work,
            limit: MAX_PATCH_WORK_BYTES,
        });
    }
    Ok(())
}

fn check_output_size(
    patch: &str,
    entry: &FileEntry<'_>,
    content: &[u8],
    work: &mut u64,
) -> Result<(), PatchError> {
    charge_work(work, content.len() as u64)?;
    if content.len() as u64 > MAX_PATCH_TARGET_BYTES {
        return Err(PatchError::TargetTooLarge {
            patch: patch.to_owned(),
            file: entry.path.clone(),
            size: content.len() as u64,
            limit: MAX_PATCH_TARGET_BYTES,
            unit: "bytes",
        });
    }
    // The same plain scan as the read side: once per patched file
    // over an already-bounded buffer.
    #[allow(clippy::naive_bytecount)]
    let line_count = content.iter().filter(|byte| **byte == b'\n').count() + 1;
    if line_count > MAX_PATCH_TARGET_LINES {
        return Err(PatchError::TargetTooLarge {
            patch: patch.to_owned(),
            file: entry.path.clone(),
            size: line_count as u64,
            limit: MAX_PATCH_TARGET_LINES as u64,
            unit: "lines",
        });
    }
    Ok(())
}

/// Whether `rel` (a forward-slash relative path) names an entry under
/// `root` whose every component matches a real directory entry
/// byte-for-byte.  Unlike `Path::exists`, this is case-sensitive on
/// every host, so a case-mismatched path a case-insensitive
/// filesystem would resolve is reported absent - matching the Linux
/// verifier's case-sensitive lookup and keeping behavior
/// platform-independent.  The trees involved hold no symlinks (extraction skips them), so a
/// plain per-component `read_dir` scan is sufficient.
///
/// # Errors
/// The underlying `read_dir` failure when a directory on the path
/// cannot be scanned - a filesystem fault the caller must treat as
/// operational (leave pending), never as an absent component.
pub fn path_is_case_exact(root: &Path, rel: &str) -> io::Result<bool> {
    let mut current = root.to_path_buf();
    for component in rel.split('/') {
        let entries = match fs::read_dir(&current) {
            Ok(entries) => entries,
            // A missing directory, or a non-directory on the path
            // (an earlier component is a regular file), means the
            // path deterministically does not exist - absent, not an
            // operational fault.  Every other error is a real
            // filesystem fault and propagates.
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                return Ok(false);
            }
            Err(err) => return Err(err),
        };
        let mut present = false;
        for entry in entries {
            if entry?.file_name().as_encoded_bytes() == component.as_bytes() {
                present = true;
                break;
            }
        }
        if !present {
            return Ok(false);
        }
        current.push(component);
    }
    Ok(true)
}

/// Case-fold a directory-entry name for collision comparison, or
/// `None` for a non-UTF-8 name (which cannot collide with a validated
/// UTF-8 path component under any folding).
fn case_fold(name: &std::ffi::OsStr) -> Option<String> {
    name.to_str().map(fold_for_collision)
}

/// The collision key: full Unicode `to_lowercase` (the package
/// archive's case-conflict rule) plus NFC normalization.  The
/// normalization step matters on macOS, whose filesystems resolve
/// lookups normalization-insensitively: an upstream entry stored in
/// decomposed form (`e\u{301}toile.c`) and a creation target spelled
/// composed (`\u{e9}toile.c`) alias the same file there but are two
/// distinct entries on Linux, so treating them as distinct would let
/// preparation silently overwrite the upstream file on macOS while
/// the Linux verifier materializes both.
fn fold_for_collision(name: &str) -> String {
    cabin_core::upstream::collision_fold(name)
}

/// Whether creating `rel` under `root` would conflict with an existing
/// entry: the leaf already exists, an ancestor is a regular file, or -
/// the case-sensitivity guard - a component only case-collides with an
/// existing entry.  The last case matters on a case-insensitive host,
/// where `create_dir_all` would silently reuse a differently-cased
/// directory (`src/` for a declared `Src/`), diverging from the
/// case-sensitive Linux verifier that would materialize a distinct
/// path and then reject the resulting case conflict.  Collisions
/// compare under the collision fold: full Unicode `to_lowercase`
/// (matching the package archive's case-conflict rule, so `Ä/` vs
/// `ä/` is caught too) plus NFC normalization (so a decomposed and a
/// composed spelling of the same name collide, matching macOS's
/// normalization-insensitive lookups).
///
/// # Errors
/// The underlying `read_dir` or ancestor `metadata` failure when the
/// path cannot be inspected - an operational fault, never a "no
/// conflict" and never a deterministic conflict.
pub fn create_would_conflict(root: &Path, rel: &str) -> io::Result<bool> {
    let mut current = root.to_path_buf();
    let components: Vec<&str> = rel.split('/').collect();
    for (index, component) in components.iter().enumerate() {
        let folded_component = fold_for_collision(component);
        let mut exact = false;
        let mut case_collision = false;
        for entry in fs::read_dir(&current)? {
            let name = entry?.file_name();
            if name.as_encoded_bytes() == component.as_bytes() {
                exact = true;
                break;
            }
            if case_fold(&name).is_some_and(|existing| existing == folded_component) {
                case_collision = true;
            }
        }
        if exact {
            // The leaf already exists, or an ancestor does; descend and
            // require each ancestor to be a directory.
            if index == components.len() - 1 {
                return Ok(true);
            }
            current.push(component);
            // An explicit stat, not `is_dir()`: a failed lookup on an
            // entry `read_dir` just produced is operational and must
            // propagate, not report a deterministic conflict.
            if !fs::metadata(&current)?.is_dir() {
                return Ok(true);
            }
        } else {
            // No exact entry: a case-only collision conflicts; a clean
            // miss means this component and everything below it is new.
            return Ok(case_collision);
        }
    }
    Ok(false)
}

/// Apply `entry`'s hunks over `old` and return the new file bytes.
fn assemble(
    patch: &str,
    entry: &FileEntry<'_>,
    old: &[TextLine<'_>],
) -> Result<Vec<u8>, PatchError> {
    let mismatch = || PatchError::ContextMismatch {
        patch: patch.to_owned(),
        file: entry.path.clone(),
    };
    let mut out: Vec<TextLine<'_>> = Vec::new();
    let mut cursor = 0usize;
    for hunk in &entry.hunks {
        // For a non-empty old side the header's start is the 1-based
        // first matched line; for a pure insertion it is the line the
        // insertion follows, so the anchor is the same expression's
        // 0-based successor.
        let anchor = if hunk.old_count == 0 {
            hunk.old_start
        } else {
            hunk.old_start - 1
        };
        if anchor < cursor {
            return Err(malformed(patch, "hunks overlap or are out of order"));
        }
        if anchor > old.len() {
            return Err(mismatch());
        }
        out.extend_from_slice(&old[cursor..anchor]);
        cursor = anchor;
        for hunk_line in &hunk.lines {
            match hunk_line.kind {
                HunkLineKind::Context | HunkLineKind::Remove => {
                    let Some(actual) = old.get(cursor) else {
                        return Err(mismatch());
                    };
                    // Byte-exact: content and terminator presence
                    // both match, or the patch does not apply.
                    if *actual != hunk_line.line {
                        return Err(mismatch());
                    }
                    cursor += 1;
                    if hunk_line.kind == HunkLineKind::Context {
                        out.push(hunk_line.line);
                    }
                }
                HunkLineKind::Add => out.push(hunk_line.line),
            }
        }
    }
    out.extend_from_slice(&old[cursor..]);
    // A line without a terminator is only coherent as the final line;
    // an interior one means the patch's no-newline markers disagree
    // with where the file actually ends.
    if out.iter().rev().skip(1).any(|line| !line.newline) {
        return Err(mismatch());
    }
    let mut bytes = Vec::new();
    for line in &out {
        bytes.extend_from_slice(line.bytes);
        if line.newline {
            bytes.push(b'\n');
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    fn tree(files: &[(&str, &str)]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for (path, content) in files {
            dir.child(path).write_str(content).unwrap();
        }
        dir
    }

    fn apply(dir: &TempDir, patch: &str) -> Result<(), PatchError> {
        apply_unified_patches(
            dir.path(),
            &[PatchInput {
                name: "patches/test.patch",
                bytes: patch.as_bytes(),
            }],
        )
    }

    fn read(dir: &TempDir, path: &str) -> Vec<u8> {
        fs::read(dir.child(path).path()).unwrap()
    }

    #[test]
    fn modifies_a_file_byte_exactly() {
        let dir = tree(&[(
            "src/lib.c",
            "int a() { return 1; }\nint b() { return 2; }\n",
        )]);
        apply(
            &dir,
            "--- a/src/lib.c\n\
             +++ b/src/lib.c\n\
             @@ -1,2 +1,2 @@\n \
             int a() { return 1; }\n\
             -int b() { return 2; }\n\
             +int b() { return 3; }\n",
        )
        .unwrap();
        assert_eq!(
            read(&dir, "src/lib.c"),
            b"int a() { return 1; }\nint b() { return 3; }\n"
        );
    }

    #[test]
    fn accepts_strict_git_preamble_and_header_timestamps() {
        let dir = tree(&[("Makefile", "all:\n\techo hi\n")]);
        apply(
            &dir,
            "diff --git a/Makefile b/Makefile\n\
             index 0000000..1111111 100644\n\
             --- a/Makefile\t2026-01-01 00:00:00\n\
             +++ b/Makefile\t2026-01-02 00:00:00\n\
             @@ -1,2 +1,2 @@\n \
             all:\n\
             -\techo hi\n\
             +\techo bye\n",
        )
        .unwrap();
        assert_eq!(read(&dir, "Makefile"), b"all:\n\techo bye\n");
    }

    #[test]
    fn free_text_around_the_diff_is_rejected() {
        // Free text is how a C/diff polyglot smuggles compilable
        // source into a file the verifier's tree comparison exempts:
        // C code before the diff, or a comment opener in a
        // loosely-checked preamble field would let the same bytes be
        // both a valid patch and a valid translation unit.  Every
        // such shape must die on the grammar check.
        let dir = tree(&[("a.txt", "x\n")]);
        for (patch, why) in [
            (
                "int evil(void);\n/*\n--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n",
                "C code and a comment opener before the diff",
            ),
            (
                "--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n-- \nsig\n",
                "format-patch signature trailer",
            ),
            (
                "index /* 100644\n--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n",
                "free text in an index line",
            ),
            (
                "diff --git a/a.txt b/a.txt\nnew file mode 100644 /*\n\
                 --- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n",
                "free text after a mode",
            ),
            (
                "diff --git a/a.txt /* b/a.txt */\n\
                 --- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n",
                "free text in diff --git paths",
            ),
            (
                "--- a/a.txt\t/*\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n",
                "comment opener in a header timestamp",
            ),
        ] {
            let err = apply(&dir, patch).unwrap_err();
            assert!(
                matches!(err, PatchError::Malformed { .. }),
                "{why}: {err:?}"
            );
        }
    }

    #[test]
    fn create_would_conflict_rejects_case_mismatched_ancestors() {
        // On any host this is case-sensitive: creating `Src/new.c`
        // when `src/` exists is a conflict (a case-insensitive host
        // would reuse `src/`, diverging from the Linux verifier).
        let dir = tree(&[("src/existing.c", "x\n"), ("ä/keep.c", "x\n")]);
        let conflict = |rel| create_would_conflict(dir.path(), rel).unwrap();
        assert!(conflict("Src/new.c"));
        assert!(conflict("src/existing.c")); // leaf exists
        assert!(conflict("src/existing.c/deeper.c")); // ancestor is a file
        // Unicode-folded ancestor collision, not just ASCII.
        assert!(conflict("Ä/new.c"));
        // A genuinely new path (exact-cased ancestor) is fine.
        assert!(!conflict("src/new.c"));
        assert!(!conflict("include/new.h"));
    }

    #[test]
    fn creates_and_deletes_files() {
        let dir = tree(&[("gone.h", "#define GONE 1\n")]);
        apply(
            &dir,
            "--- /dev/null\n\
             +++ b/include/new.h\n\
             @@ -0,0 +1,2 @@\n\
             +#pragma once\n\
             +int added(void);\n\
             --- a/gone.h\n\
             +++ /dev/null\n\
             @@ -1,1 +0,0 @@\n\
             -#define GONE 1\n",
        )
        .unwrap();
        assert_eq!(
            read(&dir, "include/new.h"),
            b"#pragma once\nint added(void);\n"
        );
        assert!(!dir.child("gone.h").path().exists());
    }

    #[test]
    fn applies_declared_patches_in_order() {
        // The second patch's context only exists after the first one
        // applied, so success proves declaration-order application.
        let dir = tree(&[("conf.h", "#define A 1\n")]);
        let first = PatchInput {
            name: "patches/0001.patch",
            bytes: b"--- a/conf.h\n+++ b/conf.h\n@@ -1,1 +1,1 @@\n-#define A 1\n+#define A 2\n",
        };
        let second = PatchInput {
            name: "patches/0002.patch",
            bytes: b"--- a/conf.h\n+++ b/conf.h\n@@ -1,1 +1,2 @@\n #define A 2\n+#define B 1\n",
        };
        apply_unified_patches(dir.path(), &[first, second]).unwrap();
        assert_eq!(read(&dir, "conf.h"), b"#define A 2\n#define B 1\n");

        // Reversed order is a deterministic context mismatch.
        let dir = tree(&[("conf.h", "#define A 1\n")]);
        let err = apply_unified_patches(dir.path(), &[second, first]).unwrap_err();
        assert!(matches!(err, PatchError::ContextMismatch { .. }), "{err:?}");
    }

    #[test]
    fn later_hunks_and_files_see_earlier_edits_within_one_patch() {
        let dir = tree(&[("a.txt", "one\ntwo\nthree\nfour\n")]);
        apply(
            &dir,
            "--- a/a.txt\n\
             +++ b/a.txt\n\
             @@ -1,2 +1,2 @@\n\
             -one\n\
             +ONE\n \
             two\n\
             @@ -4,1 +4,1 @@\n\
             -four\n\
             +FOUR\n",
        )
        .unwrap();
        assert_eq!(read(&dir, "a.txt"), b"ONE\ntwo\nthree\nFOUR\n");
    }

    #[test]
    fn preserves_crlf_content_byte_exactly() {
        // The target mixes CRLF and LF lines; the patch's context
        // carries the exact bytes, so the untouched CRLF line
        // survives and the replacement's ending is exactly what the
        // patch says.
        let dir = tree(&[("mixed.txt", "crlf line\r\nlf line\nlast\r\n")]);
        apply(
            &dir,
            "--- a/mixed.txt\n\
             +++ b/mixed.txt\n\
             @@ -1,3 +1,3 @@\n \
             crlf line\r\n \
             lf line\n\
             -last\r\n\
             +replaced\r\n",
        )
        .unwrap();
        assert_eq!(
            read(&dir, "mixed.txt"),
            b"crlf line\r\nlf line\nreplaced\r\n"
        );
    }

    #[test]
    fn line_ending_divergence_is_a_context_mismatch() {
        // LF context against CRLF content: no normalization, no fuzz.
        let dir = tree(&[("mixed.txt", "hello\r\n")]);
        let err = apply(
            &dir,
            "--- a/mixed.txt\n+++ b/mixed.txt\n@@ -1,1 +1,1 @@\n-hello\n+goodbye\n",
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::ContextMismatch { .. }), "{err:?}");
    }

    #[test]
    fn no_newline_markers_match_and_produce_unterminated_lines() {
        let dir = tree(&[("frag.txt", "line\nend")]);
        apply(
            &dir,
            "--- a/frag.txt\n\
             +++ b/frag.txt\n\
             @@ -1,2 +1,2 @@\n \
             line\n\
             -end\n\
             \\ No newline at end of file\n\
             +end!\n\
             \\ No newline at end of file\n",
        )
        .unwrap();
        assert_eq!(read(&dir, "frag.txt"), b"line\nend!");
    }

    #[test]
    fn missing_trailing_newline_must_be_declared() {
        // The file ends without a newline but the patch's removed
        // line claims one: byte-exact matching rejects it.
        let dir = tree(&[("frag.txt", "end")]);
        let err = apply(
            &dir,
            "--- a/frag.txt\n+++ b/frag.txt\n@@ -1,1 +1,1 @@\n-end\n+end!\n",
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::ContextMismatch { .. }), "{err:?}");
    }

    #[test]
    fn a_patch_not_ending_in_newline_is_rejected() {
        // The patch's own last content line lost its terminator
        // (truncation, or a tool that omits the final newline).  Left
        // unchecked this would be silently reinterpreted as "the
        // target's last line has no newline"; git reports the same
        // bytes as a corrupt patch.
        let dir = tree(&[("a.txt", "old\n")]);
        let err = apply(
            &dir,
            "--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-old\n+new",
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::Malformed { .. }), "{err:?}");
        assert_eq!(read(&dir, "a.txt"), b"old\n", "no partial application");

        // A truncated creation is rejected the same way, so the file
        // is never created from an incomplete final line.
        let err = apply(
            &dir,
            "--- /dev/null\n+++ b/n.h\n@@ -0,0 +1,1 @@\n+#pragma on",
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::Malformed { .. }), "{err:?}");
        assert!(!dir.child("n.h").path().exists());
    }

    #[test]
    fn a_bogus_no_newline_marker_is_rejected() {
        // Only the exact `\ No newline at end of file` marker is a
        // marker; `\ garbage` must be malformed, not silently treated
        // as a no-newline signal.
        let dir = tree(&[("frag.txt", "line\nend")]);
        let err = apply(
            &dir,
            "--- a/frag.txt\n\
             +++ b/frag.txt\n\
             @@ -1,2 +1,2 @@\n \
             line\n\
             -end\n\
             \\ garbage\n\
             +end!\n\
             \\ No newline at end of file\n",
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::Malformed { .. }), "{err:?}");
        assert_eq!(
            read(&dir, "frag.txt"),
            b"line\nend",
            "no partial application"
        );
    }

    #[test]
    fn an_oversized_target_is_rejected_without_reading_it() {
        // A target within the extraction caps but past the patch
        // engine's per-file cap must reject before it is split into
        // line records (the OOM lever).
        let dir = TempDir::new().unwrap();
        let big = vec![b'\n'; usize::try_from(MAX_PATCH_TARGET_BYTES + 1).unwrap()];
        dir.child("big.txt").write_binary(&big).unwrap();
        let err = apply(
            &dir,
            "--- a/big.txt\n+++ b/big.txt\n@@ -1,1 +1,1 @@\n-\n+x\n",
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::TargetTooLarge { .. }), "{err:?}");
    }

    #[test]
    fn an_all_newline_target_over_the_line_cap_is_rejected() {
        // Under the byte cap but over the line cap: proves the line
        // count guard catches the metadata-amplification the byte cap
        // alone misses.
        let dir = TempDir::new().unwrap();
        let newlines = vec![b'\n'; MAX_PATCH_TARGET_LINES + 1];
        assert!((newlines.len() as u64) < MAX_PATCH_TARGET_BYTES);
        dir.child("lines.txt").write_binary(&newlines).unwrap();
        let err = apply(
            &dir,
            "--- a/lines.txt\n+++ b/lines.txt\n@@ -1,1 +1,1 @@\n-\n+x\n",
        )
        .unwrap_err();
        match err {
            PatchError::TargetTooLarge { unit, .. } => assert_eq!(unit, "lines"),
            other => panic!("expected TargetTooLarge(lines), got {other:?}"),
        }
    }

    #[test]
    fn header_prefixes_other_than_a_and_b_are_rejected() {
        // The `-p1` component is discarded, so accepting an arbitrary
        // one would leave free text in the verifier-exempt patch file
        // - enough to smuggle C comment punctuation (`a*/`) past the
        // grammar.  `git diff` only ever emits `a/` and `b/`.
        let dir = tree(&[("a.txt", "x\n")]);
        for (patch, why) in [
            (
                "--- x/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n",
                "old side",
            ),
            (
                "--- a/a.txt\n+++ a/a.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n",
                "new side",
            ),
            (
                "--- a*/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n",
                "comment closer in the prefix",
            ),
        ] {
            let err = apply(&dir, patch).unwrap_err();
            assert!(
                matches!(err, PatchError::UnsafePath { .. }),
                "{why}: {err:?}"
            );
        }
        assert_eq!(read(&dir, "a.txt"), b"x\n");
    }

    #[test]
    fn the_aggregate_work_budget_bounds_a_multi_patch_declaration() {
        // Tested at the accumulator seam: proving a 128 MiB budget
        // with real I/O would cost seconds, and what matters is that
        // both halves of the amplification are charged and that the
        // count carries across the slice.
        let inputs = [PatchInput {
            name: "patches/p.patch",
            bytes: b"--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n",
        }];
        let dir = tree(&[("a.txt", "x\n")]);
        let mut work = 0;
        apply_with_budget(dir.path(), &inputs, &mut work).unwrap();
        assert_eq!(work, 4, "the 2-byte read and the 2-byte write both charge");

        // Whatever earlier patches spent, the next application that
        // would cross the line is refused - which is what stops 16
        // declarations from each rewriting every near-cap file.
        let dir = tree(&[("a.txt", "x\n")]);
        let mut work = MAX_PATCH_WORK_BYTES - 1;
        let err = apply_with_budget(dir.path(), &inputs, &mut work).unwrap_err();
        match err {
            PatchError::WorkBudgetExceeded { limit, .. } => {
                assert_eq!(limit, MAX_PATCH_WORK_BYTES);
            }
            other => panic!("expected WorkBudgetExceeded, got {other:?}"),
        }
        assert_eq!(
            read(&dir, "a.txt"),
            b"x\n",
            "the refused patch left no trace"
        );
    }

    #[test]
    fn an_entry_flood_is_rejected_before_anything_is_applied() {
        // Minimal creations are ~40 bytes of diff each, so the 1 MiB
        // per-patch cap admits tens of thousands - far too small to
        // move the byte budget, but every one is a directory scan.
        let dir = tree(&[("keep.txt", "x\n")]);
        let patch = (0..=MAX_PATCH_FILE_ENTRIES).fold(String::new(), |mut patch, i| {
            use std::fmt::Write as _;
            let _ = write!(
                patch,
                "--- /dev/null\n+++ b/f{i}.txt\n@@ -0,0 +1,1 @@\n+x\n"
            );
            patch
        });
        let err = apply(&dir, &patch).unwrap_err();
        match err {
            PatchError::TooManyFileEntries { limit, .. } => {
                assert_eq!(limit, MAX_PATCH_FILE_ENTRIES);
            }
            other => panic!("expected TooManyFileEntries, got {other:?}"),
        }
        // Rejected at parse time: not one of the floods files exists.
        assert!(!dir.child("f0.txt").path().exists());
    }

    #[test]
    fn repeated_file_entries_for_one_path_are_rejected() {
        // `git diff` emits at most one entry per file; repeats are
        // corruption - and an amplification vector, since every
        // modification rewrites its entire target.
        let dir = tree(&[("a.txt", "x\ny\n")]);
        let err = apply(
            &dir,
            "--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-x\n+z\n\
             --- a/a.txt\n+++ b/a.txt\n@@ -2,1 +2,1 @@\n-y\n+w\n",
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::Malformed { .. }), "{err:?}");
        assert_eq!(read(&dir, "a.txt"), b"x\ny\n");
    }

    #[test]
    fn a_forged_new_side_hunk_start_is_rejected() {
        // The new-side start is redundant in a faithful diff (the old
        // start plus the cumulative delta); a disagreeing value is
        // corruption, and applying it silently would accept
        // hand-edited hunk math.
        let dir = tree(&[("a.txt", "x\ny\n")]);
        for patch in [
            "--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +999,1 @@\n-x\n+z\n",
            // The second hunk ignores the +1 delta the first created.
            "--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,2 @@\n-x\n+x\n+w\n@@ -2,1 +2,1 @@\n-y\n+q\n",
        ] {
            let err = apply(&dir, patch).unwrap_err();
            assert!(matches!(err, PatchError::Malformed { .. }), "{err:?}");
        }
        assert_eq!(read(&dir, "a.txt"), b"x\ny\n");
    }

    #[test]
    fn normalization_equivalent_creation_paths_conflict() {
        // Upstream ships the decomposed spelling; the patch creates
        // the composed one.  macOS resolves both to one file (its
        // lookups are normalization-insensitive), Linux materializes
        // two - so the collision must be a deterministic conflict on
        // every host, or preparation silently overwrites the upstream
        // file on macOS while the verifier sees a diverged tree.
        let nfd = "e\u{301}toile.c";
        let nfc = "\u{e9}toile.c";
        let dir = tree(&[(nfd, "int lib();\n")]);
        assert!(create_would_conflict(dir.path(), nfc).unwrap());
        assert!(!create_would_conflict(dir.path(), "etoile.c").unwrap());
        let err = apply(
            &dir,
            &format!("--- /dev/null\n+++ b/{nfc}\n@@ -0,0 +1,1 @@\n+int evil();\n"),
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::TargetConflict { .. }), "{err:?}");
        // The upstream file must be untouched under either spelling.
        assert_eq!(read(&dir, nfd), b"int lib();\n");
    }

    #[test]
    fn assembled_output_over_the_line_cap_is_rejected() {
        // The output invariant mirrors the read side: a target at the
        // line cap plus a line-adding patch must not materialize a
        // file the next read would reject.
        let dir = TempDir::new().unwrap();
        let newlines = vec![b'\n'; MAX_PATCH_TARGET_LINES - 1];
        dir.child("lines.txt").write_binary(&newlines).unwrap();
        let err = apply(
            &dir,
            "--- a/lines.txt\n+++ b/lines.txt\n@@ -1,1 +1,3 @@\n-\n+\n+\n+\n",
        )
        .unwrap_err();
        match err {
            PatchError::TargetTooLarge { unit, .. } => assert_eq!(unit, "lines"),
            other => panic!("expected TargetTooLarge(lines), got {other:?}"),
        }
        // The over-cap output must not have been written.
        assert_eq!(read(&dir, "lines.txt"), newlines);
    }

    #[test]
    fn path_is_case_exact_is_case_sensitive_on_every_host() {
        // The case-exact lookup must reject a case-mismatched spelling even
        // on a case-insensitive filesystem, so port preparation and
        // the Linux verifier agree on whether a target exists.
        let dir = tree(&[("src/foo.c", "x\n")]);
        let exact = |rel| path_is_case_exact(dir.path(), rel).unwrap();
        assert!(exact("src/foo.c"));
        assert!(!exact("Src/foo.c"));
        assert!(!exact("src/Foo.c"));
        assert!(!exact("src/missing.c"));
        // A regular file used as a directory component is absent, not
        // an I/O error.
        assert!(!exact("src/foo.c/deeper.c"));
    }

    #[test]
    fn a_patch_growing_a_target_past_the_cap_is_rejected() {
        // The input is within the cap, but the assembled output grows
        // past it; a later patch could not even read the result, so
        // the growth is rejected rather than materialized.
        let dir = TempDir::new().unwrap();
        // A target at exactly the cap (all newlines) passes the input
        // check; appending one line pushes the output over.
        let at_cap = vec![b'\n'; usize::try_from(MAX_PATCH_TARGET_BYTES).unwrap()];
        dir.child("big.txt").write_binary(&at_cap).unwrap();
        let err = apply(
            &dir,
            "--- a/big.txt\n+++ b/big.txt\n@@ -1,0 +2,1 @@\n+added\n",
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::TargetTooLarge { .. }), "{err:?}");
    }

    #[test]
    fn a_patch_ending_in_a_no_newline_marker_without_a_trailing_newline_applies() {
        // The legitimate end-of-file shape: the final `\ No newline`
        // marker itself carries no trailing newline, and that must
        // still be accepted (it is not a truncated content line).
        let dir = tree(&[("frag.txt", "keep\nend\n")]);
        apply(
            &dir,
            "--- a/frag.txt\n\
             +++ b/frag.txt\n\
             @@ -1,2 +1,2 @@\n \
             keep\n\
             -end\n\
             +end!\n\
             \\ No newline at end of file",
        )
        .unwrap();
        assert_eq!(read(&dir, "frag.txt"), b"keep\nend!");
    }

    #[test]
    fn inserts_at_the_top_and_appends_at_the_end() {
        let dir = tree(&[("list.txt", "middle\n")]);
        apply(
            &dir,
            "--- a/list.txt\n\
             +++ b/list.txt\n\
             @@ -0,0 +1,1 @@\n\
             +first\n\
             @@ -1,1 +2,1 @@\n \
             middle\n\
             @@ -1,0 +3,1 @@\n\
             +last\n",
        )
        .unwrap();
        assert_eq!(read(&dir, "list.txt"), b"first\nmiddle\nlast\n");
    }

    #[test]
    fn context_divergence_is_rejected_without_offset_search() {
        // The removed line exists one line further down; `patch`
        // would find it with an offset, this engine must not.
        let dir = tree(&[("shifted.c", "// new comment\nint a;\nint b;\n")]);
        let err = apply(
            &dir,
            "--- a/shifted.c\n+++ b/shifted.c\n@@ -1,2 +1,2 @@\n-int a;\n+int a2;\n int b;\n",
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::ContextMismatch { .. }), "{err:?}");
    }

    #[test]
    fn malformed_diffs_are_rejected() {
        let dir = tree(&[("a.txt", "x\n")]);
        for (patch, why) in [
            ("not a diff at all\n", "no file entries"),
            ("--- a/a.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n", "missing +++"),
            ("--- a/a.txt\n+++ b/a.txt\n", "no hunks"),
            ("--- a/a.txt\n+++ b/a.txt\n@@ garbage @@\n", "bad header"),
            (
                "--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,1 @@\n-x\n",
                "truncated",
            ),
            (
                "--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n@@ -1,1 +2,1 @@\n-x\n+z\n",
                "overlapping hunks",
            ),
            (
                "--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n+z\n",
                "stray content after a completed hunk",
            ),
            (
                "--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,2 @@\n-x\n+y\n\n+z\n",
                "bare empty line inside a hunk",
            ),
            (
                "--- /dev/null\n+++ /dev/null\n@@ -0,0 +1,1 @@\n+x\n",
                "both /dev/null",
            ),
            (
                "--- a/a.txt\n+++ b/b.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n",
                "rename via headers",
            ),
            (
                "rename from a.txt\nrename to b.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n",
                "rename header",
            ),
            (
                "--- a/a.txt\n+++ b/a.txt\n@@ -0,0 +1,1 @@\n+y\n@@ -1,1 +2,1 @@\n-x\n+z\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1,1 @@\n+q\n@@ -1,1 +1,1 @@\n-q\n+r\n",
                "multi-hunk creation",
            ),
        ] {
            let err = apply(&dir, patch).unwrap_err();
            assert!(
                matches!(err, PatchError::Malformed { .. }),
                "{why}: {err:?}"
            );
        }
    }

    #[test]
    fn a_zero_length_hunk_is_rejected() {
        // `@@ -0,0 +0,0 @@` is a no-op git never emits; accepting it
        // would create a zero-byte file (or delete an empty one),
        // slipping past the hunkless-section guard.
        let dir = tree(&[("keep.txt", "x\n")]);
        let err = apply(&dir, "--- /dev/null\n+++ b/empty.txt\n@@ -0,0 +0,0 @@\n").unwrap_err();
        assert!(matches!(err, PatchError::Malformed { .. }), "{err:?}");
        assert!(!dir.child("empty.txt").path().exists());
    }

    #[test]
    fn duplicate_no_newline_markers_are_rejected() {
        // Two markers on the same line is malformed; only one may
        // clear a line's terminator.
        let dir = tree(&[("a.txt", "old")]);
        let err = apply(
            &dir,
            "--- a/a.txt\n\
             +++ b/a.txt\n\
             @@ -1,1 +1,1 @@\n\
             -old\n\
             \\ No newline at end of file\n\
             \\ No newline at end of file\n\
             +new\n\
             \\ No newline at end of file\n",
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::Malformed { .. }), "{err:?}");
        assert_eq!(read(&dir, "a.txt"), b"old", "no partial application");
    }

    #[test]
    fn a_symlink_mode_add_is_rejected() {
        // `git diff` adds a symlink as `new file mode 120000` plus a
        // /dev/null hunk holding the link target; applying it as a
        // regular file would misrepresent the patch.
        let dir = tree(&[("keep.txt", "x\n")]);
        let err = apply(
            &dir,
            "diff --git a/link b/link\n\
             new file mode 120000\n\
             index 0000000..1111111\n\
             --- /dev/null\n\
             +++ b/link\n\
             @@ -0,0 +1,1 @@\n\
             +target/path\n\
             \\ No newline at end of file\n",
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::Malformed { .. }), "{err:?}");
        assert!(!dir.child("link").path().exists());
    }

    #[test]
    fn a_hunkless_git_section_is_rejected() {
        // An empty-file add is a `diff --git` + mode/index section with
        // no `---`/`+++` hunk; skipping it would half-apply a
        // multi-file patch, so it is rejected as unsupported.
        let dir = tree(&[("real.c", "x\n")]);
        let err = apply(
            &dir,
            "diff --git a/empty.txt b/empty.txt\n\
             new file mode 100644\n\
             index 0000000..e69de29\n\
             diff --git a/real.c b/real.c\n\
             --- a/real.c\n\
             +++ b/real.c\n\
             @@ -1,1 +1,1 @@\n\
             -x\n\
             +y\n",
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::Malformed { .. }), "{err:?}");
        assert_eq!(read(&dir, "real.c"), b"x\n", "no partial application");
    }

    #[test]
    fn a_normal_git_section_with_a_regular_mode_applies() {
        // Regression: a regular-file git section (with `index ... 100644`
        // and a hunk) must still apply after the section tracking.
        let dir = tree(&[("f.c", "a\n")]);
        apply(
            &dir,
            "diff --git a/f.c b/f.c\n\
             index 1111111..2222222 100644\n\
             --- a/f.c\n\
             +++ b/f.c\n\
             @@ -1,1 +1,1 @@\n\
             -a\n\
             +b\n",
        )
        .unwrap();
        assert_eq!(read(&dir, "f.c"), b"b\n");
    }

    #[test]
    fn binary_content_is_rejected() {
        let dir = tree(&[("a.bin", "x\n")]);
        for patch in [
            b"Binary files a/a.bin and b/a.bin differ\n".to_vec(),
            b"diff --git a/a.bin b/a.bin\nGIT binary patch\nliteral 4\n".to_vec(),
            b"--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-x\n+\x00y\n".to_vec(),
        ] {
            let err = apply_unified_patches(
                dir.path(),
                &[PatchInput {
                    name: "patches/bin.patch",
                    bytes: &patch,
                }],
            )
            .unwrap_err();
            assert!(matches!(err, PatchError::Binary { .. }), "{err:?}");
        }
    }

    #[test]
    fn git_quoted_non_ascii_header_paths_apply() {
        // git's default core.quotePath emits `"a/caf\303\251.c"` for
        // café.c; decoding the quoted form lets ordinary `git diff`
        // output apply without a config change.
        let dir = tree(&[("café.c", "int x;\n")]);
        apply(
            &dir,
            "--- \"a/caf\\303\\251.c\"\n\
             +++ \"b/caf\\303\\251.c\"\n\
             @@ -1,1 +1,1 @@\n\
             -int x;\n\
             +int y;\n",
        )
        .unwrap();
        assert_eq!(read(&dir, "café.c"), b"int y;\n");
    }

    #[test]
    fn a_quoted_header_path_that_escapes_is_still_rejected() {
        // Decoding does not weaken safety: a quoted `..` traversal
        // decodes and then fails the archive-path rule.
        let dir = tree(&[("a.txt", "x\n")]);
        let err = apply(
            &dir,
            "--- \"a/../escape.txt\"\n+++ \"b/../escape.txt\"\n@@ -1,1 +1,1 @@\n-x\n+y\n",
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::UnsafePath { .. }), "{err:?}");
    }

    #[test]
    fn unsafe_header_paths_are_rejected() {
        let dir = tree(&[("a.txt", "x\n")]);
        for header in [
            "a/../escape.txt",
            "/abs.txt",
            "bare-no-slash",
            "a/",
            "a/nested/../../up.txt",
            "a/patches\\win.txt",
            "a/con",
        ] {
            let patch = format!("--- {header}\n+++ {header}\n@@ -1,1 +1,1 @@\n-x\n+y\n");
            let err = apply(&dir, &patch).unwrap_err();
            assert!(
                matches!(err, PatchError::UnsafePath { .. }),
                "{header}: {err:?}"
            );
        }
    }

    #[test]
    fn missing_and_conflicting_targets_are_rejected() {
        let dir = tree(&[("dir/inner.txt", "x\n"), ("present.txt", "x\n")]);
        let missing = apply(
            &dir,
            "--- a/absent.txt\n+++ b/absent.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n",
        )
        .unwrap_err();
        assert!(
            matches!(missing, PatchError::MissingTarget { .. }),
            "{missing:?}"
        );

        // Modifying a directory, creating over an existing file, and
        // creating below a path occupied by a regular file all fail
        // deterministically.
        let dir_target =
            apply(&dir, "--- a/dir\n+++ b/dir\n@@ -1,1 +1,1 @@\n-x\n+y\n").unwrap_err();
        assert!(
            matches!(dir_target, PatchError::TargetConflict { .. }),
            "{dir_target:?}"
        );
        let exists = apply(
            &dir,
            "--- /dev/null\n+++ b/present.txt\n@@ -0,0 +1,1 @@\n+y\n",
        )
        .unwrap_err();
        assert!(
            matches!(exists, PatchError::TargetConflict { .. }),
            "{exists:?}"
        );
        let through_file = apply(
            &dir,
            "--- /dev/null\n+++ b/present.txt/below.txt\n@@ -0,0 +1,1 @@\n+y\n",
        )
        .unwrap_err();
        assert!(
            matches!(through_file, PatchError::TargetConflict { .. }),
            "{through_file:?}"
        );
    }

    #[test]
    fn a_deletion_must_cover_the_whole_file() {
        let dir = tree(&[("two.txt", "one\ntwo\n")]);
        let err = apply(
            &dir,
            "--- a/two.txt\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-one\n",
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::ContextMismatch { .. }), "{err:?}");
        // The file survives an inapplicable deletion attempt.
        assert_eq!(read(&dir, "two.txt"), b"one\ntwo\n");
    }

    #[test]
    fn crlf_patch_files_apply_to_crlf_content() {
        // A fully CRLF-encoded patch: headers tolerate the trailing
        // `\r`, and hunk content lines keep theirs, which is exactly
        // what CRLF target content needs.
        let dir = tree(&[("win.txt", "alpha\r\nbeta\r\n")]);
        apply(
            &dir,
            "--- a/win.txt\r\n\
             +++ b/win.txt\r\n\
             @@ -1,2 +1,2 @@\r\n \
             alpha\r\n\
             -beta\r\n\
             +gamma\r\n",
        )
        .unwrap();
        assert_eq!(read(&dir, "win.txt"), b"alpha\r\ngamma\r\n");
    }

    #[test]
    fn hunk_section_headings_are_tolerated() {
        let dir = tree(&[("f.c", "int f() { return 0; }\n")]);
        apply(
            &dir,
            "--- a/f.c\n+++ b/f.c\n@@ -1,1 +1,1 @@ int f()\n-int f() { return 0; }\n+int f() { return 1; }\n",
        )
        .unwrap();
        assert_eq!(read(&dir, "f.c"), b"int f() { return 1; }\n");
    }
}
