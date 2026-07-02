//! Typed C/C++ language standards.
//!
//! Owns the standard enums, the manifest declaration shape shared by
//! `[package]` and `[target.<name>]`, effective-standard resolution
//! (target ▶ package ▶ built-in default), interface-requirement
//! relevance and fallback, the escape-hatch conflict detector, and
//! the per-package summary that feeds `BuildConfiguration`
//! fingerprinting and the metadata view.  Pure data and logic only;
//! no I/O.  See `docs/language-standards.md` for the user-facing
//! contract.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ResolvedProfileFlags, SourceLanguage, Target, classify_source};

/// C language standards Cabin can request, oldest to newest.  The
/// `Ord` derive follows declaration order, which the interface
/// compatibility check relies on: each GNU dialect sits just above
/// its ISO twin, so a GNU consumer satisfies its twin's interface
/// requirement while an ISO consumer never satisfies a GNU one
/// (the dialect is a superset of the twin).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CStandard {
    #[serde(rename = "c89")]
    C89,
    #[serde(rename = "gnu89")]
    Gnu89,
    #[serde(rename = "c99")]
    C99,
    #[serde(rename = "gnu99")]
    Gnu99,
    #[serde(rename = "c11")]
    C11,
    #[serde(rename = "gnu11")]
    Gnu11,
    #[serde(rename = "c17")]
    C17,
    #[serde(rename = "gnu17")]
    Gnu17,
    #[serde(rename = "c23")]
    C23,
    #[serde(rename = "gnu23")]
    Gnu23,
}

impl CStandard {
    /// ISO spellings first so parse errors read chronologically per
    /// dialect; iteration order is not the `Ord` order.
    pub const ALL: [Self; 10] = [
        Self::C89,
        Self::C99,
        Self::C11,
        Self::C17,
        Self::C23,
        Self::Gnu89,
        Self::Gnu99,
        Self::Gnu11,
        Self::Gnu17,
        Self::Gnu23,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::C89 => "c89",
            Self::Gnu89 => "gnu89",
            Self::C99 => "c99",
            Self::Gnu99 => "gnu99",
            Self::C11 => "c11",
            Self::Gnu11 => "gnu11",
            Self::C17 => "c17",
            Self::Gnu17 => "gnu17",
            Self::C23 => "c23",
            Self::Gnu23 => "gnu23",
        }
    }

    /// Whether this is a GNU dialect (`gnu*`) rather than an ISO
    /// standard.
    #[must_use]
    pub const fn is_gnu(self) -> bool {
        matches!(
            self,
            Self::Gnu89 | Self::Gnu99 | Self::Gnu11 | Self::Gnu17 | Self::Gnu23
        )
    }

    /// The ISO standard a GNU dialect extends; identity for ISO
    /// values.  Toolchain capability checks key on this: a GNU
    /// spelling ships alongside its twin on GCC-style compilers.
    #[must_use]
    pub const fn iso_twin(self) -> Self {
        match self {
            Self::Gnu89 => Self::C89,
            Self::Gnu99 => Self::C99,
            Self::Gnu11 => Self::C11,
            Self::Gnu17 => Self::C17,
            Self::Gnu23 => Self::C23,
            iso => iso,
        }
    }

    /// # Errors
    /// Returns [`LanguageStandardParseError`] listing the valid
    /// spellings when `value` is not a recognized C standard.
    pub fn parse(value: &str) -> Result<Self, LanguageStandardParseError> {
        Self::ALL
            .into_iter()
            .find(|s| s.as_str() == value)
            .ok_or_else(|| LanguageStandardParseError {
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

/// C++ language standards Cabin can request, oldest to newest, with
/// each GNU dialect just above its ISO twin (see [`CStandard`] for
/// the ordering contract).  `c++26` is deferred until its capability
/// thresholds are audited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CxxStandard {
    #[serde(rename = "c++98")]
    Cxx98,
    #[serde(rename = "gnu++98")]
    Gnuxx98,
    #[serde(rename = "c++03")]
    Cxx03,
    #[serde(rename = "gnu++03")]
    Gnuxx03,
    #[serde(rename = "c++11")]
    Cxx11,
    #[serde(rename = "gnu++11")]
    Gnuxx11,
    #[serde(rename = "c++14")]
    Cxx14,
    #[serde(rename = "gnu++14")]
    Gnuxx14,
    #[serde(rename = "c++17")]
    Cxx17,
    #[serde(rename = "gnu++17")]
    Gnuxx17,
    #[serde(rename = "c++20")]
    Cxx20,
    #[serde(rename = "gnu++20")]
    Gnuxx20,
    #[serde(rename = "c++23")]
    Cxx23,
    #[serde(rename = "gnu++23")]
    Gnuxx23,
}

impl CxxStandard {
    /// ISO spellings first so parse errors read chronologically per
    /// dialect; iteration order is not the `Ord` order.
    pub const ALL: [Self; 14] = [
        Self::Cxx98,
        Self::Cxx03,
        Self::Cxx11,
        Self::Cxx14,
        Self::Cxx17,
        Self::Cxx20,
        Self::Cxx23,
        Self::Gnuxx98,
        Self::Gnuxx03,
        Self::Gnuxx11,
        Self::Gnuxx14,
        Self::Gnuxx17,
        Self::Gnuxx20,
        Self::Gnuxx23,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cxx98 => "c++98",
            Self::Gnuxx98 => "gnu++98",
            Self::Cxx03 => "c++03",
            Self::Gnuxx03 => "gnu++03",
            Self::Cxx11 => "c++11",
            Self::Gnuxx11 => "gnu++11",
            Self::Cxx14 => "c++14",
            Self::Gnuxx14 => "gnu++14",
            Self::Cxx17 => "c++17",
            Self::Gnuxx17 => "gnu++17",
            Self::Cxx20 => "c++20",
            Self::Gnuxx20 => "gnu++20",
            Self::Cxx23 => "c++23",
            Self::Gnuxx23 => "gnu++23",
        }
    }

    /// Whether this is a GNU dialect (`gnu++*`) rather than an ISO
    /// standard.
    #[must_use]
    pub const fn is_gnu(self) -> bool {
        matches!(
            self,
            Self::Gnuxx98
                | Self::Gnuxx03
                | Self::Gnuxx11
                | Self::Gnuxx14
                | Self::Gnuxx17
                | Self::Gnuxx20
                | Self::Gnuxx23
        )
    }

    /// The ISO standard a GNU dialect extends; identity for ISO
    /// values.  Toolchain capability checks key on this: a GNU
    /// spelling ships alongside its twin on GCC-style compilers.
    #[must_use]
    pub const fn iso_twin(self) -> Self {
        match self {
            Self::Gnuxx98 => Self::Cxx98,
            Self::Gnuxx03 => Self::Cxx03,
            Self::Gnuxx11 => Self::Cxx11,
            Self::Gnuxx14 => Self::Cxx14,
            Self::Gnuxx17 => Self::Cxx17,
            Self::Gnuxx20 => Self::Cxx20,
            Self::Gnuxx23 => Self::Cxx23,
            iso => iso,
        }
    }

    /// # Errors
    /// Returns [`LanguageStandardParseError`] listing the valid
    /// spellings when `value` is not a recognized C++ standard.
    pub fn parse(value: &str) -> Result<Self, LanguageStandardParseError> {
        Self::ALL
            .into_iter()
            .find(|s| s.as_str() == value)
            .ok_or_else(|| LanguageStandardParseError {
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

/// Built-in defaults: what every package compiles with when no
/// standard is declared anywhere.  Changing either constant changes
/// every undeclared project's compile commands.
pub const DEFAULT_C_STANDARD: CStandard = CStandard::C11;
pub const DEFAULT_CXX_STANDARD: CxxStandard = CxxStandard::Cxx17;

/// An invalid manifest standard value, with the valid spellings for
/// the diagnostic.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error(
    "unknown {} standard `{value}`: expected one of {}",
    .language.human_label(),
    valid_standard_values(*.language)
)]
pub struct LanguageStandardParseError {
    pub language: SourceLanguage,
    pub value: String,
}

fn valid_standard_values(language: SourceLanguage) -> String {
    match language {
        SourceLanguage::C => CStandard::ALL.map(CStandard::as_str).join(", "),
        SourceLanguage::Cxx => CxxStandard::ALL.map(CxxStandard::as_str).join(", "),
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
    /// (C89/C99/C23, C++98/03/11/23); the planner rejects those
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

// `Declared` and `Inherited` serialize as the bare standard string
// so the canonical-metadata / index wire format is identical to a
// literal declaration (publish bakes inherited values).  An
// unresolved marker must never reach a serialization boundary.
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

// A bare string deserializes as `Declared`: a consumer re-parsing
// published metadata sees a plain declaration.
impl<'de, S: Deserialize<'de>> Deserialize<'de> for StandardDeclaration<S> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        S::deserialize(deserializer).map(Self::Declared)
    }
}

/// The four manifest fields, shared by `[package]` and
/// `[target.<name>]` (`c-standard` / `cxx-standard` /
/// `interface-c-standard` / `interface-cxx-standard`).  At
/// `[package]` level each field may also be the
/// `{ workspace = true }` opt-in marker; target-level fields are
/// always `Declared` (the parser rejects markers there).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageStandardSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c_standard: Option<StandardDeclaration<CStandard>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cxx_standard: Option<StandardDeclaration<CxxStandard>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface_c_standard: Option<StandardDeclaration<CStandard>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface_cxx_standard: Option<StandardDeclaration<CxxStandard>>,
}

impl LanguageStandardSettings {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.c_standard.is_none()
            && self.cxx_standard.is_none()
            && self.interface_c_standard.is_none()
            && self.interface_cxx_standard.is_none()
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

    /// Resolved C interface standard, when declared or inherited.
    #[must_use]
    pub fn interface_c_standard_value(&self) -> Option<CStandard> {
        self.interface_c_standard
            .and_then(StandardDeclaration::value)
    }

    /// Resolved C++ interface standard, when declared or
    /// inherited.
    #[must_use]
    pub fn interface_cxx_standard_value(&self) -> Option<CxxStandard> {
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
    pub interface_c_standard: Option<CStandard>,
    #[serde(
        rename = "interface-cxx-standard",
        skip_serializing_if = "Option::is_none"
    )]
    pub interface_cxx_standard: Option<CxxStandard>,
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
    BuiltinDefault,
    Package,
    Target,
    Workspace,
}

impl LanguageStandardSource {
    pub const fn as_key(self) -> &'static str {
        match self {
            Self::BuiltinDefault => "builtin-default",
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

/// A resolved interface standard plus where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterfaceStandard<S> {
    pub standard: S,
    pub source: InterfaceStandardSource,
}

/// Package-level effective implementation standards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedLanguageStandards {
    pub c: ResolvedStandard<CStandard>,
    pub cxx: ResolvedStandard<CxxStandard>,
}

impl Default for ResolvedLanguageStandards {
    fn default() -> Self {
        resolve_language_standards(&LanguageStandardSettings::default())
    }
}

/// Map a package-level declaration to its resolved standard and
/// provenance: literal → `package`, workspace-inherited →
/// `workspace`, absent (or an unresolved marker, debug-asserted
/// here) → built-in default.
fn package_resolution<S: Copy>(
    declaration: Option<StandardDeclaration<S>>,
    default: S,
) -> ResolvedStandard<S> {
    match declaration {
        Some(StandardDeclaration::Declared(standard)) => ResolvedStandard {
            standard,
            source: LanguageStandardSource::Package,
        },
        Some(StandardDeclaration::Inherited(standard)) => ResolvedStandard {
            standard,
            source: LanguageStandardSource::Workspace,
        },
        Some(StandardDeclaration::Workspace) => {
            debug_assert!(
                false,
                "unresolved `{{ workspace = true }}` standard marker reached resolution"
            );
            ResolvedStandard {
                standard: default,
                source: LanguageStandardSource::BuiltinDefault,
            }
        }
        None => ResolvedStandard {
            standard: default,
            source: LanguageStandardSource::BuiltinDefault,
        },
    }
}

/// Resolve the package-level effective standards:
/// `[package]` declaration (literal or workspace-inherited) ▶
/// built-in default.
#[must_use]
pub fn resolve_language_standards(package: &LanguageStandardSettings) -> ResolvedLanguageStandards {
    ResolvedLanguageStandards {
        c: package_resolution(package.c_standard, DEFAULT_C_STANDARD),
        cxx: package_resolution(package.cxx_standard, DEFAULT_CXX_STANDARD),
    }
}

/// Effective C implementation standard for one target:
/// target override ▶ package (literal or workspace-inherited) ▶
/// built-in default.
#[must_use]
pub fn effective_c(
    package: &ResolvedLanguageStandards,
    target: &Target,
) -> ResolvedStandard<CStandard> {
    target
        .language
        .c_standard_value()
        .map_or(package.c, |standard| ResolvedStandard {
            standard,
            source: LanguageStandardSource::Target,
        })
}

/// Effective C++ implementation standard for one target.
#[must_use]
pub fn effective_cxx(
    package: &ResolvedLanguageStandards,
    target: &Target,
) -> ResolvedStandard<CxxStandard> {
    target
        .language
        .cxx_standard_value()
        .map_or(package.cxx, |standard| ResolvedStandard {
            standard,
            source: LanguageStandardSource::Target,
        })
}

/// Map a package-level *interface* declaration to its provenance:
/// literal → `package`, workspace-inherited → `workspace`.
fn interface_resolution<S: Copy>(
    declaration: Option<StandardDeclaration<S>>,
) -> Option<InterfaceStandard<S>> {
    match declaration {
        Some(StandardDeclaration::Declared(standard)) => Some(InterfaceStandard {
            standard,
            source: InterfaceStandardSource::Package,
        }),
        Some(StandardDeclaration::Inherited(standard)) => Some(InterfaceStandard {
            standard,
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

/// Effective C interface standard for a library-like target:
/// target interface ▶ package interface (literal or
/// workspace-inherited) ▶ the target's effective implementation
/// standard (literal defaulting - built-in default included).
#[must_use]
pub fn interface_c(
    package: &ResolvedLanguageStandards,
    package_settings: &LanguageStandardSettings,
    target: &Target,
) -> InterfaceStandard<CStandard> {
    if let Some(standard) = target.language.interface_c_standard_value() {
        return InterfaceStandard {
            standard,
            source: InterfaceStandardSource::Target,
        };
    }
    if let Some(interface) = interface_resolution(package_settings.interface_c_standard) {
        return interface;
    }
    InterfaceStandard {
        standard: effective_c(package, target).standard,
        source: InterfaceStandardSource::CompileStandard,
    }
}

/// Effective C++ interface standard for a library-like target.
#[must_use]
pub fn interface_cxx(
    package: &ResolvedLanguageStandards,
    package_settings: &LanguageStandardSettings,
    target: &Target,
) -> InterfaceStandard<CxxStandard> {
    if let Some(standard) = target.language.interface_cxx_standard_value() {
        return InterfaceStandard {
            standard,
            source: InterfaceStandardSource::Target,
        };
    }
    if let Some(interface) = interface_resolution(package_settings.interface_cxx_standard) {
        return interface;
    }
    InterfaceStandard {
        standard: effective_cxx(package, target).standard,
        source: InterfaceStandardSource::CompileStandard,
    }
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

/// Per-package language-standard summary carried by
/// `BuildConfiguration`: package-level effective standards plus the
/// effective values for every target.  Values (not provenance) feed
/// the fingerprint; the whole struct feeds `cabin metadata` /
/// `cabin explain build-config`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageStandardsSummary {
    pub c: ResolvedStandard<CStandard>,
    pub cxx: ResolvedStandard<CxxStandard>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub targets: BTreeMap<String, TargetStandardsSummary>,
}

/// Effective standards for one target.  Interface entries are
/// present only for `library` / `header-only` kinds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetStandardsSummary {
    pub c: ResolvedStandard<CStandard>,
    pub cxx: ResolvedStandard<CxxStandard>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface_c: Option<InterfaceStandard<CStandard>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface_cxx: Option<InterfaceStandard<CxxStandard>>,
}

impl Default for LanguageStandardsSummary {
    fn default() -> Self {
        let resolved = ResolvedLanguageStandards::default();
        Self {
            c: resolved.c,
            cxx: resolved.cxx,
            targets: BTreeMap::new(),
        }
    }
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
                        .then(|| interface_c(&resolved, &package.language, target)),
                    interface_cxx: library_like
                        .then(|| interface_cxx(&resolved, &package.language, target)),
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
    /// fingerprint.
    #[must_use]
    pub fn fingerprint_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("c={}", self.c.standard),
            format!("cxx={}", self.cxx.standard),
        ];
        for (name, target) in &self.targets {
            lines.push(format!("target={name}"));
            lines.push(format!("c={}", target.c.standard));
            lines.push(format!("cxx={}", target.cxx.standard));
            if let Some(interface) = &target.interface_c {
                lines.push(format!("interface-c={}", interface.standard));
            }
            if let Some(interface) = &target.interface_cxx {
                lines.push(format!("interface-cxx={}", interface.standard));
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
            language,
        }
    }

    #[test]
    fn standards_parse_round_trip_and_reject_unknown_values() {
        for s in CStandard::ALL {
            assert_eq!(CStandard::parse(s.as_str()).unwrap(), s);
        }
        for s in CxxStandard::ALL {
            assert_eq!(CxxStandard::parse(s.as_str()).unwrap(), s);
        }
        let err = CStandard::parse("c++17").unwrap_err();
        assert!(
            err.to_string()
                .contains("expected one of c89, c99, c11, c17, c23"),
            "unexpected message: {err}"
        );
        let err = CxxStandard::parse("c++26").unwrap_err();
        assert!(
            err.to_string().contains("c++23"),
            "unexpected message: {err}"
        );
        assert!(
            err.to_string().contains("unknown C++ standard `c++26`"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn standards_order_chronologically() {
        assert!(CStandard::C99 < CStandard::C23);
        assert!(CStandard::C11 < CStandard::C17);
        assert!(CxxStandard::Cxx14 < CxxStandard::Cxx20);
        assert!(CxxStandard::Cxx98 < CxxStandard::Cxx03);
    }

    #[test]
    fn gnu_dialects_sit_between_their_iso_twin_and_the_next_standard() {
        // The interface check compares with `Ord`: a GNU consumer
        // satisfies its ISO twin's requirement (gnu11 > c11), an ISO
        // consumer never satisfies a GNU requirement (c11 < gnu11),
        // and the next ISO standard clears both (c17 > gnu11).
        assert!(CStandard::C11 < CStandard::Gnu11);
        assert!(CStandard::Gnu11 < CStandard::C17);
        assert!(CxxStandard::Cxx20 < CxxStandard::Gnuxx20);
        assert!(CxxStandard::Gnuxx20 < CxxStandard::Cxx23);
        for standard in CStandard::ALL {
            assert_eq!(standard.is_gnu(), standard.iso_twin() != standard);
            assert!(standard >= standard.iso_twin());
        }
        for standard in CxxStandard::ALL {
            assert_eq!(standard.is_gnu(), standard.iso_twin() != standard);
            assert!(standard >= standard.iso_twin());
        }
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
            LanguageStandard::Cxx(CxxStandard::Cxx11).msvc_spelling(),
            None
        );
        // No GNU dialect has a `/std:` spelling.
        assert_eq!(LanguageStandard::C(CStandard::Gnu11).msvc_spelling(), None);
        assert_eq!(
            LanguageStandard::Cxx(CxxStandard::Gnuxx20).msvc_spelling(),
            None
        );
    }

    #[test]
    fn effective_standard_prefers_target_then_package_then_builtin() {
        let undeclared = resolve_language_standards(&LanguageStandardSettings::default());
        let plain = target(
            TargetKind::Executable,
            &["a.cc"],
            LanguageStandardSettings::default(),
        );
        let effective = effective_cxx(&undeclared, &plain);
        assert_eq!(effective.standard, CxxStandard::Cxx17);
        assert_eq!(effective.source, LanguageStandardSource::BuiltinDefault);

        let package = resolve_language_standards(&LanguageStandardSettings {
            cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx14)),
            ..Default::default()
        });
        let effective = effective_cxx(&package, &plain);
        assert_eq!(effective.standard, CxxStandard::Cxx14);
        assert_eq!(effective.source, LanguageStandardSource::Package);

        let overridden = target(
            TargetKind::Executable,
            &["a.cc"],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                ..Default::default()
            },
        );
        let effective = effective_cxx(&package, &overridden);
        assert_eq!(effective.standard, CxxStandard::Cxx20);
        assert_eq!(effective.source, LanguageStandardSource::Target);
    }

    #[test]
    fn interface_standard_falls_back_to_effective_compile_standard() {
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
        let interface = interface_cxx(&resolved, &package_settings, &lib);
        assert_eq!(interface.standard, CxxStandard::Cxx20);
        assert_eq!(interface.source, InterfaceStandardSource::CompileStandard);

        let package_interface = LanguageStandardSettings {
            cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
            interface_cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
            ..Default::default()
        };
        let resolved = resolve_language_standards(&package_interface);
        let interface = interface_cxx(&resolved, &package_interface, &lib);
        assert_eq!(interface.standard, CxxStandard::Cxx17);
        assert_eq!(interface.source, InterfaceStandardSource::Package);

        let lib_override = target(
            TargetKind::Library,
            &["a.cc"],
            LanguageStandardSettings {
                interface_cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx14)),
                ..Default::default()
            },
        );
        let interface = interface_cxx(&resolved, &package_interface, &lib_override);
        assert_eq!(interface.standard, CxxStandard::Cxx14);
        assert_eq!(interface.source, InterfaceStandardSource::Target);
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
                interface_cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
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
            interface_cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
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
                    language: LanguageStandardSettings {
                        cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                        interface_cxx_standard: Some(StandardDeclaration::Declared(
                            CxxStandard::Cxx17,
                        )),
                        ..Default::default()
                    },
                },
            ],
            Vec::new(),
        )
        .unwrap();
        let summary = LanguageStandardsSummary::from_package(&package);
        assert_eq!(summary.cxx.standard, CxxStandard::Cxx17);
        assert_eq!(summary.targets.len(), 2);
        let exe = &summary.targets["t"];
        assert!(exe.interface_c.is_none() && exe.interface_cxx.is_none());
        let core = &summary.targets["core"];
        assert_eq!(core.cxx.standard, CxxStandard::Cxx20);
        assert_eq!(core.interface_cxx.unwrap().standard, CxxStandard::Cxx17);
        assert_eq!(
            core.interface_c.unwrap().source,
            InterfaceStandardSource::CompileStandard
        );
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
            interface_c_standard: Some(StandardDeclaration::Declared(CStandard::C17)),
            interface_cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
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
        let lines = summary.fingerprint_lines();
        assert_eq!(lines, vec!["c=c11".to_owned(), "cxx=c++17".to_owned()]);

        // Provenance must not appear in the lines.
        summary.cxx = ResolvedStandard {
            standard: CxxStandard::Cxx17,
            source: LanguageStandardSource::Package,
        };
        assert_eq!(summary.fingerprint_lines(), lines);

        summary.targets.insert(
            "core".to_owned(),
            TargetStandardsSummary {
                c: summary.c,
                cxx: ResolvedStandard {
                    standard: CxxStandard::Cxx20,
                    source: LanguageStandardSource::Target,
                },
                interface_c: None,
                interface_cxx: Some(InterfaceStandard {
                    standard: CxxStandard::Cxx17,
                    source: InterfaceStandardSource::Target,
                }),
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
                "interface-cxx=c++17".to_owned(),
            ]
        );
    }

    #[test]
    fn standard_declaration_serde_is_a_bare_string_and_rejects_markers() {
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
    }

    #[test]
    fn inherited_standard_resolves_with_workspace_source() {
        let settings = LanguageStandardSettings {
            cxx_standard: Some(StandardDeclaration::Inherited(CxxStandard::Cxx20)),
            ..Default::default()
        };
        let resolved = resolve_language_standards(&settings);
        assert_eq!(resolved.cxx.standard, CxxStandard::Cxx20);
        assert_eq!(resolved.cxx.source, LanguageStandardSource::Workspace);
        assert_eq!(resolved.c.source, LanguageStandardSource::BuiltinDefault);
    }

    #[test]
    fn inherited_interface_standard_resolves_with_workspace_source() {
        let settings = LanguageStandardSettings {
            cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
            interface_cxx_standard: Some(StandardDeclaration::Inherited(CxxStandard::Cxx17)),
            ..Default::default()
        };
        let resolved = resolve_language_standards(&settings);
        let lib = target(
            TargetKind::Library,
            &["a.cc"],
            LanguageStandardSettings::default(),
        );
        let interface = interface_cxx(&resolved, &settings, &lib);
        assert_eq!(interface.standard, CxxStandard::Cxx17);
        assert_eq!(interface.source, InterfaceStandardSource::Workspace);
    }

    #[test]
    fn inherited_values_behave_like_declarations_for_conflicts_and_relevance() {
        let flags = ResolvedProfileFlags {
            cxxflags: vec!["-std=gnu++20".to_owned()],
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
            interface_cxx_standard: Some(StandardDeclaration::Inherited(CxxStandard::Cxx20)),
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
