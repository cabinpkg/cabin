//! Source-archive packaging and single-package scaffolding for Cabin.
//!
//! Two related responsibilities live behind this crate:
//!
//! - `cabin package` (and the `cabin publish` dry-run flow):
//!   a single-package manifest is validated, the source tree is
//!   enumerated under a fixed include / exclude policy, and a
//!   deterministic `.tar.gz` plus a canonical per-version metadata
//!   document are written to an output directory.
//! - `cabin init` and `cabin new`: a minimal `cabin.toml` plus an
//!   `src/main.cc` are generated at a target directory through the
//!   shared [`scaffold`] entry point so both CLI surfaces produce
//!   byte-identical layouts.
//!
//! Crate boundaries:
//! - this crate must not mutate any registry, run the resolver,
//!   fetch Artifacts, or invoke C/C++ compilers;
//! - it must not implement networking, server-side functionality, or
//!   Publishing — `cabin-publish` orchestrates the dry-run flow on top
//!   Of this crate;
//! - the archive format is intentionally narrow: `tar.gz` only,
//!   Regular files and directories only, deterministic byte-for-byte
//!   For the same logical input.

// `PackageError` aggregates manifest, archive, and validation
// errors. Each variant is small on its own; the union crosses
// clippy's default `result_large_err` threshold once newer
// validation variants (e.g., the `[patch]`-table rejection)
// land. Boxing every result here would obscure the variant on
// the happy path; we accept the larger `Result` instead.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::return_self_not_must_use,
    clippy::doc_markdown,
    clippy::needless_pass_by_value
)]

pub mod archive;
pub mod error;
pub mod metadata;
pub mod scaffold;
pub mod validate;

use std::path::{Path, PathBuf};

use cabin_core::PackageName;

pub use error::PackageError;
pub use metadata::{PackageMetadata, SourceMetadata};

/// Inputs to [`package_with_project`].
#[derive(Debug, Clone)]
pub struct PackageRequest<'a> {
    /// Path to the package's `cabin.toml`. Must point at a single
    /// package; pure-workspace roots are rejected.
    pub manifest_path: &'a Path,
    /// Directory where the archive (`<name>-<version>.tar.gz`) and the
    /// metadata document (`<name>-<version>.json`) are written.
    pub output_dir: &'a Path,
}

/// What [`package_with_project`] produced.
#[derive(Debug, Clone)]
pub struct PackagedArtifact {
    pub name: PackageName,
    pub version: semver::Version,
    pub archive_path: PathBuf,
    pub metadata_path: PathBuf,
    /// Full `sha256:<hex>` digest of the archive bytes.
    pub checksum: String,
}

/// In-memory representation of a packaged source tree.
/// [`stage_with_project`] produces this;
/// [`package_with_project`] writes it to disk and
/// `cabin-publish` hands it to `cabin-registry-file` on the
/// registry-publish path.
///
/// The pieces (`archive_bytes`, `checksum`, `metadata`) are
/// byte-deterministic for the same logical input — see
/// [`archive::build_tar_gz`].
#[derive(Debug, Clone)]
pub struct StagedPackage {
    pub name: PackageName,
    pub version: semver::Version,
    /// Bytes of the deterministic `.tar.gz` source archive.
    pub archive_bytes: Vec<u8>,
    /// Full `sha256:<hex>` digest of `archive_bytes`.
    pub checksum: String,
    /// Canonical per-version metadata document, ready to serialise.
    pub metadata: PackageMetadata,
}

/// Validate the package, walk the source tree under the fixed include
/// / exclude policy, build the deterministic `.tar.gz`, hash it, and
/// generate canonical per-version metadata — all in memory. No files
/// are written.
///
/// `cabin-publish` calls this when handing a package to a downstream
/// writer (e.g. `cabin-registry-file`); [`package_with_project`] is
/// the `cabin-package`-level convenience that combines this with a
/// disk write into `output_dir`.
///
/// The `project_override` argument lets the CLI hand in a
/// pre-resolved `Package` from inside a workspace so member
/// manifests with `dep = { workspace = true }` can be resolved
/// against `[workspace.dependencies]` *before* their package
/// metadata is generated. Standalone callers leave it as `None`
/// and trigger the documented "unresolved workspace dependency"
/// error rather than silently emitting incomplete metadata.
///
/// `output_dir`, when set, names the directory the caller will
/// later write the staged archive into. The staging walker omits
/// it from the archive so a previous run's artefacts living in an
/// in-tree output directory cannot leak back in. Passing `None`
/// disables the exclusion (used by `cabin-publish`, which never
/// writes the archive back into the source tree).
pub fn stage_with_project(
    manifest_path: &Path,
    project_override: Option<cabin_core::Package>,
    output_dir: Option<&Path>,
) -> Result<StagedPackage, PackageError> {
    let validated = validate::load_and_validate_with_project(manifest_path, project_override)?;
    let staging_exclude = match output_dir {
        Some(dir) => {
            let normalized = resolve_for_walker_comparison(dir);
            if normalized == validated.package_root {
                return Err(PackageError::OutputDirIsPackageRoot { path: normalized });
            }
            Some(normalized)
        }
        None => None,
    };
    let files =
        archive::collect_package_files(&validated.package_root, staging_exclude.as_deref())?;
    archive::ensure_manifest_included(&files)?;

    let archive_bytes = archive::build_tar_gz(&files)?;
    let archive_hex = archive::sha256_hex(&archive_bytes);
    let checksum = format!("sha256:{archive_hex}");

    let metadata = metadata::canonical_metadata(&validated.package, &checksum);

    Ok(StagedPackage {
        name: validated.package.name,
        version: validated.package.version,
        archive_bytes,
        checksum,
        metadata,
    })
}

/// Produce a `PathBuf` that compares equal to anything the staging
/// walker emits under the canonicalised package root.
///
/// The walker descends from a canonicalised `package_root` and
/// produces entries via `Path::join`, so a comparable path needs
/// the same canonical form. `output_dir` may not exist yet (it is
/// created by `package_with_project` after staging completes), so
/// `Path::canonicalize` would fail outright. Walk up to the
/// closest existing ancestor, canonicalise that, then reattach the
/// unresolved tail — the walker can never enter that tail anyway,
/// so leaving it lexical is fine.
fn resolve_for_walker_comparison(path: &Path) -> PathBuf {
    let normalized = lexically_normalize(path);
    if let Ok(canonical) = normalized.canonicalize() {
        return canonical;
    }
    let mut head = normalized.clone();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !head.exists() {
        match (head.parent(), head.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                head = parent.to_path_buf();
            }
            _ => return normalized,
        }
    }
    let mut out = head.canonicalize().unwrap_or(head);
    for piece in tail.into_iter().rev() {
        out.push(piece);
    }
    out
}

/// Collapse `.` and `..` components from `path` without touching
/// the filesystem. The CLI absolutises `--output-dir` against the
/// process cwd, which can leave `.` segments or `..` traversals
/// that would otherwise compare unequal to the walker's emitted
/// paths.
fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                out.push(component.as_os_str());
            }
            std::path::Component::Normal(name) => out.push(name),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
        }
    }
    out
}

/// Validate the package, build a deterministic source archive, hash
/// it, and write the archive plus its canonical metadata into
/// `request.output_dir`.
///
/// The archive is byte-deterministic for the same logical input: tar
/// entries are sorted, mtimes / uid / gid / uname / gname are zeroed,
/// the gzip header carries `mtime = 0` and an OS field of `0xff`
/// (unknown), and the include / exclude policy is fixed.
///
/// Rules around overwriting existing files in `output_dir`:
/// - if the archive at the target path is byte-identical, the run
///   Succeeds without rewriting it;
/// - the same goes for the metadata file;
/// - if either file already exists with different bytes,
///   [`PackageError::OutputAlreadyExists`] is returned.
///
/// `project_override` lets the CLI hand in a pre-resolved `Package`
/// when packaging a workspace member so any `{ workspace = true }`
/// deps the member declared are resolved against
/// `[workspace.dependencies]` before metadata is written.
pub fn package_with_project(
    request: PackageRequest<'_>,
    project_override: Option<cabin_core::Package>,
) -> Result<PackagedArtifact, PackageError> {
    let staged = stage_with_project(
        request.manifest_path,
        project_override,
        Some(request.output_dir),
    )?;
    let archive_path = request
        .output_dir
        .join(archive_filename(staged.name.as_str(), &staged.version));
    let metadata_path = request
        .output_dir
        .join(metadata_filename(staged.name.as_str(), &staged.version));

    let metadata_bytes = metadata::render_canonical_json(&staged.metadata)?;

    std::fs::create_dir_all(request.output_dir).map_err(|source| PackageError::Io {
        path: request.output_dir.to_path_buf(),
        source,
    })?;

    write_idempotent(&archive_path, &staged.archive_bytes)?;
    write_idempotent(&metadata_path, metadata_bytes.as_bytes())?;

    Ok(PackagedArtifact {
        name: staged.name,
        version: staged.version,
        archive_path,
        metadata_path,
        checksum: staged.checksum,
    })
}

/// Conventional `<name>-<version>.tar.gz` archive filename.
pub(crate) fn archive_filename(name: &str, version: &semver::Version) -> String {
    format!("{name}-{version}.tar.gz")
}

/// Conventional `<name>-<version>.json` metadata filename.
pub(crate) fn metadata_filename(name: &str, version: &semver::Version) -> String {
    format!("{name}-{version}.json")
}

/// Write `body` to `path`, succeeding silently when the file already
/// holds the same bytes. Mismatched existing content is reported as
/// [`PackageError::OutputAlreadyExists`] so a stale `dist/` is not
/// quietly clobbered.
fn write_idempotent(path: &Path, body: &[u8]) -> Result<(), PackageError> {
    if path.exists() {
        let existing = std::fs::read(path).map_err(|source| PackageError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if existing == body {
            return Ok(());
        }
        return Err(PackageError::OutputAlreadyExists {
            path: path.to_path_buf(),
        });
    }
    std::fs::write(path, body).map_err(|source| PackageError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const VALID_MANIFEST: &str = r#"[package]
name = "demo"
version = "0.1.0"

[target.demo]
type = "cpp_library"
sources = ["src/lib.cc"]
"#;

    fn write_package(root: &Path) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("cabin.toml"), VALID_MANIFEST).unwrap();
        std::fs::write(
            root.join("src").join("lib.cc"),
            "int demo() { return 0; }\n",
        )
        .unwrap();
    }

    #[test]
    fn stage_with_project_rejects_output_dir_equal_to_package_root() {
        let dir = TempDir::new().unwrap();
        write_package(dir.path());
        let err = stage_with_project(&dir.path().join("cabin.toml"), None, Some(dir.path()))
            .expect_err("output_dir == package_root must be rejected");
        match err {
            PackageError::OutputDirIsPackageRoot { .. } => {}
            other => panic!("expected OutputDirIsPackageRoot, got {other:?}"),
        }
    }

    #[test]
    fn stage_with_project_excludes_in_tree_output_dir_even_when_intermediate_missing() {
        // The walker compares the user-supplied output_dir against
        // each walked directory's absolute path, so the
        // normalization must produce a path comparable to the
        // canonical package root even when intermediate
        // directories do not exist on disk yet.
        let dir = TempDir::new().unwrap();
        write_package(dir.path());
        std::fs::write(dir.path().join("note.md"), "keep").unwrap();
        let output_dir = dir.path().join("missing-parent").join("output");
        // Ensure neither the parent nor the output dir exist.
        assert!(!output_dir.exists());
        let staged =
            stage_with_project(&dir.path().join("cabin.toml"), None, Some(&output_dir)).unwrap();
        // Round-trip the archive to confirm the file list and that
        // nothing under `missing-parent/` snuck in.
        let dec = flate2::read::GzDecoder::new(std::io::Cursor::new(staged.archive_bytes));
        let mut tar = tar::Archive::new(dec);
        let mut seen: Vec<String> = Vec::new();
        for entry in tar.entries().unwrap() {
            let entry = entry.unwrap();
            seen.push(entry.path().unwrap().to_string_lossy().into_owned());
        }
        assert!(seen.contains(&"cabin.toml".to_owned()));
        assert!(seen.contains(&"src/lib.cc".to_owned()));
        assert!(
            !seen.iter().any(|p| p.starts_with("missing-parent")),
            "missing-parent leaked: {seen:?}"
        );
    }

    #[test]
    fn stage_with_project_excludes_supplied_output_dir() {
        let dir = TempDir::new().unwrap();
        write_package(dir.path());
        let out = dir.path().join("custom-out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("stale.tar.gz"), b"old").unwrap();
        let staged = stage_with_project(&dir.path().join("cabin.toml"), None, Some(&out)).unwrap();
        let dec = flate2::read::GzDecoder::new(std::io::Cursor::new(staged.archive_bytes));
        let mut tar = tar::Archive::new(dec);
        for entry in tar.entries().unwrap() {
            let path = entry
                .unwrap()
                .path()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            assert!(
                !path.starts_with("custom-out"),
                "stale output leaked: {path}"
            );
        }
    }
}
