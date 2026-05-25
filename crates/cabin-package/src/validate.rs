use std::path::{Component, Path, PathBuf};

use cabin_core::{DependencySource, Package};

use crate::error::PackageError;

/// Result of validating a package manifest plus its source-tree
/// metadata. Every consumer in `cabin-package` works against this
/// shape.
#[derive(Debug, Clone)]
pub struct ValidatedPackage {
    pub package: Package,
    /// Canonical absolute path to the package's `cabin.toml`.
    pub manifest_path: PathBuf,
    /// Canonical absolute path to the directory containing
    /// `manifest_path`. Used as the package root for archive
    /// enumeration.
    pub package_root: PathBuf,
}

/// Load a package manifest from `manifest_path` and run every
/// pre-archive validation. The optional `project_override` lets the
/// CLI pass a `Package` whose `DependencySource::Workspace` entries
/// have already been resolved by `cabin-workspace`; standalone
/// invocations leave it `None` and trigger the workspace-dep error
/// Path so a workspace-rooted dep is never silently dropped from
/// the package metadata.
///
/// Validation rules:
///
/// - the manifest must contain a `[package]` table (workspace-only
///   Roots are rejected);
/// - the package name must be safe for registry publishing
///   (`/`, `\\`, `..`, leading dots, and platform path prefixes are
///   Rejected);
/// - target source paths and include directories must not escape
///   The package root;
/// - declared dependencies must not include path entries (path
///   Dependencies are not publishable);
/// - declared dependencies must not include unresolved
///   `{ workspace = true }` entries.
pub fn load_and_validate(manifest_path: &Path) -> Result<ValidatedPackage, PackageError> {
    load_and_validate_with_project(manifest_path, None)
}

/// Variant of [`load_and_validate`] that accepts a pre-resolved
/// `Package`. The CLI uses it to inject a `Package` whose
/// `workspace = true` deps have been substituted by
/// `cabin-workspace::load_workspace`. If `project_override` is
/// `Some`, the on-disk manifest is still parsed (we keep the
/// canonical manifest path) but the override drives validation and
/// metadata generation.
pub fn load_and_validate_with_project(
    manifest_path: &Path,
    project_override: Option<cabin_core::Package>,
) -> Result<ValidatedPackage, PackageError> {
    let parsed =
        cabin_manifest::load_manifest(manifest_path).map_err(|source| PackageError::Manifest {
            path: manifest_path.to_path_buf(),
            source: Box::new(source),
        })?;
    let package = match project_override {
        Some(p) => p,
        None => parsed
            .package
            .ok_or(PackageError::WorkspaceRootHasNoProject)?,
    };

    let manifest_path =
        std::fs::canonicalize(manifest_path).map_err(|source| PackageError::Io {
            path: manifest_path.to_path_buf(),
            source,
        })?;
    let package_root = manifest_path
        .parent()
        .ok_or_else(|| PackageError::ManifestPathHasNoParent {
            path: manifest_path.clone(),
        })?
        .to_path_buf();

    // package names must be safe to use as
    // registry filesystem paths. The shared predicate now lives
    // in `cabin-core` so this validator, the file-registry
    // publisher, and the sparse HTTP fetcher cannot drift on the
    // rule.
    if !cabin_core::is_path_safe_package_name(package.name.as_str()) {
        return Err(PackageError::UnsafeRegistryPackageName {
            name: package.name.as_str().to_owned(),
        });
    }

    // Patches are local development policy. Including a `[patch]`
    // table in a published archive would silently leak local
    // override state into every consumer, so we reject the
    // package step before any bytes are written.
    if !package.patches.is_empty() {
        return Err(PackageError::PatchTableNotPublishable {
            name: package.name.as_str().to_owned(),
        });
    }

    for dep in &package.dependencies {
        match &dep.source {
            DependencySource::Path(_) => {
                return Err(PackageError::PathDependencyNotPublishable {
                    name: dep.name.as_str().to_owned(),
                });
            }
            DependencySource::Port(_) => {
                return Err(PackageError::PortDependencyNotPublishable {
                    name: dep.name.as_str().to_owned(),
                });
            }
            DependencySource::Workspace => {
                return Err(PackageError::UnresolvedWorkspaceDependency {
                    name: dep.name.as_str().to_owned(),
                });
            }
            DependencySource::Version(_) => {}
        }
    }

    for target in &package.targets {
        for source in &target.sources {
            ensure_within_root(&package_root, source).map_err(|path| {
                PackageError::SourceEscapesPackageRoot {
                    target: target.name.as_str().to_owned(),
                    path,
                }
            })?;
        }
        for include in &target.include_dirs {
            ensure_within_root(&package_root, include).map_err(|path| {
                PackageError::IncludeEscapesPackageRoot {
                    target: target.name.as_str().to_owned(),
                    path,
                }
            })?;
        }
    }

    Ok(ValidatedPackage {
        package,
        manifest_path,
        package_root,
    })
}

/// Re-export the shared `cabin-core` predicate so
/// existing callers that already pulled `cabin_package::is_path_safe_package_name`
/// Keep compiling. New code should call `cabin_core::is_path_safe_package_name`
/// Directly.
pub use cabin_core::is_path_safe_package_name;

/// Verify that a manifest-relative path stays inside the package
/// root, *lexically*. Symlinks and other filesystem trickery are
/// caught later, during archive enumeration.
fn ensure_within_root(_root: &Path, candidate: &Path) -> Result<(), PathBuf> {
    if candidate.is_absolute() {
        return Err(candidate.to_path_buf());
    }
    let mut depth: i32 = 0;
    for component in candidate.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(candidate.to_path_buf());
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(candidate.to_path_buf());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    #[test]
    fn accepts_simple_package() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str("[package]\nname = \"fmt\"\nversion = \"10.2.1\"\n")
            .unwrap();
        let validated = load_and_validate(&dir.path().join("cabin.toml")).unwrap();
        assert_eq!(validated.package.name.as_str(), "fmt");
    }

    #[test]
    fn rejects_workspace_root_without_project() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/*"]
"#,
            )
            .unwrap();
        let err = load_and_validate(&dir.path().join("cabin.toml")).unwrap_err();
        assert!(matches!(err, PackageError::WorkspaceRootHasNoProject));
    }

    #[test]
    fn rejects_path_dependency() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
local = { path = "../local" }
"#,
            )
            .unwrap();
        let err = load_and_validate(&dir.path().join("cabin.toml")).unwrap_err();
        match err {
            PackageError::PathDependencyNotPublishable { name } => assert_eq!(name, "local"),
            other => panic!("expected PathDependencyNotPublishable, got {other:?}"),
        }
    }

    #[test]
    fn accepts_versioned_dependency() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
            )
            .unwrap();
        let validated = load_and_validate(&dir.path().join("cabin.toml")).unwrap();
        assert_eq!(validated.package.dependencies.len(), 1);
    }

    #[test]
    fn rejects_target_source_outside_root() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[target.app]
type = "cpp_executable"
sources = ["../outside.cc"]
"#,
            )
            .unwrap();
        let err = load_and_validate(&dir.path().join("cabin.toml")).unwrap_err();
        assert!(matches!(err, PackageError::SourceEscapesPackageRoot { .. }));
    }

    #[test]
    fn rejects_include_dir_outside_root() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[target.app]
type = "cpp_library"
sources = ["src/app.cc"]
include_dirs = ["../include"]
"#,
            )
            .unwrap();
        let err = load_and_validate(&dir.path().join("cabin.toml")).unwrap_err();
        assert!(matches!(
            err,
            PackageError::IncludeEscapesPackageRoot { .. }
        ));
    }

    #[test]
    fn rejects_absolute_target_source() {
        let dir = TempDir::new().unwrap();
        let abs = if cfg!(windows) {
            "C:/abs/main.cc"
        } else {
            "/abs/main.cc"
        };
        dir.child("cabin.toml")
            .write_str(&format!(
                r#"[package]
name = "app"
version = "0.1.0"

[target.app]
type = "cpp_executable"
sources = ["{abs}"]
"#
            ))
            .unwrap();
        let err = load_and_validate(&dir.path().join("cabin.toml")).unwrap_err();
        assert!(matches!(err, PackageError::SourceEscapesPackageRoot { .. }));
    }
}
