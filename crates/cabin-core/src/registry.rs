//! Shared file-registry `config.json` contract.
//!
//! The schema version, the `kind` discriminant, and the
//! relative-subdirectory safety rule for a file registry's
//! `config.json` live here so the readers (`cabin-index`,
//! `cabin-index-http`) and the writer (`cabin-registry-file`)
//! validate one identical contract instead of three drifting copies.
//! `cabin-core` carries no I/O — each crate keeps its own error type
//! and maps the boolean predicate into its own diagnostic.

use std::path::{Component, Path};

/// Supported `config.json` `schema` version.
pub const REGISTRY_CONFIG_SCHEMA: u32 = 1;

/// Required `config.json` `kind` discriminant for a file registry.
pub const REGISTRY_KIND: &str = "file-registry";

/// Whether `value` is a safe relative subdirectory for a registry
/// config field (`packages` / `artifacts`): non-empty, not absolute,
/// and composed only of normal path components (a leading / interior
/// `.` is tolerated). Rejects `..`, absolute paths, and OS root /
/// prefix components so a config cannot point outside the registry.
pub fn relative_subdir_is_safe(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let candidate = Path::new(value);
    if candidate.is_absolute() {
        return false;
    }
    candidate
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_relative_subdirs() {
        assert!(relative_subdir_is_safe("packages"));
        assert!(relative_subdir_is_safe("artifacts"));
        assert!(relative_subdir_is_safe("a/b"));
    }

    #[test]
    fn rejects_empty_absolute_and_traversal() {
        assert!(!relative_subdir_is_safe(""));
        assert!(!relative_subdir_is_safe("/abs"));
        assert!(!relative_subdir_is_safe("../escape"));
        assert!(!relative_subdir_is_safe("a/../b"));
    }
}
