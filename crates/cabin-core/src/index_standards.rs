//! Published-index standard-compatibility metadata (the `standards`
//! table of `docs/design/standard-compatibility/registry-index.md`).
//!
//! `cabin publish` derives the **declared** per-target requirement
//! table from the manifest and stores it in each version's index
//! entry, so index consumers - preference mode and publish lints - can
//! read a candidate version's per-target interface requirements without
//! downloading its source archive.  The stored cells are the declared
//! per-target `ReqOf` of spec D9 (header-only inference applied); the
//! transitive composition `R_L` (spec D10) depends on which dependency
//! version a resolution picks, so consumers compose - the publisher
//! stores declarations, not effective requirements.
//!
//! Absence encodes `unconstrained` at every granularity (whole table,
//! target row, or language key), so every pre-`standards` index entry
//! is a valid instance of this schema unchanged.  The field is additive
//! and the index document stays `schema = 1`; loaders must accept it
//! because the index rejects unknown fields.

use std::collections::BTreeMap;
use std::marker::PhantomData;

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::language_standard::{
    CStandard, CxxStandard, effective_gnu_extensions, resolve_language_standards,
};
use crate::model::Package;
use crate::standard_compatibility::{
    EffectiveRequirements, Requirement, dependency_attributes, req_of_c, req_of_cxx,
};

/// The `standards` table for one package version: the declared
/// per-target interface requirement table plus the per-target flags
/// index consumers need.  Keyed by library-like target name in a
/// `BTreeMap` for deterministic, sorted output.  An empty table is
/// the same as absence: everything unconstrained.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StandardsMetadata {
    /// Per-target rows, keyed by target name.  `cabin publish` writes
    /// one entry per library-like target of the version (`library` and
    /// `header-only` kinds); executables, tests, and examples never
    /// constrain consumers and are omitted.
    pub targets: BTreeMap<String, TargetStandards>,
}

impl StandardsMetadata {
    /// Whether the table carries no rows.  An empty table is omitted
    /// from the serialized index entry (absence = unconstrained).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// The version-wide effective requirement: per language, the join
    /// (spec D4) over every published target row's declared interface
    /// requirement.  An absent or all-unconstrained table joins to
    /// `unconstrained`.
    ///
    /// This is the candidate-version requirement that preference mode
    /// checks a consumer against when it lacks per-edge target scoping
    /// (the version-wide fallback described in section 1 of
    /// `docs/design/standard-compatibility/preference-mode.md`).  It can
    /// only over-constrain (a stricter `extras` target the consumer
    /// never links still counts), which is a preference-only lossiness
    /// the post-resolution validation corrects.
    #[must_use]
    pub fn version_wide_join(&self) -> EffectiveRequirements {
        EffectiveRequirements {
            c: Requirement::join_all(self.targets.values().map(|row| row.interface_c)),
            cxx: Requirement::join_all(self.targets.values().map(|row| row.interface_cxx)),
        }
    }

    /// Derive the declared per-target table from a resolved package.
    ///
    /// Every library-like target gets a row - even one whose
    /// requirements are all unconstrained and whose flags are false
    /// (the target existing and imposing nothing is itself
    /// information).  Each cell is the target's declared `ReqOf`
    /// (spec D9) for that language, computed through the shared
    /// [`dependency_attributes`] mapping so the stored table matches
    /// what the resolver-graph pass evaluates.
    #[must_use]
    pub fn from_package(package: &Package) -> Self {
        let resolved = resolve_language_standards(&package.language);
        let targets = package
            .targets
            .iter()
            .filter(|target| target.kind.is_library_like())
            .map(|target| {
                let attributes = dependency_attributes(target, &resolved, &package.language);
                let row = TargetStandards {
                    header_only: target.kind.is_header_only(),
                    gnu_extensions: effective_gnu_extensions(&package.language, target),
                    interface_c: req_of_c(&attributes),
                    interface_cxx: req_of_cxx(&attributes),
                };
                (target.name.as_str().to_owned(), row)
            })
            .collect();
        Self { targets }
    }
}

/// One target's row: the declared interface requirement for each
/// language (spec D9's `ReqOf`, header-only inference applied) plus
/// the two per-target flags.  A row whose requirements are all
/// unconstrained and whose flags are false serializes as `{}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetStandards {
    /// Spec D6 `kind`: whether the target is header-only.  Never
    /// enters the satisfaction predicate (D11 consumes only the
    /// requirement); carried so index consumers know it without an
    /// archive download.
    pub header_only: bool,
    /// The target's lowering-time GNU-dialect flag (spec D8,
    /// invariant I1).  Never participates in compatibility; carried
    /// so toolchain-aware tooling can surface per-target buildability.
    pub gnu_extensions: bool,
    /// Declared C interface requirement `ReqOf(t, C)` (spec D9).
    /// `Unconstrained` is serialized as an omitted language key.
    pub interface_c: Requirement<CStandard>,
    /// Declared C++ interface requirement `ReqOf(t, C++)` (spec D9).
    pub interface_cxx: Requirement<CxxStandard>,
}

impl Default for TargetStandards {
    fn default() -> Self {
        Self {
            header_only: false,
            gnu_extensions: false,
            interface_c: Requirement::Unconstrained,
            interface_cxx: Requirement::Unconstrained,
        }
    }
}

// ---------------------------------------------------------------------------
// Wire format.  A dedicated shape (rather than reusing the manifest's
// `InterfaceRequirement` serde) because the index cell omits the
// reserved `max` on write and rejects a populated `max` on read, and
// because absence of a cell means unconstrained.
// ---------------------------------------------------------------------------

impl Serialize for StandardsMetadata {
    fn serialize<Ser: Serializer>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error> {
        let raw = RawStandards {
            targets: self
                .targets
                .iter()
                .map(|(name, row)| (name.clone(), RawTarget::from(*row)))
                .collect(),
        };
        raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StandardsMetadata {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawStandards::deserialize(deserializer)?;
        Ok(Self {
            targets: raw
                .targets
                .into_iter()
                .map(|(name, row)| (name, TargetStandards::from(row)))
                .collect(),
        })
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawStandards {
    #[serde(default)]
    targets: BTreeMap<String, RawTarget>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawTarget {
    #[serde(
        rename = "header-only",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    header_only: bool,
    #[serde(
        rename = "gnu-extensions",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    gnu_extensions: bool,
    #[serde(default, skip_serializing_if = "RawInterface::is_empty")]
    interface: RawInterface,
}

/// The `interface` sub-table: a language key (`"c"`, `"c++"`, in that
/// fixed order) maps to a cell.  A missing key is unconstrained.
#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawInterface {
    #[serde(rename = "c", default, skip_serializing_if = "Option::is_none")]
    c: Option<Cell<CStandard>>,
    #[serde(rename = "c++", default, skip_serializing_if = "Option::is_none")]
    cxx: Option<Cell<CxxStandard>>,
}

impl RawInterface {
    fn is_empty(&self) -> bool {
        self.c.is_none() && self.cxx.is_none()
    }
}

impl From<TargetStandards> for RawTarget {
    fn from(row: TargetStandards) -> Self {
        Self {
            header_only: row.header_only,
            gnu_extensions: row.gnu_extensions,
            interface: RawInterface {
                c: cell_of(row.interface_c),
                cxx: cell_of(row.interface_cxx),
            },
        }
    }
}

impl From<RawTarget> for TargetStandards {
    fn from(raw: RawTarget) -> Self {
        Self {
            header_only: raw.header_only,
            gnu_extensions: raw.gnu_extensions,
            interface_c: requirement_of(raw.interface.c),
            interface_cxx: requirement_of(raw.interface.cxx),
        }
    }
}

/// One serialized `(target, language)` cell: the literal string
/// `"none"` for `Requirement::Forbidden`, or a `{ "min": "<level>" }`
/// table for `Requirement::Min`.  `Requirement::Unconstrained` is
/// never a cell - the language key is omitted instead - so this type
/// only encodes the two constrained shapes, which are deliberately
/// distinct wire forms (never the same requirement).
#[derive(Debug, Clone, Copy)]
enum Cell<S> {
    Forbidden,
    Min(S),
}

fn cell_of<S>(requirement: Requirement<S>) -> Option<Cell<S>> {
    match requirement {
        Requirement::Unconstrained => None,
        Requirement::Min(min) => Some(Cell::Min(min)),
        Requirement::Forbidden => Some(Cell::Forbidden),
    }
}

fn requirement_of<S>(cell: Option<Cell<S>>) -> Requirement<S> {
    match cell {
        None => Requirement::Unconstrained,
        Some(Cell::Min(min)) => Requirement::Min(min),
        Some(Cell::Forbidden) => Requirement::Forbidden,
    }
}

impl<S: Serialize> Serialize for Cell<S> {
    fn serialize<Ser: Serializer>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error> {
        match self {
            // `max` is reserved and never written in v1 (spec D4
            // remark), so a minimum serializes as a single-key table.
            Cell::Forbidden => serializer.serialize_str("none"),
            Cell::Min(min) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("min", min)?;
                map.end()
            }
        }
    }
}

impl<'de, S: Deserialize<'de>> Deserialize<'de> for Cell<S> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CellVisitor<S>(PhantomData<S>);

        impl<'de, S: Deserialize<'de>> Visitor<'de> for CellVisitor<S> {
            type Value = Cell<S>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(r#"`"none"` or a `{ "min": "<level>" }` requirement table"#)
            }

            // A bare level string (`"c++17"`) is not a valid cell:
            // writers must use the object form for minima.
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value == "none" {
                    Ok(Cell::Forbidden)
                } else {
                    Err(E::invalid_value(
                        de::Unexpected::Str(value),
                        &r#"the string "none" or a `{ "min": "<level>" }` table (a bare standard string is not a valid requirement cell)"#,
                    ))
                }
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut min: Option<S> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "min" => {
                            if min.is_some() {
                                return Err(de::Error::duplicate_field("min"));
                            }
                            min = Some(map.next_value()?);
                        }
                        // `max` is reserved for a future version (spec
                        // D4 remark).  A populated `max` is rejected
                        // with the same "range reserved" diagnostic the
                        // manifest parser gives range-like inputs; an
                        // explicit `null` is accepted as unpopulated.
                        "max" => {
                            let max: Option<S> = map.next_value()?;
                            if max.is_some() {
                                return Err(de::Error::custom(
                                    "range requirements are reserved for a future version of Cabin; the `max` field of an interface requirement must not be set",
                                ));
                            }
                        }
                        other => {
                            return Err(de::Error::unknown_field(other, &["min", "max"]));
                        }
                    }
                }
                match min {
                    Some(min) => Ok(Cell::Min(min)),
                    None => Err(de::Error::missing_field("min")),
                }
            }
        }

        deserializer.deserialize_any(CellVisitor(PhantomData))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language_standard::{InterfaceRequirement, StandardRequirement};
    use crate::model::{Package, PackageName, Target, TargetKind, TargetName};
    use crate::{LanguageStandardSettings, StandardDeclaration};
    use camino::Utf8PathBuf;

    fn interface_min<S>(min: S) -> InterfaceRequirement<S> {
        InterfaceRequirement::Requirement(StandardRequirement { min, max: None })
    }

    fn target(
        name: &str,
        kind: TargetKind,
        sources: &[&str],
        language: LanguageStandardSettings,
    ) -> Target {
        Target {
            name: TargetName::new(name).unwrap(),
            kind,
            sources: sources.iter().map(Utf8PathBuf::from).collect(),
            include_dirs: Vec::new(),
            defines: Vec::new(),
            deps: Vec::new(),
            required_features: Vec::new(),
            language,
        }
    }

    fn package(targets: Vec<Target>, language: LanguageStandardSettings) -> Package {
        let mut package = Package::new(
            PackageName::new("demo").unwrap(),
            semver_ver(),
            targets,
            Vec::new(),
        )
        .unwrap();
        package.language = language;
        package
    }

    fn semver_ver() -> semver::Version {
        semver::Version::parse("1.0.0").unwrap()
    }

    fn to_json(metadata: &StandardsMetadata) -> serde_json::Value {
        serde_json::to_value(metadata).unwrap()
    }

    /// A compiled C++ library declaring `interface-cxx-standard =
    /// "c++17"` gets `c++` = min c++17 (D9 row 2) and `c` = `"none"`
    /// (D9 row 6, the strict C++-to-C default written explicitly);
    /// executables are omitted, and an undeclared library is `{}`.
    #[test]
    fn from_package_derives_declared_table() {
        let lib = target(
            "fmt",
            TargetKind::Library,
            &["src/fmt.cc"],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                interface_cxx_standard: Some(StandardDeclaration::Declared(interface_min(
                    CxxStandard::Cxx17,
                ))),
                ..Default::default()
            },
        );
        let bin = target(
            "app",
            TargetKind::Executable,
            &["src/main.cc"],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                ..Default::default()
            },
        );
        let metadata = StandardsMetadata::from_package(&package(
            vec![lib, bin],
            LanguageStandardSettings::default(),
        ));

        // Only the library-like target appears.
        assert_eq!(metadata.targets.keys().collect::<Vec<_>>(), ["fmt"]);
        let row = &metadata.targets["fmt"];
        assert_eq!(row.interface_cxx, Requirement::Min(CxxStandard::Cxx17));
        // Row 6: no C implementation, no C interface -> forbidden.
        assert_eq!(row.interface_c, Requirement::Forbidden);
        assert!(!row.header_only);
        assert!(!row.gnu_extensions);

        assert_eq!(
            to_json(&metadata),
            serde_json::json!({
                "targets": {
                    "fmt": { "interface": { "c": "none", "c++": { "min": "c++17" } } }
                }
            })
        );
    }

    /// Header-only inference (D9 row 3): a header-only C++ library
    /// with no interface declaration infers `c++` = min from its
    /// implementation standard, and records the `header-only` flag.
    #[test]
    fn header_only_target_infers_and_flags() {
        let header_only = target(
            "hdr",
            TargetKind::HeaderOnly,
            &[],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                ..Default::default()
            },
        );
        let metadata = StandardsMetadata::from_package(&package(
            vec![header_only],
            LanguageStandardSettings::default(),
        ));
        let row = &metadata.targets["hdr"];
        assert!(row.header_only);
        assert_eq!(row.interface_cxx, Requirement::Min(CxxStandard::Cxx20));
        assert_eq!(
            to_json(&metadata),
            serde_json::json!({
                "targets": {
                    "hdr": {
                        "header-only": true,
                        "interface": { "c": "none", "c++": { "min": "c++20" } }
                    }
                }
            })
        );
    }

    /// A C library declaring `interface-c-standard` gets `c` = min
    /// (D9 row 2), `c++` = unconstrained (D9 row 5, the permissive
    /// C-to-C++ default, omitted), and its `gnu-extensions` flag is
    /// carried.
    #[test]
    fn c_library_with_gnu_extensions() {
        let lib = target(
            "clib",
            TargetKind::Library,
            &["src/clib.c"],
            LanguageStandardSettings {
                c_standard: Some(StandardDeclaration::Declared(CStandard::C11)),
                interface_c_standard: Some(StandardDeclaration::Declared(interface_min(
                    CStandard::C11,
                ))),
                gnu_extensions: Some(true),
                ..Default::default()
            },
        );
        let metadata = StandardsMetadata::from_package(&package(
            vec![lib],
            LanguageStandardSettings::default(),
        ));
        let row = &metadata.targets["clib"];
        assert_eq!(row.interface_c, Requirement::Min(CStandard::C11));
        // Permissive C-to-C++ default: unconstrained, so no `c++` key.
        assert_eq!(row.interface_cxx, Requirement::Unconstrained);
        assert!(row.gnu_extensions);
        assert_eq!(
            to_json(&metadata),
            serde_json::json!({
                "targets": {
                    "clib": {
                        "gnu-extensions": true,
                        "interface": { "c": { "min": "c11" } }
                    }
                }
            })
        );
    }

    /// An undeclared library-like target still gets a row, serialized
    /// as `{}` - the target existing and imposing nothing is itself
    /// information.  (Row 5 for C++, but row 6 makes C forbidden, so a
    /// truly empty `{}` needs no declared C either; use a header-only
    /// target with no standards at all, which the manifest layer would
    /// reject in practice but the derivation handles.)
    #[test]
    fn unconstrained_row_serializes_as_empty_object() {
        let mut metadata = StandardsMetadata::default();
        metadata
            .targets
            .insert("lib".to_owned(), TargetStandards::default());
        assert_eq!(
            to_json(&metadata),
            serde_json::json!({ "targets": { "lib": {} } })
        );
        // And it round-trips back to an all-unconstrained row.
        let parsed: StandardsMetadata = serde_json::from_value(to_json(&metadata)).unwrap();
        assert_eq!(parsed, metadata);
    }

    /// `version_wide_join` folds every row's declared requirement per
    /// language (spec D4): the strictest `c++` minimum and the `"none"`
    /// forbidden C cell both surface; an all-unconstrained table joins
    /// to unconstrained.
    #[test]
    fn version_wide_join_takes_the_strictest_per_language() {
        assert_eq!(
            StandardsMetadata::default().version_wide_join(),
            EffectiveRequirements {
                c: Requirement::Unconstrained,
                cxx: Requirement::Unconstrained,
            }
        );
        let mut metadata = StandardsMetadata::default();
        metadata.targets.insert(
            "core".to_owned(),
            TargetStandards {
                interface_c: Requirement::Forbidden,
                interface_cxx: Requirement::Min(CxxStandard::Cxx17),
                ..Default::default()
            },
        );
        metadata.targets.insert(
            "extras".to_owned(),
            TargetStandards {
                interface_cxx: Requirement::Min(CxxStandard::Cxx20),
                ..Default::default()
            },
        );
        assert_eq!(
            metadata.version_wide_join(),
            EffectiveRequirements {
                c: Requirement::Forbidden,
                cxx: Requirement::Min(CxxStandard::Cxx20),
            }
        );
    }

    /// Absence of the whole table is the empty table.
    #[test]
    fn empty_table_round_trips() {
        let metadata = StandardsMetadata::default();
        assert!(metadata.is_empty());
        assert_eq!(to_json(&metadata), serde_json::json!({ "targets": {} }));
        let parsed: StandardsMetadata =
            serde_json::from_value(serde_json::json!({ "targets": {} })).unwrap();
        assert_eq!(parsed, metadata);
        // A `standards` object may omit `targets` entirely.
        let parsed: StandardsMetadata = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(parsed, metadata);
    }

    /// The three requirement shapes round-trip through JSON: omitted
    /// key (unconstrained), `{min}` (minimum), and `"none"`
    /// (forbidden).
    #[test]
    fn cells_round_trip() {
        let mut metadata = StandardsMetadata::default();
        metadata.targets.insert(
            "lib".to_owned(),
            TargetStandards {
                header_only: false,
                gnu_extensions: false,
                interface_c: Requirement::Forbidden,
                interface_cxx: Requirement::Min(CxxStandard::Cxx20),
            },
        );
        let json = to_json(&metadata);
        assert_eq!(
            json,
            serde_json::json!({
                "targets": {
                    "lib": { "interface": { "c": "none", "c++": { "min": "c++20" } } }
                }
            })
        );
        let parsed: StandardsMetadata = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, metadata);
    }

    /// A populated `max` is rejected with the reserved-range
    /// diagnostic; an explicit `max: null` is accepted as unpopulated.
    #[test]
    fn populated_max_is_rejected() {
        let err = serde_json::from_value::<StandardsMetadata>(serde_json::json!({
            "targets": { "lib": { "interface": { "c++": { "min": "c++17", "max": "c++20" } } } }
        }))
        .unwrap_err();
        assert!(
            err.to_string().contains("reserved for a future version"),
            "unexpected error: {err}"
        );

        let parsed: StandardsMetadata = serde_json::from_value(serde_json::json!({
            "targets": { "lib": { "interface": { "c++": { "min": "c++17", "max": null } } } }
        }))
        .unwrap();
        assert_eq!(
            parsed.targets["lib"].interface_cxx,
            Requirement::Min(CxxStandard::Cxx17)
        );
    }

    /// A bare standard string is not a valid cell (writers must use
    /// the object form for minima).
    #[test]
    fn bare_standard_string_is_rejected() {
        let err = serde_json::from_value::<StandardsMetadata>(serde_json::json!({
            "targets": { "lib": { "interface": { "c++": "c++17" } } }
        }))
        .unwrap_err();
        assert!(
            err.to_string().contains("bare standard string")
                || err.to_string().contains("invalid value"),
            "unexpected error: {err}"
        );
    }

    /// Unknown fields anywhere in the table are rejected.
    #[test]
    fn unknown_fields_are_rejected() {
        assert!(
            serde_json::from_value::<StandardsMetadata>(serde_json::json!({
                "targets": { "lib": { "surprise": true } }
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<StandardsMetadata>(serde_json::json!({
                "targets": { "lib": { "interface": { "rust": { "min": "c++17" } } } }
            }))
            .is_err()
        );
    }
}
