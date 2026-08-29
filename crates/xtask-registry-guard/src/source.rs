//! Reading the Worker's source tree the way the guards need it: a
//! deterministic file list, byte-oriented line matching, and the two
//! character classes the lexical scans share.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

/// One file under the scanned tree.
pub struct Source {
    /// Slash-separated and prefixed exactly as the guard reports it
    /// (`src/glue/read.rs`), so diagnostics read the same on every platform.
    pub relative: String,
    pub path: PathBuf,
}

/// Every file under `dir`, sorted by reported path.
///
/// A symlink to a file is listed and read through, like the `find` walk
/// this replaces; a symlink to a directory is neither descended into nor
/// listed (`find` does not descend one, and listing it would hand a
/// directory to `read`), and a dangling one is skipped. A guard that
/// aborts on an ordinary symlink reports no violations at all, which is
/// the one outcome it must never produce - but a link the process cannot
/// resolve at all is not "dangling", it is unknown, and refusing is the
/// only safe reading of it.
///
/// A file name that is not valid UTF-8 refuses too. Both the allowlists
/// and every diagnostic are keyed by the reported path, so a name that
/// only round-trips lossily would make the pins and the sort order
/// ambiguous.
fn walk(dir: &Path, prefix: &str) -> Result<Vec<Source>> {
    let mut found = Vec::new();
    let entries = fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("read {}", dir.display()))?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .with_context(|| format!("{prefix}/{} is not valid UTF-8", name.to_string_lossy()))?;
        let relative = format!("{prefix}/{name}");
        let kind = entry.file_type()?;
        if kind.is_dir() {
            found.extend(walk(&entry.path(), &relative)?);
        } else if !kind.is_symlink() || resolves_to_a_file(&entry.path(), &relative)? {
            found.push(Source {
                relative,
                path: entry.path(),
            });
        }
    }
    found.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(found)
}

/// Whether `path` resolves to a regular file. A dangling link is `false`;
/// any other lookup failure (a permission denial, a symlink loop) refuses
/// rather than reading as "nothing to scan here".
fn resolves_to_a_file(path: &Path, relative: &str) -> Result<bool> {
    match fs::metadata(path) {
        Ok(target) => Ok(target.is_file()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(anyhow::Error::new(err).context(format!("resolve the symlink {relative}"))),
    }
}

/// Every `*.rs` file under `dir`, sorted by reported path.
///
/// # Errors
///
/// Fails when the tree cannot be read.
pub fn rust_sources(dir: &Path, prefix: &str) -> Result<Vec<Source>> {
    Ok(walk(dir, prefix)?
        .into_iter()
        // Byte-wise on the reported path: `find -name '*.rs'` matched a
        // leading-dot name too, which `Path::extension` would not.
        .filter(|source| source.relative.as_bytes().ends_with(b".rs"))
        .collect())
}

/// Every line under `dir` containing `pattern`, rendered as
/// `<path>:<line>:<text>`.
///
/// The match is on the bytes as written - comments and strings
/// included - which is the point: these patterns are commissioned
/// spellings that must not appear at all.
///
/// # Errors
///
/// Fails when the tree cannot be read.
pub fn matching_lines(dir: &Path, prefix: &str, pattern: &[u8]) -> Result<Vec<String>> {
    let mut found = Vec::new();
    for source in walk(dir, prefix)? {
        let bytes =
            fs::read(&source.path).with_context(|| format!("read {}", source.path.display()))?;
        for (index, line) in bytes.split(|&byte| byte == b'\n').enumerate() {
            if line.windows(pattern.len()).any(|window| window == pattern) {
                found.push(format!(
                    "{}:{}:{}",
                    source.relative,
                    index + 1,
                    String::from_utf8_lossy(line),
                ));
            }
        }
    }
    Ok(found)
}

/// The 1-based line `offset` falls on.
#[must_use]
pub fn line_of(source: &[u8], offset: usize) -> usize {
    source[..offset].split(|&byte| byte == b'\n').count()
}

/// The whitespace class the scans skip over.
#[must_use]
pub fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// The identifier class the scans use for word boundaries.
#[must_use]
pub fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}
