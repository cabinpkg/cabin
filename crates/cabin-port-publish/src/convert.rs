//! Recipe → registry-package conversion.
//!
//! Turns a committed foundation-port recipe (its `port.toml`
//! descriptor plus overlay `cabin.toml` text) into the manifest text
//! of an ordinary registry package under the `cabin-ports` scope.
//! The committed overlay is never mutated: the conversion rewrites a
//! copy of its text with `toml_edit`, so upstream comments carry
//! into the published manifest.
//!
//! The conversion:
//! - renames `[package].name` to `cabin-ports/<lowercase-name>`;
//! - keeps `[package].version` at the upstream version verbatim
//!   (packaging corrections republish it as a new registry
//!   revision derived from the archive bytes);
//! - stamps `[package.upstream]` provenance from the recipe's
//!   `[source]` + `[[copy]]` tables;
//! - renames target keys to the intended native artifact stems
//!   (a target key directly determines its artifact stem, so
//!   `zlib`'s sole library target becomes `z` → `libz.a` / `z.lib`);
//! - follows those renames through the overlay's own target `deps`
//!   references, including the self-qualified `port:target` form.

use std::collections::BTreeMap;

use anyhow::{Context, Result, anyhow, bail, ensure};
use cabin_core::{PackageName, UpstreamCopy, UpstreamProvenance};
use cabin_port::{ArchiveKind, PortDescriptor};
use semver::Version;
use toml_edit::{DocumentMut, Item, Key, Table, value};

/// Registry scope every converted port publishes under.
pub const REGISTRY_SCOPE: &str = "cabin-ports";

/// Target keys whose lowercased recipe spelling is not the intended
/// native artifact stem.  Keyed by `(port name, recipe target key)`;
/// every other key lowercases (`cJSON` → `cjson`).  The stem choice
/// follows the upstream artifact each library ships as: `libz.a`,
/// `libpng.a`, `libgtest.a`.
const NATIVE_TARGET_KEYS: &[(&str, &str, &str)] = &[
    ("zlib", "zlib", "z"),
    ("libpng", "libpng", "png"),
    ("googletest", "googletest", "gtest"),
];

/// The registry target key for a recipe target.
#[must_use]
pub fn registry_target_key(port_name: &str, target_key: &str) -> String {
    NATIVE_TARGET_KEYS
        .iter()
        .find(|(port, key, _)| *port == port_name && *key == target_key)
        .map_or_else(
            || target_key.to_lowercase(),
            |(_, _, stem)| (*stem).to_owned(),
        )
}

/// The scoped registry name for a port: `cabin-ports/<lowercase>`.
///
/// # Errors
/// Returns an error when the lowercased name does not satisfy the
/// canonical registry grammar (`[a-z0-9][a-z0-9_-]*`), which the
/// hosted registry and the publish gates both enforce.
pub fn scoped_package_name(port_name: &str) -> Result<PackageName> {
    let lower = port_name.to_lowercase();
    let canonical = lower
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && lower
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if !canonical {
        bail!(
            "port name `{port_name}` does not lowercase to a canonical registry package name \
             (`[a-z0-9][a-z0-9_-]*`)"
        );
    }
    PackageName::new(format!("{REGISTRY_SCOPE}/{lower}"))
        .map_err(|err| anyhow!("scoped name for port `{port_name}` is invalid: {err}"))
}

/// The published identity a port converts to: its scoped registry
/// name, and the converted keys of its library-like targets — which
/// decide between the bare-package dependency shorthand and an
/// explicit `package:target` reference for anything consuming it.
#[derive(Debug, Clone)]
pub struct RecipeSummary {
    /// Scoped registry name (`cabin-ports/<lowercase>`).
    pub scoped: PackageName,
    /// Converted keys of the overlay's library-like targets
    /// (`library` / `header-only`), in manifest (name-sorted) order.
    pub library_like_target_keys: Vec<String>,
}

/// Summarize a parsed overlay under its published identity.
///
/// # Errors
/// Returns an error when the scoped name cannot be formed.
pub fn summarize(port_name: &str, overlay: &cabin_core::Package) -> Result<RecipeSummary> {
    Ok(RecipeSummary {
        scoped: scoped_package_name(port_name)?,
        library_like_target_keys: overlay
            .targets
            .iter()
            .filter(|t| t.kind.is_library_like())
            .map(|t| registry_target_key(port_name, t.name.as_str()))
            .collect(),
    })
}

/// Inputs to [`convert_overlay`].
#[derive(Debug)]
pub struct ConvertRequest<'a> {
    /// Parsed `port.toml` of the recipe being converted.
    pub descriptor: &'a PortDescriptor,
    /// Committed overlay `cabin.toml` text.
    pub overlay_text: &'a str,
    /// Summaries of every committed port, keyed by original port
    /// name; the conversion reads its own entry to rewrite
    /// self-qualified target references.
    pub summaries: &'a BTreeMap<String, RecipeSummary>,
}

/// Convert a committed overlay into the published-package manifest.
///
/// # Errors
/// Returns an error when the overlay does not parse, carries no
/// `[package]` table, or the converted manifest fails re-validation.
pub fn convert_overlay(request: &ConvertRequest<'_>) -> Result<String> {
    let port_name = request.descriptor.name.as_str();
    let summary = request
        .summaries
        .get(port_name)
        .ok_or_else(|| anyhow!("no summary for port `{port_name}`"))?;
    // The published version is the upstream version, verbatim: a
    // recipe correction republishes it as a new registry revision
    // derived from the changed archive bytes.
    let published = request.descriptor.version.clone();

    let overlay = cabin_manifest::parse_manifest_str(request.overlay_text)
        .with_context(|| format!("parsing committed overlay for port `{port_name}`"))?;
    ensure!(
        overlay.package.is_some(),
        "overlay for port `{port_name}` has no [package] table"
    );

    let mut doc: DocumentMut = request
        .overlay_text
        .parse()
        .with_context(|| format!("re-parsing overlay for port `{port_name}`"))?;

    doc["package"]["name"] = value(summary.scoped.as_str());
    doc["package"]["version"] = value(published.to_string());
    insert_upstream_table(&mut doc, request.descriptor)?;
    let target_renames = rename_targets(&mut doc, port_name)?;
    rewrite_target_dep_references(&mut doc, port_name, summary, &target_renames);

    let converted = doc.to_string();
    validate_converted(&converted, request, summary, &published)?;
    Ok(converted)
}

/// Build and insert the `[package.upstream]` table from the recipe's
/// pinned source and copy plan.  The declaration is validated by
/// constructing the typed [`UpstreamProvenance`] first, so a recipe
/// that could never verify fails the conversion here.
fn insert_upstream_table(doc: &mut DocumentMut, descriptor: &PortDescriptor) -> Result<()> {
    let provenance = descriptor_provenance(descriptor)?;

    let mut upstream = Table::new();
    upstream["url"] = value(provenance.url().as_str());
    upstream["sha256"] = value(provenance.sha256_hex());
    upstream["format"] = value(provenance.format().as_str());
    if let Some(prefix) = provenance.strip_prefix() {
        upstream["strip-prefix"] = value(prefix);
    }
    if !provenance.patches().is_empty() {
        // Before the `[[copy]]` array-of-tables: a plain key after it
        // would belong to the last copy entry.
        let mut patches = toml_edit::Array::new();
        for patch in provenance.patches() {
            patches.push(patch.as_str());
        }
        upstream["patches"] = value(patches);
    }
    if !provenance.copies().is_empty() {
        let mut copies = toml_edit::ArrayOfTables::new();
        for step in provenance.copies() {
            let mut copy = Table::new();
            copy["from"] = value(step.from().as_str());
            copy["to"] = value(step.to().as_str());
            copies.push(copy);
        }
        upstream["copy"] = Item::ArrayOfTables(copies);
    }

    let package = doc["package"]
        .as_table_mut()
        .ok_or_else(|| anyhow!("overlay [package] is not a table"))?;
    // Render the provenance right after the [package] keys instead
    // of at the document tail, where a position-less table would
    // land after the [target.*] sections.
    let mut upstream = upstream;
    upstream.set_position(Some(0));
    package.insert("upstream", Item::Table(upstream));
    Ok(())
}

/// The typed `[package.upstream]` declaration a recipe stamps.
///
/// # Errors
/// Returns an error when the recipe's source pin violates the
/// published-provenance rules (non-HTTPS URL, unverifiable copy
/// plan, ...).
pub fn descriptor_provenance(descriptor: &PortDescriptor) -> Result<UpstreamProvenance> {
    let copies = descriptor
        .copies
        .iter()
        .map(|step| UpstreamCopy::new(step.from.to_string(), step.to.to_string()))
        .collect::<Result<Vec<_>, _>>()
        .context("recipe [[copy]] step is not publishable as upstream provenance")?;
    UpstreamProvenance::new(
        descriptor.source.url.as_str(),
        &descriptor.source.sha256.to_hex(),
        ArchiveKind::from_url(&descriptor.source.url).extension(),
        descriptor.source.strip_prefix.clone(),
        copies,
        descriptor
            .patches
            .iter()
            .map(|patch| patch.as_str().to_owned())
            .collect(),
    )
    .with_context(|| {
        format!(
            "recipe source for port `{}` is not publishable as upstream provenance",
            descriptor.name.as_str()
        )
    })
}

/// Rename every buildable target key to its registry key.  Returns
/// the complete old-key → new-key map for the package's non-cfg
/// targets (identity entries included, so callers can recognize a
/// same-package target reference).  cfg-gated `[target.'cfg(...)']`
/// tables are conditions, not targets, and keep their keys.
fn rename_targets(doc: &mut DocumentMut, port_name: &str) -> Result<BTreeMap<String, String>> {
    let mut renames = BTreeMap::new();
    let Some(targets) = doc.get_mut("target").and_then(Item::as_table_mut) else {
        return Ok(renames);
    };
    let keys: Vec<String> = targets
        .iter()
        .map(|(key, _)| key.to_owned())
        .filter(|key| !is_cfg_key(key))
        .collect();
    for old_key in keys {
        let new_key = registry_target_key(port_name, &old_key);
        if new_key == old_key {
            renames.insert(old_key, new_key);
            continue;
        }
        if targets.contains_key(&new_key) {
            bail!("target key `{old_key}` renames to `{new_key}`, which already exists");
        }
        let (key, item) = targets
            .remove_entry(&old_key)
            .ok_or_else(|| anyhow!("target `{old_key}` disappeared during rename"))?;
        // Re-key in place: the table item keeps its position and
        // decor (the comment block above the header), so the
        // rendered manifest only changes in the header spelling.
        let new_key = Key::new(new_key.clone()).with_leaf_decor(key.leaf_decor().clone());
        targets.insert_formatted(&new_key, item);
        renames.insert(old_key, new_key.get().to_owned());
    }
    Ok(renames)
}

fn is_cfg_key(key: &str) -> bool {
    key.starts_with("cfg(") && key.ends_with(')')
}

/// Follow the target-key renames through the overlay's own `deps`
/// references.  Only same-package references move: a dependency is
/// already named by its published scoped identity in the overlay, so
/// nothing about it changes in the conversion.
fn rewrite_target_dep_references(
    doc: &mut DocumentMut,
    port_name: &str,
    self_summary: &RecipeSummary,
    target_renames: &BTreeMap<String, String>,
) {
    if target_renames.is_empty() {
        return;
    }
    let Some(targets) = doc.get_mut("target").and_then(Item::as_table_mut) else {
        return;
    };
    let keys: Vec<String> = targets
        .iter()
        .map(|(key, _)| key.to_owned())
        .filter(|key| !is_cfg_key(key))
        .collect();
    for key in keys {
        let Some(deps) = targets
            .get_mut(&key)
            .and_then(Item::as_table_like_mut)
            .and_then(|t| t.get_mut("deps"))
            .and_then(Item::as_value_mut)
            .and_then(toml_edit::Value::as_array_mut)
        else {
            continue;
        };
        for entry in deps.iter_mut() {
            let reference = match entry {
                toml_edit::Value::String(s) => s.value().clone(),
                toml_edit::Value::InlineTable(table) => {
                    match table.get("name").and_then(toml_edit::Value::as_str) {
                        Some(name) => name.to_owned(),
                        None => continue,
                    }
                }
                _ => continue,
            };
            let Some(replacement) =
                rewritten_dep_reference(&reference, port_name, self_summary, target_renames)
            else {
                continue;
            };
            match entry {
                toml_edit::Value::String(_) => {
                    let decor = entry.decor().clone();
                    let mut new_value = toml_edit::Value::from(replacement);
                    *new_value.decor_mut() = decor;
                    *entry = new_value;
                }
                toml_edit::Value::InlineTable(table) => {
                    table.insert("name", replacement.into());
                }
                _ => {}
            }
        }
    }
}

/// The rewritten form of one target `deps` reference, or `None` when
/// the reference needs no change - which is every reference naming
/// something outside this package.
fn rewritten_dep_reference(
    reference: &str,
    port_name: &str,
    self_summary: &RecipeSummary,
    target_renames: &BTreeMap<String, String>,
) -> Option<String> {
    if let Some((package_part, target_part)) = reference.split_once(':') {
        // Qualified `package:target`: the self-qualified form needs
        // the package half renamed, and the target half follows the
        // package's own key mapping.
        if package_part == port_name {
            let new_target = target_renames
                .get(target_part)
                .cloned()
                .unwrap_or_else(|| target_part.to_owned());
            return Some(format!("{}:{new_target}", self_summary.scoped.as_str()));
        }
        return None;
    }
    if let Some(new_key) = target_renames.get(reference) {
        return (new_key != reference).then(|| new_key.clone());
    }
    None
}

/// Re-parse the converted manifest and assert the conversion's
/// invariants, so a bug here fails the run instead of publishing a
/// malformed package.
fn validate_converted(
    converted: &str,
    request: &ConvertRequest<'_>,
    summary: &RecipeSummary,
    published: &Version,
) -> Result<()> {
    let parsed = cabin_manifest::parse_manifest_str(converted)
        .context("converted manifest does not parse")?;
    let package = parsed
        .package
        .ok_or_else(|| anyhow!("converted manifest has no [package]"))?;
    if package.name != summary.scoped {
        bail!(
            "converted manifest names `{}`, expected `{}`",
            package.name.as_str(),
            summary.scoped.as_str()
        );
    }
    if package.version != *published {
        bail!(
            "converted manifest version `{}`, expected `{published}`",
            package.version
        );
    }
    let expected = descriptor_provenance(request.descriptor)?;
    if package.upstream.as_ref() != Some(&expected) {
        bail!("converted manifest's [package.upstream] does not match the recipe source");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_port::model::CopyStep;
    use cabin_port::{ArchiveSource, OverlayManifest, PortChecksum, PortMetadata};
    use camino::Utf8PathBuf;
    use url::Url;

    const SHA: &str = "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23";

    fn descriptor(
        name: &str,
        version: &str,
        url: &str,
        strip_prefix: Option<&str>,
    ) -> PortDescriptor {
        PortDescriptor {
            name: PackageName::new(name).unwrap(),
            version: Version::parse(version).unwrap(),
            metadata: PortMetadata::default(),
            source: ArchiveSource {
                url: Url::parse(url).unwrap(),
                sha256: PortChecksum::parse_hex(SHA).unwrap(),
                strip_prefix: strip_prefix.map(str::to_owned),
            },
            overlay: OverlayManifest {
                relative_path: Utf8PathBuf::from("cabin.toml"),
            },
            copies: Vec::new(),
            patches: Vec::new(),
        }
    }

    const ZLIB_OVERLAY: &str = r#"[package]
name = "zlib"
version = "1.3.1"

# zlib's sole library target compiles the portable C sources.
[target.zlib]
type = "library"
sources = ["adler32.c"]
include-dirs = ["."]
c-standard = "c11"
links = "z"

[target.'cfg(family = "unix")'.profile]
defines = ["HAVE_UNISTD_H=1"]
"#;

    const LIBPNG_OVERLAY: &str = r#"[package]
name = "libpng"
version = "1.6.50"

[target.libpng]
type = "library"
sources = ["png.c"]
include-dirs = ["."]
c-standard = "c11"
"#;

    fn zlib_descriptor() -> PortDescriptor {
        descriptor(
            "zlib",
            "1.3.1",
            "https://example.com/zlib-1.3.1.tar.gz",
            Some("zlib-1.3.1"),
        )
    }

    fn summaries() -> BTreeMap<String, RecipeSummary> {
        let zlib = cabin_manifest::parse_manifest_str(ZLIB_OVERLAY)
            .unwrap()
            .package
            .unwrap();
        let libpng = cabin_manifest::parse_manifest_str(LIBPNG_OVERLAY)
            .unwrap()
            .package
            .unwrap();
        BTreeMap::from([
            ("zlib".to_owned(), summarize("zlib", &zlib).unwrap()),
            ("libpng".to_owned(), summarize("libpng", &libpng).unwrap()),
        ])
    }

    #[test]
    fn registry_target_keys_pick_native_artifact_stems() {
        assert_eq!(registry_target_key("zlib", "zlib"), "z");
        assert_eq!(registry_target_key("libpng", "libpng"), "png");
        assert_eq!(registry_target_key("googletest", "googletest"), "gtest");
        assert_eq!(registry_target_key("cJSON", "cJSON"), "cjson");
        assert_eq!(registry_target_key("CLI11", "CLI11"), "cli11");
        assert_eq!(registry_target_key("fmt", "fmt"), "fmt");
    }

    #[test]
    fn scoped_names_lowercase_under_the_ports_scope() {
        assert_eq!(
            scoped_package_name("cJSON").unwrap().as_str(),
            "cabin-ports/cjson"
        );
        assert_eq!(
            scoped_package_name("nlohmann_json").unwrap().as_str(),
            "cabin-ports/nlohmann_json"
        );
        // `.` is valid in a local package name but not in the
        // registry grammar, so the conversion refuses it up front.
        assert!(scoped_package_name("foo.bar").is_err());
    }

    #[test]
    fn converts_a_sole_library_port() {
        let descriptor = zlib_descriptor();
        let converted = convert_overlay(&ConvertRequest {
            descriptor: &descriptor,
            overlay_text: ZLIB_OVERLAY,
            summaries: &summaries(),
        })
        .unwrap();

        assert!(
            converted.contains("name = \"cabin-ports/zlib\""),
            "{converted}"
        );
        assert!(converted.contains("[target.z]"), "{converted}");
        assert!(!converted.contains("[target.zlib]"), "{converted}");
        // The links identity is independent of the target key: the
        // rename to `z` must carry the claim through unchanged.
        assert!(converted.contains("links = \"z\""), "{converted}");
        // The explanatory comment stays attached to the renamed target.
        assert!(
            converted.contains(
                "# zlib's sole library target compiles the portable C sources.\n[target.z]"
            ),
            "{converted}"
        );
        // cfg-gated profile tables are conditions, not targets.
        assert!(
            converted.contains("[target.'cfg(family = \"unix\")'.profile]"),
            "{converted}"
        );
        // Provenance is stamped from the recipe pin, before the targets.
        assert!(converted.contains("[package.upstream]"), "{converted}");
        assert!(
            converted.find("[package.upstream]").unwrap() < converted.find("[target.z]").unwrap(),
            "{converted}"
        );
        assert!(
            converted.contains("url = \"https://example.com/zlib-1.3.1.tar.gz\""),
            "{converted}"
        );
        assert!(
            converted.contains(&format!("sha256 = \"{SHA}\"")),
            "{converted}"
        );
        assert!(converted.contains("format = \"tar.gz\""), "{converted}");
        assert!(
            converted.contains("strip-prefix = \"zlib-1.3.1\""),
            "{converted}"
        );
    }

    #[test]
    fn stamps_copy_steps_into_the_upstream_table() {
        let mut descriptor = descriptor(
            "libpng",
            "1.6.50",
            "https://example.com/libpng-1.6.50.tar.gz",
            Some("libpng-1.6.50"),
        );
        descriptor.copies = vec![CopyStep {
            from: Utf8PathBuf::from("scripts/pnglibconf.h.prebuilt"),
            to: Utf8PathBuf::from("pnglibconf.h"),
        }];
        let converted = convert_overlay(&ConvertRequest {
            descriptor: &descriptor,
            overlay_text: LIBPNG_OVERLAY,
            summaries: &summaries(),
        })
        .unwrap();
        assert!(
            converted.contains("[[package.upstream.copy]]"),
            "{converted}"
        );
        assert!(
            converted.contains("from = \"scripts/pnglibconf.h.prebuilt\""),
            "{converted}"
        );
        assert!(converted.contains("to = \"pnglibconf.h\""), "{converted}");
    }

    #[test]
    fn stamps_patch_declarations_into_the_upstream_table() {
        let mut descriptor = descriptor(
            "libpng",
            "1.6.50",
            "https://example.com/libpng-1.6.50.tar.gz",
            Some("libpng-1.6.50"),
        );
        descriptor.copies = vec![CopyStep {
            from: Utf8PathBuf::from("scripts/pnglibconf.h.prebuilt"),
            to: Utf8PathBuf::from("pnglibconf.h"),
        }];
        descriptor.patches = vec![Utf8PathBuf::from("patches/0001-fix-msvc-build.patch")];
        let converted = convert_overlay(&ConvertRequest {
            descriptor: &descriptor,
            overlay_text: LIBPNG_OVERLAY,
            summaries: &summaries(),
        })
        .unwrap();
        assert!(
            converted.contains("patches = [\"patches/0001-fix-msvc-build.patch\"]"),
            "{converted}"
        );
        // The plain `patches` key must precede the `[[copy]]`
        // array-of-tables, or it would parse as a key of the last
        // copy entry.
        assert!(
            converted.find("patches = ").unwrap()
                < converted.find("[[package.upstream.copy]]").unwrap(),
            "{converted}"
        );
        // The stamped declaration round-trips through the validated
        // model (validate_converted re-parses the manifest).
        let parsed = cabin_manifest::parse_manifest_str(&converted).unwrap();
        let upstream = parsed.package.unwrap().upstream.unwrap();
        assert_eq!(
            upstream.patches(),
            [Utf8PathBuf::from("patches/0001-fix-msvc-build.patch")]
        );
    }

    #[test]
    fn zip_sources_declare_the_zip_format() {
        let descriptor = descriptor(
            "miniz",
            "3.1.2",
            "https://example.com/miniz-3.1.2.zip",
            None,
        );
        let overlay = r#"[package]
name = "miniz"
version = "3.1.2"

[target.miniz]
type = "library"
sources = ["miniz.c"]
include-dirs = ["."]
c-standard = "c11"
"#;
        let package = cabin_manifest::parse_manifest_str(overlay)
            .unwrap()
            .package
            .unwrap();
        let summaries =
            BTreeMap::from([("miniz".to_owned(), summarize("miniz", &package).unwrap())]);
        let converted = convert_overlay(&ConvertRequest {
            descriptor: &descriptor,
            overlay_text: overlay,
            summaries: &summaries,
        })
        .unwrap();
        assert!(converted.contains("format = \"zip\""), "{converted}");
        assert!(!converted.contains("strip-prefix"), "{converted}");
    }

    /// Self-qualified `package:target` references rename both halves.
    #[test]
    fn rewrites_self_qualified_references() {
        let overlay = r#"[package]
name = "zlib"
version = "1.3.1"

[target.zlib]
type = "library"
sources = ["adler32.c"]
c-standard = "c11"

[target.ztool]
type = "executable"
sources = ["ztool.c"]
c-standard = "c11"
deps = ["zlib:zlib"]
"#;
        let descriptor = zlib_descriptor();
        let converted = convert_overlay(&ConvertRequest {
            descriptor: &descriptor,
            overlay_text: overlay,
            summaries: &summaries(),
        })
        .unwrap();
        assert!(
            converted.contains("deps = [\"cabin-ports/zlib:z\"]"),
            "{converted}"
        );
    }

    #[test]
    fn conversion_is_deterministic() {
        let descriptor = zlib_descriptor();
        let request = ConvertRequest {
            descriptor: &descriptor,
            overlay_text: ZLIB_OVERLAY,
            summaries: &summaries(),
        };
        assert_eq!(
            convert_overlay(&request).unwrap(),
            convert_overlay(&request).unwrap()
        );
    }
}
