//! Format-preserving editing of `cabin.toml` dependency tables.
//!
//! `cabin add` / `cabin remove` need to insert and delete
//! `[dependencies]` / `[dev-dependencies]` entries without disturbing
//! the rest of the manifest. The serde-based parser in the crate's
//! `parse` module is lossy (it discards comments, ordering, and
//! whitespace), so this module wraps [`toml_edit`]'s document model
//! instead, which round-trips formatting and comments.
//!
//! The schema written here — the `port` / `version` / `path` /
//! `features` / `default-features` keys — must stay in step with the
//! parsing schema in the `raw` and `parse::dependency` modules;
//! keeping both in the same crate is deliberate so a schema change
//! touches one place.

use thiserror::Error;
use toml_edit::{Array, Item, Table, Value};

pub use toml_edit::DocumentMut;

/// Errors returned while editing a manifest document.
#[derive(Debug, Error)]
pub enum EditError {
    /// The manifest text was not valid TOML.
    #[error("failed to parse manifest: {0}")]
    Parse(#[from] toml_edit::TomlError),
    /// A dependency table key was present but held a non-table value
    /// (e.g. `dependencies = 5`).
    #[error("`{table}` exists in the manifest but is not a table")]
    NotATable {
        /// The offending table header.
        table: &'static str,
    },
}

/// Which dependency table an edit targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepTable {
    /// `[dependencies]`.
    Normal,
    /// `[dev-dependencies]`.
    Dev,
}

impl DepTable {
    /// The TOML table header this kind maps to.
    #[must_use]
    pub fn header(self) -> &'static str {
        match self {
            DepTable::Normal => "dependencies",
            DepTable::Dev => "dev-dependencies",
        }
    }
}

/// A dependency to insert or update via [`upsert_dependency`].
///
/// The combination of fields determines the rendered form: a bare
/// version string (`name = "^1"`) when only [`version`] is set,
/// otherwise an inline table (`name = { port = true, version = "^1" }`).
///
/// [`version`]: NewDependency::version
#[derive(Debug, Clone, Default)]
pub struct NewDependency {
    /// Manifest key (the package name).
    pub name: String,
    /// Version requirement string (e.g. `^1.3.1`). `None` for a
    /// path-only entry.
    pub version: Option<String>,
    /// Emit `port = true` (foundation-port dependency).
    pub port: bool,
    /// Emit `path = "..."` (local path dependency).
    pub path: Option<String>,
    /// Features to enable on the dependency (`features = [...]`).
    pub features: Vec<String>,
    /// Emit `default-features = false` when `true`.
    pub no_default_features: bool,
}

/// Parse manifest text into an editable document.
///
/// # Errors
/// Returns [`EditError::Parse`] when `text` is not valid TOML.
pub fn parse_document(text: &str) -> Result<DocumentMut, EditError> {
    text.parse::<DocumentMut>().map_err(EditError::from)
}

/// Insert `dep` into the chosen table, or update it in place if a key
/// of the same name already exists.
///
/// When the table's existing keys are already sorted, the table is kept
/// sorted after the insert (matching `cargo add`); an
/// intentionally-unsorted table is left in its original order with the
/// new entry appended.
///
/// # Errors
/// Returns [`EditError::NotATable`] when the target table header is
/// present but does not hold a table.
pub fn upsert_dependency(
    doc: &mut DocumentMut,
    table: DepTable,
    dep: &NewDependency,
) -> Result<(), EditError> {
    let header = table.header();
    let created = doc.get(header).is_none();
    let item = doc
        .entry(header)
        .or_insert_with(|| Item::Table(Table::new()));
    let Some(tbl) = item.as_table_mut() else {
        return Err(EditError::NotATable { table: header });
    };
    if created {
        // A freshly created table renders its header flush against the
        // preceding line; a blank-line prefix matches the spacing cabin
        // scaffolds and cargo emits between tables.
        tbl.set_implicit(false);
        tbl.decor_mut().set_prefix("\n");
    }

    // Decide before inserting: a table that is already sorted stays
    // sorted, an unsorted one keeps its order with the entry appended.
    let was_sorted = is_sorted(tbl);
    tbl.insert(&dep.name, Item::Value(dependency_value(dep)));
    if was_sorted {
        tbl.sort_values();
    }
    Ok(())
}

/// Remove `name` from the chosen table.
///
/// Returns `true` when an entry was removed, `false` when it was
/// absent. If the table becomes empty, the table header itself is
/// removed too (matching `cargo remove`).
pub fn remove_dependency(doc: &mut DocumentMut, table: DepTable, name: &str) -> bool {
    let header = table.header();
    let Some(tbl) = doc.get_mut(header).and_then(Item::as_table_mut) else {
        return false;
    };
    let removed = tbl.remove(name).is_some();
    if removed && tbl.is_empty() {
        doc.remove(header);
    }
    removed
}

/// Build the rendered value (bare string or inline table) for `dep`.
fn dependency_value(dep: &NewDependency) -> Value {
    let needs_table =
        dep.port || dep.path.is_some() || !dep.features.is_empty() || dep.no_default_features;
    if !needs_table && let Some(version) = &dep.version {
        return Value::from(version.as_str());
    }

    let mut table = toml_edit::InlineTable::new();
    if dep.port {
        table.insert("port", Value::from(true));
    }
    if let Some(path) = &dep.path {
        table.insert("path", Value::from(path.as_str()));
    }
    if let Some(version) = &dep.version {
        table.insert("version", Value::from(version.as_str()));
    }
    if !dep.features.is_empty() {
        let mut features = Array::new();
        for feature in &dep.features {
            features.push(feature.as_str());
        }
        table.insert("features", Value::Array(features));
    }
    if dep.no_default_features {
        table.insert("default-features", Value::from(false));
    }
    Value::InlineTable(table)
}

/// Whether the table's keys are in non-decreasing order.
fn is_sorted(table: &Table) -> bool {
    let mut prev: Option<&str> = None;
    for (key, _) in table {
        if let Some(previous) = prev
            && key < previous
        {
            return false;
        }
        prev = Some(key);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKAGE_ONLY: &str = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n";

    fn port_dep(name: &str, version: &str) -> NewDependency {
        NewDependency {
            name: name.to_owned(),
            version: Some(version.to_owned()),
            port: true,
            ..NewDependency::default()
        }
    }

    #[test]
    fn upserts_port_dependency_as_inline_table() {
        let mut doc = parse_document(PACKAGE_ONLY).unwrap();
        upsert_dependency(&mut doc, DepTable::Normal, &port_dep("zlib", "^1.3.1")).unwrap();
        let out = doc.to_string();
        assert!(out.contains("[dependencies]"), "got:\n{out}");
        assert!(
            out.contains("zlib = { port = true, version = \"^1.3.1\" }"),
            "got:\n{out}"
        );
    }

    #[test]
    fn upserts_path_dependency_without_version() {
        let mut doc = parse_document(PACKAGE_ONLY).unwrap();
        let dep = NewDependency {
            name: "mylib".to_owned(),
            path: Some("../mylib".to_owned()),
            ..NewDependency::default()
        };
        upsert_dependency(&mut doc, DepTable::Normal, &dep).unwrap();
        let out = doc.to_string();
        assert!(
            out.contains("mylib = { path = \"../mylib\" }"),
            "got:\n{out}"
        );
    }

    #[test]
    fn upserts_features_and_no_default_features() {
        let mut doc = parse_document(PACKAGE_ONLY).unwrap();
        let dep = NewDependency {
            name: "fmt".to_owned(),
            version: Some("^10".to_owned()),
            port: true,
            features: vec!["a".to_owned(), "b".to_owned()],
            no_default_features: true,
            ..NewDependency::default()
        };
        upsert_dependency(&mut doc, DepTable::Normal, &dep).unwrap();
        let out = doc.to_string();
        assert!(
            out.contains(
                "fmt = { port = true, version = \"^10\", features = [\"a\", \"b\"], default-features = false }"
            ),
            "got:\n{out}"
        );
    }

    #[test]
    fn bare_version_when_only_version_is_set() {
        let mut doc = parse_document(PACKAGE_ONLY).unwrap();
        let dep = NewDependency {
            name: "foo".to_owned(),
            version: Some("^1".to_owned()),
            ..NewDependency::default()
        };
        upsert_dependency(&mut doc, DepTable::Normal, &dep).unwrap();
        assert!(doc.to_string().contains("foo = \"^1\""), "got:\n{doc}");
    }

    #[test]
    fn keeps_sorted_table_sorted() {
        let src = format!("{PACKAGE_ONLY}\n[dependencies]\nalpha = \"^1\"\ncharlie = \"^3\"\n");
        let mut doc = parse_document(&src).unwrap();
        upsert_dependency(&mut doc, DepTable::Normal, &port_dep("bravo", "^2")).unwrap();
        let out = doc.to_string();
        let a = out.find("alpha").unwrap();
        let b = out.find("bravo").unwrap();
        let c = out.find("charlie").unwrap();
        assert!(
            a < b && b < c,
            "expected alpha < bravo < charlie, got:\n{out}"
        );
    }

    #[test]
    fn appends_to_unsorted_table() {
        let src = format!("{PACKAGE_ONLY}\n[dependencies]\ncharlie = \"^3\"\nalpha = \"^1\"\n");
        let mut doc = parse_document(&src).unwrap();
        upsert_dependency(&mut doc, DepTable::Normal, &port_dep("bravo", "^2")).unwrap();
        let out = doc.to_string();
        let c = out.find("charlie").unwrap();
        let a = out.find("alpha").unwrap();
        let b = out.find("bravo").unwrap();
        assert!(
            c < a && a < b,
            "expected charlie < alpha < bravo, got:\n{out}"
        );
    }

    #[test]
    fn updates_existing_entry_in_place() {
        let src = format!(
            "{PACKAGE_ONLY}\n[dependencies]\nzlib = {{ port = true, version = \"^1.0\" }}\n"
        );
        let mut doc = parse_document(&src).unwrap();
        upsert_dependency(&mut doc, DepTable::Normal, &port_dep("zlib", "^2.0")).unwrap();
        let out = doc.to_string();
        assert!(out.contains("version = \"^2.0\""), "got:\n{out}");
        assert!(
            !out.contains("^1.0"),
            "old version should be gone, got:\n{out}"
        );
        assert_eq!(
            out.matches("zlib").count(),
            1,
            "exactly one zlib entry, got:\n{out}"
        );
    }

    #[test]
    fn preserves_surrounding_comments() {
        let src = format!("{PACKAGE_ONLY}\n[dependencies]\n# keep me\nalpha = \"^1\"\n");
        let mut doc = parse_document(&src).unwrap();
        upsert_dependency(&mut doc, DepTable::Normal, &port_dep("bravo", "^2")).unwrap();
        assert!(doc.to_string().contains("# keep me"), "got:\n{doc}");
    }

    #[test]
    fn inserts_into_dev_dependencies() {
        let mut doc = parse_document(PACKAGE_ONLY).unwrap();
        upsert_dependency(&mut doc, DepTable::Dev, &port_dep("gtest", "^1.14")).unwrap();
        assert!(
            doc.to_string().contains("[dev-dependencies]"),
            "got:\n{doc}"
        );
    }

    #[test]
    fn remove_existing_returns_true() {
        let src = format!("{PACKAGE_ONLY}\n[dependencies]\nzlib = \"^1\"\nfmt = \"^10\"\n");
        let mut doc = parse_document(&src).unwrap();
        assert!(remove_dependency(&mut doc, DepTable::Normal, "zlib"));
        let out = doc.to_string();
        assert!(!out.contains("zlib"), "got:\n{out}");
        assert!(out.contains("fmt"), "got:\n{out}");
    }

    #[test]
    fn remove_missing_returns_false() {
        let src = format!("{PACKAGE_ONLY}\n[dependencies]\nfmt = \"^10\"\n");
        let mut doc = parse_document(&src).unwrap();
        assert!(!remove_dependency(&mut doc, DepTable::Normal, "zlib"));
    }

    #[test]
    fn remove_last_entry_drops_empty_table() {
        let src = format!("{PACKAGE_ONLY}\n[dependencies]\nzlib = \"^1\"\n");
        let mut doc = parse_document(&src).unwrap();
        assert!(remove_dependency(&mut doc, DepTable::Normal, "zlib"));
        assert!(
            !doc.to_string().contains("[dependencies]"),
            "empty table should be removed, got:\n{doc}"
        );
    }

    #[test]
    fn remove_missing_table_returns_false() {
        let mut doc = parse_document(PACKAGE_ONLY).unwrap();
        assert!(!remove_dependency(&mut doc, DepTable::Dev, "anything"));
    }

    #[test]
    fn upsert_errors_when_table_key_is_not_a_table() {
        let mut doc =
            parse_document("dependencies = 5\n[package]\nname = \"demo\"\nversion = \"0.1.0\"\n")
                .unwrap();
        let err =
            upsert_dependency(&mut doc, DepTable::Normal, &port_dep("zlib", "^1")).unwrap_err();
        assert!(
            matches!(
                err,
                EditError::NotATable {
                    table: "dependencies"
                }
            ),
            "expected NotATable, got {err:?}"
        );
    }
}
