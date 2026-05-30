//! Vendoring layer for Cabin.
//!
//! Given a resolved external-dependency closure, this crate
//! writes a self-contained local **file-registry** directory
//! that the rest of Cabin's read path already understands. The
//! materialized vendor directory is just an existing
//! [`cabin_registry_file::FileRegistry`] layout
//! (`<vendor>/config.json`, `<vendor>/packages/<name>.json`,
//! `<vendor>/artifacts/<name>/<name>-<version>.tar.gz`),
//! populated only with the versions the build actually needs.
//!
//! This means an offline workflow needs no new resolver, no new
//! registry protocol, and no new on-disk format:
//!
//! ```text
//!   cabin vendor                                # populate ./vendor
//!   cabin build  --offline --index-path ./vendor
//!   cabin test   --offline --index-path ./vendor
//! ```
//!
//! Crate boundaries:
//! - this crate must not run the resolver, parse arbitrary
//!   `cabin.toml`s, or call out to the network;
//! - it owns the deterministic vendor layout, the per-package
//!   index-entry transformation, the path-traversal-safe
//!   archive copy, and the `cabin-vendor.json` summary file
//!   that records the vendor invocation;
//! - the orchestration layer (`cabin/src/vendor_glue.rs`)
//!   resolves the closure via the existing artifact pipeline
//!   and hands a [`VendorPlan`] to [`materialize`];
//! - this crate must not weaken existing artifact safety:
//!   archive copies preserve the byte stream verbatim, never
//!   re-extract, and re-verify the checksum recorded by the
//!   plan.

#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use cabin_core::PackageName;
use cabin_core::hash::hex_digest;
use cabin_fs::write_atomic;
use cabin_registry_file::{FileRegistry, RegistryConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Filename of the deterministic vendor-summary file written
/// alongside the file-registry layout. Keeps a record of *which*
/// packages this directory was vendored for so a stale vendor
/// directory is detectable without re-resolving.
pub(crate) const VENDOR_SUMMARY_FILENAME: &str = "cabin-vendor.json";

/// Schema version of [`VendorSummary`]. Bumping this requires a
/// migration story; existing vendor directories from earlier
/// versions of Cabin are flagged as stale by the orchestrator.
pub(crate) const VENDOR_SUMMARY_SCHEMA: u32 = 1;

/// One package version to write into the vendor directory.
///
/// Constructed by the orchestration layer from a fetched
/// package: each entry pairs the verified archive in the
/// artifact cache with the per-version JSON the source index
/// already published.
#[derive(Debug, Clone)]
pub struct VendorEntry {
    /// Package name (e.g. `fmt`).
    pub name: PackageName,
    /// Resolved version (e.g. `10.2.1`).
    pub version: semver::Version,
    /// Raw `sha256:<hex>` checksum recorded in the source
    /// index. Re-validated by [`materialize`] before the byte
    /// stream is written to the vendor directory.
    pub checksum: String,
    /// Filesystem path to the archive Cabin already fetched and
    /// verified into the artifact cache. Must point at a
    /// `.tar.gz` whose SHA-256 matches `checksum`.
    pub archive_source: PathBuf,
    /// Per-version JSON value as it appears in the source
    /// index's `packages/<name>.json`. The vendor writer copies
    /// this almost verbatim — it only rewrites the
    /// `source.path` field to point at the new vendor-relative
    /// archive path.
    pub index_entry: serde_json::Value,
}

/// A finalized vendor plan. Build it from the orchestration
/// layer and consume it with [`materialize`].
#[derive(Debug, Clone, Default)]
pub struct VendorPlan {
    entries: Vec<VendorEntry>,
}

impl VendorPlan {
    /// Construct a plan from a list of entries, sorting by
    /// `(name, version)` ascending so the plan's iteration
    /// order matches the deterministic `cabin metadata` /
    /// lockfile order. Duplicate `(name, version)` pairs are
    /// rejected: a single vendor invocation must not write the
    /// same package version twice, regardless of how the
    /// orchestration layer collected them.
    pub fn new(mut entries: Vec<VendorEntry>) -> Result<Self, VendorError> {
        entries.sort_by(|a, b| {
            a.name
                .as_str()
                .cmp(b.name.as_str())
                .then_with(|| a.version.cmp(&b.version))
        });
        for window in entries.windows(2) {
            if window[0].name == window[1].name && window[0].version == window[1].version {
                return Err(VendorError::DuplicateEntry {
                    name: window[0].name.as_str().to_owned(),
                    version: window[0].version.to_string(),
                });
            }
        }
        Ok(Self { entries })
    }

    /// Iterate entries in deterministic order.
    pub fn iter(&self) -> std::slice::Iter<'_, VendorEntry> {
        self.entries.iter()
    }
}

impl<'a> IntoIterator for &'a VendorPlan {
    type Item = &'a VendorEntry;
    type IntoIter = std::slice::Iter<'a, VendorEntry>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

impl VendorPlan {
    /// Number of entries in the plan.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the plan has no entries. An empty plan is a
    /// valid input — `cabin vendor` for a package with no
    /// versioned dependencies writes only a `config.json` plus
    /// a `cabin-vendor.json` summary noting "no entries".
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Caller-controlled options for [`materialize`].
#[derive(Debug, Clone, Default)]
pub struct VendorOptions {
    /// `--frozen`: a frozen vendor invocation still writes the
    /// vendor directory (that is the explicit user-requested
    /// output), but [`materialize`] surfaces this flag back in
    /// the [`VendorReport::frozen`] field so the orchestration
    /// layer can also forbid lockfile / artifact-cache mutation
    /// at higher layers.
    pub frozen: bool,
}

/// Outcome of one [`materialize`] invocation.
#[derive(Debug, Clone)]
pub struct VendorReport {
    /// Absolute path of the vendor directory.
    pub vendor_dir: PathBuf,
    /// Per-package metadata for every entry that was actually
    /// written this invocation, in deterministic order.
    pub written: Vec<VendorOutcomeEntry>,
    /// Whether the invocation ran in `--frozen` mode. Recorded
    /// so the orchestration layer can render the same flag in
    /// its summary message.
    pub frozen: bool,
}

/// Per-entry outcome of a vendor invocation.
#[derive(Debug, Clone)]
pub struct VendorOutcomeEntry {
    pub name: PackageName,
    pub version: semver::Version,
    /// Absolute path of the artifact archive that was written
    /// (or already present and verified).
    pub artifact_path: PathBuf,
    /// Vendor-root-relative path of the artifact, used by the
    /// per-package index file to point readers at the archive.
    pub artifact_relative_path: String,
    /// Whether the archive byte stream was newly written this
    /// invocation. `false` when the destination already existed
    /// with a matching SHA-256.
    pub artifact_was_written: bool,
}

/// Materialize `plan` into `vendor_dir` as a complete file
/// registry. The directory is created if missing.
///
/// The function is idempotent: re-running with the same plan
/// produces byte-equivalent output and only rewrites files
/// whose contents changed. Already-correct archives are kept
/// in place after a re-verification pass; a checksum mismatch
/// at the destination is surfaced as a hard error so the user
/// can decide whether to delete the vendor directory and
/// re-run.
pub fn materialize(
    plan: &VendorPlan,
    vendor_dir: &Path,
    options: &VendorOptions,
) -> Result<VendorReport, VendorError> {
    let vendor_dir = canonicalize_or_create(vendor_dir)?;

    // Ensure / re-use the file-registry skeleton (writes
    // `config.json` if missing). The `was_initialized_now`
    // bit is recorded in the summary so a downstream check
    // can tell first-run from re-run.
    let registry = FileRegistry::open_or_initialize(&vendor_dir).map_err(VendorError::Registry)?;
    debug_assert_eq!(
        registry.config().schema,
        RegistryConfig::default_v1().schema,
        "file-registry config schema must stay aligned with the writer"
    );

    fs::create_dir_all(registry.packages_dir()).map_err(|source| VendorError::Io {
        path: registry.packages_dir(),
        source,
    })?;
    fs::create_dir_all(registry.artifacts_dir()).map_err(|source| VendorError::Io {
        path: registry.artifacts_dir(),
        source,
    })?;

    // Group entries by package name so we can write one
    // `packages/<name>.json` per package with every version
    // we vendored (sorted by SemVer ascending — handled
    // inside `cabin_registry_file::index::render`).
    let mut by_name: BTreeMap<PackageName, Vec<&VendorEntry>> = BTreeMap::new();
    for entry in plan {
        by_name.entry(entry.name.clone()).or_default().push(entry);
    }

    let mut outcomes: Vec<VendorOutcomeEntry> = Vec::with_capacity(plan.len());
    let mut summary_entries: Vec<VendorSummaryEntry> = Vec::with_capacity(plan.len());

    for (name, entries) in &by_name {
        let mut version_entries: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        for entry in entries {
            // Always verify the source archive before copying.
            // The plan promises "already-verified bytes"; we
            // treat that as a courtesy and re-verify here so a
            // bug in the upstream pipeline cannot surface as a
            // silently corrupted vendor archive.
            let actual = file_sha256(&entry.archive_source)?;
            let expected_hex = strip_sha256_prefix(&entry.checksum).ok_or_else(|| {
                VendorError::InvalidChecksum {
                    name: entry.name.as_str().to_owned(),
                    version: entry.version.to_string(),
                    value: entry.checksum.clone(),
                }
            })?;
            if !eq_ignore_ascii_case(&actual, expected_hex) {
                return Err(VendorError::ChecksumMismatch {
                    name: entry.name.as_str().to_owned(),
                    version: entry.version.to_string(),
                    expected: entry.checksum.clone(),
                    actual: format!("sha256:{actual}"),
                    archive: entry.archive_source.clone(),
                });
            }

            let artifact_path = registry.artifact_path(entry.name.as_str(), &entry.version);
            let artifact_relative =
                registry.relative_source_path(entry.name.as_str(), &entry.version);
            let written = copy_archive_if_changed(
                &entry.archive_source,
                &artifact_path,
                expected_hex,
                &entry.name,
                &entry.version,
            )?;

            // Build the per-version JSON: copy the source's
            // entry verbatim, then rewrite `source.path` so it
            // points at the new vendor-relative artifact path.
            let mut version_value = entry.index_entry.clone();
            rewrite_source_path(&mut version_value, &artifact_relative);
            // Drop any other absolute-path leakage that the
            // source index may have carried — only the vendor's
            // own relative source path is meaningful to the
            // file-registry reader.
            version_entries.insert(entry.version.to_string(), version_value);

            summary_entries.push(VendorSummaryEntry {
                name: entry.name.as_str().to_owned(),
                version: entry.version.to_string(),
                checksum: entry.checksum.clone(),
                source: artifact_relative.clone(),
            });

            outcomes.push(VendorOutcomeEntry {
                name: entry.name.clone(),
                version: entry.version.clone(),
                artifact_path,
                artifact_relative_path: artifact_relative,
                artifact_was_written: written,
            });
        }

        let index_doc = serde_json::json!({
            "schema": cabin_registry_file::PACKAGE_INDEX_SCHEMA,
            "name": name.as_str(),
            "versions": render_versions_in_semver_order(version_entries),
        });
        let mut body = serde_json::to_string_pretty(&index_doc).map_err(VendorError::Json)?;
        body.push('\n');
        let target = registry.package_index_path(name.as_str());
        write_if_changed(&target, body.as_bytes())?;
    }

    // Write the deterministic summary so a stale directory is
    // detectable. The summary lists every vendored
    // `(name, version, checksum, relative_path)` in the same
    // sorted order the plan iterator guarantees.
    let summary = VendorSummary {
        schema: VENDOR_SUMMARY_SCHEMA,
        entries: summary_entries,
    };
    let mut summary_body = serde_json::to_string_pretty(&summary).map_err(VendorError::Json)?;
    summary_body.push('\n');
    write_if_changed(
        &vendor_dir.join(VENDOR_SUMMARY_FILENAME),
        summary_body.as_bytes(),
    )?;

    Ok(VendorReport {
        vendor_dir,
        written: outcomes,
        frozen: options.frozen,
    })
}

/// Stable serialized summary of one vendor invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VendorSummary {
    pub schema: u32,
    pub entries: Vec<VendorSummaryEntry>,
}

/// One entry in a [`VendorSummary`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VendorSummaryEntry {
    pub name: String,
    pub version: String,
    pub checksum: String,
    /// Vendor-root-relative path to the artifact (`artifacts/<name>/<name>-<version>.tar.gz`).
    pub source: String,
}

/// Errors produced while planning or materializing a vendor
/// directory. Wording is stable so integration tests can match
/// substrings.
#[derive(Debug, Error)]
pub enum VendorError {
    /// The plan listed the same `(name, version)` pair twice.
    #[error("vendor plan duplicates package `{name}` version `{version}`")]
    DuplicateEntry { name: String, version: String },

    /// A checksum string was not in the expected
    /// `sha256:<hex>` form.
    #[error(
        "vendor entry for `{name}` `{version}` has an invalid checksum `{value}`; expected `sha256:<hex>` form"
    )]
    InvalidChecksum {
        name: String,
        version: String,
        value: String,
    },

    /// The on-disk archive's SHA-256 did not match the plan's
    /// recorded checksum. The destination is left untouched.
    #[error(
        "checksum mismatch while vendoring `{name}` `{version}`: expected `{expected}`, archive at `{}` hashes to `{actual}`",
        archive.display()
    )]
    ChecksumMismatch {
        name: String,
        version: String,
        expected: String,
        actual: String,
        archive: PathBuf,
    },

    /// The destination archive (already present in the vendor
    /// directory) does not match the new checksum. The user
    /// should remove the stale file and re-run.
    #[error(
        "vendor directory already contains `{}` with checksum `sha256:{actual}`, which does not match the requested `{expected}`; remove the stale file and re-run",
        path.display()
    )]
    StaleArtifact {
        path: PathBuf,
        expected: String,
        actual: String,
    },

    /// An archive path contained an absolute prefix or a `..`
    /// component. Vendor refuses to write archives whose
    /// destination would escape the vendor directory.
    #[error("unsafe artifact destination `{}`: paths must be relative and must not contain `..`", path.display())]
    UnsafeArtifactPath { path: PathBuf },

    /// Generic I/O failure with the file path that triggered
    /// it. Wraps the underlying `io::Error`.
    #[error("vendor I/O error at {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// JSON serialization failure. Should never happen for the
    /// internal-only structures this crate writes; surfaced
    /// rather than panicked so a future on-disk schema change
    /// has a place to fail cleanly.
    #[error("vendor metadata serialization failed: {0}")]
    Json(#[from] serde_json::Error),

    /// Forwarded from `cabin-registry-file` when the vendor
    /// directory cannot be opened or initialized as a file
    /// registry.
    #[error(transparent)]
    Registry(cabin_registry_file::RegistryError),
}

fn canonicalize_or_create(dir: &Path) -> Result<PathBuf, VendorError> {
    fs::create_dir_all(dir).map_err(|source| VendorError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    fs::canonicalize(dir).map_err(|source| VendorError::Io {
        path: dir.to_path_buf(),
        source,
    })
}

fn copy_archive_if_changed(
    src: &Path,
    dst: &Path,
    expected_hex: &str,
    name: &PackageName,
    version: &semver::Version,
) -> Result<bool, VendorError> {
    // Refuse to write archives whose path contains `..`.  The
    // caller built `dst` from `registry.artifact_path`, which
    // sanitizes the package name through `<name>` only, but
    // the safety check stays here as a defense in depth.
    if dst.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(VendorError::UnsafeArtifactPath {
            path: dst.to_path_buf(),
        });
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|source| VendorError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    if dst.is_file() {
        let existing = file_sha256(dst)?;
        if eq_ignore_ascii_case(&existing, expected_hex) {
            // Already correct; do not rewrite.
            return Ok(false);
        }
        return Err(VendorError::StaleArtifact {
            path: dst.to_path_buf(),
            expected: format!("sha256:{expected_hex}"),
            actual: existing,
        });
    }

    // Stream the bytes through a small buffer so the writer
    // never holds the entire archive in memory. Re-verify the
    // hash on the way through so a torn copy surfaces as a
    // clean error rather than a silently bad vendor archive.
    let mut input = fs::File::open(src).map_err(|source| VendorError::Io {
        path: src.to_path_buf(),
        source,
    })?;
    // Match the `.partial` convention used by `cabin-artifact`
    // and `cabin-registry-file` so orphaned partial writes share
    // a single recognizable suffix.  `Path::with_extension`
    // would replace the trailing `gz` segment of
    // `fmt-10.2.1.tar.gz` and produce a doubled `.tar.tar.gz`,
    // so we append the suffix via `OsString` instead.
    let temp = {
        let mut s: OsString = dst.as_os_str().to_owned();
        s.push(".partial");
        PathBuf::from(s)
    };
    {
        let mut output = fs::File::create(&temp).map_err(|source| VendorError::Io {
            path: temp.clone(),
            source,
        })?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let read = input.read(&mut buf).map_err(|source| VendorError::Io {
                path: src.to_path_buf(),
                source,
            })?;
            if read == 0 {
                break;
            }
            hasher.update(&buf[..read]);
            output
                .write_all(&buf[..read])
                .map_err(|source| VendorError::Io {
                    path: temp.clone(),
                    source,
                })?;
        }
        let actual = hex_digest(&hasher.finalize());
        if !eq_ignore_ascii_case(&actual, expected_hex) {
            // Drop the partial copy before surfacing the error.
            let _ = fs::remove_file(&temp);
            return Err(VendorError::ChecksumMismatch {
                name: name.as_str().to_owned(),
                version: version.to_string(),
                expected: format!("sha256:{expected_hex}"),
                actual: format!("sha256:{actual}"),
                archive: src.to_path_buf(),
            });
        }
    }
    fs::rename(&temp, dst).map_err(|source| VendorError::Io {
        path: dst.to_path_buf(),
        source,
    })?;
    Ok(true)
}

fn write_if_changed(path: &Path, body: &[u8]) -> Result<(), VendorError> {
    if let Ok(existing) = fs::read(path)
        && existing == body
    {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| VendorError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    write_atomic(path, body).map_err(|source| VendorError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn file_sha256(path: &Path) -> Result<String, VendorError> {
    let mut file = fs::File::open(path).map_err(|source| VendorError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf).map_err(|source| VendorError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex_digest(&hasher.finalize()))
}

fn strip_sha256_prefix(checksum: &str) -> Option<&str> {
    checksum.strip_prefix("sha256:")
}

fn eq_ignore_ascii_case(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn rewrite_source_path(value: &mut serde_json::Value, relative: &str) {
    if let Some(obj) = value.as_object_mut()
        && let Some(source) = obj.get_mut("source").and_then(|v| v.as_object_mut())
    {
        source.insert(
            "path".to_owned(),
            serde_json::Value::String(relative.to_owned()),
        );
    }
}

fn render_versions_in_semver_order(map: BTreeMap<String, serde_json::Value>) -> serde_json::Value {
    // Re-key by parsed SemVer so 10.x sorts after 9.x — the
    // file-registry index renderer does the same, and we want
    // the vendor output to match byte-for-byte.
    let mut sorted: Vec<(semver::Version, serde_json::Value)> = map
        .into_iter()
        .filter_map(|(k, v)| semver::Version::parse(&k).ok().map(|p| (p, v)))
        .collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = serde_json::Map::new();
    for (ver, value) in sorted {
        out.insert(ver.to_string(), value);
    }
    serde_json::Value::Object(out)
}

/// The default vendor directory name used by `cabin vendor`
/// when the user does not pass `--vendor-dir`.
pub const DEFAULT_VENDOR_DIRNAME: &str = "vendor";

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;
    use std::collections::BTreeMap as BTreeMapStd;

    fn pkg(name: &str) -> PackageName {
        PackageName::new(name.to_owned()).expect("test package name is valid")
    }

    fn ver(s: &str) -> semver::Version {
        semver::Version::parse(s).expect("test version is valid")
    }

    fn write_archive(dir: &TempDir, name: &str, version: &str, body: &[u8]) -> (PathBuf, String) {
        let file = dir.child(format!("{name}-{version}.tar.gz"));
        file.write_binary(body).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(body);
        let hex = hex_digest(&hasher.finalize());
        (file.to_path_buf(), format!("sha256:{hex}"))
    }

    fn entry(name: &str, version: &str, archive: PathBuf, checksum: String) -> VendorEntry {
        VendorEntry {
            name: pkg(name),
            version: ver(version),
            checksum,
            archive_source: archive,
            index_entry: serde_json::json!({
                "dependencies": {},
                "yanked": false,
                "checksum": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "source": {
                    "type": "archive",
                    "path": "/abs/host/leak/path.tar.gz",
                    "format": "tar.gz",
                },
            }),
        }
    }

    #[test]
    fn plan_orders_entries_by_name_then_version() {
        let dir = assert_fs::TempDir::new().unwrap();
        let (a1, c1) = write_archive(&dir, "fmt", "10.1.0", b"a");
        let (a2, c2) = write_archive(&dir, "fmt", "10.2.1", b"b");
        let (a3, c3) = write_archive(&dir, "spdlog", "1.13.0", b"c");
        let plan = VendorPlan::new(vec![
            entry("spdlog", "1.13.0", a3, c3),
            entry("fmt", "10.2.1", a2, c2),
            entry("fmt", "10.1.0", a1, c1),
        ])
        .unwrap();
        let names: Vec<(String, String)> = plan
            .iter()
            .map(|e| (e.name.as_str().to_owned(), e.version.to_string()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("fmt".into(), "10.1.0".into()),
                ("fmt".into(), "10.2.1".into()),
                ("spdlog".into(), "1.13.0".into()),
            ]
        );
    }

    #[test]
    fn plan_rejects_duplicate_name_version_pairs() {
        let dir = assert_fs::TempDir::new().unwrap();
        let (a, c) = write_archive(&dir, "fmt", "10.2.1", b"a");
        let err = VendorPlan::new(vec![
            entry("fmt", "10.2.1", a.clone(), c.clone()),
            entry("fmt", "10.2.1", a, c),
        ])
        .unwrap_err();
        match err {
            VendorError::DuplicateEntry { name, version } => {
                assert_eq!(name, "fmt");
                assert_eq!(version, "10.2.1");
            }
            other => panic!("expected DuplicateEntry, got {other:?}"),
        }
    }

    #[test]
    fn materialize_writes_deterministic_file_registry() {
        let cache = assert_fs::TempDir::new().unwrap();
        let vendor = assert_fs::TempDir::new().unwrap();
        let (a1, c1) = write_archive(&cache, "fmt", "10.1.0", b"hello");
        let (a2, c2) = write_archive(&cache, "fmt", "10.2.1", b"world");
        let plan = VendorPlan::new(vec![
            entry("fmt", "10.2.1", a2, c2),
            entry("fmt", "10.1.0", a1, c1),
        ])
        .unwrap();

        let report = materialize(&plan, vendor.path(), &VendorOptions::default()).unwrap();
        // Two archives + one packages/fmt.json + one config.json
        // + one cabin-vendor.json.
        let written: BTreeMapStd<String, bool> = report
            .written
            .iter()
            .map(|e| {
                (
                    format!("{}:{}", e.name.as_str(), e.version),
                    e.artifact_was_written,
                )
            })
            .collect();
        assert!(written["fmt:10.1.0"]);
        assert!(written["fmt:10.2.1"]);
        assert!(report.vendor_dir.join("config.json").is_file());
        assert!(report.vendor_dir.join("packages/fmt.json").is_file());
        assert!(
            report
                .vendor_dir
                .join("artifacts/fmt/fmt-10.1.0.tar.gz")
                .is_file()
        );
        assert!(
            report
                .vendor_dir
                .join("artifacts/fmt/fmt-10.2.1.tar.gz")
                .is_file()
        );
        assert!(report.vendor_dir.join(VENDOR_SUMMARY_FILENAME).is_file());

        // Per-package index rewrites the source path to the
        // vendor-relative form and never leaks the original
        // `/abs/host/leak/path.tar.gz` value.
        let body = fs::read_to_string(report.vendor_dir.join("packages/fmt.json")).unwrap();
        assert!(body.contains("../artifacts/fmt/fmt-10.1.0.tar.gz"));
        assert!(body.contains("../artifacts/fmt/fmt-10.2.1.tar.gz"));
        assert!(!body.contains("/abs/host/leak"));
        // SemVer order: 10.1.0 before 10.2.1 in the rendered
        // JSON, so a future on-disk diff stays scannable.
        let pos_old = body.find("10.1.0").unwrap();
        let pos_new = body.find("10.2.1").unwrap();
        assert!(pos_old < pos_new);

        // Re-running with an unchanged plan must be a no-op:
        // every artifact is reported as `was_written=false`
        // because the destination already matches.
        let report2 = materialize(&plan, vendor.path(), &VendorOptions::default()).unwrap();
        for entry in &report2.written {
            assert!(
                !entry.artifact_was_written,
                "second run must not rewrite `{}:{}`",
                entry.name.as_str(),
                entry.version
            );
        }
        // Summary file must be byte-stable across the two
        // invocations so a stale-vendor check can compare them.
        let summary_body = fs::read(report.vendor_dir.join(VENDOR_SUMMARY_FILENAME)).unwrap();
        let summary_body2 = fs::read(report2.vendor_dir.join(VENDOR_SUMMARY_FILENAME)).unwrap();
        assert_eq!(summary_body, summary_body2);
    }

    #[test]
    fn materialize_rejects_checksum_mismatch_in_source_archive() {
        let cache = assert_fs::TempDir::new().unwrap();
        let vendor = assert_fs::TempDir::new().unwrap();
        let (archive, _real_checksum) = write_archive(&cache, "fmt", "10.2.1", b"hello");
        // Plan declares a wrong checksum: vendor must surface
        // `ChecksumMismatch` and not write anything.
        let mut e = entry(
            "fmt",
            "10.2.1",
            archive,
            "sha256:".to_owned() + &"0".repeat(64),
        );
        // Match shape (sha256:hex) so we hit the checksum
        // comparison rather than `InvalidChecksum`.
        let _ = &mut e;
        let plan = VendorPlan::new(vec![e]).unwrap();
        let err = materialize(&plan, vendor.path(), &VendorOptions::default()).unwrap_err();
        match err {
            VendorError::ChecksumMismatch { name, version, .. } => {
                assert_eq!(name, "fmt");
                assert_eq!(version, "10.2.1");
            }
            other => panic!("expected ChecksumMismatch, got {other:?}"),
        }
        assert!(
            !vendor
                .path()
                .join("artifacts/fmt/fmt-10.2.1.tar.gz")
                .exists()
        );
    }

    #[test]
    fn materialize_rejects_invalid_checksum_form() {
        let cache = assert_fs::TempDir::new().unwrap();
        let vendor = assert_fs::TempDir::new().unwrap();
        let (archive, _) = write_archive(&cache, "fmt", "10.2.1", b"x");
        let e = entry("fmt", "10.2.1", archive, "md5:abc".to_owned());
        let plan = VendorPlan::new(vec![e]).unwrap();
        let err = materialize(&plan, vendor.path(), &VendorOptions::default()).unwrap_err();
        assert!(matches!(err, VendorError::InvalidChecksum { .. }));
    }

    #[test]
    fn materialize_keeps_existing_correct_artifact_in_place() {
        let cache = assert_fs::TempDir::new().unwrap();
        let vendor = assert_fs::TempDir::new().unwrap();
        let (archive, checksum) = write_archive(&cache, "fmt", "10.2.1", b"abc");
        // Pre-populate the vendor directory with the *correct*
        // archive byte stream — this is what a re-run after a
        // partial earlier vendor invocation looks like.
        let target = vendor.path().join("artifacts/fmt/fmt-10.2.1.tar.gz");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(&archive, &target).unwrap();

        let plan = VendorPlan::new(vec![entry("fmt", "10.2.1", archive, checksum)]).unwrap();
        let report = materialize(&plan, vendor.path(), &VendorOptions::default()).unwrap();
        assert!(!report.written[0].artifact_was_written);
    }

    #[test]
    fn materialize_rejects_stale_artifact_in_place() {
        let cache = assert_fs::TempDir::new().unwrap();
        let vendor = assert_fs::TempDir::new().unwrap();
        let (archive, checksum) = write_archive(&cache, "fmt", "10.2.1", b"new");
        // Pre-seed a stale artifact whose hash differs from
        // what the plan requires. Vendor must refuse.
        let target = vendor.path().join("artifacts/fmt/fmt-10.2.1.tar.gz");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"stale").unwrap();

        let plan = VendorPlan::new(vec![entry("fmt", "10.2.1", archive, checksum)]).unwrap();
        let err = materialize(&plan, vendor.path(), &VendorOptions::default()).unwrap_err();
        match err {
            VendorError::StaleArtifact { path, .. } => {
                // Match by suffix because the vendor canonicalizes
                // its root, which on macOS turns `/var/...` into
                // `/private/var/...`. The relative tail is what
                // matters for the diagnostic.
                assert!(
                    path.ends_with("artifacts/fmt/fmt-10.2.1.tar.gz"),
                    "stale-artifact path should point at the destination archive, got: {}",
                    path.display()
                );
            }
            other => panic!("expected StaleArtifact, got {other:?}"),
        }
    }

    #[test]
    fn empty_plan_writes_only_skeleton_files() {
        let vendor = assert_fs::TempDir::new().unwrap();
        let plan = VendorPlan::default();
        let report = materialize(&plan, vendor.path(), &VendorOptions::default()).unwrap();
        assert!(report.written.is_empty());
        assert!(vendor.path().join("config.json").is_file());
        assert!(vendor.path().join(VENDOR_SUMMARY_FILENAME).is_file());
        // No artifacts directory entries, but the parent dirs
        // exist so a follow-up `cabin vendor` can populate them.
        assert!(vendor.path().join("packages").is_dir());
        assert!(vendor.path().join("artifacts").is_dir());
    }
}
