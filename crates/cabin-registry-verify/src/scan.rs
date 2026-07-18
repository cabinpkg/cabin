//! The structure pass: a hand-rolled parser for the strict zip
//! profile (`registry/docs/archive-format.md`), running the size
//! discipline and every container rule without extracting to disk.
//!
//! The container is read into memory once and parsed by fixed
//! offsets: the end-of-central-directory record (EOCD) sits at
//! `len - 22`, the central directory and the local records tile the
//! file contiguously, and the ambiguities a general-purpose zip
//! library papers over (zip64, data descriptors, extra fields,
//! duplicate names, local/central disagreement) are each rejected.
//! A general library is deliberately not used: its conveniences -
//! last-wins deduplication, transparent zip64, hidden local/central
//! mismatch - are exactly the hostile shapes this profile forbids.
//!
//! Every decompressed byte flows through [`CappedReader`] against a
//! single archive-global budget, so the bomb caps hold regardless of
//! what the deflate layer does; the retained state is the
//! `cabin.toml` bytes plus the set of entry paths, both bounded by
//! the caps.  Following the verifier's enforce/client-only split
//! (see the archive-format spec's "Determinism"), only what changes
//! what an extractor materializes or how the container parses is
//! enforced here - producer cosmetics (timestamps, permission bits,
//! entry order) are pinned by the client and its own tests, not by
//! this pass.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use flate2::{Decompress, FlushDecompress, Status};

use crate::{Limits, Reason, VerifyError};

/// The archive root entry the consistency pass parses; the one
/// layout invariant `cabin-artifact` extraction relies on.
const ROOT_MANIFEST: &str = "cabin.toml";

/// The ratio cap only engages above a floor of this base plus
/// [`FRAMING_BYTES_PER_ENTRY`] per permitted entry.  Zip framing
/// (local and central headers, the EOCD) is fixed overhead that does
/// not compress, so a small legitimate archive of many tiny files
/// still "expands" past any sane ratio; the floor must cover the
/// framing the entry cap permits, or such an archive would trip the
/// ratio cap before the entry cap.  The resulting default floor
/// (4 MiB + 10000 x 2 KiB = 24 MiB) is far below anything that could
/// distress the runner.
const RATIO_FLOOR_BASE_BYTES: u64 = 4 * 1024 * 1024;

/// Worst-case framing per permitted entry, covering a local header
/// (30 bytes + name), a central header (46 bytes + name), and the
/// name itself twice; 2 KiB comfortably bounds zip's ~600-byte
/// per-entry framing at the path-length cap.
const FRAMING_BYTES_PER_ENTRY: u64 = 2048;

// Zip record signatures (little-endian on disk).
const LOCAL_SIG: u32 = 0x0403_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const EOCD_SIG: u32 = 0x0605_4b50;

// Fixed record sizes with no extra fields, comments, or descriptors.
const EOCD_LEN: usize = 22;
const LOCAL_HEADER_LEN: usize = 30;
const CENTRAL_HEADER_LEN: usize = 46;

// Unix `st_mode` type field, read from the high 16 bits of the
// external attributes independently of the version-made-by system
// byte (see the archive-format spec's "Entry types").
const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;
/// DOS directory attribute, in the low byte of the external
/// attributes.
const DOS_DIRECTORY: u32 = 0x10;

// General-purpose bit flags.
const GP_UTF8: u16 = 0x0800;
const GP_DATA_DESCRIPTOR: u16 = 0x0008;

// The zip64 sentinels: a real value at their width means "read the
// zip64 record instead", which this profile bans outright.
const U16_SENTINEL: u16 = 0xFFFF;
const U32_SENTINEL: u32 = 0xFFFF_FFFF;

/// What the structure pass concluded.
pub(crate) enum ScanOutcome {
    /// Structure is sound; the embedded manifest bytes plus the set
    /// of entry paths (so the consistency pass can check the
    /// manifest's declared sources are present).
    Manifest {
        bytes: Vec<u8>,
        files: Contents,
    },
    Reject(Reason),
}

/// The regular-file entry paths the scan saw.
pub(crate) type Contents = HashSet<String>;

/// Inspect `archive`.
///
/// # Errors
///
/// [`VerifyError::Io`] when the container cannot be read from disk;
/// once it is in memory, every parse failure is a verdict, not an
/// error (a container that will not parse in the strict profile is a
/// hostile or corrupt archive).
pub(crate) fn scan_archive(archive: &Path, limits: &Limits) -> Result<ScanOutcome, VerifyError> {
    let bytes = fs::read(archive).map_err(|source| VerifyError::Io {
        path: archive.to_path_buf(),
        source,
    })?;
    let compressed_size = bytes.len() as u64;
    let floor = RATIO_FLOOR_BASE_BYTES
        .saturating_add((limits.max_entries as u64).saturating_mul(FRAMING_BYTES_PER_ENTRY));
    let cap = limits
        .ratio_cap
        .saturating_mul(compressed_size)
        .max(floor)
        .min(limits.abs_cap_bytes);
    Ok(match parse(&bytes, cap, limits) {
        Ok((bytes, files)) => ScanOutcome::Manifest { bytes, files },
        Err(reason) => ScanOutcome::Reject(reason),
    })
}

/// One central-directory record, retained to drive the local-record
/// walk (a local header must reproduce every field a hostile
/// producer could have disagreed on).
struct Central {
    name: String,
    method: u16,
    gp: u16,
    crc: u32,
    compressed: u32,
    uncompressed: u32,
    local_offset: u32,
}

/// Parse the strict-profile container.  `Ok((manifest, files))` on
/// success; `Err(reason)` is a rejection verdict.  Bounds failures
/// map to [`Reason::ArchiveInvalid`]; specific violations return
/// their own code.
// One linear pass over the strict profile: the shared cursor, decode
// budget, and dedup sets thread through the whole container, so
// splitting it would scatter that state across helpers rather than
// clarify it.
#[allow(clippy::too_many_lines)]
fn parse(bytes: &[u8], cap: u64, limits: &Limits) -> Result<(Vec<u8>, Contents), Reason> {
    // Step 1: a container is at least a bare EOCD.
    if bytes.len() < EOCD_LEN {
        return Err(Reason::ArchiveInvalid);
    }

    // Step 2: the EOCD sits at exactly `len - 22` (comment length 0),
    // is single-disk, has matching counts, carries no zip64 sentinel,
    // and abuts a central directory that abuts the local records.
    let eocd = bytes.len() - EOCD_LEN;
    if u32_at(bytes, eocd)? != EOCD_SIG
        || u16_at(bytes, eocd + 4)? != 0
        || u16_at(bytes, eocd + 6)? != 0
        || u16_at(bytes, eocd + 20)? != 0
    {
        return Err(Reason::ArchiveInvalid);
    }
    let count_disk = u16_at(bytes, eocd + 8)?;
    let total = u16_at(bytes, eocd + 10)?;
    let cd_size = u32_at(bytes, eocd + 12)?;
    let cd_offset = u32_at(bytes, eocd + 16)?;
    // A sentinel in any count/size/offset field is the marker that the
    // real value lives in a zip64 record. The profile forbids zip64
    // outright and the reason table names it distinctly, so classify it
    // before the ordinary count-agreement check.
    if count_disk == U16_SENTINEL
        || total == U16_SENTINEL
        || cd_size == U32_SENTINEL
        || cd_offset == U32_SENTINEL
    {
        return Err(Reason::UnsupportedZipFeature("zip64"));
    }
    if count_disk != total {
        return Err(Reason::ArchiveInvalid);
    }
    // The central directory abuts the EOCD, and (once the local walk
    // ends at `cd_offset`) the local records abut the directory.
    let tiled = (u64::from(cd_offset))
        .checked_add(u64::from(cd_size))
        .and_then(|end| end.checked_add(EOCD_LEN as u64));
    if tiled != Some(bytes.len() as u64) {
        return Err(Reason::ArchiveInvalid);
    }
    let cd_start = cd_offset as usize;
    let cd_end = eocd; // == cd_start + cd_size, from the tiling equation above.

    // Step 3: reject the entry count before allocating for it.
    if usize::from(total) > limits.max_entries {
        return Err(Reason::TooManyEntries);
    }

    // Step 4: walk the central directory, consuming exactly
    // `cd_count` records and exactly `cd_size` bytes.
    let mut files: Contents = HashSet::new();
    let mut folded: HashSet<String> = HashSet::new();
    let mut records: Vec<Central> = Vec::with_capacity(usize::from(total));
    let mut off = cd_start;
    for _ in 0..total {
        if off + CENTRAL_HEADER_LEN > cd_end || u32_at(bytes, off)? != CENTRAL_SIG {
            return Err(Reason::ArchiveInvalid);
        }
        let gp = u16_at(bytes, off + 8)?;
        let method = u16_at(bytes, off + 10)?;
        let crc = u32_at(bytes, off + 16)?;
        let compressed = u32_at(bytes, off + 20)?;
        let uncompressed = u32_at(bytes, off + 24)?;
        let name_len = usize::from(u16_at(bytes, off + 28)?);
        let extra_len = usize::from(u16_at(bytes, off + 30)?);
        let comment_len = usize::from(u16_at(bytes, off + 32)?);
        let disk_start = u16_at(bytes, off + 34)?;
        let external_attrs = u32_at(bytes, off + 38)?;
        let local_offset = u32_at(bytes, off + 42)?;
        let name_end = off + CENTRAL_HEADER_LEN + name_len;
        let record_end = name_end + extra_len + comment_len;
        if record_end > cd_end {
            return Err(Reason::ArchiveInvalid);
        }
        let name_bytes = &bytes[off + CENTRAL_HEADER_LEN..name_end];

        // Banned features (structural single-disk violation aside).
        if extra_len != 0 {
            return Err(Reason::UnsupportedZipFeature("extra field"));
        }
        if comment_len != 0 {
            return Err(Reason::UnsupportedZipFeature("comment"));
        }
        if compressed == U32_SENTINEL
            || uncompressed == U32_SENTINEL
            || local_offset == U32_SENTINEL
        {
            return Err(Reason::UnsupportedZipFeature("zip64"));
        }
        // The EOCD already pinned the archive to a single disk; a
        // per-record disk index that disagrees is a malformed
        // container, not a feature request.
        if disk_start != 0 {
            return Err(Reason::ArchiveInvalid);
        }
        if method != 0 && method != 8 {
            return Err(Reason::UnsupportedZipFeature("method"));
        }
        if gp & GP_DATA_DESCRIPTOR != 0 {
            return Err(Reason::UnsupportedZipFeature("data descriptor"));
        }
        let non_ascii = name_bytes.iter().any(|byte| !byte.is_ascii());
        let allowed_gp = if non_ascii { GP_UTF8 } else { 0 };
        if gp != allowed_gp {
            return Err(Reason::UnsupportedZipFeature("gp flag"));
        }
        // A stored entry cannot claim compression it does not have.
        if method == 0 && compressed != uncompressed {
            return Err(Reason::HeaderMismatch("size"));
        }

        // Name rules (raw-byte length cap, UTF-8, portability).
        if name_len > limits.max_path_len {
            return Err(Reason::PathTooLong);
        }
        let Ok(name) = std::str::from_utf8(name_bytes) else {
            return Err(Reason::InvalidPath(None));
        };
        if let Some(reason) = classify_path(name) {
            return Err(reason);
        }
        // Type gate: regular files only.
        if let Some(reason) = entry_type_gate(external_attrs) {
            return Err(reason);
        }

        if !files.insert(name.to_owned()) {
            return Err(Reason::DuplicatePath);
        }
        if !folded.insert(name.to_lowercase()) {
            return Err(Reason::CaseConflict);
        }
        records.push(Central {
            name: name.to_owned(),
            method,
            gp,
            crc,
            compressed,
            uncompressed,
            local_offset,
        });
        off = record_end;
    }
    if off != cd_end {
        return Err(Reason::ArchiveInvalid);
    }

    // A regular file used as another entry's parent directory has no
    // consistent extraction.  Exact-name form is `path_conflict`;
    // case-folded form (`a` vs `A/b`) is `case_conflict`, checked
    // second so the more specific exact form wins.
    for path in &files {
        let mut boundary = 0;
        while let Some(slash) = path[boundary..].find('/') {
            boundary += slash;
            let prefix = &path[..boundary];
            if files.contains(prefix) {
                return Err(Reason::PathConflict);
            }
            if folded.contains(&prefix.to_lowercase()) {
                return Err(Reason::CaseConflict);
            }
            boundary += 1;
        }
    }

    // Step 5: a cheap pre-check on the sum of declared uncompressed
    // sizes, before inflating anything.
    let declared: u64 = records
        .iter()
        .map(|record| u64::from(record.uncompressed))
        .fold(0, u64::saturating_add);
    if declared > cap {
        return Err(Reason::DecompressedTooLarge);
    }

    // Steps 6-7: walk the local records in central-directory order,
    // requiring contiguous tiling and local == central, then
    // decompress each against the archive-global budget and verify
    // its CRC.
    let mut budget = cap;
    let mut manifest: Option<Vec<u8>> = None;
    let mut pos = 0usize;
    for record in &records {
        // Tiling and bijection: the local record the directory names
        // must start exactly where the previous one ended.
        if record.local_offset as usize != pos
            || pos + LOCAL_HEADER_LEN > cd_start
            || u32_at(bytes, pos)? != LOCAL_SIG
        {
            return Err(Reason::ArchiveInvalid);
        }
        let l_gp = u16_at(bytes, pos + 6)?;
        let l_method = u16_at(bytes, pos + 8)?;
        let l_crc = u32_at(bytes, pos + 14)?;
        let l_compressed = u32_at(bytes, pos + 18)?;
        let l_uncompressed = u32_at(bytes, pos + 22)?;
        let l_name_len = usize::from(u16_at(bytes, pos + 26)?);
        let l_extra_len = usize::from(u16_at(bytes, pos + 28)?);
        if l_extra_len != 0 {
            return Err(Reason::UnsupportedZipFeature("extra field"));
        }
        let name_end = pos + LOCAL_HEADER_LEN + l_name_len;
        if name_end > cd_start {
            return Err(Reason::ArchiveInvalid);
        }
        if &bytes[pos + LOCAL_HEADER_LEN..name_end] != record.name.as_bytes()
            || l_method != record.method
            || l_gp != record.gp
            || l_crc != record.crc
            || l_compressed != record.compressed
            || l_uncompressed != record.uncompressed
        {
            return Err(Reason::HeaderMismatch("local header"));
        }
        let data_end = name_end
            .checked_add(record.compressed as usize)
            .ok_or(Reason::ArchiveInvalid)?;
        if data_end > cd_start {
            return Err(Reason::ArchiveInvalid);
        }
        let data = &bytes[name_end..data_end];

        let mut collector = (record.name == ROOT_MANIFEST).then(Vec::new);
        let (crc, produced, consumed) =
            decode_entry(record.method, data, budget, collector.as_mut())?;
        if record.method == 8
            && (consumed != u64::from(record.compressed)
                || produced != u64::from(record.uncompressed))
        {
            return Err(Reason::HeaderMismatch("deflate"));
        }
        if crc != record.crc {
            return Err(Reason::HeaderMismatch("crc"));
        }
        budget -= produced;
        if let Some(bytes) = collector {
            manifest = Some(bytes);
        }
        pos = data_end;
    }
    if pos != cd_start {
        return Err(Reason::ArchiveInvalid);
    }

    // Step 8: the archive root manifest must be present.
    manifest
        .map(|bytes| (bytes, files))
        .ok_or(Reason::ManifestMissing)
}

/// Read the external attributes' type bits independently of the
/// version-made-by system byte and reject any present non-regular
/// type or the DOS directory attribute.  An absent or zero mode is a
/// regular file; permission bits are ignored.
fn entry_type_gate(external_attrs: u32) -> Option<Reason> {
    let fmt = (external_attrs >> 16) & S_IFMT;
    if (fmt != 0 && fmt != S_IFREG) || external_attrs & DOS_DIRECTORY != 0 {
        return Some(Reason::ForbiddenEntryType);
    }
    None
}

/// Decompress one entry through the archive-global [`CappedReader`]
/// budget, returning `(crc, produced, consumed)` where `consumed` is
/// the compressed input the deflate layer read (equal to `produced`
/// for a stored entry).  The manifest entry's bytes are collected
/// into `collect`.
fn decode_entry(
    method: u16,
    data: &[u8],
    budget: u64,
    collect: Option<&mut Vec<u8>>,
) -> Result<(u32, u64, u64), Reason> {
    match method {
        // Store: the compressed and uncompressed bytes are identical
        // (enforced `compressed == uncompressed` above), so the slice
        // itself is the output.
        0 => {
            let mut capped = CappedReader::new(data, budget);
            let (crc, produced) = drain(&mut capped, collect)?;
            Ok((crc, produced, produced))
        }
        // Deflate: the stream must reach its final block, not merely
        // run out of input. `flate2`'s reader maps an input EOF that
        // arrives mid-stream to a silent `Ok(0)`, so reading to EOF
        // does not prove the stream ended - a non-final block followed
        // by EOF could otherwise produce exactly the declared bytes and
        // pass. Drive the raw inflater directly and require
        // `Status::StreamEnd`; `consumed`/`produced` are then checked
        // against the declared sizes by the caller.
        8 => inflate(data, budget, collect),
        _ => unreachable!("method restricted to store/deflate in the central-directory walk"),
    }
}

/// Inflate a raw-deflate span, requiring it to reach `Status::StreamEnd`
/// rather than merely exhaust its input. Returns `(crc, produced,
/// consumed)`; the caller checks those against the declared sizes.
/// Output is metered against the archive-global `budget`, and any
/// stall short of the stream end is a rejection, so a truncated or
/// non-terminated stream cannot pass by producing the declared byte
/// count.
fn inflate(
    data: &[u8],
    budget: u64,
    mut collect: Option<&mut Vec<u8>>,
) -> Result<(u32, u64, u64), Reason> {
    let mut dec = Decompress::new(false);
    let mut hasher = crc32fast::Hasher::new();
    let mut out = [0u8; 16 * 1024];
    let mut produced = 0u64;
    loop {
        let (in_before, out_before) = (dec.total_in(), dec.total_out());
        let input = &data[usize::try_from(in_before).map_err(|_| Reason::ArchiveInvalid)?..];
        let status = dec
            .decompress(input, &mut out, FlushDecompress::None)
            .map_err(|_| Reason::HeaderMismatch("deflate"))?;
        let written = usize::try_from(dec.total_out() - out_before).expect("output fits usize");
        if written > 0 {
            produced += written as u64;
            if produced > budget {
                return Err(Reason::DecompressedTooLarge);
            }
            hasher.update(&out[..written]);
            if let Some(sink) = collect.as_deref_mut() {
                sink.extend_from_slice(&out[..written]);
            }
        }
        if status == Status::StreamEnd {
            return Ok((hasher.finalize(), produced, dec.total_in()));
        }
        // No forward progress and the stream has not ended: the input is
        // exhausted mid-stream (truncation) or wedged on garbage.
        if dec.total_in() == in_before && written == 0 {
            return Err(Reason::HeaderMismatch("deflate"));
        }
    }
}

/// Read `capped` to its end, hashing the bytes and optionally
/// collecting them, and return `(crc, produced)`.
fn drain<R: Read>(
    capped: &mut CappedReader<R>,
    mut collect: Option<&mut Vec<u8>>,
) -> Result<(u32, u64), Reason> {
    let mut hasher = crc32fast::Hasher::new();
    let mut buf = [0u8; 16 * 1024];
    let mut produced = 0u64;
    loop {
        match capped.read(&mut buf) {
            Ok(0) => break,
            Ok(read) => {
                hasher.update(&buf[..read]);
                produced += read as u64;
                if let Some(sink) = collect.as_deref_mut() {
                    sink.extend_from_slice(&buf[..read]);
                }
            }
            // The cap was crossed (a bomb), or the deflate stream would
            // not decode (truncation or garbage inside its span).
            Err(_) if capped.exceeded => return Err(Reason::DecompressedTooLarge),
            Err(_) => return Err(Reason::HeaderMismatch("deflate")),
        }
    }
    Ok((hasher.finalize(), produced))
}

/// Read a little-endian `u16` at `offset`, or [`Reason::ArchiveInvalid`]
/// if it runs past the buffer.
fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, Reason> {
    bytes
        .get(offset..offset + 2)
        .map(|slice| u16::from_le_bytes(slice.try_into().expect("2-byte slice")))
        .ok_or(Reason::ArchiveInvalid)
}

/// Read a little-endian `u32` at `offset`, or [`Reason::ArchiveInvalid`]
/// if it runs past the buffer.
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, Reason> {
    bytes
        .get(offset..offset + 4)
        .map(|slice| u32::from_le_bytes(slice.try_into().expect("4-byte slice")))
        .ok_or(Reason::ArchiveInvalid)
}

/// Classify an entry name against the safe-relative-path rules and
/// the shared portability set.  `cabin package` emits forward-slash
/// relative paths with `Normal` components only, so anything else is
/// rejected: absolute paths (POSIX or Windows-drive form), `..`
/// traversal, structural hazards (`\`, empty or `.` components, a
/// trailing directory marker), and - via the shared `cabin-fs`
/// predicate that pack time enforces too - the Windows-hostile
/// component shapes, whose violated rule becomes the parenthesized
/// detail.
fn classify_path(path: &str) -> Option<Reason> {
    if path.is_empty() {
        return Some(Reason::InvalidPath(None));
    }
    if path.starts_with('/') {
        return Some(Reason::AbsolutePath);
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Some(Reason::AbsolutePath);
    }
    if path.contains('\\') {
        return Some(Reason::InvalidPath(None));
    }
    for component in path.split('/') {
        match component {
            ".." => return Some(Reason::PathTraversal),
            // Covers empty (`a//b`), a `.` component, and the empty
            // last component a trailing `/` directory marker leaves.
            "" | "." => return Some(Reason::InvalidPath(None)),
            _ => {}
        }
    }
    cabin_fs::path::relative_path_portability(path)
        .map(|violation| Reason::InvalidPath(Some(violation.detail())))
}

/// A reader that refuses to produce more than `remaining` bytes.
/// Unlike [`io::Take`], crossing the cap is an error (with the
/// `exceeded` flag set so the caller can tell a bomb from a corrupt
/// stream), not a silent EOF - a deflate stream that lies about its
/// uncompressed size must never pass as complete.
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
    fn classify_path_rejects_structural_shapes() {
        for (path, reason) in [
            ("", Reason::InvalidPath(None)),
            ("/etc/passwd", Reason::AbsolutePath),
            ("c:/evil", Reason::AbsolutePath),
            ("C:\\evil", Reason::AbsolutePath),
            ("..", Reason::PathTraversal),
            ("../escape", Reason::PathTraversal),
            ("src/../../escape", Reason::PathTraversal),
            ("a\\b", Reason::InvalidPath(None)),
            ("a//b", Reason::InvalidPath(None)),
            ("./a", Reason::InvalidPath(None)),
            ("a/./b", Reason::InvalidPath(None)),
            ("src/", Reason::InvalidPath(None)),
        ] {
            assert_eq!(classify_path(path), Some(reason), "path: {path:?}");
        }
    }

    #[test]
    fn classify_path_reports_portability_violations_with_a_detail() {
        for (path, detail) in [
            ("src/a:b.h", "colon"),
            ("CON", "windows device name"),
            ("file.", "trailing dot"),
            ("file ", "trailing space"),
            ("a\u{7}b", "control character"),
        ] {
            assert_eq!(
                classify_path(path),
                Some(Reason::InvalidPath(Some(detail))),
                "path: {path:?}"
            );
        }
    }

    #[test]
    fn entry_type_gate_accepts_regular_and_absent_modes() {
        assert_eq!(entry_type_gate(0), None);
        assert_eq!(entry_type_gate((S_IFREG | 0o644) << 16), None);
    }

    #[test]
    fn entry_type_gate_rejects_non_regular_types_and_the_dos_dir_attr() {
        // S_IFLNK in the high bits, and the DOS directory attribute
        // in the low byte, are both forbidden.
        assert_eq!(
            entry_type_gate(0o120_777 << 16),
            Some(Reason::ForbiddenEntryType)
        );
        assert_eq!(
            entry_type_gate(DOS_DIRECTORY),
            Some(Reason::ForbiddenEntryType)
        );
    }
}
