//! Placing a verified revision's archive in ARCHIVES and the documents
//! in INDEX: the pure half of the passes in `glue/publication.rs`.
//!
//! A verified revision is served only once its archive is confirmed in
//! ARCHIVES under the revision's own key (`docs/remote-registry.md`,
//! "Verification lifecycle"). The key names one byte sequence - the
//! revision id is the leading hex of the archive's SHA-256 - so placing
//! it again converges on the object already there, and an object that
//! holds anything else is never replaced.
//!
//! A document's key names whatever the document says now, so each
//! package's document and the catalog record in INDEX the generation
//! they were rendered at, and a writer replaces only an object it
//! found at an earlier one (`glue/publication.rs` argues what that
//! guarantees). The configuration records none: a pass replaces one
//! whose bytes differ.

use crate::checksum;

/// The `Cache-Control` R2 stores with a placed archive, and answers
/// with when the bucket's own domain serves it: the key never names
/// other bytes.
pub const CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// The `verification_reason` of a verified revision whose archive could
/// not be placed because its upload no longer holds the verified bytes.
/// The revision was never served and may be published again.
pub const SOURCE_LOST: &str = "publication_source_lost";

/// The ARCHIVES key of a revision, which is also its path under the
/// registry's `archives` area.
#[must_use]
pub fn archive_key(scope: &str, name: &str, version: &str, revision: &str) -> String {
    format!("archives/{scope}/{name}/{scope}-{name}-{version}-{revision}.zip")
}

/// The 32 bytes a canonical checksum names, for R2's integrity check
/// of a put. A non-canonical value (a corrupt row) names none.
#[must_use]
pub fn digest(checksum: &str) -> Option<Vec<u8>> {
    let hex = checksum::hex(checksum);
    if hex.is_empty() {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).ok())
        .collect()
}

/// Whether an object R2 reports with this size and SHA-256 holds the
/// archive a revision row records. R2 records a SHA-256 only for an
/// object put with one, which every put of this Worker is; an object
/// without it was not written here and holds nothing known.
#[must_use]
pub fn holds_archive(size: u64, sha256: Option<&[u8]>, archive_size: u64, checksum: &str) -> bool {
    size == archive_size && sha256.is_some() && sha256 == digest(checksum).as_deref()
}

/// The INDEX key of a package's document, which is also its path under
/// the registry's `packages` area.
#[must_use]
pub fn document_key(scope: &str, name: &str) -> String {
    format!("packages/{scope}/{name}.json")
}

/// The INDEX key of the catalog.
pub const CATALOG_KEY: &str = "catalog.json";

/// The `Cache-Control` R2 stores with a document, and answers with
/// when the bucket's own domain serves it.
pub const DOCUMENT_CACHE_CONTROL: &str = "public, max-age=60";

/// The INDEX key of the registry's configuration.
pub const CONFIG_KEY: &str = "config.json";

/// The `Cache-Control` R2 stores with the configuration, which changes
/// only with a deployment.
pub const CONFIG_CACHE_CONTROL: &str = "public, max-age=3600";

/// The bodies of the purge requests that name `urls`: Cloudflare takes
/// at most a hundred URLs in one request.
#[must_use]
pub fn purge_requests(urls: &[String]) -> Vec<String> {
    urls.chunks(100)
        .map(|files| serde_json::json!({ "files": files }).to_string())
        .collect()
}

/// Whether the body of a purge response reports that Cloudflare
/// accepted the purge.
#[must_use]
pub fn purged(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body).is_ok_and(|body| body["success"] == true)
}

/// How long a change stays open after it is recorded, in milliseconds:
/// the publish pass completes a change only when it started this long
/// after the change (`glue/publication.rs` argues why).
pub const SETTLE_MS: u64 = 15 * 60 * 1000;

/// The custom metadata key under which an INDEX object records the
/// generation its document was rendered at.
pub const GENERATION: &str = "generation";

/// Whether an INDEX object recording `held` is the document rendered
/// at `generation` or a later rendering of it. An object without a
/// generation was not written here and is replaced like an earlier
/// rendering.
#[must_use]
pub fn holds_document(held: Option<&str>, generation: i64) -> bool {
    held.and_then(|held| held.parse::<i64>().ok())
        .is_some_and(|held| held >= generation)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHECKSUM: &str =
        "sha256:00ff10a0b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72";

    #[test]
    fn digest_decodes_canonical_checksums_only() {
        let bytes = digest(CHECKSUM).expect("canonical");
        assert_eq!(bytes.len(), 32);
        assert_eq!(bytes[..4], [0x00, 0xff, 0x10, 0xa0]);
        for value in ["", "sha256:00ff", &CHECKSUM["sha256:".len()..]] {
            assert_eq!(digest(value), None, "{value:?}");
        }
    }

    #[test]
    fn an_object_holds_the_archive_only_with_its_size_and_recorded_digest() {
        let sha256 = digest(CHECKSUM).expect("canonical");
        assert!(holds_archive(10, Some(&sha256), 10, CHECKSUM));
        assert!(!holds_archive(11, Some(&sha256), 10, CHECKSUM));
        assert!(!holds_archive(10, None, 10, CHECKSUM));
        let mut other = sha256.clone();
        other[31] ^= 1;
        assert!(!holds_archive(10, Some(&other), 10, CHECKSUM));
        // A corrupt row names no digest, so nothing matches it.
        assert!(!holds_archive(10, Some(&sha256), 10, "sha256:00ff"));
        assert!(!holds_archive(10, None, 10, "sha256:00ff"));
    }

    #[test]
    fn an_object_holds_the_document_only_at_its_generation_or_a_later_one() {
        assert!(holds_document(Some("7"), 7));
        assert!(holds_document(Some("10"), 7), "compared as numbers");
        assert!(!holds_document(Some("6"), 7));
        assert!(!holds_document(None, 0));
        assert!(!holds_document(Some("seven"), 0));
    }

    #[test]
    fn a_purge_request_names_at_most_a_hundred_urls() {
        let urls: Vec<String> = (0..101)
            .map(|n| format!("https://index.example/{n}"))
            .collect();
        let requests = purge_requests(&urls);
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1], r#"{"files":["https://index.example/100"]}"#);
        assert!(purge_requests(&[]).is_empty());
    }

    #[test]
    fn only_a_reported_success_is_a_purge() {
        assert!(purged(
            r#"{"success":true,"errors":[],"messages":[],"result":{"id":"0"}}"#
        ));
        assert!(!purged(r#"{"success":false,"errors":[{"code":1}]}"#));
        assert!(!purged(r#"{"result":{"id":"0"}}"#));
        assert!(!purged("<html>"));
    }
}
