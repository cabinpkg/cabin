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
        // Public verified reads: no credential is required to read.
        // Stored credentials still ride along when the client has one
        // (the verifier's pending fetches depend on that).
        auth_required: false,
        api: api_origin,
    })
    .expect("config document serializes")
}

/// One composed version: the **current** revision the read plane
/// serves (`current_revisions`), as the glue hands it over.
#[derive(Debug)]
pub struct VersionRow {
    pub version: String,
    /// The served packaging revision's id.
    pub revision: String,
    /// The current revision's canonical index entry, stored verbatim
    /// at publish time.
    pub metadata_json: String,
    /// Current yanked state - overrides whatever the stored entry says.
    pub yanked: bool,
}

/// One **verified** revision of the package, for the per-version
/// `revisions` maps: superseded revisions stay listed so pinned
/// lockfiles keep fetching them.
#[derive(Debug)]
pub struct RevisionRow {
    pub version: String,
    pub revision: String,
    /// Canonical `sha256:<64 lowercase hex>` value, as stored.
    pub checksum: String,
    pub published_at: String,
}

#[derive(Serialize)]
struct PackageDoc<'a> {
    schema: u32,
    name: &'a str,
    versions: Map<String, Value>,
}

/// Composes `packages/<scope>/<name>.json` from each verified version's
/// current revision plus the package's full verified-revision set.
/// The stored canonical document's `schema` / `name` / `version`
/// envelope **and** its per-revision `checksum` / `source` fields are
/// stripped - the served entry carries them inside its `revisions` map
/// instead, and the client's index parser rejects unknown fields in
/// version entries (`package-index.md`).  Each entry's `yanked` field
/// is overwritten from its version row, its `revision` field names the
/// served revision, and its `revisions` map lists every verified
/// revision with its checksum, publish time, and canonical source
/// path.  Deterministic: versions in lexicographic order, revisions in
/// revision-id order.
///
/// # Errors
///
/// When a stored entry is not valid JSON or not a JSON object - an internal
/// invariant break the caller reports as a 500, never a client error.
// ponytail: lexicographic, not semver, order - the client treats `versions`
// as a map; switch to semver ordering if a consumer ever compares bytes with
// the local file registry.
#[allow(clippy::missing_panics_doc)] // serializing a `PackageDoc` cannot fail
pub fn package_json(
    scope: &str,
    package: &str,
    rows: &[VersionRow],
    revisions: &[RevisionRow],
) -> Result<String, String> {
    let name = format!("{scope}/{package}");
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
        for stripped in ["schema", "name", "version", "checksum", "source"] {
            // shift_remove: plain `remove` is a swap_remove under
            // `preserve_order` and would scramble the entry's key order.
            fields.shift_remove(stripped);
        }
        fields.insert("yanked".to_owned(), Value::Bool(row.yanked));
        fields.insert("revision".to_owned(), Value::String(row.revision.clone()));
        let mut revision_map = Map::new();
        for revision in revisions.iter().filter(|r| r.version == row.version) {
            // The canonical per-revision source path, the same grammar
            // publish validates (`crate::publish::validate_metadata`)
            // and the local file registry writes.
            let path = format!(
                "../../artifacts/{scope}/{package}/{scope}-{package}-{version}-{rev}.zip",
                version = revision.version,
                rev = revision.revision,
            );
            revision_map.insert(
                revision.revision.clone(),
                serde_json::json!({
                    "checksum": revision.checksum,
                    "published-at": revision.published_at,
                    "source": { "type": "archive", "path": path, "format": "zip" },
                }),
            );
        }
        if !revision_map.contains_key(&row.revision) {
            return Err(format!(
                "current revision {} of {name}@{} is missing from the verified revision set",
                row.revision, row.version
            ));
        }
        fields.insert("revisions".to_owned(), Value::Object(revision_map));
        versions.insert(row.version.clone(), entry);
    }
    Ok(serde_json::to_string(&PackageDoc {
        schema: 1,
        name: &name,
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
            r#"{"schema":1,"kind":"file-registry","packages":"packages","artifacts":"artifacts","auth-required":false,"api":"https://cabinpkg.com"}"#
        );
    }

    const REV_A: &str = "aaaaaaaaaaaaaaaa";
    const REV_B: &str = "bbbbbbbbbbbbbbbb";

    fn row(version: &str, revision: &str, metadata_json: &str, yanked: bool) -> VersionRow {
        VersionRow {
            version: version.to_owned(),
            revision: revision.to_owned(),
            metadata_json: metadata_json.to_owned(),
            yanked,
        }
    }

    fn revision(version: &str, revision: &str, seed: char) -> RevisionRow {
        RevisionRow {
            version: version.to_owned(),
            revision: revision.to_owned(),
            checksum: format!(
                "sha256:{}",
                std::iter::repeat_n(seed, 64).collect::<String>()
            ),
            published_at: format!("2026-01-01T00:00:0{seed}Z"),
        }
    }

    #[test]
    fn package_json_overrides_yanked_and_composes_the_revision_map() {
        let stored = r#"{"dependencies":{},"yanked":false,"checksum":"sha256:aa","source":{"type":"archive","path":"x.zip","format":"zip"}}"#;
        let body = package_json(
            "fmtlib",
            "fmt",
            &[row("1.0.0", REV_A, stored, true)],
            &[revision("1.0.0", REV_A, 'a')],
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let entry = &value["versions"]["1.0.0"];
        // The row state wins over the stored `yanked`, and the stored
        // per-revision `checksum` / `source` fields are stripped in
        // favor of the composed `revisions` map.
        assert_eq!(entry["yanked"], true);
        assert!(entry.get("checksum").is_none(), "{body}");
        assert!(entry.get("source").is_none(), "{body}");
        assert_eq!(entry["revision"], REV_A);
        let rev = &entry["revisions"][REV_A];
        assert_eq!(rev["checksum"], format!("sha256:{}", "a".repeat(64)));
        assert_eq!(rev["published-at"], "2026-01-01T00:00:0aZ");
        assert_eq!(
            rev["source"]["path"],
            format!("../../artifacts/fmtlib/fmt/fmtlib-fmt-1.0.0-{REV_A}.zip")
        );
        assert_eq!(rev["source"]["type"], "archive");
        assert_eq!(rev["source"]["format"], "zip");
    }

    #[test]
    fn package_json_strips_the_canonical_envelope_from_stored_entries() {
        // Publish stores the canonical per-version document verbatim, so
        // stored entries carry the `schema`/`name`/`version` envelope; the
        // served version entry must not (the client's index parser rejects
        // unknown fields in version entries).
        let stored = r#"{"schema":1,"name":"fmtlib/fmt","version":"1.0.0","dependencies":{},"yanked":false,"checksum":"sha256:aa","source":{"type":"archive","path":"x.zip","format":"zip"}}"#;
        let body = package_json(
            "fmtlib",
            "fmt",
            &[row("1.0.0", REV_A, stored, false)],
            &[revision("1.0.0", REV_A, 'a')],
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let entry = &value["versions"]["1.0.0"];
        for stripped in ["schema", "name", "version", "checksum", "source"] {
            assert!(entry.get(stripped).is_none(), "{stripped} leaked: {body}");
        }
        assert_eq!(entry["dependencies"], serde_json::json!({}));
    }

    /// A superseded verified revision stays listed beside the current
    /// one - that is the fetchability guarantee for pinned lockfiles -
    /// while pending/rejected rows never reach this function at all
    /// (the queries filter them).
    #[test]
    fn package_json_lists_superseded_revisions_beside_the_current_one() {
        let stored = r#"{"dependencies":{}}"#;
        let body = package_json(
            "fmtlib",
            "fmt",
            &[row("1.0.0", REV_B, stored, false)],
            &[revision("1.0.0", REV_A, 'a'), revision("1.0.0", REV_B, 'b')],
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let entry = &value["versions"]["1.0.0"];
        assert_eq!(entry["revision"], REV_B);
        let revisions = entry["revisions"].as_object().unwrap();
        assert_eq!(revisions.len(), 2);
        assert_eq!(
            revisions[REV_A]["checksum"],
            format!("sha256:{}", "a".repeat(64))
        );
    }

    /// A current revision missing from the verified set is an internal
    /// invariant break, reported as an error (the caller 500s).
    #[test]
    fn package_json_rejects_a_current_revision_outside_the_verified_set() {
        let err = package_json(
            "fmtlib",
            "fmt",
            &[row("1.0.0", REV_B, r#"{"dependencies":{}}"#, false)],
            &[revision("1.0.0", REV_A, 'a')],
        )
        .unwrap_err();
        assert!(
            err.contains("missing from the verified revision set"),
            "{err}"
        );
    }

    #[test]
    fn package_json_orders_versions_deterministically() {
        let rows = [
            row("2.0.0", REV_A, r#"{"a":1}"#, false),
            row("1.0.0", REV_A, r#"{"a":2}"#, false),
            row("1.0.0-rc.1", REV_A, r#"{"a":3}"#, false),
        ];
        let revisions = [
            revision("2.0.0", REV_A, 'a'),
            revision("1.0.0", REV_A, 'a'),
            revision("1.0.0-rc.1", REV_A, 'a'),
        ];
        let body = package_json("fmtlib", "fmt", &rows, &revisions).unwrap();
        let expected_order = ["1.0.0", "1.0.0-rc.1", "2.0.0"];
        let positions: Vec<usize> = expected_order
            .iter()
            .map(|v| body.find(&format!("\"{v}\":")).unwrap())
            .collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "body: {body}");
    }

    #[test]
    fn package_json_rejects_non_object_metadata() {
        let err = package_json(
            "fmtlib",
            "fmt",
            &[row("1.0.0", REV_A, "[1,2]", false)],
            &[revision("1.0.0", REV_A, 'a')],
        )
        .unwrap_err();
        assert!(err.contains("fmt@1.0.0"), "err: {err}");
        let err = package_json(
            "fmtlib",
            "fmt",
            &[row("1.0.0", REV_A, "not json", false)],
            &[revision("1.0.0", REV_A, 'a')],
        )
        .unwrap_err();
        assert!(err.contains("not valid JSON"), "err: {err}");
    }
}
