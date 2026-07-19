//! Format-preserving editing of `cabin.toml` dependency tables.
//!
//! `cabin add` / `cabin remove` need to insert and delete
//! `[dependencies]` / `[dev-dependencies]` entries without disturbing
//! the rest of the manifest.  The serde-based parser in the crate's
//! `parse` module is lossy (it discards comments, ordering, and
//! whitespace), so this module wraps [`toml_edit`]'s document model
//! instead, which round-trips formatting and comments.
//!
//! The schema written here - the `port` / `version` / `path` /
//! `features` / `default-features` keys - must stay in step with the
//! parsing schema in the `raw` and `parse::dependency` modules;
//! keeping both in the same crate is deliberate so a schema change
//! touches one place.  The workspace-marker rewrite
//! ([`normalize_workspace_markers`]) lives here for the same reason:
//! the `{ workspace = true }` marker shapes it rewrites must stay in
//! step with the marker shapes the parser accepts.

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
    /// A `{ workspace = true }` standard marker had no resolved
    /// value to substitute.  Indicates the package was staged
    /// without being loaded through `cabin-workspace`.
    #[error("`{field}` uses `workspace = true` but no resolved workspace value was provided")]
    MissingResolvedStandard {
        /// The marker-bearing `[package]` field.
        field: &'static str,
    },
    /// A `{ workspace = true }` dependency marker had no matching
    /// requirement in the workspace tables.  Indicates the package
    /// was staged without the workspace root's
    /// `[workspace.<kind>-dependencies]` strings.
    #[error(
        "dependency `{name}` uses `workspace = true` but no workspace requirement was provided"
    )]
    MissingWorkspaceDependency {
        /// The marker-bearing dependency name.
        name: String,
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
    /// Version requirement string (e.g. `^1.3.1`).  `None` for a
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
    let was_sorted = tbl.iter().is_sorted_by_key(|(key, _)| key);
    tbl.insert(&dep.name, Item::Value(dependency_value(dep)));
    if was_sorted {
        tbl.sort_values();
    }
    Ok(())
}

/// Remove `name` from the chosen table.
///
/// Returns `true` when an entry was removed, `false` when it was
/// absent.  If the table becomes empty, the table header itself is
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

/// Replace `{ workspace = true }` markers with workspace-resolved
/// values: standard-field markers on `[package]` become the
/// resolved literal standards from `resolved_language`, and
/// dependency markers in `[dependencies]` / `[dev-dependencies]`
/// become the workspace root's original requirement strings from
/// `dep_requirements` (the author's spelling - the parsed
/// `semver::VersionReq` would respell `"0.2"` as `"^0.2"`).
///
/// All three marker spellings the parser accepts are rewritten: the
/// inline table (`fmt = { workspace = true }`), the dotted key
/// (`fmt.workspace = true`), and the header table
/// (`[dependencies.fmt]` with `workspace = true`).  A dependency
/// marker with sibling keys (`optional` / `features` /
/// `default-features`) keeps its siblings: `workspace = true` is
/// swapped for `version = "<req>"` in place.
///
/// Format-preserving: bytes not attached to a marker round-trip
/// unchanged; comments attached to a rewritten marker are dropped
/// with it, and table-spelled markers re-render as `key = "value"`
/// at the end of the enclosing table's value entries.
/// `cabin package` uses this to normalize the archived manifest so
/// a published package is self-contained (Cargo's
/// normalized-manifest approach, scoped to the marker fields).
///
/// `resolved_language` must be the loader-resolved settings: an
/// unresolved marker in it is an invariant violation
/// (debug-asserted by the `*_standard_value()` accessors, surfacing
/// as [`EditError::MissingResolvedStandard`] in release builds).
/// Likewise every dependency marker must have a matching entry in
/// `dep_requirements`.
///
/// Returns `Ok(None)` when the manifest contains no markers so
/// callers can archive the on-disk bytes untouched.
///
/// # Errors
/// Returns [`EditError::Parse`] when `text` is not valid TOML,
/// [`EditError::MissingResolvedStandard`] when a standard marker
/// has no resolved value in `resolved_language`, and
/// [`EditError::MissingWorkspaceDependency`] when a dependency
/// marker has no requirement in `dep_requirements`.
pub fn normalize_workspace_markers(
    text: &str,
    resolved_language: &cabin_core::LanguageStandardSettings,
    dep_requirements: &cabin_core::WorkspaceDepRequirements,
) -> Result<Option<String>, EditError> {
    let mut doc = parse_document(text)?;
    let standards_changed = substitute_standard_markers_in(&mut doc, *resolved_language)?;
    let deps_changed = substitute_dependency_markers_in(&mut doc, dep_requirements)?;
    Ok((standards_changed || deps_changed).then(|| doc.to_string()))
}

/// The standard-field pass of [`normalize_workspace_markers`]:
/// rewrite `{ workspace = true }` standard markers on `[package]`
/// to the resolved literal values.  Returns whether the document
/// changed.
fn substitute_standard_markers_in(
    doc: &mut DocumentMut,
    resolved: cabin_core::LanguageStandardSettings,
) -> Result<bool, EditError> {
    let Some(package) = doc.get_mut("package").and_then(Item::as_table_like_mut) else {
        return Ok(false);
    };
    let fields: [(&'static str, Option<&'static str>); 4] = [
        (
            "c-standard",
            resolved
                .c_standard_value()
                .map(cabin_core::CStandard::as_str),
        ),
        (
            "cxx-standard",
            resolved
                .cxx_standard_value()
                .map(cabin_core::CxxStandard::as_str),
        ),
        (
            "interface-c-standard",
            resolved
                .interface_c_standard_value()
                .map(|req| interface_literal(req, cabin_core::CStandard::as_str)),
        ),
        (
            "interface-cxx-standard",
            resolved
                .interface_cxx_standard_value()
                .map(|req| interface_literal(req, cabin_core::CxxStandard::as_str)),
        ),
    ];
    let mut changed = false;
    for (key, literal) in fields {
        let Some(item) = package.get(key) else {
            continue;
        };
        let is_marker = item
            .as_table_like()
            .is_some_and(|table| table.contains_key("workspace"));
        if !is_marker {
            continue;
        }
        let Some(value) = literal else {
            return Err(EditError::MissingResolvedStandard { field: key });
        };
        if item.is_table() {
            // A header-table marker (`[package.cxx-standard]`) carries
            // table decor that renders without the ` = ` spacing when
            // replaced in place; re-insert the key so it renders as a
            // normal `key = "value"` entry inside `[package]`.
            package.remove(key);
            package.insert(key, toml_edit::value(value));
        } else if let Some(item) = package.get_mut(key) {
            *item = toml_edit::value(value);
        }
        changed = true;
    }
    Ok(changed)
}

/// The manifest string literal for a resolved interface
/// requirement: `"none"` or the minimum standard.  `max` is
/// reserved for future range support and never populated, so there
/// is no range literal to render.
fn interface_literal<S: Copy>(
    requirement: cabin_core::InterfaceRequirement<S>,
    as_str: fn(S) -> &'static str,
) -> &'static str {
    match requirement {
        cabin_core::InterfaceRequirement::None => "none",
        cabin_core::InterfaceRequirement::Requirement(requirement) => {
            debug_assert!(
                requirement.max.is_none(),
                "range requirements are reserved and never populated"
            );
            as_str(requirement.min)
        }
    }
}

/// Whether a dependency entry's value is the `workspace = true`
/// opt-in marker (any of the three TOML spellings).  The legal
/// `{ version = "1", workspace = false }` form is not a marker.
fn is_workspace_dep_marker(item: &Item) -> bool {
    item.as_table_like()
        .and_then(|table| table.get("workspace"))
        .and_then(Item::as_bool)
        == Some(true)
}

/// Replace `workspace = true` with `version = "<req>"` inside an
/// inline dependency table, preserving sibling keys and entry
/// order.  The rebuilt table's key and table decor reset to default
/// rendering (cloned values keep their own decor), so comments
/// attached to the rewritten entry are dropped - matching the
/// [`normalize_workspace_markers`] caveat.
fn swap_workspace_for_version(table: &mut toml_edit::InlineTable, requirement: &str) {
    let keys: Vec<String> = table.iter().map(|(key, _)| key.to_owned()).collect();
    let mut rebuilt = toml_edit::InlineTable::new();
    for key in keys {
        if key == "workspace" {
            rebuilt.insert("version", Value::from(requirement));
        } else if let Some(value) = table.get(&key) {
            rebuilt.insert(&key, value.clone());
        }
    }
    *table = rebuilt;
}

/// The dependency pass of [`normalize_workspace_markers`]: rewrite
/// `{ workspace = true }` dependency markers in `[dependencies]` /
/// `[dev-dependencies]` to the workspace requirement strings.
/// Returns whether the document changed.
fn substitute_dependency_markers_in(
    doc: &mut DocumentMut,
    requirements: &cabin_core::WorkspaceDepRequirements,
) -> Result<bool, EditError> {
    let mut changed = false;
    for (header, kind) in [
        ("dependencies", cabin_core::DependencyKind::Normal),
        ("dev-dependencies", cabin_core::DependencyKind::Dev),
    ] {
        let Some(table) = doc.get_mut(header).and_then(Item::as_table_like_mut) else {
            continue;
        };
        let marker_keys: Vec<String> = table
            .iter()
            .filter(|(_, item)| is_workspace_dep_marker(item))
            .map(|(key, _)| key.to_owned())
            .collect();
        for key in marker_keys {
            let Some(requirement) = requirements.requirement(kind, &key) else {
                return Err(EditError::MissingWorkspaceDependency { name: key });
            };
            // Inspect first, then mutate: the re-insert branch
            // below needs `table` itself, so no `item` borrow may be
            // live across it.  `Item::Table` covers both the header
            // table and the dotted-key spelling (toml_edit parses
            // dotted keys as a dotted `Item::Table`).
            let (marker_only, needs_reinsert) = {
                let item = table.get(&key).expect("key collected from this table");
                (
                    item.as_table_like().is_some_and(|entry| entry.len() == 1),
                    item.is_table(),
                )
            };
            if marker_only {
                // `fmt = { workspace = true }` (any spelling) → the
                // bare-string form an author would write.  Table-spelled
                // markers carry table decor that renders without the
                // ` = ` spacing when replaced in place; re-insert.
                if needs_reinsert {
                    table.remove(&key);
                    table.insert(&key, toml_edit::value(requirement));
                } else if let Some(item) = table.get_mut(&key) {
                    *item = toml_edit::value(requirement);
                }
            } else if let Some(item) = table.get_mut(&key) {
                if let Some(inline) = item.as_value_mut().and_then(Value::as_inline_table_mut) {
                    swap_workspace_for_version(inline, requirement);
                } else if let Some(entry) = item.as_table_like_mut() {
                    // Table/dotted spelling with sibling keys: drop the
                    // marker key and add the requirement.  Entry order
                    // inside a header table is not load-bearing.
                    entry.remove("workspace");
                    entry.insert("version", toml_edit::value(requirement));
                }
            }
            changed = true;
        }
    }
    Ok(changed)
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

    /// A scoped name is not a legal bare TOML key; `toml_edit` must
    /// quote it on insert and the result must parse back to the same
    /// dependency key.
    #[test]
    fn upserts_scoped_dependency_with_quoted_key() {
        let mut doc = parse_document(PACKAGE_ONLY).unwrap();
        let dep = NewDependency {
            name: "fmtlib/fmt".to_owned(),
            version: Some("^10".to_owned()),
            ..NewDependency::default()
        };
        upsert_dependency(&mut doc, DepTable::Normal, &dep).unwrap();
        let out = doc.to_string();
        assert!(
            out.contains("\"fmtlib/fmt\" = \"^10\""),
            "scoped key must be quoted, got:\n{out}"
        );
        // The emitted document round-trips through the real parser
        // with the full name as the dependency identity.
        let package = crate::parse_manifest_str(&out).unwrap().package.unwrap();
        assert_eq!(package.dependencies[0].name.as_str(), "fmtlib/fmt");
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

    #[test]
    fn substitute_standard_markers_rewrites_only_marker_fields() {
        let text = r#"# top comment
[package]
name = "core"          # trailing comment
version = "0.1.0"
cxx-standard = { workspace = true }
c-standard = "c11"

[target.core]
type = "library"
sources = ["src/core.cc"]
"#;
        let resolved = cabin_core::LanguageStandardSettings {
            c_standard: Some(cabin_core::StandardDeclaration::Declared(
                cabin_core::CStandard::C11,
            )),
            cxx_standard: Some(cabin_core::StandardDeclaration::Inherited(
                cabin_core::CxxStandard::Cxx20,
            )),
            ..Default::default()
        };
        let out = normalize_workspace_markers(
            text,
            &resolved,
            &cabin_core::WorkspaceDepRequirements::default(),
        )
        .unwrap()
        .unwrap();
        assert!(out.contains("cxx-standard = \"c++20\""), "got: {out}");
        assert!(!out.contains("workspace = true"), "got: {out}");
        assert!(out.contains("# top comment"), "got: {out}");
        assert!(out.contains("# trailing comment"), "got: {out}");
        assert!(out.contains("c-standard = \"c11\""), "got: {out}");
        // Idempotent: the rewritten text has no markers left.
        assert_eq!(
            normalize_workspace_markers(
                &out,
                &resolved,
                &cabin_core::WorkspaceDepRequirements::default()
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn substitute_standard_markers_rewrites_header_table_marker_spelling() {
        let text = "[package]\nname = \"core\"\nversion = \"0.1.0\"\n\n[package.cxx-standard]\nworkspace = true\n";
        let resolved = cabin_core::LanguageStandardSettings {
            cxx_standard: Some(cabin_core::StandardDeclaration::Inherited(
                cabin_core::CxxStandard::Cxx20,
            )),
            ..Default::default()
        };
        let out = normalize_workspace_markers(
            text,
            &resolved,
            &cabin_core::WorkspaceDepRequirements::default(),
        )
        .unwrap()
        .unwrap();
        assert!(out.contains("cxx-standard = \"c++20\""), "got: {out}");
        assert!(!out.contains("workspace = true"), "got: {out}");
        // The rewritten text must still parse as a valid manifest
        // with the literal value.
        let parsed = crate::parse_manifest_str(&out).unwrap();
        assert_eq!(
            parsed.package.unwrap().language.cxx_standard,
            Some(cabin_core::StandardDeclaration::Declared(
                cabin_core::CxxStandard::Cxx20
            ))
        );
    }

    #[test]
    fn substitute_standard_markers_emits_byte_exact_output() {
        let resolved = cabin_core::LanguageStandardSettings {
            cxx_standard: Some(cabin_core::StandardDeclaration::Inherited(
                cabin_core::CxxStandard::Cxx20,
            )),
            ..Default::default()
        };
        // Inline marker: only the marker line's value is replaced.
        let inline = "[package]\nname = \"core\"\nversion = \"0.1.0\"\ncxx-standard = { workspace = true }\n";
        assert_eq!(
            normalize_workspace_markers(
                inline,
                &resolved,
                &cabin_core::WorkspaceDepRequirements::default()
            )
            .unwrap()
            .unwrap(),
            "[package]\nname = \"core\"\nversion = \"0.1.0\"\ncxx-standard = \"c++20\"\n"
        );
        // Header-table marker: the key re-renders as `key = "value"`
        // at the end of `[package]`'s value entries.
        let header = "[package]\nname = \"core\"\nversion = \"0.1.0\"\n\n[package.cxx-standard]\nworkspace = true\n";
        assert_eq!(
            normalize_workspace_markers(
                header,
                &resolved,
                &cabin_core::WorkspaceDepRequirements::default()
            )
            .unwrap()
            .unwrap(),
            "[package]\nname = \"core\"\nversion = \"0.1.0\"\ncxx-standard = \"c++20\"\n"
        );
    }

    #[test]
    fn substitute_standard_markers_rewrites_marker_in_inline_package_table() {
        let text = "package = { name = \"core\", version = \"0.1.0\", cxx-standard = { workspace = true } }\n";
        let resolved = cabin_core::LanguageStandardSettings {
            cxx_standard: Some(cabin_core::StandardDeclaration::Inherited(
                cabin_core::CxxStandard::Cxx20,
            )),
            ..Default::default()
        };
        let out = normalize_workspace_markers(
            text,
            &resolved,
            &cabin_core::WorkspaceDepRequirements::default(),
        )
        .unwrap()
        .unwrap();
        assert!(!out.contains("workspace = true"), "got: {out}");
        let parsed = crate::parse_manifest_str(&out).unwrap();
        assert_eq!(
            parsed.package.unwrap().language.cxx_standard,
            Some(cabin_core::StandardDeclaration::Declared(
                cabin_core::CxxStandard::Cxx20
            ))
        );
    }

    #[test]
    fn substitute_standard_markers_rewrites_dotted_key_marker_spelling() {
        let text =
            "[package]\nname = \"core\"\nversion = \"0.1.0\"\ncxx-standard.workspace = true\n";
        let resolved = cabin_core::LanguageStandardSettings {
            cxx_standard: Some(cabin_core::StandardDeclaration::Inherited(
                cabin_core::CxxStandard::Cxx20,
            )),
            ..Default::default()
        };
        let out = normalize_workspace_markers(
            text,
            &resolved,
            &cabin_core::WorkspaceDepRequirements::default(),
        )
        .unwrap()
        .unwrap();
        assert!(!out.contains("workspace = true"), "got: {out}");
        let parsed = crate::parse_manifest_str(&out).unwrap();
        assert_eq!(
            parsed.package.unwrap().language.cxx_standard,
            Some(cabin_core::StandardDeclaration::Declared(
                cabin_core::CxxStandard::Cxx20
            ))
        );
    }

    #[test]
    fn substitute_standard_markers_without_markers_returns_none() {
        let text = "[package]\nname = \"core\"\nversion = \"0.1.0\"\ncxx-standard = \"c++20\"\n";
        let resolved = cabin_core::LanguageStandardSettings::default();
        assert_eq!(
            normalize_workspace_markers(
                text,
                &resolved,
                &cabin_core::WorkspaceDepRequirements::default()
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn substitute_standard_markers_without_resolved_value_errors() {
        let text = "[package]\nname = \"core\"\nversion = \"0.1.0\"\ncxx-standard = { workspace = true }\n";
        let resolved = cabin_core::LanguageStandardSettings::default();
        let err = normalize_workspace_markers(
            text,
            &resolved,
            &cabin_core::WorkspaceDepRequirements::default(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            EditError::MissingResolvedStandard {
                field: "cxx-standard"
            }
        ));
    }

    #[test]
    fn substitute_standard_markers_without_package_table_returns_none() {
        let text = "[workspace]\nmembers = [\"packages/*\"]\ncxx-standard = \"c++20\"\n";
        let resolved = cabin_core::LanguageStandardSettings::default();
        assert_eq!(
            normalize_workspace_markers(
                text,
                &resolved,
                &cabin_core::WorkspaceDepRequirements::default()
            )
            .unwrap(),
            None
        );
    }

    fn dep_requirements(
        entries: &[(cabin_core::DependencyKind, &str, &str)],
    ) -> cabin_core::WorkspaceDepRequirements {
        let mut out = cabin_core::WorkspaceDepRequirements::default();
        for (kind, name, req) in entries {
            out.insert(*kind, (*name).to_owned(), (*req).to_owned());
        }
        out
    }

    #[test]
    fn normalize_collapses_marker_only_dependency_to_bare_string() {
        let text = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nfmt = { workspace = true }\n";
        let reqs = dep_requirements(&[(cabin_core::DependencyKind::Normal, "fmt", "0.2")]);
        let out = normalize_workspace_markers(
            text,
            &cabin_core::LanguageStandardSettings::default(),
            &reqs,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            out,
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nfmt = \"0.2\"\n"
        );
        // Idempotent: no markers remain.
        assert_eq!(
            normalize_workspace_markers(
                &out,
                &cabin_core::LanguageStandardSettings::default(),
                &reqs
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn normalize_swaps_workspace_key_in_place_preserving_siblings() {
        let text = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nfmt = { workspace = true, features = [\"color\"] }\n";
        let reqs = dep_requirements(&[(cabin_core::DependencyKind::Normal, "fmt", ">=10 <11")]);
        let out = normalize_workspace_markers(
            text,
            &cabin_core::LanguageStandardSettings::default(),
            &reqs,
        )
        .unwrap()
        .unwrap();
        // version replaces workspace at the same position; siblings
        // and their order survive.
        assert!(
            out.contains("fmt = { version = \">=10 <11\", features = [\"color\"] }"),
            "got: {out}"
        );
        assert!(!out.contains("workspace"), "got: {out}");
    }

    #[test]
    fn normalize_dev_dependency_lookup_is_kind_specific() {
        let text = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dev-dependencies]\ngtest = { workspace = true }\n";
        let wrong_kind = dep_requirements(&[(cabin_core::DependencyKind::Normal, "gtest", "^1")]);
        let err = normalize_workspace_markers(
            text,
            &cabin_core::LanguageStandardSettings::default(),
            &wrong_kind,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            EditError::MissingWorkspaceDependency { ref name } if name == "gtest"
        ));
        let right_kind = dep_requirements(&[(cabin_core::DependencyKind::Dev, "gtest", "^1")]);
        let out = normalize_workspace_markers(
            text,
            &cabin_core::LanguageStandardSettings::default(),
            &right_kind,
        )
        .unwrap()
        .unwrap();
        assert!(out.contains("gtest = \"^1\""), "got: {out}");
    }

    #[test]
    fn normalize_handles_dotted_and_header_table_dep_marker_spellings() {
        let reqs = dep_requirements(&[(cabin_core::DependencyKind::Normal, "fmt", "0.2")]);
        for text in [
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nfmt.workspace = true\n",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies.fmt]\nworkspace = true\n",
        ] {
            let out = normalize_workspace_markers(
                text,
                &cabin_core::LanguageStandardSettings::default(),
                &reqs,
            )
            .unwrap()
            .unwrap();
            assert!(!out.contains("workspace"), "got: {out}");
            let parsed = crate::parse_manifest_str(&out).unwrap();
            let pkg = parsed.package.unwrap();
            assert_eq!(pkg.dependencies.len(), 1);
            assert!(
                matches!(&pkg.dependencies[0].source, cabin_core::DependencySource::Version(req) if req.to_string().starts_with("^0.2")),
                "unexpected source: {:?}",
                pkg.dependencies[0].source
            );
        }
    }

    #[test]
    fn normalize_rewrites_marker_in_inline_dependencies_table() {
        // The inline parent spelling is only TOML-legal at the
        // document root, before the first `[header]`.
        let text = "dependencies = { fmt = { workspace = true } }\n\n[package]\nname = \"app\"\nversion = \"0.1.0\"\n";
        let reqs = dep_requirements(&[(cabin_core::DependencyKind::Normal, "fmt", "0.2")]);
        let out = normalize_workspace_markers(
            text,
            &cabin_core::LanguageStandardSettings::default(),
            &reqs,
        )
        .unwrap()
        .unwrap();
        assert!(!out.contains("workspace"), "got: {out}");
        let parsed = crate::parse_manifest_str(&out).unwrap();
        let pkg = parsed.package.unwrap();
        assert_eq!(pkg.dependencies.len(), 1);
        assert!(
            matches!(&pkg.dependencies[0].source, cabin_core::DependencySource::Version(req) if req.to_string().starts_with("^0.2")),
            "unexpected source: {:?}",
            pkg.dependencies[0].source
        );
    }

    #[test]
    fn normalize_leaves_workspace_false_beside_version_untouched() {
        // `{ version = "1", workspace = false }` is legal and parses
        // as a plain version dependency; the rewrite must not touch it.
        let text = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nfmt = { version = \"1\", workspace = false }\n";
        let reqs = dep_requirements(&[(cabin_core::DependencyKind::Normal, "fmt", "0.2")]);
        assert_eq!(
            normalize_workspace_markers(
                text,
                &cabin_core::LanguageStandardSettings::default(),
                &reqs
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn normalize_handles_standards_and_deps_in_one_pass() {
        let text = "[package]\nname = \"app\"\nversion = \"0.1.0\"\ncxx-standard = { workspace = true }\n\n[dependencies]\nfmt = { workspace = true }\n";
        let resolved = cabin_core::LanguageStandardSettings {
            cxx_standard: Some(cabin_core::StandardDeclaration::Inherited(
                cabin_core::CxxStandard::Cxx20,
            )),
            ..Default::default()
        };
        let reqs = dep_requirements(&[(cabin_core::DependencyKind::Normal, "fmt", "0.2")]);
        let out = normalize_workspace_markers(text, &resolved, &reqs)
            .unwrap()
            .unwrap();
        assert_eq!(
            out,
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\ncxx-standard = \"c++20\"\n\n[dependencies]\nfmt = \"0.2\"\n"
        );
    }
}
