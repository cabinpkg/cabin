//! The canonical checksum spelling, mirrored from
//! `cabin_core::checksum` because this worker is a standalone wasm
//! workspace that must not pull the client data-model crate into its
//! runtime tree (`docs/architecture.md`).  The parity test at the
//! bottom pins this mirror to the shared type the same way
//! `src/names.rs` pins its device-stem list to `cabin-fs`, so the two
//! grammars cannot drift silently.
//!
//! Every checksum this worker stores, serves, or accepts is the full
//! `sha256:<64 lowercase hex>` spelling; the bare hex tail exists
//! only inside derived values (R2 blob keys, packaging-revision ids),
//! obtained by slicing a value that already passed [`is_canonical`].

/// The one supported algorithm prefix.
pub const PREFIX: &str = "sha256:";

/// Whether `value` is the canonical spelling: exactly `sha256:`
/// followed by 64 lowercase hexadecimal characters.
#[must_use]
pub fn is_canonical(value: &str) -> bool {
    value.strip_prefix(PREFIX).is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// The 64-character bare hex tail of a canonical value.  A
/// non-canonical value (a corrupt row - the clean-break column never
/// stores one) yields the empty tail, so every derived key collapses
/// to the bare `blobs/sha256/` prefix: an address no object can ever
/// occupy - a pre-change bare-hex row must NOT round-trip back to its
/// old, still-existing key - and the read fails closed as a logged
/// miss instead of serving or panicking.
#[must_use]
pub fn hex(value: &str) -> &str {
    if is_canonical(value) {
        &value[PREFIX.len()..]
    } else {
        ""
    }
}

/// The packaging-revision id a canonical checksum mints: the leading
/// 16 characters of its bare hex tail.  A value too short to carry
/// one (never a canonical checksum) yields the empty id, which no
/// revision row can match.
#[must_use]
pub fn revision_id(value: &str) -> &str {
    hex(value).get(..16).unwrap_or_default()
}

/// Attach the algorithm prefix to a freshly computed 64-hex digest.
#[must_use]
pub fn from_hex(hex: &str) -> String {
    format!("{PREFIX}{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23";

    #[test]
    fn accepts_only_the_canonical_spelling() {
        assert!(is_canonical(&from_hex(SHA)));
        for value in [
            SHA,
            "sha256:deadbeef",
            "SHA256:9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23",
            "sha256:9A93B2B7DFDAC77CEBA5A558A580E74667DD6FEDE4585B91EEFB60F03B72DF23",
            "sha256:",
            "",
            "sha256:9a93b2b7dfdac77c",
        ] {
            assert!(!is_canonical(value), "{value:?}");
        }
    }

    #[test]
    fn derived_values_slice_the_canonical_spelling() {
        let value = from_hex(SHA);
        assert_eq!(hex(&value), SHA);
        assert_eq!(revision_id(&value), "9a93b2b7dfdac77c");
    }

    /// Corrupt column values must degrade to impossible keys and
    /// ids, never panic a request or alias a real object: the
    /// helpers are total for any string, and a pre-change BARE hex
    /// row - whose old key still exists in R2 - must not round-trip
    /// back to it.
    #[test]
    fn non_canonical_values_degrade_without_panicking() {
        let bare = "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23";
        for corrupt in ["", "aa", "sha256:", "sha256:short", "é", bare] {
            assert_eq!(hex(corrupt), "", "{corrupt:?}");
            assert_eq!(revision_id(corrupt), "", "{corrupt:?}");
        }
    }

    /// Host-only drift pin: the mirror accepts exactly what the
    /// shared client type accepts, over a corpus covering every
    /// rejection class the client's own tests enumerate.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn mirror_matches_the_shared_client_grammar() {
        let corpus: Vec<String> = vec![
            from_hex(SHA),
            SHA.to_owned(),
            format!("sha512:{SHA}"),
            format!("sha256:{}", SHA.to_uppercase()),
            format!("SHA256:{SHA}"),
            format!("sha256:g{}", &SHA[1..]),
            format!("sha256:sha256:{SHA}"),
            format!(" sha256:{SHA}"),
            format!("sha256:{SHA} "),
            format!("sha256:{SHA}ff"),
            "sha256:".to_owned(),
            String::new(),
            "sha256:9a93b2b7dfdac77c".to_owned(),
            from_hex(&"a".repeat(64)),
        ];
        for value in corpus {
            assert_eq!(
                is_canonical(&value),
                cabin_core::Checksum::parse(&value).is_ok(),
                "mirror and cabin_core::Checksum disagree on {value:?}"
            );
        }
    }
}
