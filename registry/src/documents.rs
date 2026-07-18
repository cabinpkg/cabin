//! Composition of the JSON documents the read routes serve.

use serde::Serialize;
use serde_json::{Map, Value};

/// `config.json` for this registry. Exactly the fields the Cabin client's
/// `deny_unknown_fields` parser accepts (`docs/remote-registry.md`): adding a
/// field here requires a client release first.
#[derive(Serialize)]
struct ConfigDoc<'a> {
    schema: u32,
    kind: &'a str,
    packages: &'a str,
    artifacts: &'a str,
    #[serde(rename = "auth-required")]
    auth_required: bool,
    api: &'a str,
}

/// Renders `config.json`. `api_origin` is the **website** origin
/// (`WEB_ORIGIN`) the mutation and session routes live on - crates.io's
/// `"api": "https://crates.io"` discipline - never the index origin
/// serving this document.
#[allow(clippy::missing_panics_doc)] // serializing a `ConfigDoc` cannot fail
pub fn config_json(api_origin: &str) -> String {
    serde_json::to_string(&ConfigDoc {
        schema: 1,
        kind: "file-registry",
        packages: "packages",
        artifacts: "artifacts",
        auth_required: true,
        api: api_origin,
    })
    .expect("config document serializes")
}

/// One row of the `versions` table, as the glue hands it over.
#[derive(Debug)]
pub struct VersionRow {
    pub version: String,
    /// The canonical per-version index entry stored verbatim at publish time.
    pub metadata_json: String,
    /// Current yanked state - overrides whatever the stored entry says.
    pub yanked: bool,
}

#[derive(Serialize)]
struct PackageDoc<'a> {
    schema: u32,
    name: &'a str,
    versions: Map<String, Value>,
}

/// Composes `packages/<scope>/<name>.json` from the stored canonical
/// version entries; `name` is the package's canonical `<scope>/<name>`
/// string. The canonical document's `schema` / `name` / `version` envelope
/// is stripped - those are document-level fields, and the client's index
/// parser rejects unknown fields in version entries (`package-index.md`) -
/// and each entry's `yanked` field is overwritten from its row so the
/// document always reflects current D1 state. Deterministic: versions are
/// emitted in lexicographic order.
///
/// # Errors
///
/// When a stored entry is not valid JSON or not a JSON object - an internal
/// invariant break the caller reports as a 500, never a client error.
// ponytail: lexicographic, not semver, order - the client treats `versions`
// as a map; switch to semver ordering if a consumer ever compares bytes with
// the local file registry.
#[allow(clippy::missing_panics_doc)] // serializing a `PackageDoc` cannot fail
pub fn package_json(name: &str, rows: &[VersionRow]) -> Result<String, String> {
    let mut rows: Vec<&VersionRow> = rows.iter().collect();
    rows.sort_by(|a, b| a.version.cmp(&b.version));
    let mut versions = Map::new();
    for row in rows {
        let mut entry: Value = serde_json::from_str(&row.metadata_json).map_err(|err| {
            format!(
                "stored metadata for {name}@{} is not valid JSON: {err}",
                row.version
            )
        })?;
        let Some(fields) = entry.as_object_mut() else {
            return Err(format!(
                "stored metadata for {name}@{} is not a JSON object",
                row.version
            ));
        };
        for envelope in ["schema", "name", "version"] {
            // shift_remove: plain `remove` is a swap_remove under
            // `preserve_order` and would scramble the entry's key order.
            fields.shift_remove(envelope);
        }
        fields.insert("yanked".to_owned(), Value::Bool(row.yanked));
        versions.insert(row.version.clone(), entry);
    }
    Ok(serde_json::to_string(&PackageDoc {
        schema: 1,
        name,
        versions,
    })
    .expect("package document serializes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_json_matches_the_contract_byte_for_byte() {
        // The `api` field names the website origin, not the index origin.
        assert_eq!(
            config_json("https://cabinpkg.com"),
            r#"{"schema":1,"kind":"file-registry","packages":"packages","artifacts":"artifacts","auth-required":true,"api":"https://cabinpkg.com"}"#
        );
    }

    fn row(version: &str, metadata_json: &str, yanked: bool) -> VersionRow {
        VersionRow {
            version: version.to_owned(),
            metadata_json: metadata_json.to_owned(),
            yanked,
        }
    }

    #[test]
    fn package_json_overrides_yanked_from_the_row_state() {
        let stored = r#"{"dependencies":{},"yanked":false,"checksum":"sha256:aa","source":"../artifacts/fmt/fmt-1.0.0.zip"}"#;
        let body = package_json("fmtlib/fmt", &[row("1.0.0", stored, true)]).unwrap();
        assert_eq!(
            body,
            r#"{"schema":1,"name":"fmtlib/fmt","versions":{"1.0.0":{"dependencies":{},"yanked":true,"checksum":"sha256:aa","source":"../artifacts/fmt/fmt-1.0.0.zip"}}}"#
        );
    }

    #[test]
    fn package_json_overrides_a_stale_stored_yanked_after_unyank() {
        // Un-yanking only flips the row column; the stored entry still
        // says `yanked: true` and must lose.
        let stored = r#"{"dependencies":{},"yanked":true,"checksum":"sha256:aa","source":"../artifacts/fmt/fmt-1.0.0.zip"}"#;
        let body = package_json("fmtlib/fmt", &[row("1.0.0", stored, false)]).unwrap();
        assert_eq!(
            body,
            r#"{"schema":1,"name":"fmtlib/fmt","versions":{"1.0.0":{"dependencies":{},"yanked":false,"checksum":"sha256:aa","source":"../artifacts/fmt/fmt-1.0.0.zip"}}}"#
        );
    }

    #[test]
    fn package_json_strips_the_canonical_envelope_from_stored_entries() {
        // Publish stores the canonical per-version document verbatim, so
        // stored entries carry the `schema`/`name`/`version` envelope; the
        // served version entry must not (the client's index parser rejects
        // unknown fields in version entries).
        let stored = r#"{"schema":1,"name":"fmtlib/fmt","version":"1.0.0","dependencies":{},"yanked":false,"checksum":"sha256:aa","source":{"type":"archive","path":"../../artifacts/fmtlib/fmt/fmtlib-fmt-1.0.0.zip","format":"zip"}}"#;
        let body = package_json("fmtlib/fmt", &[row("1.0.0", stored, false)]).unwrap();
        assert_eq!(
            body,
            r#"{"schema":1,"name":"fmtlib/fmt","versions":{"1.0.0":{"dependencies":{},"yanked":false,"checksum":"sha256:aa","source":{"type":"archive","path":"../../artifacts/fmtlib/fmt/fmtlib-fmt-1.0.0.zip","format":"zip"}}}}"#
        );
    }

    #[test]
    fn package_json_adds_yanked_when_the_stored_entry_lacks_it() {
        let body = package_json(
            "fmtlib/fmt",
            &[row("1.0.0", r#"{"checksum":"sha256:aa"}"#, false)],
        )
        .unwrap();
        assert_eq!(
            body,
            r#"{"schema":1,"name":"fmtlib/fmt","versions":{"1.0.0":{"checksum":"sha256:aa","yanked":false}}}"#
        );
    }

    #[test]
    fn package_json_orders_versions_deterministically() {
        let rows = [
            row("2.0.0", r#"{"a":1}"#, false),
            row("1.0.0", r#"{"a":2}"#, false),
            row("1.0.0-rc.1", r#"{"a":3}"#, false),
        ];
        let body = package_json("fmtlib/fmt", &rows).unwrap();
        let expected_order = ["1.0.0", "1.0.0-rc.1", "2.0.0"];
        let positions: Vec<usize> = expected_order
            .iter()
            .map(|v| body.find(&format!("\"{v}\":")).unwrap())
            .collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "body: {body}");
    }

    #[test]
    fn package_json_rejects_non_object_metadata() {
        let err = package_json("fmtlib/fmt", &[row("1.0.0", "[1,2]", false)]).unwrap_err();
        assert!(err.contains("fmt@1.0.0"), "err: {err}");
        let err = package_json("fmtlib/fmt", &[row("1.0.0", "not json", false)]).unwrap_err();
        assert!(err.contains("not valid JSON"), "err: {err}");
    }
}
