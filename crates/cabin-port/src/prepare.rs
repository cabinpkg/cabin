//! Port-preparation pipeline.
//!
//! The pipeline turns a [`PortPlan`] (each entry is a parsed
//! `PortDescriptor` plus a [`PortFetchSource`]) into a list of
//! [`PreparedPort`]s on disk.  Each materialized port directory is
//! an ordinary Cabin package directory: the upstream source files
//! plus the overlay `cabin.toml` at the directory root, which is
//! what the publisher packages.
//!
//! For each entry the pipeline:
//!
//! 1. resolves the cache paths (archive + extracted source dir);
//! 2. ensures the archive is on disk and hashes to the declared
//!    SHA-256, populating from the supplied [`PortFetchSource`]
//!    if necessary (refused when frozen);
//! 3. extracts the archive into the source dir with the
//!    declared `strip_prefix`, reusing `cabin-artifact`'s
//!    decompression-bomb caps and path-safety rules;
//! 4. applies any declared `[[copy]]` placements, copying an
//!    upstream file to a second in-tree location (e.g. a
//!    prebuilt config header to its build-time name);
//! 5. applies any declared `patches` as byte-exact unified diffs,
//!    then places each patch file itself into the tree at its
//!    declared `patches/<file>` path (so a published conversion
//!    ships the patches its provenance declaration names);
//! 6. copies the overlay `cabin.toml` into the extracted source
//!    dir, overwriting any in-tree copy that already existed;
//! 7. cross-checks the overlay's `[package]` identity against
//!    the authoritative `port.toml`;
//! 8. writes the `<source_dir>.ok` completion marker so a future
//!    run can reuse the prep without re-extracting.
//!
//! A crash between extraction and marker write leaves the
//! marker absent; the next run treats the directory as
//! interrupted and re-extracts from scratch.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use cabin_artifact::cache::{extraction_marker_path, partial_dir_sibling, partial_sibling};
use cabin_artifact::{SafeExtractOptions, safe_extract_tar_gz, safe_extract_zip};
use cabin_core::PackageName;
use cabin_fs::write_atomic;
use camino::Utf8PathBuf;
use semver::Version;
use url::Url;

use crate::cache::{ArchiveKind, PortCache};
use crate::error::{FsResultExt, PortError};
use crate::model::{ArchiveSource, CopyStep, PortChecksum, PortDescriptor};

/// Where to read archive bytes from. `cabin-port` stays HTTP-free:
/// callers handle any download themselves and pass the resulting
/// bytes via [`PortFetchSource::InMemoryArchive`].
#[derive(Debug, Clone)]
pub enum PortFetchSource {
    /// Filesystem path the caller has already resolved to a
    /// ready-to-open archive (e.g. a `file://` URL).
    LocalArchive(PathBuf),
    /// Archive bytes already in memory (HTTP downloads, custom
    /// fetchers, tests).
    InMemoryArchive(Vec<u8>),
}

/// Where a port's recipe came from: the directory `ensure_overlay`
/// reads the overlay text from.
#[derive(Debug, Clone)]
pub enum PortOrigin {
    /// Filesystem recipe: `<port_dir>/port.toml` plus the
    /// overlay manifest at the descriptor's relative path.
    PortDir(PathBuf),
}

/// One port to materialize.
#[derive(Debug, Clone)]
pub struct PortEntry {
    /// Parsed `port.toml`.
    pub descriptor: PortDescriptor,
    /// Where the port's recipe came from.  Determines how the
    /// overlay manifest is sourced.
    pub origin: PortOrigin,
    /// Where the archive bytes come from.
    pub source: PortFetchSource,
}

impl PortEntry {
    /// The `(name, version)` pair used to identify this port in
    /// `PortError` diagnostics.
    fn identity(&self) -> (String, String) {
        (
            self.descriptor.name.as_str().to_owned(),
            self.descriptor.version.to_string(),
        )
    }
}

/// A finalized preparation plan.  Build it from the orchestration
/// layer and pass it to [`prepare`].
#[derive(Debug, Clone, Default)]
pub struct PortPlan {
    pub entries: Vec<PortEntry>,
}

/// Caller-controlled knobs.
#[derive(Debug, Clone, Copy, Default)]
pub struct PortPrepareOptions {
    /// `--frozen`: do not populate the cache.  If a required
    /// archive or extracted source tree is not already cached
    /// and valid, fail with [`PortError::FrozenCacheMiss`].
    pub frozen: bool,
}

/// Outcome of one [`prepare`] invocation.
#[derive(Debug, Clone)]
pub struct PortPrepareResult {
    pub ports: Vec<PreparedPort>,
}

/// One fully materialized port: archive verified, source
/// extracted (with `strip_prefix`), overlay copied,
/// `[package]` identity cross-checked.
#[derive(Debug, Clone)]
pub struct PreparedPort {
    pub name: PackageName,
    pub version: Version,
    pub source_dir: PathBuf,
    pub origin: PortOrigin,
    pub provenance: PortProvenance,
    /// `true` when this run materialized the archive from
    /// freshly-provided bytes ([`PortFetchSource::InMemoryArchive`]) -
    /// i.e. the caller downloaded it this invocation - rather than
    /// reusing a local or already-cached archive
    /// ([`PortFetchSource::LocalArchive`]).
    pub downloaded: bool,
}

/// Provenance recorded for downstream observability.
#[derive(Debug, Clone)]
pub struct PortProvenance {
    pub url: Url,
    pub sha256_hex: String,
    pub strip_prefix: Option<String>,
    /// Absolute path to the overlay manifest inside the port
    /// directory (i.e. `port_dir.join(overlay.relative_path)`).
    pub overlay_manifest: Option<PathBuf>,
}

/// Materialize every entry in `plan` into the cache.
///
/// # Errors
/// Returns the first [`PortError`] produced while preparing an entry,
/// stopping on failure.  Notable variants: [`PortError::FrozenCacheMiss`]
/// when `frozen` is set and the archive or extracted source is not
/// already cached; [`PortError::MissingArchive`] for an absent local
/// archive; [`PortError::ChecksumMismatch`] when fetched bytes do not
/// hash to the declared SHA-256; [`PortError::MissingStripPrefix`] or
/// [`PortError::Extract`] from extraction; [`PortError::MissingOverlayManifest`]
/// when the overlay cannot be sourced;
/// [`PortError::OverlayManifestParse`], [`PortError::OverlayMissingPackage`],
/// or [`PortError::OverlayIdentityMismatch`] from the identity cross-check;
/// and [`PortError::Fs`] for any underlying filesystem error.
pub fn prepare(
    plan: &PortPlan,
    cache: &PortCache,
    options: PortPrepareOptions,
) -> Result<PortPrepareResult, PortError> {
    let mut ports = Vec::with_capacity(plan.entries.len());
    for entry in &plan.entries {
        ports.push(prepare_one(entry, cache, options)?);
    }
    Ok(PortPrepareResult { ports })
}

fn prepare_one(
    entry: &PortEntry,
    cache: &PortCache,
    options: PortPrepareOptions,
) -> Result<PreparedPort, PortError> {
    let ArchiveSource {
        url,
        sha256,
        strip_prefix,
    } = &entry.descriptor.source;

    let expected_hex = sha256.to_hex();
    let archive_kind = ArchiveKind::from_url(url);
    let archive_path = cache.archive_path(&expected_hex, archive_kind);
    // Extracted sources are identity-keyed (name + version) so two
    // ports that share the same upstream archive but ship different
    // overlays do not clobber each other's `cabin.toml`.
    let source_dir = cache.source_dir(
        &entry.descriptor.name,
        &entry.descriptor.version.to_string(),
        &expected_hex,
    );

    // Patch bytes are resolved up front: the plan fingerprint must
    // cover their *content* (the files live outside the hash-verified
    // archive, so an edited patch under an unchanged path would
    // otherwise warm-hit a stale tree), and a missing or oversized
    // patch file should fail before any extraction work.
    let patch_plan = resolve_patch_plan(entry)?;
    // The extracted tree is keyed by the archive hash, which does not
    // capture the `[[copy]]` or patch plan.  Fold both into the
    // completion marker so a recipe whose transformation changed
    // against an unchanged archive re-extracts clean instead of
    // reusing a tree built by the previous plan.
    let plan_fingerprint = plan_fingerprint(&entry.descriptor.copies, &patch_plan);

    ensure_archive(entry, &archive_path, sha256, options.frozen)?;
    // `Some(tmp)` when the archive had to be extracted: the rest of
    // the preparation - copies, patches, overlay, identity
    // cross-check - then runs against the scratch tree, which is
    // renamed into place only once the whole pipeline succeeds.  A
    // hostile archive, a broken `[[copy]]` plan, an inapplicable
    // patch, or a mismatched overlay therefore leaves no partially
    // prepared port at `source_dir`.  `None` is a warm cache hit: the
    // marker's fingerprint proved the existing tree was produced by
    // this exact copy + patch plan, so the transformation steps are
    // NOT re-run - re-copying would revert a patched copy target and
    // re-patching would fail its context match - and only the overlay
    // refresh and identity cross-check repeat.
    let scratch = ensure_source(
        entry,
        &archive_path,
        archive_kind,
        &source_dir,
        strip_prefix.as_deref(),
        &plan_fingerprint,
        options.frozen,
    )?;
    let prepare_dir = scratch.as_deref().unwrap_or(source_dir.as_path());
    let transformed = match &scratch {
        Some(_) => apply_copies(entry, prepare_dir)
            .and_then(|()| apply_patches(entry, prepare_dir, &patch_plan)),
        None => Ok(()),
    };
    let prepared = transformed
        .and_then(|()| ensure_overlay(entry, prepare_dir))
        .and_then(|()| cross_check_overlay_identity(entry, prepare_dir));
    if let Err(err) = prepared {
        if let Some(tmp) = &scratch {
            let _ = fs::remove_dir_all(tmp);
        }
        return Err(err);
    }
    if let Some(tmp) = &scratch {
        // Rename the fully prepared tree into place.  A failure -
        // including a concurrent process having populated `source_dir`
        // first, which makes the rename onto a non-empty directory
        // fail - removes the scratch so no partial state leaks, and
        // surfaces the error rather than adopting a tree this process
        // did not build (a concurrent run with a different overlay or
        // `[[copy]]` plan would not have produced an identical one).
        // A retry warm-hits the winner's tree when the recipe matches
        // and re-extracts otherwise.
        if let Err(source) = fs::rename(tmp, &source_dir) {
            let _ = fs::remove_dir_all(tmp);
            return Err(PortError::Fs {
                path: source_dir.clone(),
                source,
            });
        }
    }
    write_marker(&source_dir, &plan_fingerprint)?;

    let PortOrigin::PortDir(dir) = &entry.origin;
    let overlay_manifest = Some(dir.join(&entry.descriptor.overlay.relative_path));
    Ok(PreparedPort {
        name: entry.descriptor.name.clone(),
        version: entry.descriptor.version.clone(),
        source_dir,
        origin: entry.origin.clone(),
        provenance: PortProvenance {
            url: url.clone(),
            sha256_hex: expected_hex,
            strip_prefix: strip_prefix.clone(),
            overlay_manifest,
        },
        downloaded: matches!(entry.source, PortFetchSource::InMemoryArchive(_)),
    })
}

fn ensure_archive(
    entry: &PortEntry,
    archive_path: &Path,
    expected: &PortChecksum,
    frozen: bool,
) -> Result<(), PortError> {
    let expected_hex = expected.to_hex();
    if archive_path.is_file() {
        let actual = hash_file(archive_path)?;
        if actual == expected_hex {
            return Ok(());
        }
    }
    if frozen {
        let (name, version) = entry.identity();
        return Err(PortError::FrozenCacheMiss { name, version });
    }

    if let PortFetchSource::LocalArchive(path) = &entry.source
        && !path.is_file()
    {
        let (name, version) = entry.identity();
        return Err(PortError::MissingArchive {
            name,
            version,
            path: path.clone(),
        });
    }

    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent).with_path(parent)?;
    }

    let tmp_target = partial_sibling(archive_path);
    let actual = match &entry.source {
        PortFetchSource::LocalArchive(path) => stream_local_to_partial(path, &tmp_target)?,
        PortFetchSource::InMemoryArchive(bytes) => write_bytes_to_partial(bytes, &tmp_target)?,
    };

    if actual != expected_hex {
        let _ = fs::remove_file(&tmp_target);
        let (name, version) = entry.identity();
        return Err(PortError::ChecksumMismatch {
            name,
            version,
            expected: expected_hex,
            actual,
        });
    }
    // Windows refuses `fs::rename` when the destination exists,
    // so a corrupted-cache recovery (stale archive at the
    // content-addressed path with the wrong hash) cannot
    // self-heal.  Remove the stale file up-front; `NotFound` is
    // the common case (no stale file present) and surfaces as a
    // silent no-op rather than an error.
    match fs::remove_file(archive_path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(PortError::Fs {
                path: archive_path.to_path_buf(),
                source,
            });
        }
    }
    fs::rename(&tmp_target, archive_path).with_path(archive_path)?;
    Ok(())
}

/// Make sure `source_dir` holds the archive's extracted tree.
///
/// Returns `Ok(None)` on a warm cache hit, and `Ok(Some(scratch))`
/// after extracting into a sibling scratch directory the caller must
/// finish preparing and rename into place.
fn ensure_source(
    entry: &PortEntry,
    archive_path: &Path,
    archive_kind: ArchiveKind,
    source_dir: &Path,
    strip_prefix: Option<&str>,
    plan_fingerprint: &str,
    frozen: bool,
) -> Result<Option<PathBuf>, PortError> {
    let marker = extraction_marker_path(source_dir);
    if marker.is_file() && source_dir.join("cabin.toml").is_file() {
        // We trust the marker because:
        // 1. cabin-port wrote the marker only after a full
        //    successful extraction + transformation + overlay copy +
        //    identity cross-check, so the directory contents matched
        //    the port descriptor when the marker was written;
        // 2. the archive on disk has already been re-verified
        //    by `ensure_archive`, so the source tree we wrote
        //    from it is still correct under the recorded hash;
        // 3. the marker records the `[[copy]]` + patch plan (patch
        //    content included) that produced the tree, so a changed
        //    plan (which the hash-keyed directory cannot distinguish)
        //    forces a clean re-extract below rather than reusing a
        //    tree built by the previous plan.  A missing/legacy empty
        //    marker matches only the empty (no-copy, no-patch) plan,
        //    so untransformed ports keep reusing their cache
        //    untouched.
        //    The marker exists (checked above), so a read failure is a
        //    real filesystem error, not a cache miss - surface it rather
        //    than treating an unreadable marker as the empty
        //    fingerprint, which would silently reuse an unverified tree.  A
        //    legacy empty marker reads as "" and matches the empty plan.
        let recorded = fs::read_to_string(&marker).with_path(&marker)?;
        if recorded == plan_fingerprint {
            return Ok(None);
        }
    }

    if frozen {
        let (name, version) = entry.identity();
        return Err(PortError::FrozenCacheMiss { name, version });
    }

    // Drop a stale marker before re-extracting so a crash before
    // the new marker is written cannot leave a previous run's
    // "complete" flag pointing at a partially overwritten tree.
    if marker.exists() {
        fs::remove_file(&marker).with_path(&marker)?;
    }
    if source_dir.exists() {
        fs::remove_dir_all(source_dir).with_path(source_dir)?;
    }
    // Extract into a sibling scratch directory the caller renames
    // into place once the whole preparation succeeds, so a hostile
    // upstream archive rejected mid-extraction never leaves a partial
    // tree at the final path (same `.partial` + rename convention as
    // the archive download above).
    let tmp_dir = partial_dir_sibling(source_dir);
    if tmp_dir.exists() {
        fs::remove_dir_all(&tmp_dir).with_path(&tmp_dir)?;
    }
    fs::create_dir_all(&tmp_dir).with_path(&tmp_dir)?;

    // Both extractors share the same signature, options, and
    // fail-closed rules; the URL extension picked the kind.
    let extract = match archive_kind {
        ArchiveKind::TarGz => safe_extract_tar_gz,
        ArchiveKind::Zip => safe_extract_zip,
    };
    extract(
        archive_path,
        &tmp_dir,
        SafeExtractOptions {
            strip_prefix,
            // Upstream release archives commonly carry convenience
            // symlinks (uthash ships `include -> src`); skip them
            // instead of refusing the whole port. Nothing is
            // materialized for a skipped entry, and an overlay only
            // ever references real files, so the traversal-safety
            // posture is unchanged. Package archives keep the
            // strict default: Cabin produces those itself and they
            // never contain symlinks.
            skip_symlinks: true,
        },
    )
    .map_err(|err| {
        let _ = fs::remove_dir_all(&tmp_dir);
        let (name, version) = entry.identity();
        match err {
            cabin_artifact::ArtifactError::MissingStripPrefix { strip_prefix } => {
                PortError::MissingStripPrefix {
                    name,
                    version,
                    strip_prefix,
                }
            }
            other => PortError::Extract {
                name,
                version,
                source: Box::new(other),
            },
        }
    })?;
    Ok(Some(tmp_dir))
}

/// Apply the descriptor's `[[copy]]` placements to the extracted
/// source tree.  Each step copies `from` to `to`, both already
/// validated as non-empty safe relative paths (no `..`, no absolute
/// component) so neither can escape `source_dir`.
///
/// Run on a freshly extracted scratch tree only (the caller skips it
/// on a warm cache hit), before [`apply_patches`] and
/// [`ensure_overlay`] so a later patch sees the copied file and the
/// overlay `cabin.toml` wins on any conflicting `to`.  It is not
/// re-run on a warm hit: the plan fingerprint already proved the
/// cached tree was produced by this exact copy + patch plan, and
/// re-copying would revert a copy target a later patch modified.
fn apply_copies(entry: &PortEntry, source_dir: &Path) -> Result<(), PortError> {
    for step in &entry.descriptor.copies {
        let from = source_dir.join(step.from.as_std_path());
        let to = source_dir.join(step.to.as_std_path());
        if !from.is_file() {
            let (name, version) = entry.identity();
            return Err(PortError::MissingCopySource {
                name,
                version,
                path: from,
            });
        }
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).with_path(parent)?;
        }
        fs::copy(&from, &to).with_path(&to)?;
    }
    Ok(())
}

/// One declared patch, resolved to its bytes from the port
/// directory.
struct ResolvedPatch {
    rel_path: Utf8PathBuf,
    bytes: Vec<u8>,
}

/// Reject a declared patch path whose `patches/` component or leaf is
/// a symlink, WITHOUT following links.  Declared patch entries are
/// exempt from the registry verifier's tree comparison, so a followed
/// link would smuggle uncommitted host-local bytes into a verified
/// package.  An in-tree link (`patches/x -> ../real.patch`)
/// canonicalizes inside the port directory and reads a regular file,
/// so containment alone would accept it.
fn reject_symlinked_patch_path(
    entry: &PortEntry,
    port_dir: &Path,
    rel_path: &Utf8PathBuf,
    path: &Path,
) -> Result<(), PortError> {
    let unsafe_patch = || PortError::UnsafePatchPath {
        path: port_dir.to_path_buf(),
        value: rel_path.to_string(),
    };
    let patches_dir = port_dir.join("patches");
    match fs::symlink_metadata(&patches_dir) {
        Ok(meta) if meta.file_type().is_symlink() => return Err(unsafe_patch()),
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(PortError::Fs {
                path: patches_dir,
                source,
            });
        }
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(unsafe_patch()),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let (name, version) = entry.identity();
            Err(PortError::MissingPatchFile {
                name,
                version,
                path: path.to_path_buf(),
            })
        }
        Err(source) => Err(PortError::Fs {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Resolve every declared patch to its bytes, enforcing presence and
/// the shared size cap up front.
fn resolve_patch_plan(entry: &PortEntry) -> Result<Vec<ResolvedPatch>, PortError> {
    let mut plan = Vec::with_capacity(entry.descriptor.patches.len());
    for rel_path in &entry.descriptor.patches {
        let PortOrigin::PortDir(port_dir) = &entry.origin;
        let path = port_dir.join(rel_path.as_std_path());
        reject_symlinked_patch_path(entry, port_dir, rel_path, &path)?;
        // Resolve the real file and require it to stay inside
        // the port directory.  Canonicalizing follows every
        // symlink on the way - the leaf `patches/x`, an
        // intermediate `patches/` that is itself a symlink, or
        // any `..` - so a link escaping the port directory
        // (which would read and then publish bytes from
        // outside it, breaking the `patches/<file>`
        // containment) is rejected.  Kept as defense in depth
        // behind the symlink rejection above.
        let canonical = match path.canonicalize() {
            Ok(canonical) => canonical,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let (name, version) = entry.identity();
                return Err(PortError::MissingPatchFile {
                    name,
                    version,
                    path,
                });
            }
            Err(source) => return Err(PortError::Fs { path, source }),
        };
        let canonical_port = port_dir.canonicalize().with_path(port_dir)?;
        let meta = fs::metadata(&canonical).with_path(&canonical)?;
        // Beyond containment and regular-file-ness, require the
        // declared path to match the on-disk spelling
        // case-exactly: a case-insensitive host (macOS, Windows)
        // would otherwise accept `patches/Fix.patch` for an
        // on-disk `patches/fix.patch`, so the same committed recipe
        // would publish from one host and fail on a case-sensitive
        // checkout - host-dependent behavior on the path that
        // produces published bytes.
        let case_exact =
            cabin_artifact::path_is_case_exact(port_dir, rel_path.as_str()).with_path(port_dir)?;
        if !canonical.starts_with(&canonical_port) || !meta.file_type().is_file() || !case_exact {
            return Err(PortError::UnsafePatchPath {
                path: port_dir.clone(),
                value: rel_path.to_string(),
            });
        }
        // Bound the read: an over-cap patch must fail without
        // first allocating the whole file.
        if meta.len() > cabin_core::MAX_PATCH_BYTES as u64 {
            let (name, version) = entry.identity();
            return Err(PortError::PatchTooLarge {
                name,
                version,
                path,
                size: usize::try_from(meta.len()).unwrap_or(usize::MAX),
                limit: cabin_core::MAX_PATCH_BYTES,
            });
        }
        let (bytes, display_path) = (fs::read(&canonical).with_path(&canonical)?, path);
        // Backstop for a file that grew between the `fs::metadata`
        // stat above and this read: the pre-read cap saw the old
        // length.
        if bytes.len() > cabin_core::MAX_PATCH_BYTES {
            let (name, version) = entry.identity();
            return Err(PortError::PatchTooLarge {
                name,
                version,
                path: display_path,
                size: bytes.len(),
                limit: cabin_core::MAX_PATCH_BYTES,
            });
        }
        plan.push(ResolvedPatch {
            rel_path: rel_path.clone(),
            bytes,
        });
    }
    Ok(plan)
}

/// Apply the resolved patch plan to the extracted tree, then place
/// each patch file itself at its declared `patches/<file>` path.
///
/// Runs after [`apply_copies`] and before [`ensure_overlay`] - the
/// documented assembly order (extract, strip-prefix, copies, patches)
/// that the registry's external verifier reproduces.  Materialization
/// comes after application on purpose: a patch can therefore never
/// target a declared patch file, on either side.  Unlike copies this
/// is NOT idempotent (re-applying fails its context match), which is
/// why the caller only runs it on a freshly extracted scratch tree.
fn apply_patches(
    entry: &PortEntry,
    source_dir: &Path,
    plan: &[ResolvedPatch],
) -> Result<(), PortError> {
    if plan.is_empty() {
        return Ok(());
    }
    let inputs: Vec<cabin_artifact::PatchInput<'_>> = plan
        .iter()
        .map(|patch| cabin_artifact::PatchInput {
            name: patch.rel_path.as_str(),
            bytes: &patch.bytes,
        })
        .collect();
    cabin_artifact::apply_unified_patches(source_dir, &inputs).map_err(|source| {
        let (name, version) = entry.identity();
        PortError::PatchApply {
            name,
            version,
            source: Box::new(source),
        }
    })?;
    for patch in plan {
        let dest = source_dir.join(patch.rel_path.as_std_path());
        // A declared patch path that collides with the assembled
        // tree (shipped by the upstream archive, produced by a copy,
        // or created by a patch) is a shadow: the verifier excludes
        // the patch path from its tree comparison and rejects the
        // version as `upstream_patch_invalid (shadows tree)`.  The
        // lookup is the engine's case-folded one, not a plain
        // `exists()`: a case-insensitive host resolves the
        // destination onto a differently-cased entry that a
        // case-sensitive host materializes alongside, and packaging
        // then rejects the case conflict anyway.  Reject here so
        // preparation fails identically on every platform instead
        // of producing a package guaranteed to fail packaging or
        // verification.
        if cabin_artifact::create_would_conflict(source_dir, patch.rel_path.as_str())
            .with_path(&dest)?
        {
            let (name, version) = entry.identity();
            return Err(PortError::PatchShadowsTree {
                name,
                version,
                path: patch.rel_path.clone(),
            });
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_path(parent)?;
        }
        fs::write(&dest, &patch.bytes).with_path(&dest)?;
    }
    Ok(())
}

fn ensure_overlay(entry: &PortEntry, source_dir: &Path) -> Result<(), PortError> {
    let overlay_dest = source_dir.join("cabin.toml");
    let PortOrigin::PortDir(port_dir) = &entry.origin;
    let overlay_source = port_dir.join(&entry.descriptor.overlay.relative_path);
    // Checked before the read so a missing overlay surfaces as the
    // typed `MissingOverlayManifest`, not a bare io error.
    if !overlay_source.is_file() {
        let (name, version) = entry.identity();
        return Err(PortError::MissingOverlayManifest {
            name,
            version,
            path: overlay_source,
        });
    }
    let overlay_bytes = fs::read(&overlay_source).with_path(&overlay_source)?;
    // Unconditional: a warm cache hit skips the copy and patch steps
    // but still refreshes the overlay, which
    // `cross_check_overlay_identity` reads back.
    write_atomic(&overlay_dest, &overlay_bytes).with_path(&overlay_dest)?;
    Ok(())
}

fn cross_check_overlay_identity(entry: &PortEntry, source_dir: &Path) -> Result<(), PortError> {
    let overlay_manifest = source_dir.join("cabin.toml");
    let parsed = cabin_manifest::load_manifest(&overlay_manifest).map_err(|source| {
        let (name, version) = entry.identity();
        PortError::OverlayManifestParse {
            name,
            version,
            source: Box::new(source),
        }
    })?;
    let package = parsed.package.ok_or_else(|| {
        let (name, version) = entry.identity();
        PortError::OverlayMissingPackage { name, version }
    })?;
    if package.name != entry.descriptor.name || package.version != entry.descriptor.version {
        let (name, version) = entry.identity();
        return Err(PortError::OverlayIdentityMismatch {
            name,
            version,
            actual_name: package.name.as_str().to_owned(),
            actual_version: package.version.to_string(),
        });
    }
    Ok(())
}

fn write_marker(source_dir: &Path, plan_fingerprint: &str) -> Result<(), PortError> {
    let marker = extraction_marker_path(source_dir);
    fs::write(&marker, plan_fingerprint).with_path(&marker)
}

/// Deterministic fingerprint of a port's transformation plan -
/// `[[copy]]` steps plus resolved patches - stored in the completion
/// marker so `ensure_source` can detect a changed plan.  Copy lines
/// are length-prefixed so no `from`/`to` content can forge an entry
/// boundary; patch lines carry the content digest because patch
/// bytes live outside the hash-verified archive (an edited patch
/// file must invalidate the tree even when its path is unchanged).
/// The two line shapes cannot forge each other: copy lines start
/// with a digit, patch lines with `patch `.  The empty plan yields
/// the empty string, which matches a legacy empty marker (so
/// untransformed ports never spuriously re-extract).
fn plan_fingerprint(copies: &[CopyStep], patches: &[ResolvedPatch]) -> String {
    use std::fmt::Write as _;

    use sha2::Digest as _;
    let mut out = String::new();
    for step in copies {
        let from = step.from.as_str();
        let to = step.to.as_str();
        // Writing to a String is infallible, so the Result is ignored.
        let _ = writeln!(out, "{}:{from} {}:{to}", from.len(), to.len());
    }
    for patch in patches {
        let path = patch.rel_path.as_str();
        let digest = cabin_core::hash::hex_digest(&sha2::Sha256::digest(&patch.bytes));
        let _ = writeln!(out, "patch {}:{path} {digest}", path.len());
    }
    out
}

fn stream_local_to_partial(source_path: &Path, tmp_target: &Path) -> Result<String, PortError> {
    let mut src = File::open(source_path).with_path(source_path)?;
    let mut dst = File::create(tmp_target).with_path(tmp_target)?;
    // Errors mapped to `tmp_target`: a mid-stream failure is far more
    // likely to be a write to the cache target than the local source
    // going unreadable after a successful open.
    cabin_core::hash::hash_copy(&mut src, &mut dst).with_path(tmp_target)
}

fn write_bytes_to_partial(bytes: &[u8], tmp_target: &Path) -> Result<String, PortError> {
    let mut dst = File::create(tmp_target).with_path(tmp_target)?;
    cabin_core::hash::hash_copy(bytes, &mut dst).with_path(tmp_target)
}

fn hash_file(path: &Path) -> Result<String, PortError> {
    let f = File::open(path).with_path(path)?;
    cabin_core::hash::hash_reader(f).with_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{ArchiveKind, PortCache};
    use crate::model::{
        ArchiveSource, OverlayManifest, PortChecksum, PortDescriptor, PortMetadata,
    };
    use assert_fs::TempDir;
    use assert_fs::prelude::*;
    use cabin_core::PackageName;
    use cabin_core::hash::hex_digest;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use semver::Version;
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    use camino::Utf8PathBuf;
    use url::Url;

    fn pkg(name: &str) -> PackageName {
        PackageName::new(name).unwrap()
    }

    fn make_archive(dir: &Path, name: &str, entries: &[(&str, &str)]) -> (PathBuf, String) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let f = fs::File::create(&path).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
        let mut builder = tar::Builder::new(enc);
        for (rel, body) in entries {
            let bytes = body.as_bytes();
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            builder
                .append_data(&mut header, rel, &mut std::io::Cursor::new(bytes))
                .unwrap();
        }
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap().flush().unwrap();
        let bytes = fs::read(&path).unwrap();
        let mut h = Sha256::new();
        h.update(&bytes);
        (path, hex_digest(&h.finalize()))
    }

    fn lay_overlay(port_dir: &Path, body: &str) {
        assert_fs::fixture::ChildPath::new(port_dir.join("cabin.toml"))
            .write_str(body)
            .unwrap();
    }

    fn make_descriptor(url: Url, sha256_hex: &str) -> PortDescriptor {
        PortDescriptor {
            name: pkg("zlib"),
            version: Version::new(1, 3, 1),
            metadata: PortMetadata::default(),
            source: ArchiveSource {
                url,
                sha256: PortChecksum::parse_hex(sha256_hex).unwrap(),
                strip_prefix: Some("zlib-1.3.1".to_owned()),
            },
            overlay: OverlayManifest {
                relative_path: Utf8PathBuf::from("cabin.toml"),
            },
            copies: Vec::new(),
            patches: Vec::new(),
        }
    }

    fn ok_overlay() -> &'static str {
        "[package]\nname = \"zlib\"\nversion = \"1.3.1\"\nc-standard = \"c11\"\n\n[target.zlib]\ntype = \"library\"\nsources = [\"zlib.c\"]\ninclude-dirs = [\".\"]\n"
    }

    #[test]
    fn prepares_port_from_local_archive() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "#define ZLIB_VERSION \"1.3.1\"\n"),
                ("zlib-1.3.1/zlib.c", "int zlib_dummy(void) { return 0; }\n"),
            ],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir.clone()),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let result = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        assert_eq!(result.ports.len(), 1);
        let prepared = &result.ports[0];
        assert!(prepared.source_dir.join("cabin.toml").is_file());
        assert!(prepared.source_dir.join("zlib.h").is_file());
        assert!(prepared.source_dir.join("zlib.c").is_file());
        // No `zlib-1.3.1/` survives the strip.
        assert!(!prepared.source_dir.join("zlib-1.3.1").exists());
        // Marker is a sibling.
        let mut marker = prepared.source_dir.as_os_str().to_owned();
        marker.push(".ok");
        assert!(Path::new(&marker).is_file());
        // Provenance is recorded.
        assert_eq!(prepared.provenance.sha256_hex, hex);
        assert_eq!(
            prepared.provenance.strip_prefix.as_deref(),
            Some("zlib-1.3.1")
        );
        // A local/cached archive is not a network download.
        assert!(!prepared.downloaded);
    }

    #[test]
    fn prepares_port_from_in_memory_archive() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "// stub\n"),
            ],
        );
        let bytes = fs::read(&archive).unwrap();
        // No file URL for in-memory source.
        let descriptor = make_descriptor(
            Url::parse("https://example.com/zlib-1.3.1.tar.gz").unwrap(),
            &hex,
        );
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::InMemoryArchive(bytes),
            }],
        };
        let result = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        assert!(result.ports[0].source_dir.join("zlib.h").is_file());
        // Bytes supplied in memory (the caller downloaded them) mark the
        // port as freshly downloaded this run.
        assert!(result.ports[0].downloaded);
    }

    #[test]
    fn reports_checksum_mismatch() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, _hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[("zlib-1.3.1/zlib.h", "// stub\n")],
        );
        let bogus = "0".repeat(64);
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &bogus);
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        match err {
            PortError::ChecksumMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, bogus);
                assert_ne!(actual, expected);
            }
            other => panic!("expected ChecksumMismatch, got {other:?}"),
        }
    }

    #[test]
    fn reports_missing_strip_prefix() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[("other-1.0/zlib.h", "// nope\n")],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        match err {
            PortError::MissingStripPrefix {
                strip_prefix, name, ..
            } => {
                assert_eq!(strip_prefix, "zlib-1.3.1");
                assert_eq!(name, "zlib");
            }
            other => panic!("expected MissingStripPrefix, got {other:?}"),
        }
    }

    #[test]
    fn reports_overlay_identity_mismatch() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        // Overlay declares the wrong name/version.
        lay_overlay(
            &port_dir,
            "[package]\nname = \"other\"\nversion = \"9.9.9\"\nc-standard = \"c11\"\n\n[target.zlib]\ntype = \"library\"\nsources = [\"zlib.c\"]\n",
        );
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "// stub\n"),
            ],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        match err {
            PortError::OverlayIdentityMismatch {
                actual_name,
                actual_version,
                ..
            } => {
                assert_eq!(actual_name, "other");
                assert_eq!(actual_version, "9.9.9");
            }
            other => panic!("expected OverlayIdentityMismatch, got {other:?}"),
        }
    }

    #[test]
    fn second_call_reuses_cached_prep_after_archive_disappears() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "// stub\n"),
            ],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let make_plan = || PortPlan {
            entries: vec![PortEntry {
                descriptor: descriptor.clone(),
                origin: PortOrigin::PortDir(port_dir.clone()),
                source: PortFetchSource::LocalArchive(archive.clone()),
            }],
        };
        prepare(&make_plan(), &cache, PortPrepareOptions::default()).unwrap();
        fs::remove_file(&archive).unwrap();
        let r2 = prepare(&make_plan(), &cache, PortPrepareOptions::default()).unwrap();
        assert!(r2.ports[0].source_dir.join("cabin.toml").is_file());
    }

    #[test]
    fn re_extracts_when_marker_missing_even_if_manifest_present() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "// stub\n"),
            ],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let source_dir = cache.source_dir(&descriptor.name, &descriptor.version.to_string(), &hex);
        // Simulate an interrupted previous run: manifest present
        // but no completion marker.
        assert_fs::fixture::ChildPath::new(source_dir.join("cabin.toml"))
            .write_str("garbage")
            .unwrap();
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        let body = fs::read_to_string(source_dir.join("cabin.toml")).unwrap();
        assert!(
            body.contains("zlib"),
            "overlay should be re-applied: {body}"
        );
        let mut marker = source_dir.as_os_str().to_owned();
        marker.push(".ok");
        assert!(Path::new(&marker).is_file());
    }

    #[test]
    fn frozen_fails_on_cache_miss() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[("zlib-1.3.1/zlib.h", "// stub\n")],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions { frozen: true }).unwrap_err();
        assert!(matches!(err, PortError::FrozenCacheMiss { .. }), "{err:?}");
    }

    #[test]
    fn frozen_succeeds_when_cache_is_populated() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "// stub\n"),
            ],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let make_plan = || PortPlan {
            entries: vec![PortEntry {
                descriptor: descriptor.clone(),
                origin: PortOrigin::PortDir(port_dir.clone()),
                source: PortFetchSource::LocalArchive(archive.clone()),
            }],
        };
        prepare(&make_plan(), &cache, PortPrepareOptions::default()).unwrap();
        // Now run again with --frozen - should succeed.
        prepare(&make_plan(), &cache, PortPrepareOptions { frozen: true }).unwrap();
    }

    #[test]
    fn reports_missing_archive_for_nonexistent_local_path() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let descriptor = make_descriptor(
            Url::parse("file:///nonexistent/zlib.tar.gz").unwrap(),
            &"a".repeat(64),
        );
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(PathBuf::from("/nonexistent/zlib.tar.gz")),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        assert!(matches!(err, PortError::MissingArchive { .. }), "{err:?}");
    }

    #[test]
    fn reports_missing_overlay_manifest() {
        let dir = TempDir::new().unwrap();
        let port_dir_child = dir.child("port");
        // Port dir exists but overlay file does not.
        port_dir_child.create_dir_all().unwrap();
        let port_dir = port_dir_child.to_path_buf();
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "// stub\n"),
            ],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        assert!(
            matches!(err, PortError::MissingOverlayManifest { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn applies_copy_step_into_extracted_tree() {
        use crate::model::CopyStep;
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/scripts/conf.prebuilt", "// prebuilt config\n"),
            ],
        );
        let mut descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        descriptor.copies = vec![CopyStep {
            from: Utf8PathBuf::from("scripts/conf.prebuilt"),
            to: Utf8PathBuf::from("conf.h"),
        }];
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let result = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        let source_dir = &result.ports[0].source_dir;
        // The copy lands at the declared destination, and the
        // original upstream file is left in place.
        assert_eq!(
            fs::read_to_string(source_dir.join("conf.h")).unwrap(),
            "// prebuilt config\n"
        );
        assert!(source_dir.join("scripts/conf.prebuilt").is_file());
    }

    #[test]
    fn reports_missing_copy_source() {
        use crate::model::CopyStep;
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[("zlib-1.3.1/zlib.h", "// stub\n")],
        );
        let mut descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        descriptor.copies = vec![CopyStep {
            from: Utf8PathBuf::from("scripts/missing.prebuilt"),
            to: Utf8PathBuf::from("conf.h"),
        }];
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        assert!(
            matches!(err, PortError::MissingCopySource { .. }),
            "{err:?}"
        );
        // The whole preparation is staged in a scratch directory, so
        // a failure after extraction - here a `[[copy]]` step whose
        // source is absent - leaves no half-prepared port behind.
        let source_dir = cache.source_dir(
            &cabin_core::PackageName::new("zlib").unwrap(),
            "1.3.1",
            &hex,
        );
        assert!(!source_dir.exists(), "partial port tree left behind");
        assert!(!extraction_marker_path(&source_dir).exists());
        assert!(
            !partial_dir_sibling(&source_dir).exists(),
            "scratch dir left behind"
        );
    }

    #[test]
    fn a_rejected_archive_leaves_no_partial_port_tree() {
        // A hostile upstream archive: the first entry extracts, the
        // second is a fifo the entry-type gate refuses.  Nothing is
        // left at the port's source directory.
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let downloads = dir.path().join("downloads");
        fs::create_dir_all(&downloads).unwrap();
        let archive = downloads.join("zlib-1.3.1.tar.gz");
        let f = fs::File::create(&archive).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
        let mut builder = tar::Builder::new(enc);
        let body = b"// stub\n";
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
        let mut header = tar::Header::new_gnu();
        header.set_path("zlib-1.3.1/pipe").unwrap();
        header.set_size(0);
        header.set_entry_type(tar::EntryType::Fifo);
        header.set_cksum();
        builder.append(&header, std::io::empty()).unwrap();
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap().flush().unwrap();
        let hex = hex_digest(&Sha256::digest(fs::read(&archive).unwrap()));

        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        assert!(matches!(err, PortError::Extract { .. }), "{err:?}");
        let source_dir = cache.source_dir(
            &cabin_core::PackageName::new("zlib").unwrap(),
            "1.3.1",
            &hex,
        );
        assert!(!source_dir.exists(), "partial port tree left behind");
        assert!(
            !partial_dir_sibling(&source_dir).exists(),
            "scratch dir left behind"
        );
    }

    /// The overlay `cabin.toml` always wins when a `[[copy]]`
    /// targets the same destination - `apply_copies` runs before
    /// `ensure_overlay`, so a copy can never clobber the manifest.
    #[test]
    fn overlay_wins_over_conflicting_copy() {
        use crate::model::CopyStep;
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/decoy.toml", "not a manifest\n"),
            ],
        );
        let mut descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        descriptor.copies = vec![CopyStep {
            from: Utf8PathBuf::from("decoy.toml"),
            to: Utf8PathBuf::from("cabin.toml"),
        }];
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let result = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        let overlay = fs::read_to_string(result.ports[0].source_dir.join("cabin.toml")).unwrap();
        assert!(overlay.contains("name = \"zlib\""), "overlay: {overlay}");
    }

    /// Changing a `[[copy]]` plan against an unchanged archive (same
    /// name/version/hash, so the same cache directory) must re-extract
    /// clean: the previous plan's copy target must not linger as an
    /// orphan that could still be compiled.  The marker's recorded
    /// fingerprint is what distinguishes the two plans.
    #[test]
    fn changed_copy_plan_reextracts_and_drops_orphans() {
        use crate::model::CopyStep;
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/conf.prebuilt", "// prebuilt config\n"),
            ],
        );
        let cache = PortCache::new(dir.path().join("cache"));
        let make_plan = |to: &str| {
            let mut descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
            descriptor.copies = vec![CopyStep {
                from: Utf8PathBuf::from("conf.prebuilt"),
                to: Utf8PathBuf::from(to),
            }];
            PortPlan {
                entries: vec![PortEntry {
                    descriptor,
                    origin: PortOrigin::PortDir(port_dir.clone()),
                    source: PortFetchSource::LocalArchive(archive.clone()),
                }],
            }
        };

        // First plan copies to gen_a.h.
        let first = prepare(&make_plan("gen_a.h"), &cache, PortPrepareOptions::default()).unwrap();
        let source_dir = first.ports[0].source_dir.clone();
        assert!(source_dir.join("gen_a.h").is_file());

        // Second plan (same archive identity) copies to gen_b.h.  The
        // orphaned gen_a.h from the first plan must be gone.
        let second = prepare(&make_plan("gen_b.h"), &cache, PortPrepareOptions::default()).unwrap();
        assert_eq!(second.ports[0].source_dir, source_dir, "same cache dir");
        assert!(source_dir.join("gen_b.h").is_file(), "new target present");
        assert!(
            !source_dir.join("gen_a.h").exists(),
            "stale copy target from the previous plan must be dropped"
        );
    }

    /// Common scaffold for the patch tests: an archive with one C
    /// source, a port dir carrying `patches/0001-fix.patch`, and a
    /// plan wired to them.
    fn patched_plan(dir: &TempDir, patch_body: &str) -> (PortPlan, PortCache, String) {
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        assert_fs::fixture::ChildPath::new(port_dir.join("patches/0001-fix.patch"))
            .write_str(patch_body)
            .unwrap();
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "int deflate_broken(void);\n"),
            ],
        );
        let mut descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        descriptor.patches = vec![Utf8PathBuf::from("patches/0001-fix.patch")];
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        (plan, cache, hex)
    }

    const FIX_PATCH: &str = "--- a/zlib.c\n\
                             +++ b/zlib.c\n\
                             @@ -1,1 +1,1 @@\n\
                             -int deflate_broken(void);\n\
                             +int deflate_fixed(void);\n";

    #[test]
    fn applies_declared_patches_and_ships_the_patch_file() {
        let dir = TempDir::new().unwrap();
        let (plan, cache, _) = patched_plan(&dir, FIX_PATCH);
        let result = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        let source_dir = &result.ports[0].source_dir;
        // The patch applied to the extracted source, and the patch
        // file itself was placed at its declared path so a published
        // conversion ships it.
        assert_eq!(
            fs::read_to_string(source_dir.join("zlib.c")).unwrap(),
            "int deflate_fixed(void);\n"
        );
        assert_eq!(
            fs::read_to_string(source_dir.join("patches/0001-fix.patch")).unwrap(),
            FIX_PATCH
        );
    }

    #[test]
    fn warm_cache_skips_patch_reapplication() {
        // Patches are not idempotent: a second application would fail
        // its context match.  A warm hit under an unchanged plan must
        // therefore skip the transformation steps entirely and keep
        // the patched bytes.
        let dir = TempDir::new().unwrap();
        let (plan, cache, _) = patched_plan(&dir, FIX_PATCH);
        prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        let result = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        assert_eq!(
            fs::read_to_string(result.ports[0].source_dir.join("zlib.c")).unwrap(),
            "int deflate_fixed(void);\n"
        );
    }

    #[test]
    fn warm_cache_keeps_a_patched_copy_target() {
        // A copy target that a later patch modifies: if the warm path
        // re-ran the copy steps it would silently revert the patch,
        // so the fingerprint-matched warm hit must skip copies too.
        use crate::model::CopyStep;
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        assert_fs::fixture::ChildPath::new(port_dir.join("patches/0001-fix.patch"))
            .write_str(
                "--- a/conf.h\n+++ b/conf.h\n@@ -1,1 +1,1 @@\n-#define BROKEN 1\n+#define FIXED 1\n",
            )
            .unwrap();
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/conf.prebuilt", "#define BROKEN 1\n"),
            ],
        );
        let mut descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        descriptor.copies = vec![CopyStep {
            from: Utf8PathBuf::from("conf.prebuilt"),
            to: Utf8PathBuf::from("conf.h"),
        }];
        descriptor.patches = vec![Utf8PathBuf::from("patches/0001-fix.patch")];
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        let result = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        assert_eq!(
            fs::read_to_string(result.ports[0].source_dir.join("conf.h")).unwrap(),
            "#define FIXED 1\n"
        );
    }

    #[test]
    fn changed_patch_content_reextracts_the_tree() {
        // The plan fingerprint covers patch *content*, so editing the
        // patch file under an unchanged path invalidates the cached
        // tree instead of warm-hitting a stale one.
        let dir = TempDir::new().unwrap();
        let (plan, cache, _) = patched_plan(&dir, FIX_PATCH);
        prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();

        let port_dir = dir.path().join("port");
        let revised = FIX_PATCH.replace("deflate_fixed", "deflate_final");
        fs::write(port_dir.join("patches/0001-fix.patch"), &revised).unwrap();
        let result = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        assert_eq!(
            fs::read_to_string(result.ports[0].source_dir.join("zlib.c")).unwrap(),
            "int deflate_final(void);\n"
        );
        assert_eq!(
            fs::read_to_string(result.ports[0].source_dir.join("patches/0001-fix.patch")).unwrap(),
            revised
        );
    }

    #[test]
    fn an_inapplicable_patch_leaves_no_partial_tree() {
        let dir = TempDir::new().unwrap();
        let mismatched =
            "--- a/zlib.c\n+++ b/zlib.c\n@@ -1,1 +1,1 @@\n-int other(void);\n+int fixed(void);\n";
        let (plan, cache, hex) = patched_plan(&dir, mismatched);
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        assert!(matches!(err, PortError::PatchApply { .. }), "{err:?}");
        let source_dir = cache.source_dir(
            &cabin_core::PackageName::new("zlib").unwrap(),
            "1.3.1",
            &hex,
        );
        assert!(!source_dir.exists(), "partial port tree left behind");
        assert!(!extraction_marker_path(&source_dir).exists());
    }

    #[cfg(unix)]
    fn patched_plan_with_symlink(
        dir: &TempDir,
        wire_symlink: impl FnOnce(&Path),
    ) -> (PortPlan, PortCache) {
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let secret = dir.path().join("secret.patch");
        fs::write(
            &secret,
            "--- a/zlib.c\n+++ b/zlib.c\n@@ -1,1 +1,1 @@\n-x\n+y\n",
        )
        .unwrap();
        wire_symlink(&port_dir);
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[("zlib-1.3.1/zlib.c", "x\n")],
        );
        let mut descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        descriptor.patches = vec![Utf8PathBuf::from("patches/0001-fix.patch")];
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        (plan, cache)
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlinked_patch_file() {
        // A patch entry that is a symlink out of the port directory
        // would read (and then publish) external bytes; reject it.
        let dir = TempDir::new().unwrap();
        let secret = dir.path().join("secret.patch");
        let (plan, cache) = patched_plan_with_symlink(&dir, |port_dir| {
            fs::create_dir_all(port_dir.join("patches")).unwrap();
            std::os::unix::fs::symlink(&secret, port_dir.join("patches/0001-fix.patch")).unwrap();
        });
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        assert!(matches!(err, PortError::UnsafePatchPath { .. }), "{err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlinked_patches_directory() {
        // An intermediate symlink - `patches/` itself pointing out of
        // the port directory - must be caught too, not just a
        // symlinked leaf file.
        let dir = TempDir::new().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(
            outside.join("0001-fix.patch"),
            "--- a/zlib.c\n+++ b/zlib.c\n@@ -1,1 +1,1 @@\n-x\n+y\n",
        )
        .unwrap();
        let (plan, cache) = patched_plan_with_symlink(&dir, |port_dir| {
            fs::create_dir_all(port_dir).unwrap();
            std::os::unix::fs::symlink(&outside, port_dir.join("patches")).unwrap();
        });
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        assert!(matches!(err, PortError::UnsafePatchPath { .. }), "{err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_an_in_tree_symlinked_patch_file() {
        // A link whose target stays INSIDE the port directory passes
        // canonicalize-containment and reads a regular file - but the
        // published archive must carry the committed regular file
        // itself, never what a link points at.
        let dir = TempDir::new().unwrap();
        let (plan, cache) = patched_plan_with_symlink(&dir, |port_dir| {
            fs::create_dir_all(port_dir.join("patches")).unwrap();
            fs::write(
                port_dir.join("patches/real.patch"),
                "--- a/zlib.c\n+++ b/zlib.c\n@@ -1,1 +1,1 @@\n-x\n+y\n",
            )
            .unwrap();
            std::os::unix::fs::symlink("real.patch", port_dir.join("patches/0001-fix.patch"))
                .unwrap();
        });
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        assert!(matches!(err, PortError::UnsafePatchPath { .. }), "{err:?}");
    }

    #[test]
    fn reports_oversized_patch_file() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        fs::create_dir_all(port_dir.join("patches")).unwrap();
        fs::write(
            port_dir.join("patches/0001-fix.patch"),
            vec![b'x'; cabin_core::MAX_PATCH_BYTES + 1],
        )
        .unwrap();
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[("zlib-1.3.1/zlib.c", "x\n")],
        );
        let mut descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        descriptor.patches = vec![Utf8PathBuf::from("patches/0001-fix.patch")];
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        assert!(matches!(err, PortError::PatchTooLarge { .. }), "{err:?}");
    }

    #[test]
    fn rejects_a_patch_that_shadows_a_tree_file() {
        // The upstream archive already ships `patches/0001-fix.patch`;
        // placing the declared patch there would overwrite it, and the
        // verifier rejects such a version - so preparation must fail
        // too, matching the producer/verifier symmetry.
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        assert_fs::fixture::ChildPath::new(port_dir.join("patches/0001-fix.patch"))
            .write_str(FIX_PATCH)
            .unwrap();
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "int deflate_broken(void);\n"),
                (
                    "zlib-1.3.1/patches/0001-fix.patch",
                    "upstream shipped this\n",
                ),
            ],
        );
        let mut descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        descriptor.patches = vec![Utf8PathBuf::from("patches/0001-fix.patch")];
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        assert!(matches!(err, PortError::PatchShadowsTree { .. }), "{err:?}");
    }

    #[test]
    fn rejects_a_patch_path_that_case_collides_with_the_tree() {
        // The upstream archive ships `Patches/0001-fix.patch` - a
        // case-folded collision, not an exact match.  A plain
        // `exists()` check diverges by host: false on case-sensitive
        // Linux (both entries materialize, packaging later rejects
        // the case conflict), true on case-insensitive macOS and
        // Windows.  The case-folded lookup must fail preparation
        // identically everywhere.
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        assert_fs::fixture::ChildPath::new(port_dir.join("patches/0001-fix.patch"))
            .write_str(FIX_PATCH)
            .unwrap();
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "int deflate_broken(void);\n"),
                (
                    "zlib-1.3.1/Patches/0001-fix.patch",
                    "upstream shipped this\n",
                ),
            ],
        );
        let mut descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        descriptor.patches = vec![Utf8PathBuf::from("patches/0001-fix.patch")];
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        assert!(matches!(err, PortError::PatchShadowsTree { .. }), "{err:?}");
    }

    #[test]
    fn reports_missing_patch_file_before_extraction() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[("zlib-1.3.1/zlib.h", "// stub\n")],
        );
        let mut descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        descriptor.patches = vec![Utf8PathBuf::from("patches/absent.patch")];
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        assert!(matches!(err, PortError::MissingPatchFile { .. }), "{err:?}");
    }

    /// Two port descriptors that intentionally reuse the same
    /// upstream archive - different package identities (different
    /// `[package].name`) shipping different overlays - must
    /// extract into distinct directories so the later overlay
    /// cannot clobber the earlier one's `cabin.toml`.
    #[test]
    fn distinct_identities_do_not_share_one_extracted_tree() {
        let dir = TempDir::new().unwrap();
        // Build one archive whose contents both descriptors claim
        // to ship.  The archive uses neither port's name in its
        // strip prefix so we can point both descriptors at it.
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "shared.tar.gz",
            &[
                ("upstream/header.h", "// shared header\n"),
                ("upstream/source.c", "// shared source\n"),
            ],
        );

        // Two ports - different names - with the same archive.
        let alpha_dir = dir.path().join("port-a");
        lay_overlay(
            &alpha_dir,
            "[package]\nname = \"alpha\"\nversion = \"1.0.0\"\n",
        );
        let beta_dir = dir.path().join("port-b");
        lay_overlay(
            &beta_dir,
            "[package]\nname = \"beta\"\nversion = \"1.0.0\"\n",
        );

        let mk = |name_lit: &str| PortDescriptor {
            name: pkg(name_lit),
            version: Version::new(1, 0, 0),
            metadata: PortMetadata::default(),
            source: ArchiveSource {
                url: Url::from_file_path(&archive).unwrap(),
                sha256: PortChecksum::parse_hex(&hex).unwrap(),
                strip_prefix: Some("upstream".to_owned()),
            },
            overlay: OverlayManifest {
                relative_path: Utf8PathBuf::from("cabin.toml"),
            },
            copies: Vec::new(),
            patches: Vec::new(),
        };

        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![
                PortEntry {
                    descriptor: mk("alpha"),
                    origin: PortOrigin::PortDir(alpha_dir),
                    source: PortFetchSource::LocalArchive(archive.clone()),
                },
                PortEntry {
                    descriptor: mk("beta"),
                    origin: PortOrigin::PortDir(beta_dir),
                    source: PortFetchSource::LocalArchive(archive),
                },
            ],
        };

        let result = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        assert_eq!(result.ports.len(), 2);
        let alpha = &result.ports[0];
        let beta = &result.ports[1];
        assert_ne!(
            alpha.source_dir, beta.source_dir,
            "distinct identities must not collide on one source dir"
        );
        let alpha_overlay = std::fs::read_to_string(alpha.source_dir.join("cabin.toml")).unwrap();
        let beta_overlay = std::fs::read_to_string(beta.source_dir.join("cabin.toml")).unwrap();
        assert!(alpha_overlay.contains("\"alpha\""), "{alpha_overlay}");
        assert!(beta_overlay.contains("\"beta\""), "{beta_overlay}");
    }

    /// Self-healing path: when the content-addressed archive
    /// already exists but its bytes do not match the recorded
    /// hash (corrupted cache entry, interrupted write), prepare
    /// must overwrite it rather than fail.  Windows refuses
    /// `fs::rename` over an existing destination, so the recovery
    /// path has to remove the stale file first; this regression
    /// pins that behavior on every platform.
    #[test]
    fn stale_cached_archive_is_replaced_atomically() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[("zlib-1.3.1/zlib.h", "// good bytes\n")],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));

        // Pre-populate the content-addressed slot with bytes that
        // do *not* hash to `hex`.  A naive `fs::rename` over this
        // file would error on Windows.
        let cached_path = cache.archive_path(&hex, ArchiveKind::TarGz);
        assert_fs::fixture::ChildPath::new(&cached_path)
            .write_binary(b"corrupt")
            .unwrap();

        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let result = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        assert_eq!(result.ports.len(), 1);

        // The stale bytes are gone; the recovered archive hashes
        // to the declared SHA-256 again.
        let mut h = Sha256::new();
        h.update(fs::read(&cached_path).unwrap());
        assert_eq!(hex_digest(&h.finalize()), hex);
    }
}
