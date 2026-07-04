//! Best-effort span lookup for manifest fields cited by
//! diagnostics.
//!
//! The typed manifest model does not retain per-field spans, so a
//! diagnostic that wants to label a declaration re-locates it here:
//! given the manifest *text* and the scope + field name a typed
//! layer recorded (for example
//! `cabin_build::DeclSite { scope, field, .. }`), this module
//! re-parses the text with [`toml_edit`]'s span-preserving document
//! and returns the byte range covering `field = value`.
//!
//! Lookups are best-effort by design: any parse failure or missing
//! key yields `None`, and the caller renders its diagnostic without
//! a source snippet.  The text was already parsed successfully once
//! to produce the typed model, so in practice a lookup only misses
//! when the file changed on disk in between - a race not worth an
//! error path in a diagnostic renderer.

use std::ops::Range;

/// The manifest table a standard field lives in, mirroring the
/// three declaration tiers of `docs/language-standards.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardFieldScope<'a> {
    /// The `[package]` table.
    Package,
    /// The `[target.<name>]` table.
    Target(&'a str),
    /// The `[workspace]` table.
    Workspace,
}

/// Byte range of `field = value` inside `text` for a
/// language-standard field in the given scope, or `None` when the
/// text does not parse or the field is absent.
#[must_use]
pub fn standard_field_span(
    text: &str,
    scope: StandardFieldScope<'_>,
    field: &str,
) -> Option<Range<usize>> {
    let doc: toml_edit::Document<&str> = toml_edit::Document::parse(text).ok()?;
    let table = match scope {
        StandardFieldScope::Package => doc.get("package")?.as_table()?,
        StandardFieldScope::Workspace => doc.get("workspace")?.as_table()?,
        StandardFieldScope::Target(name) => doc.get("target")?.as_table()?.get(name)?.as_table()?,
    };
    let key_span = table.key(field).and_then(toml_edit::Key::span);
    let value_span = table.get(field).and_then(toml_edit::Item::span);
    // Label the whole `field = value` when both spans are known;
    // degrade to whichever half the parser retained.
    match (key_span, value_span) {
        (Some(key), Some(value)) => Some(key.start.min(value.start)..key.end.max(value.end)),
        (span, None) | (None, span) => span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = "c++17"

[workspace]
c-standard = "c11"

[target.core]
kind = "library"
sources = ["src/core.cc"]
interface-cxx-standard = "c++20"
"#;

    fn spanned(scope: StandardFieldScope<'_>, field: &str) -> &'static str {
        let range = standard_field_span(MANIFEST, scope, field).expect("field present");
        &MANIFEST[range]
    }

    #[test]
    fn locates_package_level_field() {
        assert_eq!(
            spanned(StandardFieldScope::Package, "cxx-standard"),
            r#"cxx-standard = "c++17""#
        );
    }

    #[test]
    fn locates_workspace_level_field() {
        assert_eq!(
            spanned(StandardFieldScope::Workspace, "c-standard"),
            r#"c-standard = "c11""#
        );
    }

    #[test]
    fn locates_target_level_field() {
        assert_eq!(
            spanned(StandardFieldScope::Target("core"), "interface-cxx-standard"),
            r#"interface-cxx-standard = "c++20""#
        );
    }

    #[test]
    fn absent_field_or_scope_yields_none() {
        assert_eq!(
            standard_field_span(MANIFEST, StandardFieldScope::Package, "c-standard"),
            None
        );
        assert_eq!(
            standard_field_span(MANIFEST, StandardFieldScope::Target("app"), "cxx-standard"),
            None
        );
    }

    #[test]
    fn unparsable_text_yields_none() {
        assert_eq!(
            standard_field_span("[package\nbroken", StandardFieldScope::Package, "name"),
            None
        );
    }
}
