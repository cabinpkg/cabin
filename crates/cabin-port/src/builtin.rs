//! Foundation-port recipes shipped inside the cabin binary.
//!
//! The `BUILTIN` table is generated at compile time by `build.rs`,
//! which scans the repository's `ports/` directory and embeds the
//! `port.toml` and overlay `cabin.toml` of every
//! `ports/<name>/<version>/` recipe via `include_str!`. Adding or
//! removing a recipe directory therefore bundles or retires that
//! port automatically — there is nothing to edit in this file.
//!
//! The on-disk recipe stays the source of truth: the embedded text
//! is just `include_str!` of the same files, and each entry's `name`
//! and `version` come from the `<name>`/`<version>` directory names.
//! The module maintains a triple-source-of-truth invariant: for every
//! entry the on-disk directory names, the `BuiltinPort` fields, and
//! the `[port].name`/`[port].version` parsed from the embedded
//! `port.toml` must all agree. A unit test
//! (`dir_name_matches_port_toml_and_builtin_fields`) asserts this so
//! the sources cannot drift.

use semver::{Version, VersionReq};

/// One bundled foundation-port recipe.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinPort {
    /// Package name the recipe identifies. Matches the
    /// `[port].name` in the embedded `port.toml`. Used as the
    /// lookup key in `lookup`.
    pub name: &'static str,
    /// `SemVer` version string. Equal to the parent directory of
    /// the embedded recipe (e.g. `ports/<name>/<version>/`) and
    /// to `port_toml`'s `[port].version`. Pinned by a unit test
    /// in this module so the three sources of truth can't drift.
    pub version: &'static str,
    /// Embedded contents of `ports/<name>/<version>/port.toml`.
    pub port_toml: &'static str,
    /// Embedded contents of `ports/<name>/<version>/cabin.toml` (overlay).
    pub overlay_toml: &'static str,
}

// Curated set of recipes embedded in the `cabin` binary, generated at
// compile time from the `ports/` directory by `build.rs` and sorted by
// `(name, version)` so `iter()` is deterministic. Defines
// `const BUILTIN: &[BuiltinPort]`.
include!(concat!(env!("OUT_DIR"), "/builtin_generated.rs"));

/// Resolve a bundled recipe by name + version requirement.
/// Returns the highest-versioned entry whose `version` parses
/// and satisfies `req`. Returns `None` when no entry matches.
pub fn lookup(name: &str, req: &VersionReq) -> Option<&'static BuiltinPort> {
    BUILTIN
        .iter()
        .filter(|p| p.name == name)
        .filter_map(|p| Version::parse(p.version).ok().map(|v| (p, v)))
        .filter(|(_, v)| req.matches(v))
        .max_by(|(_, a), (_, b)| a.cmp(b))
        .map(|(p, _)| p)
}

/// Iterate the bundled recipes in `name` order.
pub fn iter() -> impl Iterator<Item = &'static BuiltinPort> {
    BUILTIN.iter()
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::VersionReq;
    use std::path::Path;

    fn any() -> VersionReq {
        VersionReq::parse(">=0").unwrap()
    }

    /// On-disk `ports/` directory, resolved the same way `build.rs`
    /// does: the recipes are committed crate-local (the repo-root
    /// `ports/` is a symlink to this directory), so they live at
    /// `CARGO_MANIFEST_DIR/ports` in both a workspace checkout and an
    /// unpacked published crate. Keeps the drift-check tests working
    /// from the packaged crate, where there is no repository root above
    /// `CARGO_MANIFEST_DIR`.
    fn ports_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("ports")
    }

    #[test]
    fn lookup_returns_zlib_recipe_for_matching_req() {
        let entry = lookup("zlib", &VersionReq::parse("^1.3").unwrap()).expect("zlib bundled");
        assert_eq!(entry.name, "zlib");
        assert_eq!(entry.version, "1.3.1");
    }

    #[test]
    fn lookup_returns_none_for_unmatched_req() {
        assert!(lookup("zlib", &VersionReq::parse("^2").unwrap()).is_none());
    }

    #[test]
    fn lookup_returns_none_for_unknown_name() {
        assert!(lookup("zilb", &any()).is_none());
    }

    #[test]
    fn lookup_with_permissive_req_returns_only_entry() {
        let entry = lookup("zlib", &any()).expect("zlib bundled");
        assert_eq!(entry.version, "1.3.1");
    }

    #[test]
    fn embedded_port_toml_parses() {
        let entry = lookup("zlib", &any()).unwrap();
        let descriptor =
            crate::parse_port_str(entry.port_toml, Path::new("<builtin:zlib>/port.toml"))
                .expect("embedded port.toml parses");
        assert_eq!(descriptor.name.as_str(), "zlib");
        assert_eq!(descriptor.version.to_string(), "1.3.1");
    }

    #[test]
    fn dir_name_matches_port_toml_and_builtin_fields() {
        // Triple-source invariant: every BUILTIN entry's name/version must
        // equal both the on-disk directory names AND the [port].name /
        // [port].version parsed out of the embedded port.toml.
        let ports = ports_dir();
        for entry in iter() {
            let port_toml_path = ports.join(entry.name).join(entry.version).join("port.toml");
            let port_toml_on_disk = std::fs::read_to_string(&port_toml_path)
                .unwrap_or_else(|e| panic!("missing recipe at {port_toml_path:?}: {e}"));
            assert_eq!(
                entry.port_toml, port_toml_on_disk,
                "embedded port.toml drifted from on-disk for {} {}",
                entry.name, entry.version
            );
            let descriptor = crate::parse_port_str(entry.port_toml, &port_toml_path).unwrap();
            assert_eq!(
                descriptor.name.as_str(),
                entry.name,
                "[port].name disagrees with BUILTIN entry / directory for {}",
                entry.name
            );
            assert_eq!(
                descriptor.version.to_string(),
                entry.version,
                "[port].version disagrees with BUILTIN entry for {}",
                entry.name
            );
        }
    }

    #[test]
    fn embedded_overlay_matches_on_disk() {
        let ports = ports_dir();
        for entry in iter() {
            let overlay_path = ports
                .join(entry.name)
                .join(entry.version)
                .join("cabin.toml");
            let on_disk = std::fs::read_to_string(&overlay_path)
                .unwrap_or_else(|e| panic!("missing overlay at {overlay_path:?}: {e}"));
            assert_eq!(
                entry.overlay_toml, on_disk,
                "embedded cabin.toml drifted from on-disk for {} {}",
                entry.name, entry.version
            );
        }
    }

    #[test]
    fn iter_yields_zlib() {
        let names: Vec<_> = iter().map(|p| p.name).collect();
        assert!(names.contains(&"zlib"), "got: {names:?}");
    }
}
