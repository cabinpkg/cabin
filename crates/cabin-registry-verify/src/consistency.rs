//! The consistency pass: the embedded manifest, parsed with the
//! workspace's real manifest parser, must reproduce the canonical
//! metadata the registry stored, and the archive bytes must hash to
//! the recorded checksum.
//!
//! Client-side publish derived the stored document via
//! `cabin_package::metadata::canonical_metadata` from this same
//! manifest; the verifier re-derives the document through that exact
//! seam and requires field-by-field agreement (JSON value equality,
//! not textual: key order and whitespace do not matter, but a
//! different value, a missing field, or an extra field does), so a
//! publisher cannot ship metadata (dependencies, standards, build
//! settings, ...) that the archive's manifest does not back.  The
//! manifest must also pass `cabin-package`'s publishability rules -
//! the real client cannot produce an archive that violates them, and
//! several of them (escaping source paths, `[patch]` tables) never
//! surface in the metadata document at all.

use camino::Utf8Path;

use cabin_package::metadata::canonical_metadata;

use crate::scan::Contents;
use crate::{PendingVersion, Reason, VerifyError};

/// Check the embedded manifest and the archive hash against the
/// listing entry.  `Ok(Some(reason))` is a rejection; `Ok(None)`
/// means every check passed.
///
/// # Errors
///
/// [`VerifyError::MalformedMetadata`] when the listing's metadata is
/// not a JSON object (the registry validated the document shape at
/// publish, so this is an infrastructure fault) - the version stays
/// pending.
pub(crate) fn check(
    manifest: &[u8],
    files: &Contents,
    pending: &PendingVersion,
    archive_hex: &str,
) -> Result<Option<Reason>, VerifyError> {
    let stored = pending
        .metadata
        .as_object()
        .ok_or(VerifyError::MalformedMetadata("metadata object"))?;

    // The embedded manifest must parse as a publishable single
    // package under the same rules `cabin package` enforces before
    // archiving.  Publish normalizes `{ workspace = true }` markers
    // into resolved literals before archiving, so the archived
    // manifest is self-contained and the plain parse is the whole
    // job.
    let Ok(text) = std::str::from_utf8(manifest) else {
        return Ok(Some(Reason::ManifestInvalid));
    };
    let Ok(parsed) = cabin_manifest::parse_manifest_str(text) else {
        return Ok(Some(Reason::ManifestInvalid));
    };
    let Some(package) = parsed.package else {
        return Ok(Some(Reason::ManifestInvalid));
    };
    if cabin_package::validate::validate_publishable(&package).is_err() {
        return Ok(Some(Reason::ManifestInvalid));
    }

    // Every source a target explicitly declares must be present in
    // the archive as a regular file.  `cabin package` archives the
    // real source tree, so a declared source that is absent means the
    // package would extract but fail to build (the planner emits a
    // compile action for a file that is not there) - a verified
    // version consumers cannot build.  Header-only targets declare no
    // sources (the parser enforces it), so they check nothing.
    for target in &package.targets {
        for source in &target.sources {
            match archive_path(source) {
                Some(path) if files.contains(&path) => {}
                // A source path that does not reduce to a plain
                // relative path cannot name an archive entry;
                // `validate_publishable` already rejected escaping or
                // absolute ones, so this is the archive missing it.
                _ => return Ok(Some(Reason::MissingSource)),
            }
        }
    }

    // Name, version, and archive bytes must agree with the listing
    // row the verdict will bind to.
    if package.name.as_str() != pending.name {
        return Ok(Some(Reason::NameMismatch));
    }
    if package.version.to_string() != pending.version {
        return Ok(Some(Reason::VersionMismatch));
    }
    if pending.checksum != archive_hex {
        return Ok(Some(Reason::ChecksumMismatch));
    }

    // Re-derive the entire canonical document through the exact seam
    // publish used and require field-level agreement (JSON value
    // equality).  Both sides serialize the same types with the same
    // omit-when-empty rules, so absence must match absence; a
    // different value, a missing field, or an extra field is a
    // rejection.  Textual canonicalization (key order, whitespace)
    // is not required - JSON object equality is key-based.
    let expected = serde_json::to_value(canonical_metadata(
        &package,
        &format!("sha256:{archive_hex}"),
    ))
    .expect("manifest-derived metadata always serializes");
    let expected = expected
        .as_object()
        .expect("canonical metadata serializes as an object");
    let mut keys: Vec<&str> = CHECK_ORDER.to_vec();
    for key in expected.keys().chain(stored.keys()).map(String::as_str) {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    for key in keys {
        if expected.get(key) != stored.get(key) {
            return Ok(Some(field_reason(key)));
        }
    }

    Ok(None)
}

/// Fields with specific reason codes are compared first, in this
/// order, so a document that is wrong in several ways reports the
/// most specific code deterministically.
const CHECK_ORDER: &[&str] = &[
    "name",
    "version",
    "dependencies",
    "dev-dependencies",
    "system-dependencies",
    "language",
    "standards",
    "checksum",
];

/// Reduce a manifest source path to the forward-slash relative form
/// the scan records for archive entries.  The scan's paths are fully
/// normalized (no `.` or `..` - `classify_path` rejects them), so a
/// manifest source must be normalized the same way to compare: `.`
/// dropped, and an internal `..` resolved against the preceding
/// component (`src/../main.cc` -> `main.cc`).  `cabin package`
/// accepts such a source and archives the resolved file, so the
/// verifier must not read it as absent.  Returns `None` only for a
/// path that cannot name a safe archive entry (a root, a prefix, or
/// a `..` that escapes) - `validate_publishable` already rejects
/// those, so the caller treats `None` as "absent".
fn archive_path(source: &Utf8Path) -> Option<String> {
    use camino::Utf8Component;
    let mut parts: Vec<&str> = Vec::new();
    for component in source.components() {
        match component {
            Utf8Component::Normal(name) => parts.push(name),
            Utf8Component::CurDir => {}
            // Within-root is guaranteed upstream, so an internal
            // `..` always has a component to cancel; a `..` with
            // nothing to pop would escape and cannot name an entry.
            Utf8Component::ParentDir if parts.pop().is_some() => {}
            _ => return None,
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

fn field_reason(key: &str) -> Reason {
    match key {
        "name" => Reason::NameMismatch,
        "version" => Reason::VersionMismatch,
        "dependencies" | "dev-dependencies" | "system-dependencies" => Reason::DependencyMismatch,
        "language" | "standards" => Reason::LanguageStandardMismatch,
        "checksum" => Reason::ChecksumMismatch,
        _ => Reason::MetadataMismatch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specific_fields_map_to_their_reason_codes() {
        assert_eq!(field_reason("name"), Reason::NameMismatch);
        assert_eq!(field_reason("version"), Reason::VersionMismatch);
        for key in ["dependencies", "dev-dependencies", "system-dependencies"] {
            assert_eq!(field_reason(key), Reason::DependencyMismatch);
        }
        for key in ["language", "standards"] {
            assert_eq!(field_reason(key), Reason::LanguageStandardMismatch);
        }
        assert_eq!(field_reason("checksum"), Reason::ChecksumMismatch);
        for key in [
            "schema",
            "features",
            "profiles",
            "toolchain",
            "build",
            "yanked",
            "source",
        ] {
            assert_eq!(field_reason(key), Reason::MetadataMismatch, "key: {key}");
        }
    }

    #[test]
    fn archive_path_normalizes_like_the_scan() {
        for (source, expected) in [
            ("src/main.cc", Some("src/main.cc")),
            ("./src/main.cc", Some("src/main.cc")),
            ("src/./main.cc", Some("src/main.cc")),
            // Internal `..` resolves - `cabin package` accepts and
            // archives the resolved file.
            ("src/../main.cc", Some("main.cc")),
            ("a/b/../c.cc", Some("a/c.cc")),
            ("main.cc", Some("main.cc")),
            // Cannot name a safe archive entry (upstream rejects
            // these anyway).
            ("../escape.cc", None),
            (".", None),
        ] {
            assert_eq!(
                archive_path(Utf8Path::new(source)).as_deref(),
                expected,
                "source: {source}"
            );
        }
    }
}
