//! Foundation-port recipes shipped inside the cabin binary.
//!
//! Each entry embeds the `port.toml` and overlay `cabin.toml`
//! from `ports/<name>/<version>/` at compile time via `include_str!`.
//! Retiring a bundled port means dropping its entry here in the
//! same release that removes the `ports/<name>/<version>/` directory.
//!
//! The on-disk recipe stays the source of truth: the embedded
//! text is just `include_str!` of the same files, and the tests
//! at the bottom of this module assert the two stay in sync.

/// One bundled foundation-port recipe.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinPort {
    /// Package name the recipe identifies. Matches the
    /// `[port].name` in the embedded `port.toml`. Used as the
    /// lookup key in `lookup`.
    pub name: &'static str,
    /// Embedded contents of `ports/<name>/<version>/port.toml`.
    pub port_toml: &'static str,
    /// Embedded contents of `ports/<name>/<version>/cabin.toml` (overlay).
    pub overlay_toml: &'static str,
}

const ZLIB_PORT_TOML: &str = include_str!("../../../ports/zlib/1.3.1/port.toml");
const ZLIB_OVERLAY_TOML: &str = include_str!("../../../ports/zlib/1.3.1/cabin.toml");

/// Curated set of recipes embedded in the `cabin` binary.
/// Sorted by `name` so `iter()` is deterministic.
const BUILTIN: &[BuiltinPort] = &[BuiltinPort {
    name: "zlib",
    port_toml: ZLIB_PORT_TOML,
    overlay_toml: ZLIB_OVERLAY_TOML,
}];

/// Look up a bundled recipe by package name.
pub fn lookup(name: &str) -> Option<&'static BuiltinPort> {
    BUILTIN.iter().find(|p| p.name == name)
}

/// Iterate the bundled recipes in `name` order.
pub fn iter() -> impl Iterator<Item = &'static BuiltinPort> {
    BUILTIN.iter()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn lookup_returns_zlib_recipe() {
        let entry = lookup("zlib").expect("zlib is bundled");
        assert_eq!(entry.name, "zlib");
        assert!(entry.port_toml.contains("name = \"zlib\""));
        assert!(entry.overlay_toml.contains("[package]"));
    }

    #[test]
    fn lookup_returns_none_for_unknown_name() {
        assert!(lookup("zilb").is_none());
    }

    #[test]
    fn embedded_port_toml_parses() {
        let entry = lookup("zlib").unwrap();
        let descriptor = crate::parse_port_str(
            entry.port_toml,
            Path::new("<builtin:zlib>/port.toml"),
        )
        .expect("embedded port.toml parses");
        assert_eq!(descriptor.name.as_str(), "zlib");
    }

    #[test]
    fn embedded_recipe_matches_on_disk() {
        // Catches the case where a contributor edits ports/zlib/1.3.1/
        // and does not rebuild cabin: the embedded text would be
        // stale. cargo tracks include_str! dependencies, so this
        // never happens in practice — the test pins the
        // invariant anyway.
        let entry = lookup("zlib").unwrap();
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent() // crates/
            .unwrap()
            .parent() // workspace root
            .unwrap();
        let port_toml_on_disk =
            std::fs::read_to_string(workspace.join("ports/zlib/1.3.1/port.toml"))
                .expect("ports/zlib/1.3.1/port.toml readable");
        let overlay_on_disk =
            std::fs::read_to_string(workspace.join("ports/zlib/1.3.1/cabin.toml"))
                .expect("ports/zlib/1.3.1/cabin.toml readable");
        assert_eq!(entry.port_toml, port_toml_on_disk.as_str());
        assert_eq!(entry.overlay_toml, overlay_on_disk.as_str());
    }

    #[test]
    fn iter_yields_zlib() {
        let names: Vec<_> = iter().map(|p| p.name).collect();
        assert!(names.contains(&"zlib"), "got: {names:?}");
    }
}
