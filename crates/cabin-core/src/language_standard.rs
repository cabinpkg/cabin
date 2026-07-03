//! Typed C/C++ language standards.
//!
//! Owns the standard enums (ISO levels only; GNU extensions are the
//! orthogonal per-target `gnu-extensions` boolean), the manifest
//! declaration shape shared by `[package]` and `[target.<name>]`,
//! effective-standard resolution (target ▶ package; there is no
//! built-in default - a target that compiles a language without a
//! declared standard is a manifest error), interface-requirement
//! relevance and fallback, the escape-hatch conflict detector, the
//! interface/implementation contradiction lint, and the per-package
//! summary that feeds `BuildConfiguration` fingerprinting and the
//! metadata view.  Pure data and logic only; no I/O.  See
//! `docs/language-standards.md` for the user-facing contract.

use std::collections::BTreeMap;
use std::marker::PhantomData;

use serde::de::{MapAccess, Visitor, value::MapAccessDeserializer};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ResolvedProfileFlags, SourceLanguage, Target, classify_source};

/// C language standards Cabin can request, oldest to newest.  The
/// `Ord` derive follows declaration order, which is the plain
/// chronological chain (in particular `c11 < c17`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CStandard {
    #[serde(rename = "c89")]
    C89,
    #[serde(rename = "c99")]
    C99,
    #[serde(rename = "c11")]
    C11,
    #[serde(rename = "c17")]
    C17,
    #[serde(rename = "c23")]
    C23,
}

impl CStandard {
    pub const ALL: [Self; 5] = [Self::C89, Self::C99, Self::C11, Self::C17, Self::C23];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::C89 => "c89",
            Self::C99 => "c99",
            Self::C11 => "c11",
            Self::C17 => "c17",
            Self::C23 => "c23",
        }
    }

    /// # Errors
    /// Returns [`LanguageStandardParseError`] when `value` is not a
    /// recognized C standard: a dedicated variant for range-like
    /// inputs and for the interface-only `none`, otherwise the
    /// invalid-value error listing the accepted identifiers.
    /// `c90` parses as an alias of `c89`, normalized immediately.
    pub fn parse(value: &str) -> Result<Self, LanguageStandardParseError> {
        reject_non_identifier(SourceLanguage::C, value)?;
        let normalized = if value == "c90" { "c89" } else { value };
        Self::ALL
            .into_iter()
            .find(|s| s.as_str() == normalized)
            .ok_or_else(|| LanguageStandardParseError::Unknown {
                language: SourceLanguage::C,
                value: value.to_owned(),
            })
    }
}

impl std::fmt::Display for CStandard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for CStandard {
    type Err = LanguageStandardParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// C++ language standards Cabin can request, oldest to newest, in
/// the plain chronological chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CxxStandard {
    #[serde(rename = "c++98")]
    Cxx98,
    #[serde(rename = "c++11")]
    Cxx11,
    #[serde(rename = "c++14")]
    Cxx14,
    #[serde(rename = "c++17")]
    Cxx17,
    #[serde(rename = "c++20")]
    Cxx20,
    #[serde(rename = "c++23")]
    Cxx23,
    #[serde(rename = "c++26")]
    Cxx26,
}

impl CxxStandard {
    pub const ALL: [Self; 7] = [
        Self::Cxx98,
        Self::Cxx11,
        Self::Cxx14,
        Self::Cxx17,
        Self::Cxx20,
        Self::Cxx23,
        Self::Cxx26,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cxx98 => "c++98",
            Self::Cxx11 => "c++11",
            Self::Cxx14 => "c++14",
            Self::Cxx17 => "c++17",
            Self::Cxx20 => "c++20",
            Self::Cxx23 => "c++23",
            Self::Cxx26 => "c++26",
        }
    }

    /// # Errors
    /// Returns [`LanguageStandardParseError`] when `value` is not a
    /// recognized C++ standard: a dedicated variant for range-like
    /// inputs and for the interface-only `none`, otherwise the
    /// invalid-value error listing the accepted identifiers.
    /// `c++03` parses as an alias of `c++98`, normalized
    /// immediately.
    pub fn parse(value: &str) -> Result<Self, LanguageStandardParseError> {
        reject_non_identifier(SourceLanguage::Cxx, value)?;
        let normalized = if value == "c++03" { "c++98" } else { value };
        Self::ALL
            .into_iter()
            .find(|s| s.as_str() == normalized)
            .ok_or_else(|| LanguageStandardParseError::Unknown {
                language: SourceLanguage::Cxx,
                value: value.to_owned(),
            })
    }
}

impl std::fmt::Display for CxxStandard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for CxxStandard {
    type Err = LanguageStandardParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// The shared pre-lookup checks: range-like inputs and the
/// interface-only `none` get dedicated diagnostics on every field
/// that parses a standard value.
fn reject_non_identifier(
    language: SourceLanguage,
    value: &str,
) -> Result<(), LanguageStandardParseError> {
    // `>=` / `<=` are covered by their first character.
    if value.contains(['>', '<', ',']) {
        return Err(LanguageStandardParseError::RangeReserved {
            language,
            value: value.to_owned(),
        });
    }
    if value == "none" {
        return Err(LanguageStandardParseError::NoneOnImplementation { language });
    }
    Ok(())
}

/// An invalid manifest standard value.  Range-like inputs and the
/// misplaced interface-only `none` get dedicated variants; anything
/// else is the invalid-value error listing the accepted
/// identifiers.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LanguageStandardParseError {
    #[error(
        "unknown {} standard `{value}`: expected one of {}",
        .language.human_label(),
        valid_standard_values(*.language)
    )]
    Unknown {
        language: SourceLanguage,
        value: String,
    },
    #[error(
        "range requirement `{value}` is reserved for a future version of Cabin; declare a single {} standard",
        .language.human_label()
    )]
    RangeReserved {
        language: SourceLanguage,
        value: String,
    },
    #[error(
        "`none` is only valid on `interface-c-standard` / `interface-cxx-standard`, where it marks the target's headers as not consumable from that language; compiled {} sources need a concrete standard",
        .language.human_label()
    )]
    NoneOnImplementation { language: SourceLanguage },
}

fn valid_standard_values(language: SourceLanguage) -> String {
    match language {
        SourceLanguage::C => CStandard::ALL.map(CStandard::as_str).join(", "),
        SourceLanguage::Cxx => CxxStandard::ALL.map(CxxStandard::as_str).join(", "),
    }
}

/// The implementation-standard field family for `language`
/// (`c-standard` / `cxx-standard`), for diagnostics.
const fn implementation_field(language: SourceLanguage) -> &'static str {
    match language {
        SourceLanguage::C => "c-standard",
        SourceLanguage::Cxx => "cxx-standard",
    }
}

/// One per-compile standard value, carried by the build IR.  Encodes
/// the source language, so the dialect lowering derives both the
/// rule kind and the standard flag from this single field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LanguageStandard {
    C(CStandard),
    Cxx(CxxStandard),
}

impl LanguageStandard {
    pub const fn language(self) -> SourceLanguage {
        match self {
            Self::C(_) => SourceLanguage::C,
            Self::Cxx(_) => SourceLanguage::Cxx,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::C(s) => s.as_str(),
            Self::Cxx(s) => s.as_str(),
        }
    }

    /// The `/std:` value `cl.exe` accepts for this standard, when a
    /// stable one exists.  `None` marks the MSVC-dialect gaps
    /// (C89/C99/C23, C++98/11/23/26); the planner rejects those
    /// before lowering on the MSVC dialect.
    pub const fn msvc_spelling(self) -> Option<&'static str> {
        match self {
            Self::C(CStandard::C11) => Some("/std:c11"),
            Self::C(CStandard::C17) => Some("/std:c17"),
            Self::Cxx(CxxStandard::Cxx14) => Some("/std:c++14"),
            Self::Cxx(CxxStandard::Cxx17) => Some("/std:c++17"),
            Self::Cxx(CxxStandard::Cxx20) => Some("/std:c++20"),
            _ => None,
        }
    }
}

impl std::fmt::Display for LanguageStandard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One interface standard requirement: the minimum standard the
/// target's public headers require from consumers.  `max` is
/// reserved for future range requirements and is never populated
/// today, but it stays in the type and every serialized form so the
/// wire shape does not change when ranges land.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandardRequirement<S> {
    pub min: S,
    #[serde(default = "none")]
    pub max: Option<S>,
}

// `#[serde(default)]` needs a fn item; `Option::default` would also
// work but reads as a value default rather than "absent max".
fn none<S>() -> Option<S> {
    None
}

/// One declared interface-standard value: either a requirement or
/// the explicit `none`, meaning the target's headers are not
/// consumable from that language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceRequirement<S> {
    /// The target's headers are not consumable from this language.
    None,
    Requirement(StandardRequirement<S>),
}

impl<S> InterfaceRequirement<S> {
    /// The minimum standard, when this is a requirement.
    #[must_use]
    pub fn min(self) -> Option<S> {
        match self {
            Self::None => None,
            Self::Requirement(requirement) => Some(requirement.min),
        }
    }
}

/// Parse one `interface-c-standard` value: `none` or a single C
/// standard (ranges are reserved; see [`CStandard::parse`]).
///
/// # Errors
/// Propagates [`CStandard::parse`] errors for anything but `none`.
pub fn parse_interface_c(
    value: &str,
) -> Result<InterfaceRequirement<CStandard>, LanguageStandardParseError> {
    if value == "none" {
        return Ok(InterfaceRequirement::None);
    }
    CStandard::parse(value)
        .map(|min| InterfaceRequirement::Requirement(StandardRequirement { min, max: None }))
}

/// Parse one `interface-cxx-standard` value: `none` or a single C++
/// standard.
///
/// # Errors
/// Propagates [`CxxStandard::parse`] errors for anything but `none`.
pub fn parse_interface_cxx(
    value: &str,
) -> Result<InterfaceRequirement<CxxStandard>, LanguageStandardParseError> {
    if value == "none" {
        return Ok(InterfaceRequirement::None);
    }
    CxxStandard::parse(value)
        .map(|min| InterfaceRequirement::Requirement(StandardRequirement { min, max: None }))
}

impl<S: std::fmt::Display> std::fmt::Display for InterfaceRequirement<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Requirement(StandardRequirement { min, max: None }) => min.fmt(f),
            Self::Requirement(StandardRequirement {
                min,
                max: Some(max),
            }) => write!(f, "{min}..{max}"),
        }
    }
}

// `none` serializes as the bare string; a requirement serializes as
// its `{ min, max }` table (with `max` present even while reserved)
// so the canonical-metadata / index wire format is stable when
// range support lands.
impl<S: Serialize> Serialize for InterfaceRequirement<S> {
    fn serialize<Ser>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: serde::Serializer,
    {
        match self {
            Self::None => serializer.serialize_str("none"),
            Self::Requirement(requirement) => requirement.serialize(serializer),
        }
    }
}

impl<'de, S: Deserialize<'de>> Deserialize<'de> for InterfaceRequirement<S> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct InterfaceRequirementVisitor<S>(PhantomData<S>);

        impl<'de, S: Deserialize<'de>> Visitor<'de> for InterfaceRequirementVisitor<S> {
            type Value = InterfaceRequirement<S>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("`none` or a `{ min, max }` requirement table")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if v == "none" {
                    Ok(InterfaceRequirement::None)
                } else {
                    Err(E::invalid_value(serde::de::Unexpected::Str(v), &self))
                }
            }

            fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                StandardRequirement::deserialize(MapAccessDeserializer::new(map))
                    .map(InterfaceRequirement::Requirement)
            }
        }

        deserializer.deserialize_any(InterfaceRequirementVisitor(PhantomData))
    }
}

/// One declared standard-field value as it travels from the
/// manifest to the resolved package model.  Mirrors the
/// `DependencySource::Workspace` contract: `cabin-manifest`
/// constructs `Declared` (literal) or `Workspace` (the
/// `{ workspace = true }` opt-in marker), `cabin-workspace`
/// rewrites every marker into `Inherited(value)` before any
/// consumer sees the `Package`, and a marker that survives past
/// the loader is a workspace invariant violation.  Marker
/// semantics deliberately split by consumer: `.is_some()`-based
/// relevance checks (`imposes_requirement`,
/// `find_standard_flag_conflicts`, `is_empty`) count an
/// unresolved marker as a declaration, while the `*_value()`
/// accessors treat it as absent - both cases are unreachable
/// post-loader under the rewrite invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardDeclaration<S> {
    /// Literal value written in this manifest → source `package`
    /// (or `target` for a target-level field).
    Declared(S),
    /// Unresolved `{ workspace = true }` opt-in marker.
    Workspace,
    /// Value resolved from the workspace root's `[workspace]`
    /// declaration → source `workspace`.
    Inherited(S),
}

impl<S> StandardDeclaration<S> {
    /// The resolved standard value.  `None` only for an unresolved
    /// marker, which must not reach consumers (debug-asserted).
    #[must_use]
    pub fn value(self) -> Option<S> {
        match self {
            Self::Declared(s) | Self::Inherited(s) => Some(s),
            Self::Workspace => {
                debug_assert!(
                    false,
                    "unresolved `{{ workspace = true }}` standard marker reached a consumer"
                );
                None
            }
        }
    }
}

// `Declared` and `Inherited` serialize as the bare value so the
// canonical-metadata / index wire format is identical to a literal
// declaration (publish bakes inherited values).  An unresolved
// marker must never reach a serialization boundary.
impl<S: Serialize> Serialize for StandardDeclaration<S> {
    fn serialize<Ser>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: serde::Serializer,
    {
        match self {
            Self::Declared(s) | Self::Inherited(s) => s.serialize(serializer),
            Self::Workspace => Err(serde::ser::Error::custom(
                "unresolved `{ workspace = true }` standard marker cannot be serialized",
            )),
        }
    }
}

// A bare value deserializes as `Declared`: a consumer re-parsing
// published metadata sees a plain declaration.
impl<'de, S: Deserialize<'de>> Deserialize<'de> for StandardDeclaration<S> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        S::deserialize(deserializer).map(Self::Declared)
    }
}

/// The language fields shared by `[package]` and `[target.<name>]`:
/// the four standard fields (`c-standard` / `cxx-standard` /
/// `interface-c-standard` / `interface-cxx-standard`) plus the
/// `gnu-extensions` boolean.  At `[package]` level each standard
/// field may also be the `{ workspace = true }` opt-in marker;
/// target-level fields are always `Declared` (the parser rejects
/// markers there).  `gnu-extensions` is a plain boolean (no marker
/// form): target level overrides package level, defaulting to
/// `false`.  It selects GNU-extension compiler flag spellings only
/// and never participates in interface compatibility.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageStandardSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c_standard: Option<StandardDeclaration<CStandard>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cxx_standard: Option<StandardDeclaration<CxxStandard>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface_c_standard: Option<StandardDeclaration<InterfaceRequirement<CStandard>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface_cxx_standard: Option<StandardDeclaration<InterfaceRequirement<CxxStandard>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gnu_extensions: Option<bool>,
}

impl LanguageStandardSettings {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.c_standard.is_none()
            && self.cxx_standard.is_none()
            && self.interface_c_standard.is_none()
            && self.interface_cxx_standard.is_none()
            && self.gnu_extensions.is_none()
    }

    /// Resolved C implementation standard, when declared or
    /// inherited.
    #[must_use]
    pub fn c_standard_value(&self) -> Option<CStandard> {
        self.c_standard.and_then(StandardDeclaration::value)
    }

    /// Resolved C++ implementation standard, when declared or
    /// inherited.
    #[must_use]
    pub fn cxx_standard_value(&self) -> Option<CxxStandard> {
        self.cxx_standard.and_then(StandardDeclaration::value)
    }

    /// Resolved C interface requirement, when declared or inherited.
    #[must_use]
    pub fn interface_c_standard_value(&self) -> Option<InterfaceRequirement<CStandard>> {
        self.interface_c_standard
            .and_then(StandardDeclaration::value)
    }

    /// Resolved C++ interface requirement, when declared or
    /// inherited.
    #[must_use]
    pub fn interface_cxx_standard_value(&self) -> Option<InterfaceRequirement<CxxStandard>> {
        self.interface_cxx_standard
            .and_then(StandardDeclaration::value)
    }

    /// First field carrying the unresolved `{ workspace = true }`
    /// marker, for error reporting.
    #[must_use]
    pub fn workspace_marker_field(&self) -> Option<&'static str> {
        if self.c_standard == Some(StandardDeclaration::Workspace) {
            return Some("c-standard");
        }
        if self.cxx_standard == Some(StandardDeclaration::Workspace) {
            return Some("cxx-standard");
        }
        if self.interface_c_standard == Some(StandardDeclaration::Workspace) {
            return Some("interface-c-standard");
        }
        if self.interface_cxx_standard == Some(StandardDeclaration::Workspace) {
            return Some("interface-cxx-standard");
        }
        None
    }
}

/// Effective `gnu-extensions` value for one target: target override
/// ▶ package ▶ `false`.
#[must_use]
pub fn effective_gnu_extensions(package: &LanguageStandardSettings, target: &Target) -> bool {
    target
        .language
        .gnu_extensions
        .or(package.gnu_extensions)
        .unwrap_or(false)
}

/// Literal `[workspace]`-level standard default values that member
/// packages opt into per field with `<field> = { workspace = true }`
/// on `[package]`.  Plain values only - the opt-in marker is not
/// accepted on the `[workspace]` table itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct WorkspaceStandardDefaults {
    #[serde(rename = "c-standard", skip_serializing_if = "Option::is_none")]
    pub c_standard: Option<CStandard>,
    #[serde(rename = "cxx-standard", skip_serializing_if = "Option::is_none")]
    pub cxx_standard: Option<CxxStandard>,
    #[serde(
        rename = "interface-c-standard",
        skip_serializing_if = "Option::is_none"
    )]
    pub interface_c_standard: Option<InterfaceRequirement<CStandard>>,
    #[serde(
        rename = "interface-cxx-standard",
        skip_serializing_if = "Option::is_none"
    )]
    pub interface_cxx_standard: Option<InterfaceRequirement<CxxStandard>>,
}

impl WorkspaceStandardDefaults {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.c_standard.is_none()
            && self.cxx_standard.is_none()
            && self.interface_c_standard.is_none()
            && self.interface_cxx_standard.is_none()
    }
}

/// Provenance of an effective implementation standard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LanguageStandardSource {
    Package,
    Target,
    Workspace,
}

impl LanguageStandardSource {
    pub const fn as_key(self) -> &'static str {
        match self {
            Self::Package => "package",
            Self::Target => "target",
            Self::Workspace => "workspace",
        }
    }
}

/// Provenance of an effective interface standard.
/// `CompileStandard` marks the documented default: no interface
/// field was declared, so the requirement equals the target's
/// effective implementation standard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InterfaceStandardSource {
    Target,
    Package,
    CompileStandard,
    Workspace,
}

impl InterfaceStandardSource {
    pub const fn as_key(self) -> &'static str {
        match self {
            Self::Target => "target",
            Self::Package => "package",
            Self::CompileStandard => "compile-standard",
            Self::Workspace => "workspace",
        }
    }
}

/// A resolved implementation standard plus where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedStandard<S> {
    pub standard: S,
    pub source: LanguageStandardSource,
}

/// A resolved interface requirement plus where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterfaceStandard<S> {
    pub requirement: InterfaceRequirement<S>,
    pub source: InterfaceStandardSource,
}

/// Package-level effective implementation standards.  `None` means
/// no declaration anywhere - there is no built-in default, and a
/// target that compiles the language without an effective standard
/// is rejected at manifest load.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedLanguageStandards {
    pub c: Option<ResolvedStandard<CStandard>>,
    pub cxx: Option<ResolvedStandard<CxxStandard>>,
}

/// Map a package-level declaration to its resolved standard and
/// provenance: literal → `package`, workspace-inherited →
/// `workspace`, absent (or an unresolved marker, debug-asserted
/// here) → `None`.
fn package_resolution<S: Copy>(
    declaration: Option<StandardDeclaration<S>>,
) -> Option<ResolvedStandard<S>> {
    match declaration {
        Some(StandardDeclaration::Declared(standard)) => Some(ResolvedStandard {
            standard,
            source: LanguageStandardSource::Package,
        }),
        Some(StandardDeclaration::Inherited(standard)) => Some(ResolvedStandard {
            standard,
            source: LanguageStandardSource::Workspace,
        }),
        Some(StandardDeclaration::Workspace) => {
            debug_assert!(
                false,
                "unresolved `{{ workspace = true }}` standard marker reached resolution"
            );
            None
        }
        None => None,
    }
}

/// Resolve the package-level effective standards from the
/// `[package]` declarations (literal or workspace-inherited).
#[must_use]
pub fn resolve_language_standards(package: &LanguageStandardSettings) -> ResolvedLanguageStandards {
    ResolvedLanguageStandards {
        c: package_resolution(package.c_standard),
        cxx: package_resolution(package.cxx_standard),
    }
}

/// Effective C implementation standard for one target:
/// target override ▶ package (literal or workspace-inherited).
/// `None` when neither tier declares one.
#[must_use]
pub fn effective_c(
    package: &ResolvedLanguageStandards,
    target: &Target,
) -> Option<ResolvedStandard<CStandard>> {
    target
        .language
        .c_standard_value()
        .map_or(package.c, |standard| {
            Some(ResolvedStandard {
                standard,
                source: LanguageStandardSource::Target,
            })
        })
}

/// Effective C++ implementation standard for one target.
#[must_use]
pub fn effective_cxx(
    package: &ResolvedLanguageStandards,
    target: &Target,
) -> Option<ResolvedStandard<CxxStandard>> {
    target
        .language
        .cxx_standard_value()
        .map_or(package.cxx, |standard| {
            Some(ResolvedStandard {
                standard,
                source: LanguageStandardSource::Target,
            })
        })
}

/// Map a package-level *interface* declaration to its provenance:
/// literal → `package`, workspace-inherited → `workspace`.
fn interface_resolution<S: Copy>(
    declaration: Option<StandardDeclaration<InterfaceRequirement<S>>>,
) -> Option<InterfaceStandard<S>> {
    match declaration {
        Some(StandardDeclaration::Declared(requirement)) => Some(InterfaceStandard {
            requirement,
            source: InterfaceStandardSource::Package,
        }),
        Some(StandardDeclaration::Inherited(requirement)) => Some(InterfaceStandard {
            requirement,
            source: InterfaceStandardSource::Workspace,
        }),
        Some(StandardDeclaration::Workspace) => {
            debug_assert!(
                false,
                "unresolved `{{ workspace = true }}` standard marker reached resolution"
            );
            None
        }
        None => None,
    }
}

/// Effective C interface requirement for a library-like target:
/// target interface ▶ package interface (literal or
/// workspace-inherited) ▶ the target's effective implementation
/// standard, when one is declared (an interface may still default
/// from an explicit implementation standard).  `None` when no tier
/// yields a value.
#[must_use]
pub fn interface_c(
    package: &ResolvedLanguageStandards,
    package_settings: &LanguageStandardSettings,
    target: &Target,
) -> Option<InterfaceStandard<CStandard>> {
    if let Some(requirement) = target.language.interface_c_standard_value() {
        return Some(InterfaceStandard {
            requirement,
            source: InterfaceStandardSource::Target,
        });
    }
    if let Some(interface) = interface_resolution(package_settings.interface_c_standard) {
        return Some(interface);
    }
    effective_c(package, target).map(|resolved| InterfaceStandard {
        requirement: InterfaceRequirement::Requirement(StandardRequirement {
            min: resolved.standard,
            max: None,
        }),
        source: InterfaceStandardSource::CompileStandard,
    })
}

/// Effective C++ interface requirement for a library-like target.
#[must_use]
pub fn interface_cxx(
    package: &ResolvedLanguageStandards,
    package_settings: &LanguageStandardSettings,
    target: &Target,
) -> Option<InterfaceStandard<CxxStandard>> {
    if let Some(requirement) = target.language.interface_cxx_standard_value() {
        return Some(InterfaceStandard {
            requirement,
            source: InterfaceStandardSource::Target,
        });
    }
    if let Some(interface) = interface_resolution(package_settings.interface_cxx_standard) {
        return Some(interface);
    }
    effective_cxx(package, target).map(|resolved| InterfaceStandard {
        requirement: InterfaceRequirement::Requirement(StandardRequirement {
            min: resolved.standard,
            max: None,
        }),
        source: InterfaceStandardSource::CompileStandard,
    })
}

/// Whether a dependency target imposes an interface requirement for
/// `language` on its consumers.  A language is relevant when the
/// target has sources of that language, declares a target-level
/// field for it (implementation or interface), or is header-only
/// while the package declares a package-level *interface* standard
/// for it.  Package-level implementation defaults never create
/// relevance by themselves.
#[must_use]
pub fn imposes_requirement(
    target: &Target,
    package_settings: &LanguageStandardSettings,
    language: SourceLanguage,
) -> bool {
    let has_sources = target
        .sources
        .iter()
        .any(|s| classify_source(s) == Some(language));
    let target_declares = match language {
        SourceLanguage::C => {
            target.language.c_standard.is_some() || target.language.interface_c_standard.is_some()
        }
        SourceLanguage::Cxx => {
            target.language.cxx_standard.is_some()
                || target.language.interface_cxx_standard.is_some()
        }
    };
    let header_only_package_interface = target.kind.is_header_only()
        && match language {
            SourceLanguage::C => package_settings.interface_c_standard.is_some(),
            SourceLanguage::Cxx => package_settings.interface_cxx_standard.is_some(),
        };
    has_sources || target_declares || header_only_package_interface
}

/// Token prefixes that select a language standard inside an
/// escape-hatch flag list.
pub const STANDARD_FLAG_PREFIXES: [&str; 3] = ["-std=", "--std=", "/std:"];

/// A first-class standard declaration conflicting with an explicit
/// standard flag in the same package's manifest-derived flags.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error(
    "package `{package}` declares a first-class {} standard (`{field}`) but its `{flag_list}` also select one via `{flag}`; remove the flag, or drop the `{field}` declaration and keep the raw flag",
    .language.human_label()
)]
pub struct StandardFlagConflict {
    pub package: String,
    pub language: SourceLanguage,
    /// The manifest field family that was declared (`c-standard` or
    /// `cxx-standard`, at package or target level).
    pub field: &'static str,
    /// The flag list carrying the conflicting token (`cflags` or
    /// `cxxflags`).
    pub flag_list: &'static str,
    pub flag: String,
    /// Scope of the conflicting declaration: `Some(target)` when a
    /// target-level field created it (the ambiguity exists only on
    /// that target's compiles), `None` when the package-level field
    /// did (every compile of the language is ambiguous).  The build
    /// planner uses the scope to surface a conflict only when a
    /// matching compile is planned.
    pub target: Option<String>,
}

fn first_standard_token(flags: &[String]) -> Option<String> {
    flags
        .iter()
        .find(|f| STANDARD_FLAG_PREFIXES.iter().any(|p| f.starts_with(p)))
        .cloned()
}

/// Detect the documented conflict candidates: an explicit
/// first-class implementation standard declaration (package or
/// target level) for a language whose manifest-derived flag list
/// also pins a standard.  Runs on resolved flags *before* env /
/// pkg-config augmentation so `CFLAGS` / `CXXFLAGS` remain exempt.
///
/// These are *candidates*, scoped per declaration: the build
/// planner surfaces one only when a compile its scope covers is
/// planned, so an unbuilt sibling target's declaration
/// never gates a command that does not compile it.
#[must_use]
pub fn find_standard_flag_conflicts(
    package: &str,
    settings: &LanguageStandardSettings,
    targets: &[Target],
    flags: &ResolvedProfileFlags,
) -> Vec<StandardFlagConflict> {
    let mut out = Vec::new();
    // C and C++ follow identical conflict logic; only the language,
    // field / flag-list names, and standard declarations differ.
    let mut check = |language: SourceLanguage,
                     field: &'static str,
                     flag_list: &'static str,
                     list: &[String],
                     package_declares: bool,
                     target_declares: fn(&Target) -> bool| {
        let Some(flag) = first_standard_token(list) else {
            return;
        };
        let mut push = |flag: String, target: Option<String>| {
            out.push(StandardFlagConflict {
                package: package.to_owned(),
                language,
                field,
                flag_list,
                flag,
                target,
            });
        };
        if package_declares {
            push(flag, None);
        } else {
            for target in targets {
                if target_declares(target) {
                    push(flag.clone(), Some(target.name.as_str().to_owned()));
                }
            }
        }
    };
    check(
        SourceLanguage::C,
        "c-standard",
        "cflags",
        &flags.cflags,
        settings.c_standard.is_some(),
        |target| target.language.c_standard.is_some(),
    );
    check(
        SourceLanguage::Cxx,
        "cxx-standard",
        "cxxflags",
        &flags.cxxflags,
        settings.cxx_standard.is_some(),
        |target| target.language.cxx_standard.is_some(),
    );
    out
}

/// A target whose declared interface minimum is newer than the
/// implementation standard its own sources compile with - a
/// manifest contradiction, rejected at load.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error(
    "target `{target}` in package `{package}` sets `{field} = \"{interface_min}\"` but compiles its {} sources as `{implementation}`; the target's own translation units could not include its own public headers - raise `{}` or lower the interface minimum",
    .language.human_label(),
    implementation_field(*.language)
)]
pub struct InterfaceStandardContradiction {
    pub package: String,
    pub target: String,
    pub language: SourceLanguage,
    /// The interface field family that was declared
    /// (`interface-c-standard` or `interface-cxx-standard`).
    pub field: &'static str,
    pub implementation: LanguageStandard,
    pub interface_min: LanguageStandard,
}

/// Detect interface/implementation contradictions: for each
/// library-like target and language it compiles, the effective
/// interface minimum must not be newer than the effective
/// implementation standard (the target's own translation units
/// include its own public headers).  Runs on resolved declarations,
/// after workspace-marker resolution.  The compile-standard
/// interface fallback equals the implementation standard, so it can
/// never contradict.
#[must_use]
pub fn find_interface_standard_contradictions(
    package: &crate::Package,
) -> Vec<InterfaceStandardContradiction> {
    let resolved = resolve_language_standards(&package.language);
    let mut out = Vec::new();
    for target in &package.targets {
        let library_like = target.kind.produces_archive() || target.kind.is_header_only();
        if !library_like {
            continue;
        }
        let compiles = |language: SourceLanguage| {
            target
                .sources
                .iter()
                .any(|s| classify_source(s) == Some(language))
        };
        if compiles(SourceLanguage::C)
            && let (Some(implementation), Some(interface)) = (
                effective_c(&resolved, target),
                interface_c(&resolved, &package.language, target),
            )
            && let Some(min) = interface.requirement.min()
            && min > implementation.standard
        {
            out.push(InterfaceStandardContradiction {
                package: package.name.as_str().to_owned(),
                target: target.name.as_str().to_owned(),
                language: SourceLanguage::C,
                field: "interface-c-standard",
                implementation: LanguageStandard::C(implementation.standard),
                interface_min: LanguageStandard::C(min),
            });
        }
        if compiles(SourceLanguage::Cxx)
            && let (Some(implementation), Some(interface)) = (
                effective_cxx(&resolved, target),
                interface_cxx(&resolved, &package.language, target),
            )
            && let Some(min) = interface.requirement.min()
            && min > implementation.standard
        {
            out.push(InterfaceStandardContradiction {
                package: package.name.as_str().to_owned(),
                target: target.name.as_str().to_owned(),
                language: SourceLanguage::Cxx,
                field: "interface-cxx-standard",
                implementation: LanguageStandard::Cxx(implementation.standard),
                interface_min: LanguageStandard::Cxx(min),
            });
        }
    }
    out
}

/// Per-package language-standard summary carried by
/// `BuildConfiguration`: package-level effective standards plus the
/// effective values for every target.  Values (not provenance) feed
/// the fingerprint; the whole struct feeds `cabin metadata` /
/// `cabin explain build-config`.  Absent entries mean the language
/// has no declared standard anywhere for that scope.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageStandardsSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c: Option<ResolvedStandard<CStandard>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cxx: Option<ResolvedStandard<CxxStandard>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub targets: BTreeMap<String, TargetStandardsSummary>,
}

/// Effective standards for one target.  Interface entries are
/// present only for `library` / `header-only` kinds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetStandardsSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c: Option<ResolvedStandard<CStandard>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cxx: Option<ResolvedStandard<CxxStandard>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface_c: Option<InterfaceStandard<CStandard>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface_cxx: Option<InterfaceStandard<CxxStandard>>,
    /// Effective `gnu-extensions` value (target ▶ package ▶
    /// `false`).  Omitted from the serialized form when `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gnu_extensions: bool,
}

impl LanguageStandardsSummary {
    /// Compute the summary from a package's declarations.
    #[must_use]
    pub fn from_package(package: &crate::Package) -> Self {
        let resolved = resolve_language_standards(&package.language);
        let targets = package
            .targets
            .iter()
            .map(|target| {
                let library_like = target.kind.produces_archive() || target.kind.is_header_only();
                let summary = TargetStandardsSummary {
                    c: effective_c(&resolved, target),
                    cxx: effective_cxx(&resolved, target),
                    interface_c: library_like
                        .then(|| interface_c(&resolved, &package.language, target))
                        .flatten(),
                    interface_cxx: library_like
                        .then(|| interface_cxx(&resolved, &package.language, target))
                        .flatten(),
                    gnu_extensions: effective_gnu_extensions(&package.language, target),
                };
                (target.name.as_str().to_owned(), summary)
            })
            .collect();
        Self {
            c: resolved.c,
            cxx: resolved.cxx,
            targets,
        }
    }

    /// Stable line serialization for the build-configuration
    /// fingerprint.  Values only - provenance must not move the
    /// fingerprint, and absent standards (and the default
    /// `gnu-extensions = false`) contribute no line.
    #[must_use]
    pub fn fingerprint_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(resolved) = &self.c {
            lines.push(format!("c={}", resolved.standard));
        }
        if let Some(resolved) = &self.cxx {
            lines.push(format!("cxx={}", resolved.standard));
        }
        for (name, target) in &self.targets {
            lines.push(format!("target={name}"));
            if let Some(resolved) = &target.c {
                lines.push(format!("c={}", resolved.standard));
            }
            if let Some(resolved) = &target.cxx {
                lines.push(format!("cxx={}", resolved.standard));
            }
            if let Some(interface) = &target.interface_c {
                lines.push(format!("interface-c={}", interface.requirement));
            }
            if let Some(interface) = &target.interface_cxx {
                lines.push(format!("interface-cxx={}", interface.requirement));
            }
            if target.gnu_extensions {
                lines.push("gnu-extensions=true".to_owned());
            }
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TargetKind, TargetName};
    use camino::Utf8PathBuf;

    fn target(kind: TargetKind, sources: &[&str], language: LanguageStandardSettings) -> Target {
        Target {
            name: TargetName::new("t").unwrap(),
            kind,
            sources: sources.iter().map(Utf8PathBuf::from).collect(),
            include_dirs: Vec::new(),
            defines: Vec::new(),
            deps: Vec::new(),
            required_features: Vec::new(),
            language,
        }
    }

    fn requirement<S>(min: S) -> InterfaceRequirement<S> {
        InterfaceRequirement::Requirement(StandardRequirement { min, max: None })
    }

    #[test]
    fn every_accepted_identifier_parses_and_round_trips() {
        for s in CStandard::ALL {
            assert_eq!(CStandard::parse(s.as_str()).unwrap(), s);
        }
        for s in CxxStandard::ALL {
            assert_eq!(CxxStandard::parse(s.as_str()).unwrap(), s);
        }
    }

    #[test]
    fn aliases_normalize_immediately() {
        assert_eq!(CStandard::parse("c90").unwrap(), CStandard::C89);
        assert_eq!(CxxStandard::parse("c++03").unwrap(), CxxStandard::Cxx98);
        // The alias never survives as a spelling of its own.
        assert_eq!(CStandard::parse("c90").unwrap().as_str(), "c89");
        assert_eq!(CxxStandard::parse("c++03").unwrap().as_str(), "c++98");
    }

    #[test]
    fn unknown_values_list_the_accepted_identifiers() {
        let err = CStandard::parse("c++17").unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown C standard `c++17`: expected one of c89, c99, c11, c17, c23"
        );
        let err = CxxStandard::parse("c++29").unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown C++ standard `c++29`: expected one of c++98, c++11, c++14, c++17, c++20, c++23, c++26"
        );
    }

    #[test]
    fn gnu_spellings_are_ordinary_unknown_values() {
        for value in ["gnu89", "gnu99", "gnu11", "gnu17", "gnu23", "gnu90"] {
            let err = CStandard::parse(value).unwrap_err();
            assert!(
                matches!(&err, LanguageStandardParseError::Unknown { value: v, .. } if v == value),
                "unexpected error for {value}: {err}"
            );
            // No special-cased hint: gnu spellings are unknown
            // values like any other.
            assert!(!err.to_string().contains("gnu-extensions"));
        }
        for value in [
            "gnu++98", "gnu++03", "gnu++11", "gnu++14", "gnu++17", "gnu++20", "gnu++23", "gnu++26",
        ] {
            let err = CxxStandard::parse(value).unwrap_err();
            assert!(
                matches!(&err, LanguageStandardParseError::Unknown { value: v, .. } if v == value),
                "unexpected error for {value}: {err}"
            );
            assert!(!err.to_string().contains("gnu-extensions"));
        }
    }

    #[test]
    fn range_like_values_get_the_reserved_diagnostic() {
        for value in [">=c11", "<=c17", ">c99", "<c23", "c11,c17", "c11, c17"] {
            let err = CStandard::parse(value).unwrap_err();
            assert!(
                matches!(err, LanguageStandardParseError::RangeReserved { .. }),
                "expected reserved-range error for {value}, got: {err}"
            );
            assert!(err.to_string().contains("reserved for a future version"));
        }
        for value in [">=c++17", "<=c++20", ">c++11", "<c++23", "c++17,c++20"] {
            let err = CxxStandard::parse(value).unwrap_err();
            assert!(
                matches!(err, LanguageStandardParseError::RangeReserved { .. }),
                "expected reserved-range error for {value}, got: {err}"
            );
        }
        // The interface parsers share the same rejection.
        assert!(matches!(
            parse_interface_c(">=c11").unwrap_err(),
            LanguageStandardParseError::RangeReserved { .. }
        ));
        assert!(matches!(
            parse_interface_cxx(">=c++17").unwrap_err(),
            LanguageStandardParseError::RangeReserved { .. }
        ));
    }

    #[test]
    fn none_is_interface_only() {
        assert_eq!(
            parse_interface_c("none").unwrap(),
            InterfaceRequirement::None
        );
        assert_eq!(
            parse_interface_cxx("none").unwrap(),
            InterfaceRequirement::None
        );
        let err = CStandard::parse("none").unwrap_err();
        assert!(
            matches!(err, LanguageStandardParseError::NoneOnImplementation { .. }),
            "expected misplaced-none error, got: {err}"
        );
        assert!(err.to_string().contains("interface-c-standard"));
        let err = CxxStandard::parse("none").unwrap_err();
        assert!(matches!(
            err,
            LanguageStandardParseError::NoneOnImplementation { .. }
        ));
    }

    #[test]
    fn interface_parsers_accept_every_identifier_and_alias() {
        for s in CStandard::ALL {
            assert_eq!(parse_interface_c(s.as_str()).unwrap(), requirement(s));
        }
        for s in CxxStandard::ALL {
            assert_eq!(parse_interface_cxx(s.as_str()).unwrap(), requirement(s));
        }
        assert_eq!(
            parse_interface_c("c90").unwrap(),
            requirement(CStandard::C89)
        );
        assert_eq!(
            parse_interface_cxx("c++03").unwrap(),
            requirement(CxxStandard::Cxx98)
        );
    }

    #[test]
    fn standards_order_chronologically() {
        assert!(CStandard::C89 < CStandard::C99);
        assert!(CStandard::C99 < CStandard::C11);
        assert!(CStandard::C11 < CStandard::C17);
        assert!(CStandard::C17 < CStandard::C23);
        assert!(CxxStandard::Cxx98 < CxxStandard::Cxx11);
        assert!(CxxStandard::Cxx11 < CxxStandard::Cxx14);
        assert!(CxxStandard::Cxx14 < CxxStandard::Cxx17);
        assert!(CxxStandard::Cxx17 < CxxStandard::Cxx20);
        assert!(CxxStandard::Cxx20 < CxxStandard::Cxx23);
        assert!(CxxStandard::Cxx23 < CxxStandard::Cxx26);
    }

    #[test]
    fn msvc_spellings_cover_exactly_the_stable_flags() {
        assert_eq!(
            LanguageStandard::Cxx(CxxStandard::Cxx20).msvc_spelling(),
            Some("/std:c++20")
        );
        assert_eq!(
            LanguageStandard::C(CStandard::C17).msvc_spelling(),
            Some("/std:c17")
        );
        assert_eq!(LanguageStandard::C(CStandard::C99).msvc_spelling(), None);
        assert_eq!(
            LanguageStandard::Cxx(CxxStandard::Cxx23).msvc_spelling(),
            None
        );
        assert_eq!(
            LanguageStandard::Cxx(CxxStandard::Cxx26).msvc_spelling(),
            None
        );
        assert_eq!(
            LanguageStandard::Cxx(CxxStandard::Cxx11).msvc_spelling(),
            None
        );
    }

    #[test]
    fn standard_requirement_serde_round_trips_preserving_max() {
        let min_only = StandardRequirement {
            min: CxxStandard::Cxx17,
            max: None,
        };
        let json = serde_json::to_string(&min_only).unwrap();
        // `max` stays in the serialized form even while reserved.
        assert_eq!(json, r#"{"min":"c++17","max":null}"#);
        let parsed: StandardRequirement<CxxStandard> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, min_only);

        let with_max = StandardRequirement {
            min: CStandard::C11,
            max: Some(CStandard::C17),
        };
        let json = serde_json::to_string(&with_max).unwrap();
        assert_eq!(json, r#"{"min":"c11","max":"c17"}"#);
        let parsed: StandardRequirement<CStandard> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, with_max);

        // A missing `max` still deserializes (as unpopulated).
        let parsed: StandardRequirement<CStandard> =
            serde_json::from_str(r#"{"min":"c11"}"#).unwrap();
        assert_eq!(parsed.max, None);
        // Unknown future range syntax falls through
        // `deny_unknown_fields`.
        assert!(
            serde_json::from_str::<StandardRequirement<CStandard>>(
                r#"{"min":"c11","exact":"c17"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn interface_requirement_serde_round_trips() {
        let none: InterfaceRequirement<CxxStandard> = InterfaceRequirement::None;
        let json = serde_json::to_string(&none).unwrap();
        assert_eq!(json, "\"none\"");
        let parsed: InterfaceRequirement<CxxStandard> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, none);

        let req = requirement(CxxStandard::Cxx20);
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"min":"c++20","max":null}"#);
        let parsed: InterfaceRequirement<CxxStandard> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, req);

        // A bare standard string is not a serialized requirement.
        assert!(serde_json::from_str::<InterfaceRequirement<CxxStandard>>("\"c++20\"").is_err());
    }

    #[test]
    fn interface_requirement_displays_min_max_and_none() {
        assert_eq!(
            InterfaceRequirement::<CxxStandard>::None.to_string(),
            "none"
        );
        assert_eq!(requirement(CxxStandard::Cxx17).to_string(), "c++17");
        assert_eq!(
            InterfaceRequirement::Requirement(StandardRequirement {
                min: CStandard::C11,
                max: Some(CStandard::C17),
            })
            .to_string(),
            "c11..c17"
        );
    }

    #[test]
    fn gnu_extensions_default_false_with_target_over_package() {
        let plain = target(
            TargetKind::Executable,
            &["a.cc"],
            LanguageStandardSettings::default(),
        );
        let none = LanguageStandardSettings::default();
        assert!(!effective_gnu_extensions(&none, &plain));

        let package_on = LanguageStandardSettings {
            gnu_extensions: Some(true),
            ..Default::default()
        };
        assert!(effective_gnu_extensions(&package_on, &plain));

        let target_off = target(
            TargetKind::Executable,
            &["a.cc"],
            LanguageStandardSettings {
                gnu_extensions: Some(false),
                ..Default::default()
            },
        );
        assert!(!effective_gnu_extensions(&package_on, &target_off));

        let target_on = target(
            TargetKind::Executable,
            &["a.cc"],
            LanguageStandardSettings {
                gnu_extensions: Some(true),
                ..Default::default()
            },
        );
        assert!(effective_gnu_extensions(&none, &target_on));
    }

    #[test]
    fn effective_standard_prefers_target_then_package_then_none() {
        let undeclared = resolve_language_standards(&LanguageStandardSettings::default());
        let plain = target(
            TargetKind::Executable,
            &["a.cc"],
            LanguageStandardSettings::default(),
        );
        assert_eq!(effective_cxx(&undeclared, &plain), None);
        assert_eq!(effective_c(&undeclared, &plain), None);

        let package = resolve_language_standards(&LanguageStandardSettings {
            cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx14)),
            ..Default::default()
        });
        let effective = effective_cxx(&package, &plain).unwrap();
        assert_eq!(effective.standard, CxxStandard::Cxx14);
        assert_eq!(effective.source, LanguageStandardSource::Package);
        // A declared C++ standard yields no effective C standard.
        assert_eq!(effective_c(&package, &plain), None);

        let overridden = target(
            TargetKind::Executable,
            &["a.cc"],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                ..Default::default()
            },
        );
        let effective = effective_cxx(&package, &overridden).unwrap();
        assert_eq!(effective.standard, CxxStandard::Cxx20);
        assert_eq!(effective.source, LanguageStandardSource::Target);
    }

    #[test]
    fn interface_standard_falls_back_to_explicit_compile_standard_or_none() {
        let package_settings = LanguageStandardSettings {
            cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
            ..Default::default()
        };
        let resolved = resolve_language_standards(&package_settings);
        let lib = target(
            TargetKind::Library,
            &["a.cc"],
            LanguageStandardSettings::default(),
        );
        let interface = interface_cxx(&resolved, &package_settings, &lib).unwrap();
        assert_eq!(interface.requirement, requirement(CxxStandard::Cxx20));
        assert_eq!(interface.source, InterfaceStandardSource::CompileStandard);
        // No implementation or interface standard anywhere: no
        // interface value either (there is no built-in default).
        let undeclared = LanguageStandardSettings::default();
        let resolved_undeclared = resolve_language_standards(&undeclared);
        assert_eq!(interface_cxx(&resolved_undeclared, &undeclared, &lib), None);
        assert_eq!(interface_c(&resolved_undeclared, &undeclared, &lib), None);

        let package_interface = LanguageStandardSettings {
            cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
            interface_cxx_standard: Some(StandardDeclaration::Declared(requirement(
                CxxStandard::Cxx17,
            ))),
            ..Default::default()
        };
        let resolved = resolve_language_standards(&package_interface);
        let interface = interface_cxx(&resolved, &package_interface, &lib).unwrap();
        assert_eq!(interface.requirement, requirement(CxxStandard::Cxx17));
        assert_eq!(interface.source, InterfaceStandardSource::Package);

        let lib_override = target(
            TargetKind::Library,
            &["a.cc"],
            LanguageStandardSettings {
                interface_cxx_standard: Some(StandardDeclaration::Declared(requirement(
                    CxxStandard::Cxx14,
                ))),
                ..Default::default()
            },
        );
        let interface = interface_cxx(&resolved, &package_interface, &lib_override).unwrap();
        assert_eq!(interface.requirement, requirement(CxxStandard::Cxx14));
        assert_eq!(interface.source, InterfaceStandardSource::Target);
    }

    #[test]
    fn declared_none_interface_survives_resolution() {
        let package_settings = LanguageStandardSettings {
            cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
            ..Default::default()
        };
        let resolved = resolve_language_standards(&package_settings);
        let lib = target(
            TargetKind::Library,
            &["a.cc"],
            LanguageStandardSettings {
                interface_cxx_standard: Some(StandardDeclaration::Declared(
                    InterfaceRequirement::None,
                )),
                ..Default::default()
            },
        );
        let interface = interface_cxx(&resolved, &package_settings, &lib).unwrap();
        assert_eq!(interface.requirement, InterfaceRequirement::None);
        assert_eq!(interface.source, InterfaceStandardSource::Target);
        assert_eq!(interface.requirement.min(), None);
    }

    #[test]
    fn imposes_requirement_relevance_rules() {
        let none = LanguageStandardSettings::default();
        // A pure-C library imposes no C++ requirement.
        let c_lib = target(
            TargetKind::Library,
            &["a.c"],
            LanguageStandardSettings::default(),
        );
        assert!(imposes_requirement(&c_lib, &none, SourceLanguage::C));
        assert!(!imposes_requirement(&c_lib, &none, SourceLanguage::Cxx));

        // A package-level *implementation* default alone creates no
        // relevance for a target without that language.
        let package_impl = LanguageStandardSettings {
            cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
            ..Default::default()
        };
        assert!(!imposes_requirement(
            &c_lib,
            &package_impl,
            SourceLanguage::Cxx
        ));

        // A target-level field (implementation or interface) does.
        let declared = target(
            TargetKind::Library,
            &["a.c"],
            LanguageStandardSettings {
                interface_cxx_standard: Some(StandardDeclaration::Declared(requirement(
                    CxxStandard::Cxx17,
                ))),
                ..Default::default()
            },
        );
        assert!(imposes_requirement(&declared, &none, SourceLanguage::Cxx));

        // Header-only + package-level *interface* standard does.
        let header_only = target(
            TargetKind::HeaderOnly,
            &[],
            LanguageStandardSettings::default(),
        );
        assert!(!imposes_requirement(
            &header_only,
            &none,
            SourceLanguage::Cxx
        ));
        let package_interface = LanguageStandardSettings {
            interface_cxx_standard: Some(StandardDeclaration::Declared(requirement(
                CxxStandard::Cxx20,
            ))),
            ..Default::default()
        };
        assert!(imposes_requirement(
            &header_only,
            &package_interface,
            SourceLanguage::Cxx
        ));
        // ... but not via a package-level implementation default.
        assert!(!imposes_requirement(
            &header_only,
            &package_impl,
            SourceLanguage::Cxx
        ));
    }

    #[test]
    fn conflict_fires_only_for_declared_language_and_matching_bucket() {
        let flags = ResolvedProfileFlags {
            cxxflags: vec!["-std=c++14".to_owned()],
            ..Default::default()
        };
        let declared_cxx = LanguageStandardSettings {
            cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
            ..Default::default()
        };

        // Nothing declared: never a conflict.
        assert!(
            find_standard_flag_conflicts("p", &LanguageStandardSettings::default(), &[], &flags)
                .is_empty()
        );

        // Declared C++ + `-std=` in cxxflags: a package-scoped
        // conflict candidate.
        let conflicts = find_standard_flag_conflicts("p", &declared_cxx, &[], &flags);
        let conflict = conflicts.first().unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflict.language, SourceLanguage::Cxx);
        assert_eq!(conflict.flag, "-std=c++14");
        assert_eq!(conflict.field, "cxx-standard");
        assert_eq!(conflict.target, None);
        assert!(conflict.to_string().contains("cxx-standard"));

        // Declared C++ + `-std=` in cflags only: no conflict.
        let c_only_flags = ResolvedProfileFlags {
            cflags: vec!["-std=c99".to_owned()],
            ..Default::default()
        };
        assert!(find_standard_flag_conflicts("p", &declared_cxx, &[], &c_only_flags).is_empty());

        // A target-level declaration counts as declared.
        let t = target(
            TargetKind::Executable,
            &["a.c"],
            LanguageStandardSettings {
                c_standard: Some(StandardDeclaration::Declared(CStandard::C17)),
                ..Default::default()
            },
        );
        let conflicts = find_standard_flag_conflicts(
            "p",
            &LanguageStandardSettings::default(),
            std::slice::from_ref(&t),
            &c_only_flags,
        );
        let conflict = conflicts.first().unwrap();
        assert_eq!(conflict.language, SourceLanguage::C);
        assert_eq!(conflict.flag_list, "cflags");
        // A target-level declaration scopes the candidate to that
        // target so the planner only surfaces it when the target's
        // compile is planned.
        assert_eq!(conflict.target.as_deref(), Some("t"));

        // `/std:` and `--std=` prefixes are recognized too.
        let msvc_flags = ResolvedProfileFlags {
            cxxflags: vec!["/std:c++latest".to_owned()],
            ..Default::default()
        };
        assert!(!find_standard_flag_conflicts("p", &declared_cxx, &[], &msvc_flags).is_empty());
    }

    fn package_with(targets: Vec<Target>, language: LanguageStandardSettings) -> crate::Package {
        use crate::{Package, PackageName};
        Package::new(
            PackageName::new("demo").unwrap(),
            semver::Version::parse("0.1.0").unwrap(),
            targets,
            Vec::new(),
        )
        .unwrap()
        .with_language(language)
    }

    #[test]
    fn contradiction_fires_when_interface_minimum_exceeds_implementation() {
        // Target-level interface newer than the package
        // implementation standard the target compiles with.
        let lib = target(
            TargetKind::Library,
            &["a.cc"],
            LanguageStandardSettings {
                interface_cxx_standard: Some(StandardDeclaration::Declared(requirement(
                    CxxStandard::Cxx20,
                ))),
                ..Default::default()
            },
        );
        let package = package_with(
            vec![lib],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
                ..Default::default()
            },
        );
        let contradictions = find_interface_standard_contradictions(&package);
        assert_eq!(contradictions.len(), 1);
        let contradiction = &contradictions[0];
        assert_eq!(contradiction.target, "t");
        assert_eq!(contradiction.field, "interface-cxx-standard");
        assert_eq!(
            contradiction.implementation,
            LanguageStandard::Cxx(CxxStandard::Cxx17)
        );
        assert_eq!(
            contradiction.interface_min,
            LanguageStandard::Cxx(CxxStandard::Cxx20)
        );
        let message = contradiction.to_string();
        assert!(
            message.contains("could not include its own public headers"),
            "message must state the reason plainly: {message}"
        );

        // Same shape on the C side, via package-level interface.
        let c_lib = target(
            TargetKind::Library,
            &["a.c"],
            LanguageStandardSettings::default(),
        );
        let package = package_with(
            vec![c_lib],
            LanguageStandardSettings {
                c_standard: Some(StandardDeclaration::Declared(CStandard::C11)),
                interface_c_standard: Some(StandardDeclaration::Declared(requirement(
                    CStandard::C23,
                ))),
                ..Default::default()
            },
        );
        let contradictions = find_interface_standard_contradictions(&package);
        assert_eq!(contradictions.len(), 1);
        assert_eq!(contradictions[0].field, "interface-c-standard");
    }

    #[test]
    fn contradiction_ignores_equal_older_none_and_non_compiling_targets() {
        // Interface at or below the implementation standard is fine.
        for interface in [CxxStandard::Cxx17, CxxStandard::Cxx14] {
            let lib = target(
                TargetKind::Library,
                &["a.cc"],
                LanguageStandardSettings {
                    interface_cxx_standard: Some(StandardDeclaration::Declared(requirement(
                        interface,
                    ))),
                    ..Default::default()
                },
            );
            let package = package_with(
                vec![lib],
                LanguageStandardSettings {
                    cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
                    ..Default::default()
                },
            );
            assert!(find_interface_standard_contradictions(&package).is_empty());
        }

        // `none` imposes no minimum, so it cannot contradict.
        let lib = target(
            TargetKind::Library,
            &["a.cc"],
            LanguageStandardSettings {
                interface_cxx_standard: Some(StandardDeclaration::Declared(
                    InterfaceRequirement::None,
                )),
                ..Default::default()
            },
        );
        let package = package_with(
            vec![lib],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
                ..Default::default()
            },
        );
        assert!(find_interface_standard_contradictions(&package).is_empty());

        // A header-only target has no translation units, so a newer
        // interface minimum is not a contradiction.
        let header_only = target(
            TargetKind::HeaderOnly,
            &[],
            LanguageStandardSettings::default(),
        );
        let package = package_with(
            vec![header_only],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
                interface_cxx_standard: Some(StandardDeclaration::Declared(requirement(
                    CxxStandard::Cxx20,
                ))),
                ..Default::default()
            },
        );
        assert!(find_interface_standard_contradictions(&package).is_empty());

        // A pure-C library with a newer C++ interface minimum has no
        // C++ translation units of its own.
        let c_lib = target(
            TargetKind::Library,
            &["a.c"],
            LanguageStandardSettings {
                interface_cxx_standard: Some(StandardDeclaration::Declared(requirement(
                    CxxStandard::Cxx26,
                ))),
                ..Default::default()
            },
        );
        let package = package_with(
            vec![c_lib],
            LanguageStandardSettings {
                c_standard: Some(StandardDeclaration::Declared(CStandard::C11)),
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
                ..Default::default()
            },
        );
        assert!(find_interface_standard_contradictions(&package).is_empty());

        // Executables never carry interface requirements.
        let exe = target(
            TargetKind::Executable,
            &["a.cc"],
            LanguageStandardSettings::default(),
        );
        let package = package_with(
            vec![exe],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
                interface_cxx_standard: Some(StandardDeclaration::Declared(requirement(
                    CxxStandard::Cxx20,
                ))),
                ..Default::default()
            },
        );
        assert!(find_interface_standard_contradictions(&package).is_empty());
    }

    #[test]
    fn summary_lists_every_target_with_interface_only_for_library_like() {
        use crate::{Package, PackageName};
        let package = Package::new(
            PackageName::new("demo").unwrap(),
            semver::Version::parse("0.1.0").unwrap(),
            vec![
                target(
                    TargetKind::Executable,
                    &["main.cc"],
                    LanguageStandardSettings::default(),
                ),
                Target {
                    name: TargetName::new("core").unwrap(),
                    kind: TargetKind::Library,
                    sources: vec![Utf8PathBuf::from("core.cc")],
                    include_dirs: Vec::new(),
                    defines: Vec::new(),
                    deps: Vec::new(),
                    required_features: Vec::new(),
                    language: LanguageStandardSettings {
                        cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                        interface_cxx_standard: Some(StandardDeclaration::Declared(requirement(
                            CxxStandard::Cxx17,
                        ))),
                        gnu_extensions: Some(true),
                        ..Default::default()
                    },
                },
            ],
            Vec::new(),
        )
        .unwrap()
        .with_language(LanguageStandardSettings {
            cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
            ..Default::default()
        });
        let summary = LanguageStandardsSummary::from_package(&package);
        assert_eq!(summary.cxx.unwrap().standard, CxxStandard::Cxx17);
        assert_eq!(summary.c, None);
        assert_eq!(summary.targets.len(), 2);
        let exe = &summary.targets["t"];
        assert!(exe.interface_c.is_none() && exe.interface_cxx.is_none());
        assert!(!exe.gnu_extensions);
        let core = &summary.targets["core"];
        assert_eq!(core.cxx.unwrap().standard, CxxStandard::Cxx20);
        assert_eq!(
            core.interface_cxx.unwrap().requirement,
            requirement(CxxStandard::Cxx17)
        );
        assert!(core.gnu_extensions);
        // No C standard is declared anywhere, so the library gets
        // no C interface entry either.
        assert_eq!(core.interface_c, None);
    }

    #[test]
    fn package_level_interface_fields_are_inert_without_library_like_targets() {
        use crate::{Package, PackageName};
        // docs/language-standards.md: package-level interface fields
        // are "allowed, and inert, in packages without any"
        // library-like target - the summary must not attach them to
        // executables.
        let package = Package::new(
            PackageName::new("demo").unwrap(),
            semver::Version::parse("0.1.0").unwrap(),
            vec![target(
                TargetKind::Executable,
                &["main.cc"],
                LanguageStandardSettings::default(),
            )],
            Vec::new(),
        )
        .unwrap()
        .with_language(LanguageStandardSettings {
            interface_c_standard: Some(StandardDeclaration::Declared(requirement(CStandard::C17))),
            interface_cxx_standard: Some(StandardDeclaration::Declared(requirement(
                CxxStandard::Cxx20,
            ))),
            ..Default::default()
        });
        let summary = LanguageStandardsSummary::from_package(&package);
        let exe = &summary.targets["t"];
        assert!(
            exe.interface_c.is_none() && exe.interface_cxx.is_none(),
            "package-level interface fields must stay inert on executables"
        );
    }

    #[test]
    fn fingerprint_lines_are_values_only_and_deterministic() {
        let mut summary = LanguageStandardsSummary::default();
        // Nothing declared anywhere: nothing to fingerprint.
        assert!(summary.fingerprint_lines().is_empty());

        summary.c = Some(ResolvedStandard {
            standard: CStandard::C11,
            source: LanguageStandardSource::Package,
        });
        summary.cxx = Some(ResolvedStandard {
            standard: CxxStandard::Cxx17,
            source: LanguageStandardSource::Package,
        });
        let lines = summary.fingerprint_lines();
        assert_eq!(lines, vec!["c=c11".to_owned(), "cxx=c++17".to_owned()]);

        // Provenance must not appear in the lines.
        summary.cxx = Some(ResolvedStandard {
            standard: CxxStandard::Cxx17,
            source: LanguageStandardSource::Workspace,
        });
        assert_eq!(summary.fingerprint_lines(), lines);

        summary.targets.insert(
            "core".to_owned(),
            TargetStandardsSummary {
                c: summary.c,
                cxx: Some(ResolvedStandard {
                    standard: CxxStandard::Cxx20,
                    source: LanguageStandardSource::Target,
                }),
                interface_c: Some(InterfaceStandard {
                    requirement: InterfaceRequirement::None,
                    source: InterfaceStandardSource::Target,
                }),
                interface_cxx: Some(InterfaceStandard {
                    requirement: requirement(CxxStandard::Cxx17),
                    source: InterfaceStandardSource::Target,
                }),
                gnu_extensions: true,
            },
        );
        assert_eq!(
            summary.fingerprint_lines(),
            vec![
                "c=c11".to_owned(),
                "cxx=c++17".to_owned(),
                "target=core".to_owned(),
                "c=c11".to_owned(),
                "cxx=c++20".to_owned(),
                "interface-c=none".to_owned(),
                "interface-cxx=c++17".to_owned(),
                "gnu-extensions=true".to_owned(),
            ]
        );
    }

    #[test]
    fn standard_declaration_serde_is_a_bare_value_and_rejects_markers() {
        let declared: StandardDeclaration<CxxStandard> =
            StandardDeclaration::Declared(CxxStandard::Cxx20);
        let inherited: StandardDeclaration<CxxStandard> =
            StandardDeclaration::Inherited(CxxStandard::Cxx20);
        assert_eq!(serde_json::to_string(&declared).unwrap(), "\"c++20\"");
        assert_eq!(serde_json::to_string(&inherited).unwrap(), "\"c++20\"");
        let marker: StandardDeclaration<CxxStandard> = StandardDeclaration::Workspace;
        assert!(serde_json::to_string(&marker).is_err());
        let parsed: StandardDeclaration<CxxStandard> = serde_json::from_str("\"c++20\"").unwrap();
        assert_eq!(parsed, StandardDeclaration::Declared(CxxStandard::Cxx20));

        // Interface declarations carry the `{ min, max }` shape (or
        // `none`) through the same bare-value contract.
        let declared_interface: StandardDeclaration<InterfaceRequirement<CxxStandard>> =
            StandardDeclaration::Declared(requirement(CxxStandard::Cxx20));
        let json = serde_json::to_string(&declared_interface).unwrap();
        assert_eq!(json, r#"{"min":"c++20","max":null}"#);
        let parsed: StandardDeclaration<InterfaceRequirement<CxxStandard>> =
            serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed,
            StandardDeclaration::Declared(requirement(CxxStandard::Cxx20))
        );
        let parsed: StandardDeclaration<InterfaceRequirement<CxxStandard>> =
            serde_json::from_str("\"none\"").unwrap();
        assert_eq!(
            parsed,
            StandardDeclaration::Declared(InterfaceRequirement::None)
        );
    }

    #[test]
    fn inherited_standard_resolves_with_workspace_source() {
        let settings = LanguageStandardSettings {
            cxx_standard: Some(StandardDeclaration::Inherited(CxxStandard::Cxx20)),
            ..Default::default()
        };
        let resolved = resolve_language_standards(&settings);
        let cxx = resolved.cxx.unwrap();
        assert_eq!(cxx.standard, CxxStandard::Cxx20);
        assert_eq!(cxx.source, LanguageStandardSource::Workspace);
        assert_eq!(resolved.c, None);
    }

    #[test]
    fn inherited_interface_standard_resolves_with_workspace_source() {
        let settings = LanguageStandardSettings {
            cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
            interface_cxx_standard: Some(StandardDeclaration::Inherited(requirement(
                CxxStandard::Cxx17,
            ))),
            ..Default::default()
        };
        let resolved = resolve_language_standards(&settings);
        let lib = target(
            TargetKind::Library,
            &["a.cc"],
            LanguageStandardSettings::default(),
        );
        let interface = interface_cxx(&resolved, &settings, &lib).unwrap();
        assert_eq!(interface.requirement, requirement(CxxStandard::Cxx17));
        assert_eq!(interface.source, InterfaceStandardSource::Workspace);
    }

    #[test]
    fn inherited_values_behave_like_declarations_for_conflicts_and_relevance() {
        let flags = ResolvedProfileFlags {
            cxxflags: vec!["-std=c++20".to_owned()],
            ..Default::default()
        };
        let inherited = LanguageStandardSettings {
            cxx_standard: Some(StandardDeclaration::Inherited(CxxStandard::Cxx17)),
            ..Default::default()
        };
        assert_eq!(
            find_standard_flag_conflicts("p", &inherited, &[], &flags).len(),
            1
        );

        let header_only = target(
            TargetKind::HeaderOnly,
            &[],
            LanguageStandardSettings::default(),
        );
        let pkg = LanguageStandardSettings {
            interface_cxx_standard: Some(StandardDeclaration::Inherited(requirement(
                CxxStandard::Cxx20,
            ))),
            ..Default::default()
        };
        assert!(imposes_requirement(&header_only, &pkg, SourceLanguage::Cxx));
    }

    #[test]
    fn fingerprint_is_identical_for_declared_and_inherited_values() {
        use crate::{Package, PackageName};
        let make = |decl: StandardDeclaration<CxxStandard>| {
            let package = Package::new(
                PackageName::new("demo").unwrap(),
                semver::Version::parse("0.1.0").unwrap(),
                vec![target(
                    TargetKind::Executable,
                    &["main.cc"],
                    LanguageStandardSettings::default(),
                )],
                Vec::new(),
            )
            .unwrap()
            .with_language(LanguageStandardSettings {
                cxx_standard: Some(decl),
                ..Default::default()
            });
            LanguageStandardsSummary::from_package(&package).fingerprint_lines()
        };
        assert_eq!(
            make(StandardDeclaration::Declared(CxxStandard::Cxx20)),
            make(StandardDeclaration::Inherited(CxxStandard::Cxx20))
        );
    }

    #[test]
    fn workspace_marker_field_reports_the_first_marker() {
        assert_eq!(
            LanguageStandardSettings::default().workspace_marker_field(),
            None
        );
        let settings = LanguageStandardSettings {
            interface_c_standard: Some(StandardDeclaration::Workspace),
            ..Default::default()
        };
        assert_eq!(
            settings.workspace_marker_field(),
            Some("interface-c-standard")
        );
        let settings = LanguageStandardSettings {
            c_standard: Some(StandardDeclaration::Workspace),
            interface_c_standard: Some(StandardDeclaration::Workspace),
            ..Default::default()
        };
        assert_eq!(settings.workspace_marker_field(), Some("c-standard"));
    }
}
