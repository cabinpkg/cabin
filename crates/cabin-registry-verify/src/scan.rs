//! The streaming pass: size discipline and structure checks over
//! the gunzipped tar stream, without extracting anything to disk.
//!
//! Every decompressed byte flows through [`CappedReader`], so the
//! bomb caps hold no matter what the tar layer does.  Memory stays
//! within a small constant factor of the decompressed-total cap
//! (the tar reader buffers metadata records - GNU long names, PAX
//! payloads, sparse maps - before the type gate can reject their
//! carrier, and all of those reads are cap-accounted; the retained
//! state is the `cabin.toml` contents plus the set of entry paths
//! seen, both bounded by the caps).
//!
//! The structure rules mirror what `cabin package` emits
//! (`cabin_package::archive::build_tar_gz`): regular-file entries
//! with safe relative paths and `cabin.toml` at the archive root.
//! Directory entries are tolerated (extractors materialize them
//! anyway); producer determinism details (entry order, mode, mtime)
//! are deliberately not enforced - they do not change what an
//! extractor materializes.  After the entries, the stream must run
//! clean to EOF: only zero bytes may follow the tar terminator, so
//! no nonzero content can ride behind it - trailing garbage and any
//! extra gzip member carrying content are rejected, and draining
//! through [`MultiGzDecoder`] validates every member's gzip trailer.
//! (An extra member that decompresses to only zeros materializes
//! nothing and is tolerated, like the mode/mtime differences above.)

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use flate2::read::MultiGzDecoder;
use thiserror::Error;

use crate::{Limits, Reason, VerifyError};

/// The archive root entry the consistency pass parses; the one
/// layout invariant `cabin-artifact` extraction relies on.
const ROOT_MANIFEST: &str = "cabin.toml";

/// The ratio cap only engages above a floor of this base plus
/// [`FRAMING_BYTES_PER_ENTRY`] per permitted entry.  Tar framing
/// (headers, padding, the EOF marker) is mostly zeros and
/// compresses at far better than 10x, so a small legitimate archive
/// routinely "expands" 15-30x; the floor must cover the framing the
/// entry cap permits, or an archive of many tiny files would trip
/// the ratio cap before the entry cap.  The resulting default floor
/// (4 MiB + 10000 x 2 KiB = 24 MiB) is far below anything that
/// could distress the runner.
const RATIO_FLOOR_BASE_BYTES: u64 = 4 * 1024 * 1024;

/// Worst-case framing per permitted entry: a 512-byte header plus
/// up to 511 bytes of padding, doubled to cover a GNU long-name
/// record and its padded payload.
const FRAMING_BYTES_PER_ENTRY: u64 = 2048;

/// What the streaming pass concluded.
pub(crate) enum ScanOutcome {
    /// Structure is sound; the embedded manifest bytes plus the set
    /// of regular-file entry paths (so the consistency pass can check
    /// the manifest's declared sources are present).
    Manifest {
        bytes: Vec<u8>,
        files: Contents,
    },
    Reject(Reason),
}

/// The regular-file entry paths the scan saw.
pub(crate) type Contents = HashSet<String>;

/// Stream-inspect `archive`.
///
/// # Errors
///
/// [`VerifyError::Io`] when the file cannot be opened, its size
/// read, or a raw read from it fails mid-stream (tagged by
/// [`FileRead`], so a flaky disk stays an operational error).
/// Decode failures *inside* the gzip/tar stream are verdicts, not
/// errors: a stream that will not parse - or that carries anything
/// but zero padding after the tar terminator - is a hostile or
/// corrupt archive ([`Reason::ArchiveInvalid`]), and crossing the
/// decompression cap is [`Reason::DecompressedTooLarge`].
pub(crate) fn scan_archive(archive: &Path, limits: &Limits) -> Result<ScanOutcome, VerifyError> {
    let io_error = |source: io::Error| VerifyError::Io {
        path: archive.to_path_buf(),
        source,
    };
    let compressed_size = fs::metadata(archive).map_err(io_error)?.len();
    let floor = RATIO_FLOOR_BASE_BYTES
        .saturating_add((limits.max_entries as u64).saturating_mul(FRAMING_BYTES_PER_ENTRY));
    let cap = limits
        .ratio_cap
        .saturating_mul(compressed_size)
        .max(floor)
        .min(limits.abs_cap_bytes);
    let file = File::open(archive).map_err(io_error)?;
    let mut tar = tar::Archive::new(CappedReader::new(
        MultiGzDecoder::new(FileRead::new(file)),
        cap,
    ));

    let mut outcome = scan_entries(&mut tar, limits);
    let mut reader = tar.into_inner();
    if let Ok(ScanOutcome::Manifest { .. }) = &outcome {
        // The tar layer stops at the terminator; the rest of the
        // stream (remaining padding, and any further gzip members
        // the decoder concatenates) must be zero bytes to EOF, so
        // nothing rides behind the terminator and the gzip trailers
        // get validated.
        match drain_is_all_zeros(&mut reader) {
            Ok(true) => {}
            Ok(false) => outcome = Ok(ScanOutcome::Reject(Reason::ArchiveInvalid)),
            Err(err) => outcome = Err(err),
        }
    }
    match outcome {
        Ok(outcome) => Ok(outcome),
        Err(_) if reader.exceeded => Ok(ScanOutcome::Reject(Reason::DecompressedTooLarge)),
        Err(err) if is_file_read_error(&err) => Err(io_error(err)),
        Err(_) => Ok(ScanOutcome::Reject(Reason::ArchiveInvalid)),
    }
}

fn drain_is_all_zeros(reader: &mut impl Read) -> io::Result<bool> {
    let mut buf = [0u8; 4096];
    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            return Ok(true);
        }
        if buf[..read].iter().any(|byte| *byte != 0) {
            return Ok(false);
        }
    }
}

fn scan_entries<R: Read>(tar: &mut tar::Archive<R>, limits: &Limits) -> io::Result<ScanOutcome> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut files: HashSet<String> = HashSet::new();
    let mut manifest: Option<Vec<u8>> = None;
    let mut count: usize = 0;

    for entry in tar.entries()? {
        let mut entry = entry?;
        count += 1;
        if count > limits.max_entries {
            return Ok(ScanOutcome::Reject(Reason::TooManyEntries));
        }

        let kind = entry.header().entry_type();
        match kind {
            tar::EntryType::Regular | tar::EntryType::Directory => {}
            // The tar reader consumes GNU long-name and PAX records
            // internally to decorate the entry that follows (their
            // payload still counts against the decompression cap),
            // so these arms are defense in depth against that
            // behavior changing.  Legitimate `cabin package`
            // archives ride long paths on GNU long-name records.
            tar::EntryType::GNULongName | tar::EntryType::GNULongLink => continue,
            _ => return Ok(ScanOutcome::Reject(Reason::ForbiddenEntryType)),
        }
        // PAX-decorated entries are rejected outright: `cabin
        // package` never emits PAX records, and a PAX `path`
        // override rewrites the path an extractor materializes.
        if entry.pax_extensions()?.is_some() {
            return Ok(ScanOutcome::Reject(Reason::ForbiddenEntryType));
        }

        let path = {
            let raw = entry.path_bytes();
            if raw.len() > limits.max_path_len {
                return Ok(ScanOutcome::Reject(Reason::PathTooLong));
            }
            let Ok(path) = std::str::from_utf8(&raw) else {
                return Ok(ScanOutcome::Reject(Reason::InvalidPath));
            };
            // Tar directory entries conventionally carry a trailing
            // slash; strip exactly one before validating.
            let path = match kind {
                tar::EntryType::Directory => path.strip_suffix('/').unwrap_or(path),
                _ => path,
            };
            if let Some(reason) = classify_path(path) {
                return Ok(ScanOutcome::Reject(reason));
            }
            path.to_owned()
        };
        if !seen.insert(path.clone()) {
            return Ok(ScanOutcome::Reject(Reason::DuplicatePath));
        }

        if kind == tar::EntryType::Regular {
            files.insert(path.clone());
            if path == ROOT_MANIFEST {
                // Bounded by the decompression cap like every other
                // byte; the only entry ever held in memory.
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes)?;
                manifest = Some(bytes);
            }
        }
    }

    // A regular file used as another entry's parent directory - e.g.
    // a file `src` alongside `src/main.cc` - passes the per-path
    // duplicate check (the strings differ) but has no consistent
    // extraction: `cabin-artifact` writes one, then `create_dir_all`
    // fails on the other.  A `cabin package` archive cannot contain
    // this (it walks a real tree), so a verified-but-unextractable
    // version would only come from a hostile publish.  (A directory
    // *entry* sharing the name is fine: it is not in `files`, and an
    // identical file+dir pair is already a `DuplicatePath`.)
    for path in &seen {
        let mut boundary = 0;
        while let Some(slash) = path[boundary..].find('/') {
            boundary += slash;
            if files.contains(&path[..boundary]) {
                return Ok(ScanOutcome::Reject(Reason::PathConflict));
            }
            boundary += 1;
        }
    }

    match manifest {
        Some(bytes) => Ok(ScanOutcome::Manifest { bytes, files }),
        None => Ok(ScanOutcome::Reject(Reason::ManifestMissing)),
    }
}

/// Classify an entry path against the safe-relative-path rules.
/// `cabin package` emits forward-slash relative paths with `Normal`
/// components only, so anything else is rejected: absolute paths
/// (POSIX or Windows-drive form), `..` traversal, and the
/// cross-platform hazards (`\`, empty or `.` components) that a
/// Unix-built hostile archive could aim at a Windows extractor.
fn classify_path(path: &str) -> Option<Reason> {
    if path.is_empty() {
        return Some(Reason::InvalidPath);
    }
    if path.starts_with('/') {
        return Some(Reason::AbsolutePath);
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Some(Reason::AbsolutePath);
    }
    if path.contains('\\') {
        return Some(Reason::InvalidPath);
    }
    for component in path.split('/') {
        match component {
            ".." => return Some(Reason::PathTraversal),
            "" | "." => return Some(Reason::InvalidPath),
            _ => {}
        }
    }
    None
}

/// Wraps the archive [`File`] so raw filesystem read failures stay
/// distinguishable (and operational) after the gzip/tar layers wrap
/// them: hostile bytes must be rejectable, a flaky disk must not
/// be.
struct FileRead {
    inner: File,
}

impl FileRead {
    fn new(inner: File) -> Self {
        FileRead { inner }
    }
}

impl Read for FileRead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner
            .read(buf)
            .map_err(|source| io::Error::new(source.kind(), FileReadError(source)))
    }
}

/// The marker [`FileRead`] wraps raw read failures in;
/// [`is_file_read_error`] finds it anywhere in an error's source
/// chain.
#[derive(Debug, Error)]
#[error("failed to read the archive file: {0}")]
struct FileReadError(#[source] io::Error);

fn is_file_read_error(err: &io::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(err) = source {
        if err.is::<FileReadError>() {
            return true;
        }
        source = err.source();
    }
    false
}

/// A reader that refuses to produce more than `remaining` bytes.
/// Unlike [`io::Take`], crossing the cap is an error (with the
/// `exceeded` flag set so the caller can tell a bomb from a corrupt
/// stream), not a silent EOF - a truncated tar must never pass as a
/// complete one.
struct CappedReader<R> {
    inner: R,
    remaining: u64,
    exceeded: bool,
}

impl<R: Read> CappedReader<R> {
    fn new(inner: R, cap: u64) -> Self {
        CappedReader {
            inner,
            remaining: cap,
            exceeded: false,
        }
    }
}

impl<R: Read> Read for CappedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            // At the cap: only a clean EOF may follow.  Probe one
            // byte to tell the two apart.
            let mut probe = [0u8; 1];
            if self.inner.read(&mut probe)? == 0 {
                return Ok(0);
            }
            self.exceeded = true;
            return Err(io::Error::other("decompressed size cap exceeded"));
        }
        let allowed = usize::try_from(self.remaining.min(buf.len() as u64)).unwrap_or(buf.len());
        let read = self.inner.read(&mut buf[..allowed])?;
        self.remaining -= read as u64;
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped_reader_passes_streams_within_the_cap() {
        let mut reader = CappedReader::new(&b"hello"[..], 5);
        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        assert_eq!(out, b"hello");
        assert!(!reader.exceeded);
    }

    #[test]
    fn capped_reader_errors_on_the_byte_after_the_cap() {
        let mut reader = CappedReader::new(&b"hello"[..], 4);
        let mut out = Vec::new();
        let err = reader.read_to_end(&mut out).unwrap_err();
        assert_eq!(err.to_string(), "decompressed size cap exceeded");
        assert!(reader.exceeded);
    }

    #[test]
    fn classify_path_accepts_the_shapes_cabin_package_emits() {
        for path in ["cabin.toml", "src/main.cc", "include/a/b.h", "a b/c"] {
            assert_eq!(classify_path(path), None, "path: {path:?}");
        }
    }

    #[test]
    fn classify_path_rejects_unsafe_shapes() {
        for (path, reason) in [
            ("", Reason::InvalidPath),
            ("/etc/passwd", Reason::AbsolutePath),
            ("c:/evil", Reason::AbsolutePath),
            ("C:\\evil", Reason::AbsolutePath),
            ("..", Reason::PathTraversal),
            ("../escape", Reason::PathTraversal),
            ("src/../../escape", Reason::PathTraversal),
            ("a\\b", Reason::InvalidPath),
            ("a//b", Reason::InvalidPath),
            ("./a", Reason::InvalidPath),
            ("a/./b", Reason::InvalidPath),
        ] {
            assert_eq!(classify_path(path), Some(reason), "path: {path:?}");
        }
    }
}
