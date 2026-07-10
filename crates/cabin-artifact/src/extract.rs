use std::cell::Cell;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read, Seek as _};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use cabin_core::PackageName;
use cabin_fs::path::is_safe_relative_path;

use crate::error::ArtifactError;

/// Maximum decompressed bytes Cabin will write for a single tar
/// entry.  Single source files larger than 256 MiB do not occur in
/// any C/C++ package this tool is expected to ingest; the cap
/// exists to refuse a `.tar.gz` whose entry headers claim a huge
/// `size` and whose gzip stream expands to that size from a tiny
/// compressed payload (a "decompression bomb").
const MAX_ENTRY_BYTES: u64 = 256 * 1024 * 1024;

// POSIX `st_mode` file-type bits (see `<sys/stat.h>`), used to
// classify zip entries whose type travels in the Unix mode field.
const S_IFMT: u32 = 0o170_000; // type field mask
const S_IFLNK: u32 = 0o120_000; // symbolic link
const S_IFREG: u32 = 0o100_000; // regular file
const S_IFDIR: u32 = 0o040_000; // directory

/// Maximum aggregate decompressed bytes Cabin will write across
/// every entry in one archive.  Even with the per-entry cap, an
/// attacker could ship thousands of max-size entries to fill the
/// user's disk; the aggregate cap bounds total damage to ~1 GiB.
const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

/// Maximum number of tar entries Cabin will process from one
/// archive.  Headers alone (no body) can be cheap to ship and
/// expensive to materialize as filesystem inodes, so the count
/// is capped independently of the byte caps.
const MAX_ENTRIES: usize = 10_000;

/// Maximum bytes in one entry's path.  Real package and upstream
/// source trees sit far below this, and the hosted-registry verifier
/// (`cabin-registry-verify`) enforces the same default, so a
/// verified archive can never trip it.  The cap also bounds
/// directory nesting depth implicitly: every level costs at least
/// two bytes of path.
const MAX_PATH_BYTES: usize = 256;

/// Maximum decompressed bytes the whole gzip stream may produce per
/// compressed byte (see [`ExtractLimits::stream_cap`]).  Source
/// archives gzip at single-digit ratios; 32x leaves wide margin for
/// unusually compressible trees while still refusing a small
/// download that inflates toward the absolute cap.  Deliberately
/// looser than the hosted-registry verifier's 10x: the client cap
/// must never reject an archive the verifier accepted.
const MAX_DECOMPRESSION_RATIO: u64 = 32;

/// The ratio cap only engages above this floor.  Tar framing
/// (headers, padding, the EOF marker) is mostly zeros and compresses
/// at far better than any sane content ratio, so tiny legitimate
/// archives routinely "expand" 15-30x on framing alone; the floor
/// keeps them clear of the ratio cap while still bounding what a
/// small hostile download can write.  Covers the verifier's floor
/// (4 MiB + 2 KiB per permitted entry = 24 MiB) with room to spare.
const RATIO_FLOOR_BYTES: u64 = 64 * 1024 * 1024;

/// Decompressed bytes of tar framing and metadata allowed per
/// permitted entry.  Worst case for a *legitimate* entry is 2047
/// bytes: a GNU long-name record (512-byte header plus one 512-byte
/// padded payload block, which is what a path up to
/// [`MAX_PATH_BYTES`] needs, since the plain header's name field
/// holds only 100) followed by the entry's own 512-byte header and
/// up to 511 bytes of content padding.  A 2 KiB allowance would
/// therefore clear the worst honest archive by under 10 KiB across
/// the whole entry cap; doubling it buys real headroom while
/// keeping the metadata budget - and so the peak allocation - small.
/// `the_worst_legitimate_framing_still_fits_the_metadata_budget`
/// pins this.
const FRAMING_BYTES_PER_ENTRY: u64 = 4096;

/// The extraction caps.  `Default` is the production values above;
/// tests inject smaller ones to exercise each cap cheaply.
#[derive(Debug, Clone, Copy)]
struct ExtractLimits {
    max_entry_bytes: u64,
    max_total_bytes: u64,
    max_entries: usize,
    max_path_bytes: usize,
    ratio: u64,
    ratio_floor_bytes: u64,
}

impl Default for ExtractLimits {
    fn default() -> Self {
        ExtractLimits {
            max_entry_bytes: MAX_ENTRY_BYTES,
            max_total_bytes: MAX_TOTAL_BYTES,
            max_entries: MAX_ENTRIES,
            max_path_bytes: MAX_PATH_BYTES,
            ratio: MAX_DECOMPRESSION_RATIO,
            ratio_floor_bytes: RATIO_FLOOR_BYTES,
        }
    }
}

impl ExtractLimits {
    /// Decompressed bytes of tar framing and metadata records the
    /// whole archive may spend: [`FRAMING_BYTES_PER_ENTRY`] for each
    /// permitted entry.
    ///
    /// This is the memory bound.  The tar reader buffers a GNU
    /// long-name or PAX record fully in memory before the entry it
    /// decorates - and therefore before any entry-type or path check
    /// can reject it - so without a metadata-specific budget a
    /// hostile record could grow a `Vec` all the way to the
    /// whole-stream cap.  Charging framing separately keeps the peak
    /// allocation at this budget instead.
    fn metadata_cap(&self) -> u64 {
        (self.max_entries as u64).saturating_mul(FRAMING_BYTES_PER_ENTRY)
    }

    /// Decompressed-stream cap for an archive of `compressed_size`
    /// bytes: `min(max(ratio x compressed, floor), absolute)`, where
    /// the absolute ceiling is `max_total_bytes` plus the framing
    /// allowance.  The allowance keeps the cap layers separable: an
    /// archive whose *contents* run over still gets the per-entry or
    /// aggregate error naming the entry, and the stream cap fires
    /// only for ratio violations.
    fn stream_cap(&self, compressed_size: u64) -> u64 {
        self.ratio
            .saturating_mul(compressed_size)
            .max(self.ratio_floor_bytes)
            .min(self.max_total_bytes.saturating_add(self.metadata_cap()))
    }
}

/// Which budget a [`CappedReader`] ran out of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapExceeded {
    Stream,
    Metadata,
}

/// Two budgets the decompressed byte stream is drawn against, shared
/// between the reader (buried inside `tar::Archive`) and the
/// extraction loop that knows which phase the reader is in.
///
/// Every decompressed byte is charged to `stream_remaining`.  Bytes
/// read while `in_body` is false - tar headers, padding, and the
/// metadata records the tar reader buffers - are additionally
/// charged to `metadata_remaining`.  The extraction loop marks the
/// body phase only while it is copying a regular file's contents to
/// disk, which is the one place tar bytes flow straight through
/// instead of into memory.
#[derive(Debug)]
struct StreamBudget {
    stream_remaining: Cell<u64>,
    metadata_remaining: Cell<u64>,
    in_body: Cell<bool>,
    exceeded: Cell<Option<CapExceeded>>,
}

impl StreamBudget {
    fn new(stream_cap: u64, metadata_cap: u64) -> Rc<Self> {
        Rc::new(StreamBudget {
            stream_remaining: Cell::new(stream_cap),
            metadata_remaining: Cell::new(metadata_cap),
            in_body: Cell::new(false),
            exceeded: Cell::new(None),
        })
    }

    /// Run `body`, charging the bytes it reads to the stream budget
    /// only.  Restores the metadata phase afterwards, including on
    /// the error path.
    fn in_body<T>(&self, body: impl FnOnce() -> T) -> T {
        self.in_body.set(true);
        let out = body();
        self.in_body.set(false);
        out
    }
}

/// A reader that refuses to produce more bytes than its
/// [`StreamBudget`] allows.  Unlike [`io::Take`], crossing a budget
/// is an error (recorded on the budget so the caller can tell a bomb
/// from a corrupt stream), not a silent EOF - a truncated tar must
/// never pass as a complete one.  Modeled on the hosted-registry
/// verifier's reader (`cabin-registry-verify`).
struct CappedReader<R> {
    inner: R,
    budget: Rc<StreamBudget>,
}

impl<R: Read> Read for CappedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let in_body = self.budget.in_body.get();
        let stream = self.budget.stream_remaining.get();
        let metadata = self.budget.metadata_remaining.get();
        let allowance = if in_body {
            stream
        } else {
            stream.min(metadata)
        };
        if allowance == 0 {
            // At a cap: only a clean EOF may follow.  Probe one byte
            // to tell the two apart.
            let mut probe = [0u8; 1];
            if self.inner.read(&mut probe)? == 0 {
                return Ok(0);
            }
            let kind = if in_body || stream == 0 {
                CapExceeded::Stream
            } else {
                CapExceeded::Metadata
            };
            self.budget.exceeded.set(Some(kind));
            return Err(io::Error::other("decompressed size cap exceeded"));
        }
        let allowed = usize::try_from(allowance.min(buf.len() as u64)).unwrap_or(buf.len());
        let read = self.inner.read(&mut buf[..allowed])?;
        self.budget.stream_remaining.set(stream - read as u64);
        if !in_body {
            self.budget.metadata_remaining.set(metadata - read as u64);
        }
        Ok(read)
    }
}

/// Options accepted by [`safe_extract_tar_gz`] and
/// [`safe_extract_zip`].
///
/// `Default` produces the original artifact-layer behavior: no
/// prefix stripping, symlink entries rejected, archive is expected
/// to contain `cabin.toml` at its root.
#[derive(Debug, Clone, Copy, Default)]
pub struct SafeExtractOptions<'a> {
    /// If `Some`, every archive entry must start with this single
    /// directory component; the component is stripped before the
    /// path is joined into `dest`.  The post-strip path is then
    /// re-checked by the same path-safety rules as a top-level
    /// entry, so a malicious archive that ships
    /// `<prefix>/../escape` is rejected after the strip.
    ///
    /// An archive that does not contain a single entry beginning
    /// with `strip_prefix` produces
    /// [`ArtifactError::MissingStripPrefix`].
    pub strip_prefix: Option<&'a str>,
    /// If `true`, symlink entries are silently skipped instead of
    /// failing the extraction.  Nothing is ever materialized on
    /// disk for a skipped entry, so the traversal-safety posture
    /// is unchanged; the option only decides between "refuse the
    /// whole archive" (default, right for Cabin-produced package
    /// archives, which never contain symlinks) and "extract
    /// everything else" (used for foundation-port upstream
    /// archives, where a stray convenience symlink like uthash's
    /// `include -> src` is common and never referenced by the
    /// overlay).  Every other special entry type (hard links,
    /// devices, fifos, ...) is still rejected.
    pub skip_symlinks: bool,
}

/// Safely extract a `.zip` archive into `dest`, with caller-
/// supplied options.
///
/// The same fail-closed rules as [`safe_extract_tar_gz`] apply:
/// unsafe entry paths are rejected before and after the optional
/// prefix strip (including over-long paths and duplicate
/// destinations), only regular files and directories are accepted
/// (a zip entry whose recorded Unix mode is a symlink - or any
/// other non-file, non-directory type - is refused), and the same
/// per-entry / aggregate / entry-count decompression-bomb caps are
/// enforced against the *actual* decompressed bytes, not the sizes
/// the zip headers claim.  The aggregate cap is additionally scaled
/// down to the same compressed-size-derived value the tar side uses,
/// so a small hostile zip cannot write toward the absolute ceiling
/// either.
///
/// There is no separate whole-stream cap: zip decompresses per
/// entry, so those per-entry caps already bound every decompressed
/// byte, and zip metadata is read from bytes the archive really
/// contains (names are `u16`-bounded, the count is bounded by the
/// end-of-central-directory pre-check), so it cannot be amplified
/// the way a gzip-compressed tar metadata record can.
///
/// # Errors
/// Mirrors [`safe_extract_tar_gz`]: [`ArtifactError::Io`] when
/// `archive` cannot be opened, [`ArtifactError::Extract`] when the
/// zip structure cannot be read, and the same
/// `UnsafeArchiveEntry` / `ArchiveEntryPathTooLong` /
/// `DuplicateArchiveEntry` / `ConflictingArchiveEntry` /
/// `UnsupportedArchiveEntry` / `ArchiveEntryTooLarge` /
/// `ArchiveTooLarge` / `ArchiveTooManyEntries` /
/// `MissingStripPrefix` rejections.
pub fn safe_extract_zip(
    archive: &Path,
    dest: &Path,
    options: SafeExtractOptions<'_>,
) -> Result<(), ArtifactError> {
    safe_extract_zip_with_limits(archive, dest, ExtractLimits::default(), options)
}

fn safe_extract_zip_with_limits(
    archive: &Path,
    dest: &Path,
    limits: ExtractLimits,
    options: SafeExtractOptions<'_>,
) -> Result<(), ArtifactError> {
    let mut f = File::open(archive).map_err(|source| ArtifactError::Io {
        path: archive.to_path_buf(),
        source,
    })?;
    let compressed_size = f
        .metadata()
        .map_err(|source| ArtifactError::Io {
            path: archive.to_path_buf(),
            source,
        })?
        .len();
    let max_total_bytes = limits
        .stream_cap(compressed_size)
        .min(limits.max_total_bytes);
    // Cap the archive file itself.  `zip::ZipArchive::new` parses the
    // whole central directory into memory before any per-entry check
    // below can reject it, and each record can carry ~64 KiB of name,
    // extra, and comment fields; that memory is bounded by the file
    // size (the central directory cannot exceed the file), so bounding
    // the file bounds the metadata.  Unlike a decompression bomb this
    // is not amplified - the archive is already downloaded and its
    // SHA-256 verified before extraction - so the aggregate ceiling is
    // a generous bound rather than a tight one.
    if compressed_size > limits.max_total_bytes {
        return Err(ArtifactError::ArchiveFileTooLarge {
            size: compressed_size,
            limit: limits.max_total_bytes,
        });
    }
    // Refuse an over-cap entry count from the end-of-central-directory
    // record *before* the parser materializes per-entry metadata, so a
    // crafted zip with an enormous entry count is rejected without
    // paying memory or CPU proportional to that count.  Best-effort:
    // when no EOCD is found the full parser below surfaces the
    // canonical error for the malformed archive.
    let declared_entries = zip_eocd_entry_count(&mut f);
    if let Some(count) = declared_entries
        && count > limits.max_entries as u64
    {
        return Err(ArtifactError::ArchiveTooManyEntries {
            limit: limits.max_entries,
        });
    }
    f.seek(io::SeekFrom::Start(0))
        .map_err(|source| ArtifactError::Io {
            path: archive.to_path_buf(),
            source,
        })?;
    let extract_err = |source: zip::result::ZipError| ArtifactError::Extract {
        path: archive.to_path_buf(),
        source: io::Error::other(source),
    };
    let mut zip = zip::ZipArchive::new(f).map_err(extract_err)?;

    // Authoritative re-check on the parsed directory: the EOCD scan
    // above is a fast-fail heuristic and a hostile archive could
    // understate its count there.
    if zip.len() > limits.max_entries {
        return Err(ArtifactError::ArchiveTooManyEntries {
            limit: limits.max_entries,
        });
    }
    // The `zip` crate keys entries by raw name in an `IndexMap`, so
    // two records sharing a name collapse to one ("last wins") before
    // `TargetTree` below could see the duplicate.  A well-formed
    // archive's parsed length equals its declared entry count, so a
    // shortfall means names were deduplicated - reject it, since a
    // duplicate path is exactly the last-wins confusion the tar side
    // refuses by name.
    //
    // Best-effort by nature: it reads the classic end-of-central-
    // directory count, so a ZIP64 archive (sentinel `u64::MAX`, or a
    // classic count that disagrees with the ZIP64 count the parser
    // used) or an archive with bytes appended after the EOCD (which
    // makes `zip_eocd_entry_count` decline to guess) slips past it.
    // Registry package archives are `.tar.gz` and take the tar path,
    // which refuses duplicates unconditionally; zip is reached only
    // for foundation-port upstreams, whose bytes are pinned by SHA-256
    // in an in-repo recipe, so this narrow gap is a defense-in-depth
    // shortfall, not an exposure of the primary surface.
    if let Some(declared) = declared_entries
        && declared != u64::MAX
        && (zip.len() as u64) < declared
    {
        return Err(ArtifactError::ArchiveDuplicateNames {
            declared,
            distinct: zip.len(),
        });
    }

    let mut total_bytes: u64 = 0;
    let mut saw_prefix = false;
    let mut tree = TargetTree::default();

    for index in 0..zip.len() {
        let entry = zip.by_index(index).map_err(extract_err)?;
        write_zip_entry(
            entry,
            dest,
            limits,
            max_total_bytes,
            options,
            &mut tree,
            &mut saw_prefix,
            &mut total_bytes,
        )?;
    }
    if let Some(prefix) = options.strip_prefix
        && !saw_prefix
    {
        return Err(ArtifactError::MissingStripPrefix {
            strip_prefix: prefix.to_owned(),
        });
    }
    Ok(())
}

/// Validate and materialize one zip entry.  Mirrors the tar per-entry
/// path: entry-type gate, path safety, target-tree conflict check,
/// byte caps, and the header-size truncation check.
#[allow(clippy::too_many_arguments)]
fn write_zip_entry<R: Read>(
    mut entry: zip::read::ZipFile<'_, R>,
    dest: &Path,
    limits: ExtractLimits,
    max_total_bytes: u64,
    options: SafeExtractOptions<'_>,
    tree: &mut TargetTree,
    saw_prefix: &mut bool,
    total_bytes: &mut u64,
) -> Result<(), ArtifactError> {
    if entry.name().len() > limits.max_path_bytes {
        return Err(ArtifactError::ArchiveEntryPathTooLong {
            path: truncate_for_display(entry.name().as_bytes()),
            limit: limits.max_path_bytes,
        });
    }
    let display = entry.name().to_owned();

    // Zip has no first-class entry-type field; the Unix mode bits
    // travel in the external attributes when the archive was produced
    // on a Unix host.  Reject anything that is recorded as neither a
    // regular file nor a directory (symlinks foremost), mirroring the
    // tar entry-type policy; symlinks alone may instead be skipped
    // when the caller opted in (nothing is materialized either way).
    let file_type = entry.unix_mode().map(|mode| mode & S_IFMT);
    let skip_symlink = file_type == Some(S_IFLNK) && options.skip_symlinks;
    if let Some(file_type) = file_type
        && !skip_symlink
        && file_type != 0
        && file_type != S_IFREG
        && file_type != S_IFDIR
    {
        return Err(ArtifactError::UnsupportedArchiveEntry(display));
    }

    // Path-safety and prefix checks run for skipped symlinks too,
    // exactly like the tar path: skipping avoids materializing the
    // entry, not validating the archive.
    let entry_path = PathBuf::from(entry.name());
    let Some(target) = resolve_safe_target(
        &entry_path,
        dest,
        limits.max_path_bytes,
        options,
        saw_prefix,
    )?
    else {
        return Ok(());
    };
    if skip_symlink {
        return Ok(());
    }
    let is_dir = entry.is_dir();
    tree.claim(&target, dest, is_dir, &display)?;

    if is_dir {
        fs::create_dir_all(&target).map_err(|source| ArtifactError::Io {
            path: target.clone(),
            source,
        })?;
        return Ok(());
    }
    let expected = entry.size();
    let written = write_file_capped(
        &mut entry,
        &target,
        &display,
        limits.max_entry_bytes,
        max_total_bytes,
        total_bytes,
    )?;
    // A stored entry whose local header overstates its uncompressed
    // size hands the reader a short body; the tar side refuses the
    // same mismatch, so mirror it here rather than materialize a
    // silently truncated source file.
    if written != expected {
        let _ = fs::remove_file(&target);
        return Err(ArtifactError::ArchiveEntryTruncated {
            path: display,
            expected,
            actual: written,
        });
    }
    Ok(())
}

/// Best-effort read of the entry count recorded in a zip's
/// end-of-central-directory record, without materializing any
/// central-directory metadata.  The EOCD lives within
/// `22 + 65535` bytes of EOF (fixed record plus maximum comment);
/// scan that tail backwards for the signature and read the 16-bit
/// total-entry field.  A `0xFFFF` count is the ZIP64 marker, which
/// means the archive holds at least 65535 entries - report it as
/// `u64::MAX` so any real cap rejects it without needing ZIP64
/// parsing.  Returns `None` when no EOCD is found; the caller falls
/// through to the full parser, which surfaces the canonical error
/// for a malformed archive.
fn zip_eocd_entry_count(f: &mut File) -> Option<u64> {
    const EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    const EOCD_LEN: u64 = 22;
    const MAX_COMMENT: u64 = 65_535;
    let len = f.metadata().ok()?.len();
    if len < EOCD_LEN {
        return None;
    }
    let tail_len = len.min(EOCD_LEN + MAX_COMMENT);
    f.seek(io::SeekFrom::Start(len - tail_len)).ok()?;
    let mut tail = vec![0u8; usize::try_from(tail_len).ok()?];
    f.read_exact(&mut tail).ok()?;
    // Scan candidates from EOF backwards, but only trust one whose
    // declared comment length reaches exactly EOF: the archive
    // comment *follows* the real record, so a stray signature inside
    // the comment sits nearer EOF than the real EOCD and would
    // otherwise supply arbitrary comment bytes as the entry count.
    let mut search_end = tail.len();
    while let Some(pos) = tail[..search_end].windows(4).rposition(|w| w == EOCD_SIG) {
        if let Some(cl) = tail.get(pos + 20..pos + 22) {
            let comment_len = usize::from(u16::from_le_bytes([cl[0], cl[1]]));
            if pos + 22 + comment_len == tail.len() {
                let total = &tail[pos + 10..pos + 12];
                let count = u64::from(u16::from_le_bytes([total[0], total[1]]));
                return Some(if count == 0xFFFF { u64::MAX } else { count });
            }
        }
        if pos == 0 {
            break;
        }
        search_end = pos;
    }
    None
}

/// Safely extract a `.tar.gz` archive into `dest` with the default
/// production caps and no prefix stripping.  Kept as the
/// crate-internal entry point used by the source-archive fetcher.
pub(crate) fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<(), ArtifactError> {
    safe_extract_tar_gz_with_limits(
        archive,
        dest,
        ExtractLimits::default(),
        SafeExtractOptions::default(),
    )
}

/// Safely extract a `.tar.gz` archive into `dest`, with caller-
/// supplied options.
///
/// Fail-closed rules:
/// - reject entries with absolute paths or `..` components;
/// - reject entries whose joined destination escapes `dest`;
/// - reject entry paths longer than a fixed byte cap (which also
///   bounds nesting depth) and any two entries that resolve to the
///   same destination (a duplicate would silently "last-win");
/// - accept only `Regular` files and `Directory` entries - every other
///   tar entry type (symlinks, hard links, char/block devices, fifos,
///   sparse, etc.) is rejected;
/// - cap per-entry decompressed bytes, aggregate decompressed
///   bytes, and total entry count so a decompression-bomb archive
///   (small compressed payload, huge decompressed output) cannot
///   fill the user's disk;
/// - cap the decompressed byte stream as a whole at a multiple of
///   the compressed size (with a floor and an absolute ceiling), and
///   cap the tar framing and metadata records separately, bounding
///   the memory the tar reader can spend buffering them as well as
///   the total disk written;
/// - when [`SafeExtractOptions::strip_prefix`] is set, require
///   every entry to begin with that single directory component
///   and re-run path-safety checks on the post-strip path.  An
///   archive whose entries never match the declared prefix
///   surfaces [`ArtifactError::MissingStripPrefix`].
///
/// The rules are lexical by design: no destination ever needs
/// `fs::canonicalize`, because nothing that could redirect a path
/// (a symlink, a hard link) is ever materialized, so a validated
/// destination under `dest` cannot be re-pointed elsewhere.
/// Callers are expected to extract into a directory they created
/// empty, and the fetch path does so (a sibling temp dir renamed
/// into place on success).
///
/// # Errors
/// Returns [`ArtifactError::Io`] if `archive` cannot be opened and
/// [`ArtifactError::Extract`] if the gzip/tar stream cannot be read.
/// Returns [`ArtifactError::UnsafeArchiveEntry`] for entries that are
/// absolute, contain `..`, or escape `dest`;
/// [`ArtifactError::ArchiveEntryPathTooLong`] for an over-long entry
/// path; [`ArtifactError::DuplicateArchiveEntry`] for a repeated
/// destination; [`ArtifactError::UnsupportedArchiveEntry`] for any
/// entry that is not a regular file or directory;
/// [`ArtifactError::ArchiveEntryTooLarge`],
/// [`ArtifactError::ArchiveTooLarge`], or
/// [`ArtifactError::ArchiveTooManyEntries`] when the per-entry, aggregate,
/// or entry-count cap is exceeded;
/// [`ArtifactError::ArchiveStreamTooLarge`] when the whole
/// decompressed stream crosses the compressed-size-derived cap; and
/// [`ArtifactError::MissingStripPrefix`] when `options.strip_prefix` is set
/// but no entry begins with that component.
pub fn safe_extract_tar_gz(
    archive: &Path,
    dest: &Path,
    options: SafeExtractOptions<'_>,
) -> Result<(), ArtifactError> {
    safe_extract_tar_gz_with_limits(archive, dest, ExtractLimits::default(), options)
}

fn safe_extract_tar_gz_with_limits(
    archive: &Path,
    dest: &Path,
    limits: ExtractLimits,
    options: SafeExtractOptions<'_>,
) -> Result<(), ArtifactError> {
    let io_error = |source: io::Error| ArtifactError::Io {
        path: archive.to_path_buf(),
        source,
    };
    let compressed_size = fs::metadata(archive).map_err(io_error)?.len();
    let cap = limits.stream_cap(compressed_size);
    let metadata_cap = limits.metadata_cap();
    let budget = StreamBudget::new(cap, metadata_cap);
    let f = File::open(archive).map_err(io_error)?;
    let dec = flate2::read::GzDecoder::new(f);
    let mut tar = tar::Archive::new(CappedReader {
        inner: dec,
        budget: Rc::clone(&budget),
    });

    let result = extract_tar_entries(&mut tar, archive, dest, limits, options, &budget);
    // A crossed budget surfaces through the gzip/tar layers as an
    // opaque I/O failure, so the recorded kind - not the error that
    // came back - is what distinguishes a decompression bomb from a
    // corrupt stream.  It takes priority over `result` for exactly
    // that reason.
    match budget.exceeded.get() {
        Some(CapExceeded::Stream) => Err(ArtifactError::ArchiveStreamTooLarge { cap }),
        Some(CapExceeded::Metadata) => Err(ArtifactError::ArchiveMetadataTooLarge {
            limit: metadata_cap,
        }),
        None => result,
    }
}

fn extract_tar_entries<R: Read>(
    tar: &mut tar::Archive<R>,
    archive: &Path,
    dest: &Path,
    limits: ExtractLimits,
    options: SafeExtractOptions<'_>,
    budget: &StreamBudget,
) -> Result<(), ArtifactError> {
    let entries = tar.entries().map_err(|source| ArtifactError::Extract {
        path: archive.to_path_buf(),
        source,
    })?;

    let mut total_bytes: u64 = 0;
    let mut entry_count: usize = 0;
    let mut saw_prefix = false;
    let mut tree = TargetTree::default();

    for entry_result in entries {
        entry_count += 1;
        if entry_count > limits.max_entries {
            return Err(ArtifactError::ArchiveTooManyEntries {
                limit: limits.max_entries,
            });
        }

        let mut entry = entry_result.map_err(|source| ArtifactError::Extract {
            path: archive.to_path_buf(),
            source,
        })?;
        let entry_kind = entry.header().entry_type();
        // Skip GNU/PAX metadata records (long-path markers, extended
        // headers, global PAX state) *before* path validation: the
        // standard tar reader has already consumed their payload to
        // populate the next real entry's header, and their literal
        // path is a synthetic marker like `././@LongLink` that fails
        // the prefix check even though no file is being extracted.
        // Real archives produced by GNU `tar` routinely include
        // these records, so deferring this skip to `write_entry`
        // would let `MissingStripPrefix` reject otherwise-valid
        // tarballs.
        if matches!(
            entry_kind,
            tar::EntryType::GNULongName
                | tar::EntryType::GNULongLink
                | tar::EntryType::XHeader
                | tar::EntryType::XGlobalHeader
        ) {
            continue;
        }
        // Cap the raw wire path before anything copies it: the
        // decorated path an entry reports can come from a GNU
        // long-name record, and a hostile one is only bounded by the
        // metadata budget.  Nothing over the cap is ever turned into
        // a `PathBuf` or an error string.
        let raw_path = entry.path_bytes();
        if raw_path.len() > limits.max_path_bytes {
            return Err(ArtifactError::ArchiveEntryPathTooLong {
                path: truncate_for_display(&raw_path),
                limit: limits.max_path_bytes,
            });
        }
        drop(raw_path);
        let entry_path: PathBuf = entry
            .path()
            .map_err(|source| ArtifactError::Extract {
                path: archive.to_path_buf(),
                source,
            })?
            .into_owned();
        let display = entry_path.to_string_lossy().into_owned();

        let Some(target) = resolve_safe_target(
            &entry_path,
            dest,
            limits.max_path_bytes,
            options,
            &mut saw_prefix,
        )?
        else {
            continue;
        };

        // A skipped symlink materializes nothing; every other
        // special type still fails inside `write_entry`.  It claims
        // no target either, so it is exempt from the tree checks.
        if entry_kind == tar::EntryType::Symlink && options.skip_symlinks {
            continue;
        }
        tree.claim(
            &target,
            dest,
            entry_kind == tar::EntryType::Directory,
            &display,
        )?;
        write_entry(
            &mut entry,
            entry_kind,
            &target,
            &display,
            limits,
            &mut total_bytes,
            budget,
        )?;
    }
    if let Some(prefix) = options.strip_prefix
        && !saw_prefix
    {
        return Err(ArtifactError::MissingStripPrefix {
            strip_prefix: prefix.to_owned(),
        });
    }
    Ok(())
}

/// Render at most the first 64 bytes of a raw entry path for a
/// diagnostic, so an error message never copies a hostile path whose
/// length is bounded only by the metadata budget.
fn truncate_for_display(raw: &[u8]) -> String {
    const MAX_DISPLAY_BYTES: usize = 64;
    if raw.len() <= MAX_DISPLAY_BYTES {
        return String::from_utf8_lossy(raw).into_owned();
    }
    let head = String::from_utf8_lossy(&raw[..MAX_DISPLAY_BYTES]);
    format!("{head}...")
}

/// The destinations claimed so far, used to reject two archive
/// shapes that have no consistent extraction and that `cabin
/// package` never produces:
///
/// - a regular file written twice ("last wins" confusion), and
/// - a regular file used as another entry's parent directory (`foo`
///   plus `foo/bar`, in either order).
///
/// Both would otherwise surface as a bare filesystem error, or - for
/// the duplicate - as a silent overwrite.  `dirs` holds implied
/// parent directories as well as explicit directory entries, since
/// the extractor creates them with `create_dir_all`.  Directory
/// entries themselves may repeat: `create_dir_all` is idempotent and
/// some archivers list a directory more than once.
#[derive(Debug, Default)]
struct TargetTree {
    files: HashSet<PathBuf>,
    dirs: HashSet<PathBuf>,
}

impl TargetTree {
    fn claim(
        &mut self,
        target: &Path,
        dest: &Path,
        is_dir: bool,
        display: &str,
    ) -> Result<(), ArtifactError> {
        let conflict = |at: &Path| ArtifactError::ConflictingArchiveEntry {
            path: display.to_owned(),
            conflict: at
                .strip_prefix(dest)
                .unwrap_or(at)
                .to_string_lossy()
                .into_owned(),
        };
        // Every ancestor below `dest` becomes a directory; a regular
        // file already claiming one of them cannot also be a parent.
        for parent in target.ancestors().skip(1) {
            if parent == dest || !parent.starts_with(dest) {
                break;
            }
            if self.files.contains(parent) {
                return Err(conflict(parent));
            }
            self.dirs.insert(parent.to_path_buf());
        }
        if is_dir {
            if self.files.contains(target) {
                return Err(conflict(target));
            }
            self.dirs.insert(target.to_path_buf());
            return Ok(());
        }
        if self.dirs.contains(target) {
            return Err(conflict(target));
        }
        if !self.files.insert(target.to_path_buf()) {
            return Err(ArtifactError::DuplicateArchiveEntry(display.to_owned()));
        }
        Ok(())
    }
}

/// Reserved DOS device names.  Windows resolves any path component
/// whose stem equals one of these (case-insensitively, with or
/// without an extension) to a character device rather than a file
/// under the target, so an entry named `NUL` writes to the null
/// device instead of materializing a regular file.  The `COM`/`LPT`
/// entries also reserve their Unicode superscript-digit forms
/// (`COM¹`, `LPT²`, …), handled separately in the predicate.
const DOS_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Superscript digit stems Windows also reserves for `COM`/`LPT`.
const SUPERSCRIPT_DEVICE_STEMS: &[&str] = &[
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];

/// Returns true when every component of `path` names the same file on
/// a Windows filesystem as it does lexically here.
///
/// Rejects the shapes that pass [`is_safe_relative_path`] (they stay
/// inside the target) yet alias to a different Win32 destination or a
/// device: a component containing `:` (an NTFS alternate-data-stream
/// or drive separator) or `\` (a Windows path separator), a component
/// with a leading or trailing space or a trailing `.` (Win32 strips
/// all three, so `a`, `a.`, `a `, and ` a` collide), and the reserved
/// DOS device names.
fn is_portable_relative_path(path: &Path) -> bool {
    path.components().all(|component| match component {
        Component::Normal(name) => {
            let Some(name) = name.to_str() else {
                // A non-UTF-8 component is not something `cabin
                // package` emits; refuse it rather than reason about
                // its Win32 aliasing.
                return false;
            };
            if name.contains([':', '\\']) {
                return false;
            }
            if name.ends_with('.') || name.ends_with(' ') || name.starts_with(' ') {
                return false;
            }
            // Reserved names match on the stem before the first dot.
            let stem = name.split('.').next().unwrap_or(name);
            !DOS_DEVICE_NAMES
                .iter()
                .chain(SUPERSCRIPT_DEVICE_STEMS)
                .any(|reserved| stem.eq_ignore_ascii_case(reserved))
        }
        // `CurDir` is harmless; every other component kind was
        // already refused by `is_safe_relative_path`.
        _ => true,
    })
}

/// Apply path-safety + optional `strip_prefix` to `entry_path`
/// and return the absolute target under `dest`.
///
/// Returns `Ok(None)` when the entry was the prefix directory
/// itself (nothing to extract).  Returns
/// [`ArtifactError::MissingStripPrefix`] when an entry's first
/// component does not match the declared prefix; this surfaces
/// the actionable diagnostic the user can fix by correcting
/// `port.toml`.
fn resolve_safe_target(
    entry_path: &Path,
    dest: &Path,
    max_path_bytes: usize,
    options: SafeExtractOptions<'_>,
    saw_prefix: &mut bool,
) -> Result<Option<PathBuf>, ArtifactError> {
    let display = || entry_path.to_string_lossy().into_owned();

    // Cap the raw (pre-strip) path length.  Bounds what one entry
    // can make the extractor allocate and, transitively, how deep a
    // nested tree can get; checked before any other validation so
    // an over-long hostile path is refused without being processed.
    if entry_path.as_os_str().len() > max_path_bytes {
        return Err(ArtifactError::ArchiveEntryPathTooLong {
            path: truncate_for_display(display().as_bytes()),
            limit: max_path_bytes,
        });
    }

    // First pass: the raw entry path must be a safe relative
    // path even before stripping.  Catches `../escape` and
    // absolute paths in the literal entry header.
    if !is_safe_relative_path(entry_path) {
        return Err(ArtifactError::UnsafeArchiveEntry(display()));
    }
    // Reject shapes that stay lexically inside the target but that a
    // Windows filesystem would alias to a *different* destination
    // (so two archive entries could collide) or route to a device
    // instead of a file.  Enforced on every platform so behavior is
    // deterministic and a Linux-built archive that would misbehave on
    // Windows is refused everywhere, mirroring the hosted verifier's
    // cross-platform `\` rejection.  `cabin package` never emits any
    // of these from a real C/C++ source tree.
    if !is_portable_relative_path(entry_path) {
        return Err(ArtifactError::UnsafeArchiveEntry(display()));
    }

    let stripped: PathBuf = match options.strip_prefix {
        None => entry_path.to_path_buf(),
        Some(prefix) => {
            let mut components = entry_path.components();
            // Skip leading `./` segments.  GNU tar (and several
            // common archiving tools) emit `./<prefix>/...`
            // entries; treating them as missing the prefix would
            // reject otherwise-valid tarballs.
            let mut first = components.next();
            while matches!(first, Some(Component::CurDir)) {
                first = components.next();
            }
            match first {
                // Bare `./` (or any pure `./././…` chain): this is
                // a harmless root marker `tar` emits for archives
                // built from `.`.  Skip the entry rather than
                // failing the whole extraction - the prefix gets
                // observed on subsequent real entries.
                None => return Ok(None),
                Some(Component::Normal(name)) if name == std::ffi::OsStr::new(prefix) => {
                    *saw_prefix = true;
                    components.as_path().to_path_buf()
                }
                _ => {
                    return Err(ArtifactError::MissingStripPrefix {
                        strip_prefix: prefix.to_owned(),
                    });
                }
            }
        }
    };

    if stripped.as_os_str().is_empty() {
        return Ok(None);
    }

    // Re-validate after stripping in case the post-strip path
    // picked up unsafe components.
    if !is_safe_relative_path(&stripped) {
        return Err(ArtifactError::UnsafeArchiveEntry(display()));
    }
    let target = dest.join(&stripped);
    if !target.starts_with(dest) {
        return Err(ArtifactError::UnsafeArchiveEntry(display()));
    }
    Ok(Some(target))
}

/// Write one tar entry to `target`.  Enforces the byte caps and
/// removes any partial file when a cap is exceeded.
fn write_entry<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    entry_kind: tar::EntryType,
    target: &Path,
    display: &str,
    limits: ExtractLimits,
    total_bytes: &mut u64,
    budget: &StreamBudget,
) -> Result<(), ArtifactError> {
    match entry_kind {
        tar::EntryType::Directory => {
            fs::create_dir_all(target).map_err(|source| ArtifactError::Io {
                path: target.to_path_buf(),
                source,
            })?;
            Ok(())
        }
        tar::EntryType::Regular => {
            // `Entry::size` is the *effective* size: it honors a PAX
            // `size` record, which `header().size()` does not.  Using
            // the raw header field here would reject every legitimate
            // PAX-decorated entry as truncated.
            let expected = entry.size();
            // Only the file's own bytes stream straight to disk;
            // everything else the tar reader pulls is metadata it
            // buffers in memory, and is charged accordingly.
            let written = budget.in_body(|| {
                write_file_capped(
                    entry,
                    target,
                    display,
                    limits.max_entry_bytes,
                    limits.max_total_bytes,
                    total_bytes,
                )
            })?;
            // A header whose `size` overstates the bytes the archive
            // actually holds truncates the file silently: tar hands
            // back a short read at the end of the stream.  Refuse the
            // archive instead of materializing a partial source file.
            if written != expected {
                let _ = fs::remove_file(target);
                return Err(ArtifactError::ArchiveEntryTruncated {
                    path: display.to_owned(),
                    expected,
                    actual: written,
                });
            }
            Ok(())
        }
        // Tar metadata entries carry side-band data the
        // standard tar reader already consumes (long paths, PAX
        // extended headers, global PAX state) - the subsequent
        // real file entry exposes the resolved path via its own
        // header, so skipping these is correct.  Real source
        // archives, including ones produced by `git archive` and
        // GNU `tar` for foundation-port releases, routinely
        // include such records.
        tar::EntryType::GNULongName
        | tar::EntryType::GNULongLink
        | tar::EntryType::XHeader
        | tar::EntryType::XGlobalHeader => Ok(()),
        // Reject every other entry type by design (symlinks,
        // hard links, char/block devices, fifos, sparse, etc.).
        // Cabin source archives only need regular files and
        // directories.
        _ => Err(ArtifactError::UnsupportedArchiveEntry(display.to_owned())),
    }
}

/// Write one regular-file archive entry's bytes to `target`,
/// enforcing the per-entry and aggregate decompression caps against
/// the actual decompressed byte count, and returning that count.
/// Shared by the tar and zip extractors; removes any partial file
/// when a cap is exceeded or the copy fails.
fn write_file_capped<R: Read>(
    reader: &mut R,
    target: &Path,
    display: &str,
    max_entry_bytes: u64,
    max_total_bytes: u64,
    total_bytes: &mut u64,
) -> Result<u64, ArtifactError> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|source| ArtifactError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut out = File::create(target).map_err(|source| ArtifactError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    // Cap the read at one byte over the per-entry
    // limit so a successful copy of exactly the limit
    // is distinguishable from an overflow.
    let mut limited = reader.take(max_entry_bytes + 1);
    let written = match io::copy(&mut limited, &mut out) {
        Ok(written) => written,
        // A mid-copy failure (truncated stream, stream cap crossed,
        // disk error) must not leave a half-written file behind.
        Err(source) => {
            drop(out);
            let _ = fs::remove_file(target);
            return Err(ArtifactError::Io {
                path: target.to_path_buf(),
                source,
            });
        }
    };
    if written > max_entry_bytes {
        drop(out);
        let _ = fs::remove_file(target);
        return Err(ArtifactError::ArchiveEntryTooLarge {
            path: display.to_owned(),
            limit: max_entry_bytes,
        });
    }
    *total_bytes = total_bytes.saturating_add(written);
    if *total_bytes > max_total_bytes {
        drop(out);
        let _ = fs::remove_file(target);
        return Err(ArtifactError::ArchiveTooLarge {
            limit: max_total_bytes,
        });
    }
    Ok(written)
}

/// Validate that an extracted source tree at `source_dir` matches the
/// resolved package's `name` and `version`.
pub(crate) fn validate_extracted(
    source_dir: &Path,
    name: &PackageName,
    version: &semver::Version,
) -> Result<(), ArtifactError> {
    let manifest_path = source_dir.join("cabin.toml");
    if !manifest_path.is_file() {
        return Err(ArtifactError::MissingArchiveManifest {
            name: name.as_str().to_owned(),
            version: version.to_string(),
        });
    }
    let parsed = cabin_manifest::load_manifest(&manifest_path).map_err(|source| {
        ArtifactError::Manifest {
            path: manifest_path.clone(),
            source: Box::new(source),
        }
    })?;
    let package = parsed
        .package
        .ok_or_else(|| ArtifactError::MissingArchiveManifest {
            name: name.as_str().to_owned(),
            version: version.to_string(),
        })?;
    if package.name != *name || package.version != *version {
        return Err(ArtifactError::ManifestMismatch {
            name: name.as_str().to_owned(),
            version: version.to_string(),
            actual_name: package.name.as_str().to_owned(),
            actual_version: package.version.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use predicates::prelude::*;
    use std::io::Write;

    fn pkg(name: &str) -> PackageName {
        PackageName::new(name).unwrap()
    }

    /// Production-shaped limits with the byte / count caps shrunk so
    /// tests trip them cheaply.  The path-length and stream-ratio
    /// caps keep their production values; tests that exercise those
    /// override the fields explicitly.
    fn small_limits(
        max_entry_bytes: u64,
        max_total_bytes: u64,
        max_entries: usize,
    ) -> ExtractLimits {
        ExtractLimits {
            max_entry_bytes,
            max_total_bytes,
            max_entries,
            ..ExtractLimits::default()
        }
    }

    fn ver(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    /// Build a `.tar.gz` containing the given `(path, body)` file entries.
    fn make_archive(archive_path: &Path, entries: &[(&str, &str)]) {
        if let Some(parent) = archive_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let f = File::create(archive_path).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
        let mut builder = tar::Builder::new(enc);
        for (rel_path, body) in entries {
            let bytes = body.as_bytes();
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            builder
                .append_data(&mut header, rel_path, &mut std::io::Cursor::new(bytes))
                .unwrap();
        }
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap().flush().unwrap();
    }

    /// Build a `.tar.gz` whose first entry has its `name` field written
    /// directly.  This bypasses `Header::set_path`'s validation, which
    /// would reject `..` and absolute paths.
    fn make_archive_with_raw_name(
        archive_path: &Path,
        raw_name: &str,
        entry_type: tar::EntryType,
        link_name: Option<&str>,
        body: &[u8],
    ) {
        if let Some(parent) = archive_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let f = File::create(archive_path).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
        let mut builder = tar::Builder::new(enc);

        let mut header = tar::Header::new_old();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(entry_type);
        if let Some(target) = link_name {
            // `set_link_name` validates and rejects `..` / absolutes,
            // so write the bytes directly into the OldHeader's
            // `linkname` field.
            let bytes = target.as_bytes();
            let old = header.as_old_mut();
            for b in &mut old.linkname[..] {
                *b = 0;
            }
            let n = bytes.len().min(old.linkname.len());
            old.linkname[..n].copy_from_slice(&bytes[..n]);
        }
        {
            // Same trick for the entry name.
            let bytes = raw_name.as_bytes();
            let old = header.as_old_mut();
            for b in &mut old.name[..] {
                *b = 0;
            }
            let n = bytes.len().min(old.name.len());
            old.name[..n].copy_from_slice(&bytes[..n]);
        }
        header.set_cksum();
        builder.append(&header, body).unwrap();
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap().flush().unwrap();
    }

    #[test]
    fn extracts_simple_archive() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("ok.tar.gz");
        make_archive(
            archive.path(),
            &[
                (
                    "cabin.toml",
                    "[package]\nname = \"fmt\"\nversion = \"10.2.1\"\n",
                ),
                ("src/main.cc", "int main() { return 0; }\n"),
            ],
        );

        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        extract_tar_gz(archive.path(), dest.path()).unwrap();
        dest.child("cabin.toml").assert(predicate::path::is_file());
        dest.child("src/main.cc").assert(predicate::path::is_file());
    }

    #[test]
    fn rejects_parent_dir_entry() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bad.tar.gz");
        make_archive_with_raw_name(
            archive.path(),
            "../escape.txt",
            tar::EntryType::Regular,
            None,
            b"evil",
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = extract_tar_gz(archive.path(), dest.path()).unwrap_err();
        match err {
            ArtifactError::UnsafeArchiveEntry(p) => assert!(p.contains("..")),
            other => panic!("expected UnsafeArchiveEntry, got {other:?}"),
        }
        // Nothing escaped.
        dir.child("escape.txt").assert(predicate::path::missing());
    }

    #[test]
    fn rejects_absolute_path_entry() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bad.tar.gz");
        make_archive_with_raw_name(
            archive.path(),
            "/etc/passwd",
            tar::EntryType::Regular,
            None,
            b"evil",
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = extract_tar_gz(archive.path(), dest.path()).unwrap_err();
        assert!(matches!(err, ArtifactError::UnsafeArchiveEntry(_)));
    }

    #[test]
    fn rejects_symlink_entry() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bad.tar.gz");
        make_archive_with_raw_name(
            archive.path(),
            "evil",
            tar::EntryType::Symlink,
            Some("/etc/passwd"),
            b"",
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = extract_tar_gz(archive.path(), dest.path()).unwrap_err();
        assert!(matches!(err, ArtifactError::UnsupportedArchiveEntry(_)));
    }

    #[test]
    fn rejects_hard_link_entry() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bad.tar.gz");
        make_archive_with_raw_name(
            archive.path(),
            "alias",
            tar::EntryType::Link,
            Some("cabin.toml"),
            b"",
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = extract_tar_gz(archive.path(), dest.path()).unwrap_err();
        assert!(matches!(err, ArtifactError::UnsupportedArchiveEntry(_)));
    }

    #[test]
    fn skip_symlinks_extracts_rest_of_tarball() {
        // An upstream tarball with a convenience symlink (uthash's
        // `include -> src` shape): with `skip_symlinks` the symlink
        // entry is skipped without materializing anything and every
        // regular entry still extracts.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("upstream.tar.gz");
        let f = File::create(archive.path()).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
        let mut builder = tar::Builder::new(enc);
        {
            let body = b"#define UT_OK 1\n";
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    "uthash-2.4.0/src/uthash.h",
                    &mut std::io::Cursor::new(&body[..]),
                )
                .unwrap();
        }
        {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o777);
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_link_name("src").unwrap();
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    "uthash-2.4.0/include",
                    &mut std::io::Cursor::new(b""),
                )
                .unwrap();
        }
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap().flush().unwrap();

        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        // Default policy still refuses the archive outright.
        let err = safe_extract_tar_gz(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("uthash-2.4.0"),
                ..SafeExtractOptions::default()
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, ArtifactError::UnsupportedArchiveEntry(_)),
            "{err:?}"
        );
        // Opting in skips the symlink and extracts the rest.
        safe_extract_tar_gz(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("uthash-2.4.0"),
                skip_symlinks: true,
            },
        )
        .unwrap();
        dest.child("src/uthash.h")
            .assert(predicate::path::is_file());
        dest.child("include").assert(predicate::path::missing());
    }

    #[test]
    fn skip_symlinks_still_rejects_hard_links() {
        // The opt-in is symlink-specific: every other special entry
        // type keeps failing the extraction.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bad.tar.gz");
        make_archive_with_raw_name(
            archive.path(),
            "alias",
            tar::EntryType::Link,
            Some("cabin.toml"),
            b"",
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_tar_gz(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                skip_symlinks: true,
                ..SafeExtractOptions::default()
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, ArtifactError::UnsupportedArchiveEntry(_)),
            "{err:?}"
        );
    }

    #[test]
    fn zip_skip_symlinks_extracts_rest_of_archive() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("upstream.zip");
        let f = File::create(archive.path()).unwrap();
        let mut writer = zip::ZipWriter::new(f);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("src/ok.h", options).unwrap();
        writer.write_all(b"#define OK 1\n").unwrap();
        writer.add_symlink("include", "src", options).unwrap();
        writer.finish().unwrap().flush().unwrap();

        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        safe_extract_zip(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                skip_symlinks: true,
                ..SafeExtractOptions::default()
            },
        )
        .unwrap();
        dest.child("src/ok.h").assert(predicate::path::is_file());
        dest.child("include").assert(predicate::path::missing());
    }

    #[test]
    fn zip_skip_symlinks_still_validates_symlink_paths() {
        // Skipping is not a validation bypass: a symlink entry whose
        // *name* is unsafe fails the whole extraction exactly like
        // the tar path, even though nothing would be materialized.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bad.zip");
        let f = File::create(archive.path()).unwrap();
        let mut writer = zip::ZipWriter::new(f);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("pkg-1.0/ok.h", options).unwrap();
        writer.write_all(b"#define OK 1\n").unwrap();
        writer
            .add_symlink("../escape", "/etc/passwd", options)
            .unwrap();
        writer.finish().unwrap().flush().unwrap();

        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_zip(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("pkg-1.0"),
                skip_symlinks: true,
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, ArtifactError::UnsafeArchiveEntry(_)),
            "{err:?}"
        );
        dir.child("escape").assert(predicate::path::missing());
    }

    #[test]
    fn validate_extracted_accepts_matching_manifest() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str("[package]\nname = \"fmt\"\nversion = \"10.2.1\"\n")
            .unwrap();
        validate_extracted(dir.path(), &pkg("fmt"), &ver("10.2.1")).unwrap();
    }

    #[test]
    fn validate_extracted_rejects_missing_manifest() {
        let dir = TempDir::new().unwrap();
        let err = validate_extracted(dir.path(), &pkg("fmt"), &ver("10.2.1")).unwrap_err();
        assert!(matches!(err, ArtifactError::MissingArchiveManifest { .. }));
    }

    #[test]
    fn validate_extracted_rejects_name_mismatch() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str("[package]\nname = \"other\"\nversion = \"10.2.1\"\n")
            .unwrap();
        let err = validate_extracted(dir.path(), &pkg("fmt"), &ver("10.2.1")).unwrap_err();
        match err {
            ArtifactError::ManifestMismatch {
                actual_name,
                actual_version,
                ..
            } => {
                assert_eq!(actual_name, "other");
                assert_eq!(actual_version, "10.2.1");
            }
            other => panic!("expected ManifestMismatch, got {other:?}"),
        }
    }

    #[test]
    fn rejects_archive_entry_exceeding_per_entry_limit() {
        // A single entry whose decompressed body would exceed the
        // per-entry cap is refused before the bomb is written to
        // disk.  The half-written file is removed so a bomb does
        // not leave a max-size carcass behind.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bomb.tar.gz");
        let body = "x".repeat(2048);
        make_archive(archive.path(), &[("cabin.toml", body.as_str())]);
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_tar_gz_with_limits(
            archive.path(),
            dest.path(),
            small_limits(1024, 1_000_000, 1000),
            SafeExtractOptions::default(),
        )
        .unwrap_err();
        match err {
            ArtifactError::ArchiveEntryTooLarge { path, limit } => {
                assert_eq!(path, "cabin.toml");
                assert_eq!(limit, 1024);
            }
            other => panic!("expected ArchiveEntryTooLarge, got {other:?}"),
        }
        dest.child("cabin.toml").assert(predicate::path::missing());
    }

    #[test]
    fn rejects_archive_exceeding_aggregate_size_limit() {
        // Each entry fits under the per-entry cap, but the sum
        // exceeds the aggregate cap.  Refused on the entry whose
        // write pushes the running total over.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("aggregate-bomb.tar.gz");
        let body = "x".repeat(700);
        make_archive(
            archive.path(),
            &[("a.txt", body.as_str()), ("b.txt", body.as_str())],
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_tar_gz_with_limits(
            archive.path(),
            dest.path(),
            small_limits(1024, 1000, 1000),
            SafeExtractOptions::default(),
        )
        .unwrap_err();
        match err {
            ArtifactError::ArchiveTooLarge { limit } => assert_eq!(limit, 1000),
            other => panic!("expected ArchiveTooLarge, got {other:?}"),
        }
        dest.child("b.txt").assert(predicate::path::missing());
    }

    #[test]
    fn rejects_archive_with_too_many_entries() {
        // Headers can be cheap to ship and expensive to
        // materialize as inodes; the entry-count cap fires
        // independently of byte caps.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("many.tar.gz");
        make_archive(
            archive.path(),
            &[
                ("a.txt", "x"),
                ("b.txt", "x"),
                ("c.txt", "x"),
                ("d.txt", "x"),
            ],
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_tar_gz_with_limits(
            archive.path(),
            dest.path(),
            small_limits(1024, 1_000_000, 3),
            SafeExtractOptions::default(),
        )
        .unwrap_err();
        match err {
            ArtifactError::ArchiveTooManyEntries { limit } => assert_eq!(limit, 3),
            other => panic!("expected ArchiveTooManyEntries, got {other:?}"),
        }
    }

    #[test]
    fn accepts_archive_just_under_limits() {
        // Positive control: the bomb caps must not regress the
        // happy path for archives that sit under every limit.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("ok.tar.gz");
        make_archive(
            archive.path(),
            &[
                (
                    "cabin.toml",
                    "[package]\nname = \"fmt\"\nversion = \"10.2.1\"\n",
                ),
                ("src/main.cc", "int main() { return 0; }\n"),
            ],
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        safe_extract_tar_gz_with_limits(
            archive.path(),
            dest.path(),
            small_limits(4096, 1_000_000, 1000),
            SafeExtractOptions::default(),
        )
        .unwrap();
        dest.child("cabin.toml").assert(predicate::path::is_file());
        dest.child("src/main.cc").assert(predicate::path::is_file());
    }

    #[test]
    fn strip_prefix_removes_leading_dir() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("zlib.tar.gz");
        make_archive(
            archive.path(),
            &[
                ("zlib-1.3.1/zlib.h", "#define ZLIB_VERSION \"1.3.1\"\n"),
                (
                    "zlib-1.3.1/src/adler32.c",
                    "int adler32(void) { return 0; }\n",
                ),
            ],
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        safe_extract_tar_gz(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("zlib-1.3.1"),
                ..SafeExtractOptions::default()
            },
        )
        .unwrap();
        dest.child("zlib.h").assert(predicate::path::is_file());
        dest.child("src/adler32.c")
            .assert(predicate::path::is_file());
        // The prefix directory must not have been re-created
        // inside the destination.
        dest.child("zlib-1.3.1").assert(predicate::path::missing());
    }

    /// GNU tar and `git archive --format=tar` commonly emit
    /// entries with a leading `./` segment.  The strip-prefix
    /// matcher must skip those before comparing to the declared
    /// prefix; otherwise a perfectly valid tarball is rejected.
    #[test]
    fn strip_prefix_accepts_leading_dot_slash_segments() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("zlib.tar.gz");
        make_archive(
            archive.path(),
            &[
                ("./zlib-1.3.1/zlib.h", "#define ZLIB_VERSION \"1.3.1\"\n"),
                (
                    "./zlib-1.3.1/src/adler32.c",
                    "int adler32(void) { return 0; }\n",
                ),
            ],
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        safe_extract_tar_gz(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("zlib-1.3.1"),
                ..SafeExtractOptions::default()
            },
        )
        .unwrap();
        dest.child("zlib.h").assert(predicate::path::is_file());
        dest.child("src/adler32.c")
            .assert(predicate::path::is_file());
        dest.child("zlib-1.3.1").assert(predicate::path::missing());
    }

    #[test]
    fn strip_prefix_skips_the_prefix_directory_entry_itself() {
        // Archives commonly include a directory entry for the
        // prefix dir; stripping that entry must not produce an
        // empty target path or escape the destination.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("zlib.tar.gz");
        let f = File::create(archive.path()).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
        let mut builder = tar::Builder::new(enc);
        {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o755);
            header.set_entry_type(tar::EntryType::Directory);
            header.set_cksum();
            builder
                .append_data(&mut header, "zlib-1.3.1/", &mut std::io::Cursor::new(b""))
                .unwrap();
        }
        let body = b"ok\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                "zlib-1.3.1/zlib.h",
                &mut std::io::Cursor::new(&body[..]),
            )
            .unwrap();
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap().flush().unwrap();

        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        safe_extract_tar_gz(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("zlib-1.3.1"),
                ..SafeExtractOptions::default()
            },
        )
        .unwrap();
        dest.child("zlib.h").assert(predicate::path::is_file());
    }

    #[test]
    fn strip_prefix_skips_bare_curdir_root_marker() {
        // GNU `tar` archives built from `.` typically begin with a
        // bare `./` directory entry.  The strip-prefix matcher must
        // treat that entry as a harmless root marker and skip it
        // before processing the real entries under the declared
        // prefix.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("zlib.tar.gz");
        let f = File::create(archive.path()).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
        let mut builder = tar::Builder::new(enc);
        {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o755);
            header.set_entry_type(tar::EntryType::Directory);
            header.set_cksum();
            builder
                .append_data(&mut header, "./", &mut std::io::Cursor::new(b""))
                .unwrap();
        }
        let body = b"ok\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                "zlib-1.3.1/zlib.h",
                &mut std::io::Cursor::new(&body[..]),
            )
            .unwrap();
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap().flush().unwrap();

        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        safe_extract_tar_gz(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("zlib-1.3.1"),
                ..SafeExtractOptions::default()
            },
        )
        .unwrap();
        dest.child("zlib.h").assert(predicate::path::is_file());
    }

    #[test]
    fn strip_prefix_rejects_archive_without_matching_root() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("other.tar.gz");
        make_archive(archive.path(), &[("not-zlib/zlib.h", "// nope\n")]);
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_tar_gz(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("zlib-1.3.1"),
                ..SafeExtractOptions::default()
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, ArtifactError::MissingStripPrefix { ref strip_prefix } if strip_prefix == "zlib-1.3.1"),
            "{err:?}"
        );
    }

    #[test]
    fn strip_prefix_reports_missing_prefix_on_empty_archive() {
        // An empty archive (or one whose entries never start
        // with the declared prefix) surfaces a dedicated
        // MissingStripPrefix error.  Build a minimal archive
        // containing only the gzip footer.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("empty.tar.gz");
        let f = File::create(archive.path()).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
        let builder = tar::Builder::new(enc);
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap().flush().unwrap();

        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_tar_gz(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("zlib-1.3.1"),
                ..SafeExtractOptions::default()
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, ArtifactError::MissingStripPrefix { ref strip_prefix } if strip_prefix == "zlib-1.3.1"),
            "{err:?}"
        );
    }

    #[test]
    fn strip_prefix_keeps_path_safety_after_strip() {
        // Even if the archive's root dir is stripped, the
        // post-strip path must still pass `is_safe_relative_path`.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bad.tar.gz");
        make_archive_with_raw_name(
            archive.path(),
            "zlib-1.3.1/../escape.txt",
            tar::EntryType::Regular,
            None,
            b"evil",
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_tar_gz(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("zlib-1.3.1"),
                ..SafeExtractOptions::default()
            },
        )
        .unwrap_err();
        // The pre-strip path-safety check fires first because
        // the literal entry contains `..`.
        assert!(
            matches!(err, ArtifactError::UnsafeArchiveEntry(_)),
            "{err:?}"
        );
        dir.child("escape.txt").assert(predicate::path::missing());
    }

    /// Build a `.zip` containing the given `(path, body)` file
    /// entries.  Entries ending in `/` become directory records.
    fn make_zip(archive_path: &Path, entries: &[(&str, &str)]) {
        if let Some(parent) = archive_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let f = File::create(archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(f);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (rel_path, body) in entries {
            if rel_path.ends_with('/') {
                writer.add_directory(*rel_path, options).unwrap();
            } else {
                writer.start_file(*rel_path, options).unwrap();
                writer.write_all(body.as_bytes()).unwrap();
            }
        }
        writer.finish().unwrap().flush().unwrap();
    }

    #[test]
    fn zip_extracts_simple_archive() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("ok.zip");
        make_zip(
            archive.path(),
            &[
                ("miniz.h", "#define MZ_VERSION \"11.3.2\"\n"),
                ("examples/", ""),
                ("examples/example1.c", "int main(void) { return 0; }\n"),
            ],
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        safe_extract_zip(archive.path(), dest.path(), SafeExtractOptions::default()).unwrap();
        dest.child("miniz.h").assert(predicate::path::is_file());
        dest.child("examples/example1.c")
            .assert(predicate::path::is_file());
    }

    #[test]
    fn zip_rejects_parent_dir_entry() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bad.zip");
        make_zip(archive.path(), &[("../escape.txt", "evil")]);
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_zip(archive.path(), dest.path(), SafeExtractOptions::default())
            .unwrap_err();
        assert!(
            matches!(err, ArtifactError::UnsafeArchiveEntry(_)),
            "{err:?}"
        );
        dir.child("escape.txt").assert(predicate::path::missing());
    }

    #[test]
    fn zip_rejects_absolute_path_entry() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bad.zip");
        make_zip(archive.path(), &[("/etc/passwd", "evil")]);
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_zip(archive.path(), dest.path(), SafeExtractOptions::default())
            .unwrap_err();
        assert!(
            matches!(err, ArtifactError::UnsafeArchiveEntry(_)),
            "{err:?}"
        );
    }

    #[test]
    fn zip_rejects_symlink_entry() {
        // A zip entry whose recorded Unix mode says "symlink" must be
        // refused like the tar equivalent, not written as a file.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bad.zip");
        let f = File::create(archive.path()).unwrap();
        let mut writer = zip::ZipWriter::new(f);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.add_symlink("evil", "/etc/passwd", options).unwrap();
        writer.finish().unwrap().flush().unwrap();

        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_zip(archive.path(), dest.path(), SafeExtractOptions::default())
            .unwrap_err();
        assert!(
            matches!(err, ArtifactError::UnsupportedArchiveEntry(_)),
            "{err:?}"
        );
        dest.child("evil").assert(predicate::path::missing());
    }

    #[test]
    fn zip_rejects_entry_exceeding_per_entry_limit() {
        // The cap must bind on the actual decompressed bytes, not
        // the size the zip headers claim.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bomb.zip");
        let body = "x".repeat(2048);
        make_zip(archive.path(), &[("cabin.toml", body.as_str())]);
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_zip_with_limits(
            archive.path(),
            dest.path(),
            small_limits(1024, 1_000_000, 1000),
            SafeExtractOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(err, ArtifactError::ArchiveEntryTooLarge { limit: 1024, .. }),
            "{err:?}"
        );
        dest.child("cabin.toml").assert(predicate::path::missing());
    }

    #[test]
    fn zip_rejects_archive_exceeding_aggregate_size_limit() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("aggregate-bomb.zip");
        let body = "x".repeat(700);
        make_zip(
            archive.path(),
            &[("a.txt", body.as_str()), ("b.txt", body.as_str())],
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_zip_with_limits(
            archive.path(),
            dest.path(),
            small_limits(1024, 1000, 1000),
            SafeExtractOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(err, ArtifactError::ArchiveTooLarge { limit: 1000 }),
            "{err:?}"
        );
        dest.child("b.txt").assert(predicate::path::missing());
    }

    #[test]
    fn zip_rejects_archive_with_too_many_entries() {
        // The zip central directory states the count up front, so
        // the cap fires before any entry is materialized.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("many.zip");
        make_zip(
            archive.path(),
            &[
                ("a.txt", "x"),
                ("b.txt", "x"),
                ("c.txt", "x"),
                ("d.txt", "x"),
            ],
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_zip_with_limits(
            archive.path(),
            dest.path(),
            small_limits(1024, 1_000_000, 3),
            SafeExtractOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(err, ArtifactError::ArchiveTooManyEntries { limit: 3 }),
            "{err:?}"
        );
        dest.child("a.txt").assert(predicate::path::missing());
    }

    #[test]
    fn zip_entry_count_cap_fires_before_central_directory_parse() {
        // A file with a valid EOCD record claiming an enormous entry
        // count but a garbage central directory: the pre-parse EOCD
        // scan must reject it as ArchiveTooManyEntries, proving the
        // cap fires before per-entry metadata is materialized (a
        // fall-through to the full parser would surface Extract for
        // the unreadable directory instead).
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bomb.zip");
        let mut bytes = vec![0u8; 64];
        bytes.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]); // EOCD signature
        bytes.extend_from_slice(&[0; 4]); // disk numbers
        bytes.extend_from_slice(&50_000u16.to_le_bytes()); // entries on this disk
        bytes.extend_from_slice(&50_000u16.to_le_bytes()); // total entries
        bytes.extend_from_slice(&[0; 10]); // cd size + cd offset + comment len
        fs::write(archive.path(), &bytes).unwrap();
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_zip(archive.path(), dest.path(), SafeExtractOptions::default())
            .unwrap_err();
        assert!(
            matches!(err, ArtifactError::ArchiveTooManyEntries { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn zip_comment_containing_fake_eocd_does_not_defeat_extraction() {
        // A valid archive whose *comment* embeds an EOCD signature
        // claiming an enormous entry count: the pre-parse scan must
        // reject that candidate (its zero comment-length field does
        // not reach EOF) and find the real record, so the archive
        // extracts normally instead of being falsely refused.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("commented.zip");
        let f = File::create(archive.path()).unwrap();
        let mut writer = zip::ZipWriter::new(f);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("ok.txt", options).unwrap();
        writer.write_all(b"fine\n").unwrap();
        let mut comment = vec![0x50, 0x4b, 0x05, 0x06];
        comment.extend_from_slice(&[0; 4]);
        comment.extend_from_slice(&50_000u16.to_le_bytes());
        comment.extend_from_slice(&50_000u16.to_le_bytes());
        comment.extend_from_slice(&[0; 10]);
        comment.extend_from_slice(b"trailing bytes");
        writer.set_raw_comment(comment.into()).unwrap();
        writer.finish().unwrap().flush().unwrap();

        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        safe_extract_zip(archive.path(), dest.path(), SafeExtractOptions::default()).unwrap();
        dest.child("ok.txt").assert(predicate::path::is_file());
    }

    /// CRC-32 (IEEE) of `data`, so hand-built zip entries pass the
    /// reader's checksum check without pulling in a crc dependency.
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    /// One stored (uncompressed) zip entry, described enough to
    /// hand-build a raw archive: the `uncompressed` field can lie
    /// about `data` to model a truncated entry.
    struct RawEntry<'a> {
        name: &'a str,
        data: &'a [u8],
        uncompressed: u32,
    }

    fn u16le(out: &mut Vec<u8>, v: usize) {
        out.extend_from_slice(&u16::try_from(v).unwrap().to_le_bytes());
    }
    fn u32le(out: &mut Vec<u8>, v: u32) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    fn u32len(out: &mut Vec<u8>, v: usize) {
        u32le(out, u32::try_from(v).unwrap());
    }

    fn push_local_header(out: &mut Vec<u8>, entry: &RawEntry<'_>) -> usize {
        let offset = out.len();
        u32le(out, 0x0403_4b50); // local file header signature
        u16le(out, 20); // version needed
        u16le(out, 0); // flags
        u16le(out, 0); // stored
        u16le(out, 0); // mod time
        u16le(out, 0); // mod date
        u32le(out, crc32(entry.data));
        u32len(out, entry.data.len()); // compressed size
        u32le(out, entry.uncompressed); // uncompressed size (may lie)
        u16le(out, entry.name.len());
        u16le(out, 0); // extra length
        out.extend_from_slice(entry.name.as_bytes());
        out.extend_from_slice(entry.data);
        offset
    }

    fn push_central_header(out: &mut Vec<u8>, entry: &RawEntry<'_>, offset: usize) {
        u32le(out, 0x0201_4b50); // central directory signature
        u16le(out, 20); // version made by
        u16le(out, 20); // version needed
        u16le(out, 0); // flags
        u16le(out, 0); // stored
        u16le(out, 0); // mod time
        u16le(out, 0); // mod date
        u32le(out, crc32(entry.data));
        u32len(out, entry.data.len());
        u32le(out, entry.uncompressed);
        u16le(out, entry.name.len());
        u16le(out, 0); // extra
        u16le(out, 0); // comment
        u16le(out, 0); // disk number start
        u16le(out, 0); // internal attrs
        u32le(out, 0); // external attrs
        u32len(out, offset);
        out.extend_from_slice(entry.name.as_bytes());
    }

    /// Hand-build a raw stored `.zip`.  Lets tests construct archives
    /// the `zip` writer refuses to emit: duplicate names, or an entry
    /// whose header size disagrees with its bytes.
    fn raw_zip(entries: &[RawEntry<'_>]) -> Vec<u8> {
        let mut out = Vec::new();
        let offsets: Vec<usize> = entries
            .iter()
            .map(|entry| push_local_header(&mut out, entry))
            .collect();
        let cd_start = out.len();
        for (entry, offset) in entries.iter().zip(&offsets) {
            push_central_header(&mut out, entry, *offset);
        }
        let cd_size = out.len() - cd_start;
        u32le(&mut out, 0x0605_4b50); // EOCD signature
        u16le(&mut out, 0); // disk number
        u16le(&mut out, 0); // disk with CD
        u16le(&mut out, entries.len()); // entries this disk
        u16le(&mut out, entries.len()); // total entries
        u32len(&mut out, cd_size);
        u32len(&mut out, cd_start);
        u16le(&mut out, 0); // comment length
        out
    }

    #[test]
    fn zip_rejects_duplicate_named_records() {
        // The `zip` writer refuses to emit two records with one name,
        // so hand-build the archive.  The parser deduplicates them
        // into a single `IndexMap` entry, so the guard catches the
        // shortfall against the declared count instead.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("dup.zip");
        let bytes = raw_zip(&[
            RawEntry {
                name: "cabin.toml",
                data: b"honest",
                uncompressed: 6,
            },
            RawEntry {
                name: "cabin.toml",
                data: b"evil!!",
                uncompressed: 6,
            },
        ]);
        fs::write(archive.path(), &bytes).unwrap();
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_zip(archive.path(), dest.path(), SafeExtractOptions::default())
            .unwrap_err();
        assert!(
            matches!(
                err,
                ArtifactError::ArchiveDuplicateNames {
                    declared: 2,
                    distinct: 1
                }
            ),
            "{err:?}"
        );
        dest.child("cabin.toml").assert(predicate::path::missing());
    }

    #[test]
    fn zip_rejects_an_entry_whose_header_size_overstates_its_bytes() {
        // A stored entry whose header claims more bytes than it holds:
        // the tar side refuses this, and so must the zip side, rather
        // than write a silently truncated file.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("short.zip");
        let bytes = raw_zip(&[RawEntry {
            name: "cabin.toml",
            data: b"short",
            uncompressed: 4096,
        }]);
        fs::write(archive.path(), &bytes).unwrap();
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_zip(archive.path(), dest.path(), SafeExtractOptions::default())
            .unwrap_err();
        assert!(
            matches!(
                err,
                ArtifactError::ArchiveEntryTruncated { expected: 4096, .. }
            ),
            "{err:?}"
        );
        dest.child("cabin.toml").assert(predicate::path::missing());
    }

    #[test]
    fn zip_rejects_an_over_large_archive_file() {
        // The archive-file cap bounds the central-directory memory
        // the parser allocates before any per-entry check runs.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("big.zip");
        make_zip(archive.path(), &[("cabin.toml", "x")]);
        let limits = ExtractLimits {
            max_total_bytes: 8,
            ..ExtractLimits::default()
        };
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_zip_with_limits(
            archive.path(),
            dest.path(),
            limits,
            SafeExtractOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(err, ArtifactError::ArchiveFileTooLarge { limit: 8, .. }),
            "{err:?}"
        );
    }

    #[test]
    fn zip_garbage_bytes_surface_extract_error() {
        // No EOCD anywhere: the pre-check finds nothing and the full
        // parser reports the malformed archive.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("garbage.zip");
        fs::write(archive.path(), b"not a zip at all").unwrap();
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_zip(archive.path(), dest.path(), SafeExtractOptions::default())
            .unwrap_err();
        assert!(matches!(err, ArtifactError::Extract { .. }), "{err:?}");
    }

    #[test]
    fn zip_strip_prefix_removes_leading_dir() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("miniz.zip");
        make_zip(
            archive.path(),
            &[
                ("miniz-3.1.2/miniz.h", "#define MZ_VERSION\n"),
                ("miniz-3.1.2/miniz.c", "int mz(void) { return 0; }\n"),
            ],
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        safe_extract_zip(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("miniz-3.1.2"),
                ..SafeExtractOptions::default()
            },
        )
        .unwrap();
        dest.child("miniz.h").assert(predicate::path::is_file());
        dest.child("miniz.c").assert(predicate::path::is_file());
        dest.child("miniz-3.1.2").assert(predicate::path::missing());
    }

    /// The file-vs-parent-directory conflict, exercised through the
    /// same `TargetTree::claim` call the tar path uses.  (Two records
    /// with an *identical* name are caught earlier, by the declared-
    /// count guard - see `zip_rejects_duplicate_named_records`.)
    #[test]
    fn zip_rejects_a_file_used_as_a_parent_directory() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("conflict.zip");
        make_zip(archive.path(), &[("src", "x"), ("src/main.cc", "y")]);
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_zip(archive.path(), dest.path(), SafeExtractOptions::default())
            .unwrap_err();
        assert!(
            matches!(err, ArtifactError::ConflictingArchiveEntry { ref conflict, .. } if conflict == "src"),
            "{err:?}"
        );
    }

    #[test]
    fn zip_rejects_over_long_entry_paths() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("long.zip");
        let long = format!("{}.h", "a".repeat(400));
        make_zip(archive.path(), &[(long.as_str(), "x")]);
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_zip(archive.path(), dest.path(), SafeExtractOptions::default())
            .unwrap_err();
        match err {
            ArtifactError::ArchiveEntryPathTooLong { path, limit } => {
                assert_eq!(limit, MAX_PATH_BYTES);
                assert!(path.len() <= 70, "diagnostic path not truncated: {path}");
            }
            other => panic!("expected ArchiveEntryPathTooLong, got {other:?}"),
        }
    }

    #[test]
    fn zip_aggregate_cap_scales_down_with_the_compressed_size() {
        // The zip side has no whole-stream cap, so the aggregate cap
        // is scaled to the same compressed-size-derived value the tar
        // side uses.  With a 1x ratio and no floor, a highly
        // compressible zip cannot write far more than it shipped.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bomb.zip");
        let body = "x".repeat(100_000);
        make_zip(archive.path(), &[("bomb.bin", body.as_str())]);
        let limits = ExtractLimits {
            ratio: 1,
            ratio_floor_bytes: 0,
            ..ExtractLimits::default()
        };
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_zip_with_limits(
            archive.path(),
            dest.path(),
            limits,
            SafeExtractOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(err, ArtifactError::ArchiveTooLarge { .. }),
            "{err:?}"
        );
        dest.child("bomb.bin").assert(predicate::path::missing());
    }

    #[test]
    fn zip_strip_prefix_rejects_archive_without_matching_root() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("other.zip");
        make_zip(archive.path(), &[("not-miniz/miniz.h", "// nope\n")]);
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_zip(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("miniz-3.1.2"),
                ..SafeExtractOptions::default()
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, ArtifactError::MissingStripPrefix { ref strip_prefix } if strip_prefix == "miniz-3.1.2"),
            "{err:?}"
        );
    }

    #[test]
    fn validate_extracted_rejects_version_mismatch() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str("[package]\nname = \"fmt\"\nversion = \"10.1.0\"\n")
            .unwrap();
        let err = validate_extracted(dir.path(), &pkg("fmt"), &ver("10.2.1")).unwrap_err();
        assert!(matches!(err, ArtifactError::ManifestMismatch { .. }));
    }
}
