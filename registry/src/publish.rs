//! Publish-request validation: body framing, the canonical metadata
//! schema, and the identity / checksum checks (`docs/remote-registry.md`,
//! "Publish").
//!
//! Everything here is pure and host-testable. The caller (the wasm32
//! glue) reads the body, computes the archive digest, and performs the
//! D1/R2 writes; this module decides whether the request is valid and
//! with which `400` detail it is not. Details are fixed strings - user
//! bytes are never echoed back.

use serde::Deserialize;

/// Cap on the total request body (frame headers + metadata + archive).
pub const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// A decoded publish body frame:
/// `[u32 LE metadata_len][metadata][u32 LE archive_len][archive]`.
#[derive(Debug, PartialEq, Eq)]
pub struct Frame<'a> {
    pub metadata: &'a [u8],
    pub archive: &'a [u8],
}

/// Decodes the length-prefixed publish body, enforcing [`MAX_BODY_BYTES`]
/// and that the frame lengths account for the body exactly.
///
/// # Errors
///
/// A fixed `400` detail string when the body is too large, truncated, or
/// the frame lengths disagree with the actual body length.
pub fn decode_frame(body: &[u8]) -> Result<Frame<'_>, &'static str> {
    if body.len() > MAX_BODY_BYTES {
        return Err(BODY_TOO_LARGE);
    }
    let (metadata, rest) = split_frame(body).ok_or(BAD_FRAMING)?;
    let (archive, rest) = split_frame(rest).ok_or(BAD_FRAMING)?;
    if !rest.is_empty() {
        return Err(BAD_FRAMING);
    }
    Ok(Frame { metadata, archive })
}

/// Splits one `[u32 LE len][payload]` frame off the front of `bytes`.
fn split_frame(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let len = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
    let rest = &bytes[4..];
    (len <= rest.len()).then(|| rest.split_at(len))
}

pub const BODY_TOO_LARGE: &str = "request body exceeds the 64 MiB limit";
pub const BAD_FRAMING: &str = "malformed publish body framing";
pub const METADATA_NOT_JSON: &str = "metadata is not valid JSON";
pub const METADATA_NOT_CANONICAL: &str =
    "metadata does not match the canonical package metadata schema";
pub const UNSUPPORTED_SCHEMA: &str = "unsupported metadata schema";
pub const IDENTITY_MISMATCH: &str =
    "metadata name, version, or source path does not match the request URL";
pub const INVALID_NAME: &str = "invalid package name";
pub const INVALID_VERSION: &str = "package version is not valid SemVer";
pub const YANKED_AT_PUBLISH: &str = "yanked must be false at publish";
pub const CHECKSUM_MISMATCH: &str = "checksum does not match the archive bytes";

/// The canonical per-version metadata document `cabin package` emits
/// (`cabin_package::metadata::PackageMetadata`), mirrored key for key.
/// Fields the service interprets are typed; fields it only stores and
/// serves back verbatim stay opaque. `deny_unknown_fields` makes client
/// drift a `400` here and a conformance-test failure in CI, never a
/// silently accepted document.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionMetadata {
    pub schema: u32,
    pub name: String,
    pub version: String,
    /// `name -> requirement string | rich table`; stored verbatim.
    pub dependencies: serde_json::Map<String, serde_json::Value>,
    #[serde(rename = "dev-dependencies")]
    pub dev_dependencies: Option<serde_json::Value>,
    #[serde(rename = "system-dependencies")]
    pub system_dependencies: Option<serde_json::Value>,
    pub features: Option<serde_json::Value>,
    pub profiles: Option<serde_json::Value>,
    pub toolchain: Option<serde_json::Value>,
    pub build: Option<serde_json::Value>,
    pub compiler_wrapper: Option<serde_json::Value>,
    pub language: Option<serde_json::Value>,
    pub standards: Option<serde_json::Value>,
    pub yanked: bool,
    pub checksum: String,
    pub source: SourceMetadata,
}

/// The metadata document's `source` block.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMetadata {
    #[serde(rename = "type")]
    pub kind: String,
    pub path: String,
    pub format: String,
}

/// Package names accepted at publish: both parts of the canonical
/// `<scope>/<name>`, under exactly the read-route grammars
/// (`routes::is_valid_scope`, `routes::is_valid_name`) - what cannot be
/// published cannot be routed to, and vice versa.
pub fn is_valid_publish_name(scope: &str, name: &str) -> bool {
    crate::routes::is_valid_scope(scope) && crate::routes::is_valid_name(name)
}

/// Validates the metadata frame against the request URL's `scope` /
/// `name` / `version` segments, in the documented order: parse (unknown
/// fields rejected), schema, URL identity (the document's `name` is the
/// full `<scope>/<name>`, and the archive path its `source` block
/// implies embeds the scope twice - directory and filename), scope and
/// name charsets, `SemVer`, `yanked`. The checksum is checked separately
/// by [`verify_checksum`] once the caller has digested the archive
/// bytes.
///
/// The scoped shape accepted here is deliberately ahead of the client
/// and the external verifier, which both still speak bare names until
/// the client-side scoped-names steps land - safe because production
/// publishes stay impossible (the membership gate, with no claimable
/// scopes) until the claim flow lands (`docs/architecture.md`,
/// "Scopes").
///
/// # Errors
///
/// The fixed `400` detail string for the first check that fails.
pub fn validate_metadata(
    url_scope: &str,
    url_name: &str,
    url_version: &str,
    metadata: &[u8],
) -> Result<VersionMetadata, &'static str> {
    let parsed: VersionMetadata = match serde_json::from_slice(metadata) {
        Ok(parsed) => parsed,
        Err(err) if err.is_syntax() || err.is_eof() => return Err(METADATA_NOT_JSON),
        Err(_) => return Err(METADATA_NOT_CANONICAL),
    };
    // A schema the verification pipeline cannot judge must never
    // enter the pending queue: it would sit pending forever and
    // trip the stuck-verifier alert.
    if parsed.schema != 1 {
        return Err(UNSUPPORTED_SCHEMA);
    }
    let canonical_name = format!("{url_scope}/{url_name}");
    let canonical_source_path = format!(
        "../../artifacts/{url_scope}/{url_name}/{url_scope}-{url_name}-{url_version}.tar.gz"
    );
    if parsed.name != canonical_name
        || parsed.version != url_version
        || parsed.source.kind != "archive"
        || parsed.source.format != "tar.gz"
        || parsed.source.path != canonical_source_path
    {
        return Err(IDENTITY_MISMATCH);
    }
    if !is_valid_publish_name(url_scope, url_name) {
        return Err(INVALID_NAME);
    }
    if semver::Version::parse(url_version).is_err() {
        return Err(INVALID_VERSION);
    }
    if parsed.yanked {
        return Err(YANKED_AT_PUBLISH);
    }
    Ok(parsed)
}

/// Compares the metadata's claimed checksum against the lowercase
/// SHA-256 hex the server computed from the uploaded archive bytes.
///
/// # Errors
///
/// [`CHECKSUM_MISMATCH`] (a `400` detail) unless the claim is exactly
/// `sha256:<computed_hex>`.
pub fn verify_checksum(metadata: &VersionMetadata, computed_hex: &str) -> Result<(), &'static str> {
    let claimed = &metadata.checksum;
    let matches = claimed
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex == computed_hex);
    if matches {
        Ok(())
    } else {
        Err(CHECKSUM_MISMATCH)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(metadata: &[u8], archive: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&u32::try_from(metadata.len()).unwrap().to_le_bytes());
        body.extend_from_slice(metadata);
        body.extend_from_slice(&u32::try_from(archive.len()).unwrap().to_le_bytes());
        body.extend_from_slice(archive);
        body
    }

    fn metadata_json(scope: &str, name: &str, version: &str) -> String {
        format!(
            r#"{{
  "schema": 1,
  "name": "{scope}/{name}",
  "version": "{version}",
  "dependencies": {{}},
  "yanked": false,
  "checksum": "sha256:aa",
  "source": {{
    "type": "archive",
    "path": "../../artifacts/{scope}/{name}/{scope}-{name}-{version}.tar.gz",
    "format": "tar.gz"
  }}
}}"#
        )
    }

    #[test]
    fn decode_frame_round_trips_and_rejects_slack_or_truncation() {
        let body = frame(b"meta", b"archive-bytes");
        let decoded = decode_frame(&body).unwrap();
        assert_eq!(decoded.metadata, b"meta");
        assert_eq!(decoded.archive, b"archive-bytes");

        // Empty payloads still frame.
        let empty = frame(b"", b"");
        let decoded = decode_frame(&empty).unwrap();
        assert!(decoded.metadata.is_empty() && decoded.archive.is_empty());

        // Truncated, oversized-length, trailing-slack, and too-short
        // bodies are all the same framing 400.
        let mut truncated = frame(b"meta", b"archive");
        truncated.pop();
        let mut oversized_len = frame(b"meta", b"archive");
        oversized_len[0] = 0xff; // metadata_len points past the body
        let cases: Vec<Vec<u8>> = vec![
            Vec::new(),
            vec![0u8; 7],
            truncated,
            [frame(b"m", b"a"), vec![0]].concat(),
            oversized_len,
        ];
        for bad in cases {
            assert_eq!(decode_frame(&bad), Err(BAD_FRAMING), "body: {bad:?}");
        }
    }

    #[test]
    fn decode_frame_enforces_the_body_cap() {
        // A body one byte over the cap; the frame lengths are irrelevant.
        let body = vec![0u8; MAX_BODY_BYTES + 1];
        assert_eq!(decode_frame(&body), Err(BODY_TOO_LARGE));
    }

    #[test]
    fn validate_metadata_accepts_the_canonical_document() {
        let body = metadata_json("fmtlib", "fmt", "10.2.1");
        let parsed = validate_metadata("fmtlib", "fmt", "10.2.1", body.as_bytes()).unwrap();
        assert_eq!(parsed.name, "fmtlib/fmt");
        assert_eq!(parsed.checksum, "sha256:aa");
        assert!(parsed.dependencies.is_empty());
    }

    #[test]
    fn validate_metadata_accepts_optional_blocks() {
        let body = r#"{
  "schema": 1,
  "name": "fmtlib/fmt",
  "version": "1.0.0",
  "dependencies": {"zlib": "^1.3", "rich": {"version": "^2", "optional": true}},
  "dev-dependencies": {"catch2": "^3"},
  "system-dependencies": {"openssl": {"version": ">=3", "dependency_kind": "normal"}},
  "features": {"default": [], "features": {}},
  "profiles": {},
  "toolchain": {},
  "build": {},
  "compiler_wrapper": {"kind": "use", "wrapper": "ccache"},
  "language": {"cxx-standard": "c++20"},
  "standards": {"targets": {"fmt": {"interface": {"c++": {"min": "c++17"}}}}},
  "yanked": false,
  "checksum": "sha256:bb",
  "source": {"type": "archive", "path": "../../artifacts/fmtlib/fmt/fmtlib-fmt-1.0.0.tar.gz", "format": "tar.gz"}
}"#;
        let parsed = validate_metadata("fmtlib", "fmt", "1.0.0", body.as_bytes()).unwrap();
        assert!(parsed.standards.is_some());
        assert!(parsed.features.is_some());
    }

    #[test]
    fn validate_metadata_rejects_unsupported_schemas() {
        // A schema the verification pipeline cannot judge would sit
        // pending forever; it must be a 400 at publish.
        for schema in ["0", "2"] {
            let body = metadata_json("fmtlib", "fmt", "10.2.1")
                .replace("\"schema\": 1,", &format!("\"schema\": {schema},"));
            assert_eq!(
                validate_metadata("fmtlib", "fmt", "10.2.1", body.as_bytes()).unwrap_err(),
                UNSUPPORTED_SCHEMA,
                "schema: {schema}"
            );
        }
    }

    #[test]
    fn validate_metadata_rejects_unknown_fields_and_non_json() {
        assert_eq!(
            validate_metadata("fmtlib", "fmt", "1.0.0", b"not json").unwrap_err(),
            METADATA_NOT_JSON,
        );
        let with_extra = metadata_json("fmtlib", "fmt", "1.0.0")
            .replace("\"schema\": 1,", "\"schema\": 1,\n  \"extra-key\": true,");
        assert_eq!(
            validate_metadata("fmtlib", "fmt", "1.0.0", with_extra.as_bytes()).unwrap_err(),
            METADATA_NOT_CANONICAL,
        );
        // Unknown fields inside `source` are rejected too.
        let with_extra = metadata_json("fmtlib", "fmt", "1.0.0").replace(
            "\"type\": \"archive\",",
            "\"type\": \"archive\",\n    \"mirror\": \"x\",",
        );
        assert_eq!(
            validate_metadata("fmtlib", "fmt", "1.0.0", with_extra.as_bytes()).unwrap_err(),
            METADATA_NOT_CANONICAL,
        );
    }

    #[test]
    fn validate_metadata_requires_url_and_source_identity() {
        let body = metadata_json("fmtlib", "fmt", "10.2.1");
        // URL scope / name / version disagree with the document.
        assert_eq!(
            validate_metadata("other", "fmt", "10.2.1", body.as_bytes()).unwrap_err(),
            IDENTITY_MISMATCH,
        );
        assert_eq!(
            validate_metadata("fmtlib", "other", "10.2.1", body.as_bytes()).unwrap_err(),
            IDENTITY_MISMATCH,
        );
        assert_eq!(
            validate_metadata("fmtlib", "fmt", "9.0.0", body.as_bytes()).unwrap_err(),
            IDENTITY_MISMATCH,
        );
        // A bare (unscoped) document name never matches the URL pair.
        let bare = body.replace("\"name\": \"fmtlib/fmt\"", "\"name\": \"fmt\"");
        assert_eq!(
            validate_metadata("fmtlib", "fmt", "10.2.1", bare.as_bytes()).unwrap_err(),
            IDENTITY_MISMATCH,
        );
        // A source path pointing at some other artifact.
        let moved = body.replace(
            "../../artifacts/fmtlib/fmt/fmtlib-fmt-10.2.1.tar.gz",
            "../elsewhere.tar.gz",
        );
        assert_eq!(
            validate_metadata("fmtlib", "fmt", "10.2.1", moved.as_bytes()).unwrap_err(),
            IDENTITY_MISMATCH,
        );
        // The pre-scopes source path shape (bare directory, bare
        // filename) is not the canonical path any more.
        let unscoped = body.replace(
            "../../artifacts/fmtlib/fmt/fmtlib-fmt-10.2.1.tar.gz",
            "../artifacts/fmt/fmt-10.2.1.tar.gz",
        );
        assert_eq!(
            validate_metadata("fmtlib", "fmt", "10.2.1", unscoped.as_bytes()).unwrap_err(),
            IDENTITY_MISMATCH,
        );
    }

    #[test]
    fn validate_metadata_enforces_name_version_and_yanked_rules() {
        for (scope, name, version, detail) in [
            ("fmtlib", "_fmt", "1.0.0", INVALID_NAME),
            ("fmtlib", "-fmt", "1.0.0", INVALID_NAME),
            ("fmtlib", "Fmt", "1.0.0", INVALID_NAME),
            ("-fmtlib", "fmt", "1.0.0", INVALID_NAME),
            ("fmt_lib", "fmt", "1.0.0", INVALID_NAME),
            ("Fmtlib", "fmt", "1.0.0", INVALID_NAME),
            ("fmtlib", "fmt", "01.0.0", INVALID_VERSION),
            ("fmtlib", "fmt", "1.0.0-", INVALID_VERSION),
        ] {
            // The document carries the same name / version as the URL,
            // so identity passes and the charset / SemVer checks fire.
            let body = metadata_json(scope, name, version);
            assert_eq!(
                validate_metadata(scope, name, version, body.as_bytes()).unwrap_err(),
                detail,
                "scope: {scope}, name: {name}, version: {version}"
            );
        }
        let yanked = metadata_json("fmtlib", "fmt", "1.0.0")
            .replace("\"yanked\": false", "\"yanked\": true");
        assert_eq!(
            validate_metadata("fmtlib", "fmt", "1.0.0", yanked.as_bytes()).unwrap_err(),
            YANKED_AT_PUBLISH,
        );
    }

    #[test]
    fn verify_checksum_requires_the_exact_sha256_claim() {
        let body = metadata_json("fmtlib", "fmt", "1.0.0").replace("sha256:aa", "sha256:0011");
        let parsed = validate_metadata("fmtlib", "fmt", "1.0.0", body.as_bytes()).unwrap();
        assert_eq!(verify_checksum(&parsed, "0011"), Ok(()));
        assert_eq!(verify_checksum(&parsed, "0012"), Err(CHECKSUM_MISMATCH));
        // A claim without the scheme prefix never matches.
        let body = metadata_json("fmtlib", "fmt", "1.0.0").replace("sha256:aa", "0011");
        let parsed = validate_metadata("fmtlib", "fmt", "1.0.0", body.as_bytes()).unwrap();
        assert_eq!(verify_checksum(&parsed, "0011"), Err(CHECKSUM_MISMATCH));
    }
}
