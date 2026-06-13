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
//!   fetch artifacts, or invoke C/C++ compilers;
//! - it must not implement networking, server-side functionality, or
//!   publishing — `cabin-publish` orchestrates the dry-run flow on top
//!   of this crate;
//! - the archive format is intentionally narrow: `tar.gz` only,
//!   regular files and directories only, deterministic byte-for-byte
//!   for the same logical input.

pub mod archive;
pub mod error;
pub mod metadata;
pub mod scaffold;
pub mod validate;

use std::path::{Path, PathBuf};

use cabin_core::PackageName;
use cabin_fs::write_atomic;

pub use error::PackageError;
pub use metadata::{PackageMetadata, SourceMetadata};

/// Inputs to [`package_with_project`].
#[derive(Debug, Clone, Copy)]
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
    /// Canonical per-version metadata document, ready to serialize.
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
/// it from the archive so a previous run's artifacts living in an
/// in-tree output directory cannot leak back in. Passing `None`
/// disables the exclusion (used by `cabin-publish`, which never
/// writes the archive back into the source tree).
///
/// `workspace_dep_requirements` carries the workspace root's raw
/// `[workspace.<kind>-dependencies]` strings so dependency
/// `{ workspace = true }` markers in the on-disk manifest can be
/// rewritten to the author's original requirement spelling.
/// Standalone callers pass the empty default.
///
/// # Errors
/// Returns [`PackageError::OutputDirIsPackageRoot`] when `output_dir`
/// resolves to the package root, and propagates every
/// [`PackageError`] from validation
/// ([`validate::load_and_validate_with_project`]), source-tree
/// enumeration ([`archive::collect_package_files`],
/// [`archive::ensure_manifest_included`]), and archive construction
/// ([`archive::build_tar_gz`]). When the on-disk manifest carries
/// `{ workspace = true }` markers — standard fields or dependency
/// entries — rewriting them into the resolved literals yields
/// [`PackageError::Io`] if the manifest cannot be re-read,
/// [`PackageError::ManifestNormalization`] if the rewrite fails
/// (including a dependency marker with no matching entry in
/// `workspace_dep_requirements`), and
/// [`PackageError::ManifestNormalizationIncomplete`] if the rewrite
/// finds nothing to substitute.
pub fn stage_with_project(
    manifest_path: &Path,
    project_override: Option<cabin_core::Package>,
    output_dir: Option<&Path>,
    workspace_dep_requirements: &cabin_core::WorkspaceDepRequirements,
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

    // Normalize `{ workspace = true }` markers — standard fields and
    // dependency entries — into the resolved literals so the archived
    // manifest is self-contained (the registry build honors the
    // extracted manifest).
    let manifest_substitute = if validated.manifest_has_workspace_markers {
        let text = std::fs::read_to_string(&validated.manifest_path).map_err(|source| {
            PackageError::Io {
                path: validated.manifest_path.clone(),
                source,
            }
        })?;
        let rewritten = cabin_manifest::edit::normalize_workspace_markers(
            &text,
            &validated.package.language,
            workspace_dep_requirements,
        )
        .map_err(|source| PackageError::ManifestNormalization {
            path: validated.manifest_path.clone(),
            source: Box::new(source),
        })?;
        // Validation has already proven the effective package
        // marker-free, so a rewrite that found nothing means the
        // substituter missed a marker spelling the parser accepted;
        // fail loudly rather than archive a live marker.
        let Some(rewritten) = rewritten else {
            return Err(PackageError::ManifestNormalizationIncomplete {
                path: validated.manifest_path.clone(),
            });
        };
        Some(rewritten.into_bytes())
    } else {
        None
    };

    let archive_bytes = archive::build_tar_gz(&files, manifest_substitute.as_deref())?;
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
/// walker emits under the canonicalized package root.
///
/// The walker descends from a canonicalized `package_root` and
/// produces entries via `Path::join`, so a comparable path needs
/// the same canonical form. `output_dir` may not exist yet (it is
/// created by `package_with_project` after staging completes), so
/// `Path::canonicalize` would fail outright. Walk up to the
/// closest existing ancestor, canonicalize that, then reattach the
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
/// `workspace_dep_requirements` carries the workspace root's raw
/// requirement strings for the archived-manifest rewrite — see
/// [`stage_with_project`]. Standalone callers pass the empty
/// default.
///
/// # Errors
/// Propagates every [`PackageError`] from [`stage_with_project`]
/// (including [`PackageError::ManifestNormalization`] when a
/// dependency marker has no matching entry in
/// `workspace_dep_requirements`) and metadata rendering
/// ([`metadata::render_canonical_json`], which yields
/// [`PackageError::Metadata`]). Returns [`PackageError::Io`]
/// when `output_dir` cannot be created or written, and
/// [`PackageError::OutputAlreadyExists`] when the target archive or
/// metadata file already exists with different bytes.
pub fn package_with_project(
    request: PackageRequest<'_>,
    project_override: Option<cabin_core::Package>,
    workspace_dep_requirements: &cabin_core::WorkspaceDepRequirements,
) -> Result<PackagedArtifact, PackageError> {
    let staged = stage_with_project(
        request.manifest_path,
        project_override,
        Some(request.output_dir),
        workspace_dep_requirements,
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
///
/// The write itself is atomic: bytes land in a sibling temporary
/// file and only rename onto `path` after a successful write, so an
/// interrupted run leaves the previous file (if any) in place. The
/// existence-and-equality check stays in front of the atomic write
/// so the "refuse to overwrite mismatched output" guarantee is not
/// lost.
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
    write_atomic(path, body).map_err(|source| PackageError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    const VALID_MANIFEST: &str = r#"[package]
name = "demo"
version = "0.1.0"

[target.demo]
type = "library"
sources = ["src/lib.cc"]
"#;

    fn write_package(dir: &TempDir) {
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/lib.cc")
            .write_str("int demo() { return 0; }\n")
            .unwrap();
    }

    /// Extract the archived `cabin.toml` text from a staged package.
    fn archived_manifest_text(staged: &StagedPackage) -> String {
        let gz = flate2::read::GzDecoder::new(std::io::Cursor::new(&staged.archive_bytes));
        let mut tar = tar::Archive::new(gz);
        let mut manifest_text = String::new();
        for entry in tar.entries().unwrap() {
            let mut entry = entry.unwrap();
            if entry.path().unwrap().to_str() == Some("cabin.toml") {
                use std::io::Read;
                entry.read_to_string(&mut manifest_text).unwrap();
            }
        }
        manifest_text
    }

    /// Dep-marker fixture: a manifest whose `fmt` dependency carries
    /// a `{ workspace = true }` marker.
    fn write_dep_marker_package(dir: &TempDir) {
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[target.demo]
type = "executable"
sources = ["src/main.cc"]

[dependencies]
fmt = { workspace = true, features = ["color"] }
"#,
            )
            .unwrap();
        dir.child("src/main.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();
    }

    /// The loader-shaped override for [`write_dep_marker_package`]:
    /// the marker resolved to a Version source.
    fn dep_marker_override(dir: &TempDir) -> cabin_core::Package {
        let parsed = cabin_manifest::load_manifest(dir.path().join("cabin.toml")).unwrap();
        let mut package = parsed.package.unwrap();
        package.dependencies[0].source =
            cabin_core::DependencySource::Version(semver::VersionReq::parse(">=10, <11").unwrap());
        package
    }

    #[test]
    fn stage_with_project_rejects_output_dir_equal_to_package_root() {
        let dir = TempDir::new().unwrap();
        write_package(&dir);
        let err = stage_with_project(
            &dir.path().join("cabin.toml"),
            None,
            Some(dir.path()),
            &cabin_core::WorkspaceDepRequirements::default(),
        )
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
        write_package(&dir);
        dir.child("note.md").write_str("keep").unwrap();
        let output_dir = dir.child("missing-parent/output");
        // Ensure neither the parent nor the output dir exist.
        assert!(!output_dir.path().exists());
        let staged = stage_with_project(
            &dir.path().join("cabin.toml"),
            None,
            Some(output_dir.path()),
            &cabin_core::WorkspaceDepRequirements::default(),
        )
        .unwrap();
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
    fn standalone_package_with_standard_marker_is_rejected() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = { workspace = true }

[target.demo]
type = "executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("src/main.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();
        let err = stage_with_project(
            &dir.path().join("cabin.toml"),
            None,
            None,
            &cabin_core::WorkspaceDepRequirements::default(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            PackageError::UnresolvedWorkspaceStandard {
                field: "cxx-standard"
            }
        ));
    }

    #[test]
    fn staged_archive_manifest_is_normalized_when_markers_are_resolved() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = { workspace = true }

[target.demo]
type = "executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("src/main.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();
        // Simulate what `cabin-workspace` hands the CLI: the same
        // package with the marker resolved to an inherited value.
        let parsed = cabin_manifest::load_manifest(dir.path().join("cabin.toml")).unwrap();
        let mut package = parsed.package.unwrap();
        package.language.cxx_standard = Some(cabin_core::StandardDeclaration::Inherited(
            cabin_core::CxxStandard::Cxx20,
        ));
        let staged = stage_with_project(
            &dir.path().join("cabin.toml"),
            Some(package),
            None,
            &cabin_core::WorkspaceDepRequirements::default(),
        )
        .unwrap();

        // Extract the staged tar.gz and check the manifest entry.
        let manifest_text = archived_manifest_text(&staged);
        assert!(
            manifest_text.contains("cxx-standard = \"c++20\""),
            "got: {manifest_text}"
        );
        assert!(
            !manifest_text.contains("workspace = true"),
            "got: {manifest_text}"
        );
        // The baked canonical metadata matches.
        assert_eq!(
            staged.metadata.language.cxx_standard,
            Some(cabin_core::StandardDeclaration::Inherited(
                cabin_core::CxxStandard::Cxx20
            ))
        );
    }

    #[test]
    fn stage_fails_when_override_omits_a_marked_standard() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
c-standard = { workspace = true }

[target.demo]
type = "executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("src/main.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();
        // The override passes validation (it carries no marker), but
        // resolves nothing for the marked on-disk field, so the
        // manifest rewrite cannot substitute a literal.
        let parsed = cabin_manifest::load_manifest(dir.path().join("cabin.toml")).unwrap();
        let mut package = parsed.package.unwrap();
        package.language.c_standard = None;
        let err = stage_with_project(
            &dir.path().join("cabin.toml"),
            Some(package),
            None,
            &cabin_core::WorkspaceDepRequirements::default(),
        )
        .unwrap_err();
        assert!(matches!(err, PackageError::ManifestNormalization { .. }));
    }

    #[test]
    fn staged_archive_normalizes_every_marker_and_restages_byte_identically() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = { workspace = true }
interface-cxx-standard = { workspace = true }

[target.demo]
type = "executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("src/main.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();
        // Simulate what `cabin-workspace` hands the CLI: the same
        // package with both markers resolved to inherited values.
        let resolved_override = || {
            let parsed = cabin_manifest::load_manifest(dir.path().join("cabin.toml")).unwrap();
            let mut package = parsed.package.unwrap();
            package.language.cxx_standard = Some(cabin_core::StandardDeclaration::Inherited(
                cabin_core::CxxStandard::Cxx20,
            ));
            package.language.interface_cxx_standard = Some(
                cabin_core::StandardDeclaration::Inherited(cabin_core::CxxStandard::Cxx17),
            );
            package
        };
        let staged = stage_with_project(
            &dir.path().join("cabin.toml"),
            Some(resolved_override()),
            None,
            &cabin_core::WorkspaceDepRequirements::default(),
        )
        .unwrap();

        // Extract the staged tar.gz and check the manifest entry.
        let manifest_text = archived_manifest_text(&staged);
        assert!(
            manifest_text.contains("cxx-standard = \"c++20\""),
            "got: {manifest_text}"
        );
        assert!(
            manifest_text.contains("interface-cxx-standard = \"c++17\""),
            "got: {manifest_text}"
        );
        assert!(
            !manifest_text.contains("workspace = true"),
            "got: {manifest_text}"
        );
        // Re-staging with a freshly-parsed identical override stays
        // byte-deterministic.
        let restaged = stage_with_project(
            &dir.path().join("cabin.toml"),
            Some(resolved_override()),
            None,
            &cabin_core::WorkspaceDepRequirements::default(),
        )
        .unwrap();
        assert_eq!(staged.archive_bytes, restaged.archive_bytes);
    }

    #[test]
    fn staged_archive_normalizes_workspace_dep_markers() {
        let dir = TempDir::new().unwrap();
        write_dep_marker_package(&dir);
        let mut reqs = cabin_core::WorkspaceDepRequirements::default();
        reqs.insert(
            cabin_core::DependencyKind::Normal,
            "fmt".to_owned(),
            ">=10 <11".to_owned(),
        );
        let staged = stage_with_project(
            &dir.path().join("cabin.toml"),
            Some(dep_marker_override(&dir)),
            None,
            &reqs,
        )
        .unwrap();
        let manifest_text = archived_manifest_text(&staged);
        assert!(
            manifest_text.contains("fmt = { version = \">=10 <11\", features = [\"color\"] }"),
            "got: {manifest_text}"
        );
        assert!(!manifest_text.contains("workspace"), "got: {manifest_text}");

        // Deterministic re-stage.
        let restaged = stage_with_project(
            &dir.path().join("cabin.toml"),
            Some(dep_marker_override(&dir)),
            None,
            &reqs,
        )
        .unwrap();
        assert_eq!(staged.archive_bytes, restaged.archive_bytes);
    }

    #[test]
    fn staging_dep_marker_without_requirements_errors() {
        // Same fixture + resolved override, but EMPTY requirements:
        // the normalizer cannot rewrite the on-disk marker.
        let dir = TempDir::new().unwrap();
        write_dep_marker_package(&dir);
        let err = stage_with_project(
            &dir.path().join("cabin.toml"),
            Some(dep_marker_override(&dir)),
            None,
            &cabin_core::WorkspaceDepRequirements::default(),
        )
        .unwrap_err();
        match err {
            PackageError::ManifestNormalization { source, .. } => {
                assert!(matches!(
                    *source,
                    cabin_manifest::edit::EditError::MissingWorkspaceDependency { ref name }
                        if name == "fmt"
                ));
            }
            other => panic!("expected ManifestNormalization, got {other:?}"),
        }
    }

    #[test]
    fn stage_with_project_excludes_supplied_output_dir() {
        let dir = TempDir::new().unwrap();
        write_package(&dir);
        let out = dir.child("custom-out");
        out.child("stale.tar.gz").write_binary(b"old").unwrap();
        let staged = stage_with_project(
            &dir.path().join("cabin.toml"),
            None,
            Some(out.path()),
            &cabin_core::WorkspaceDepRequirements::default(),
        )
        .unwrap();
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
