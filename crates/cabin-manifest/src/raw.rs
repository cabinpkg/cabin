//! Private serde structures that mirror `cabin.toml` shape.
//!
//! These types stay inside `cabin-manifest`.  The `parse` module turns them
//! into validated `cabin_core` values, so no other crate sees raw manifest
//! layout.

use std::collections::BTreeMap;
use std::fmt;

use camino::Utf8PathBuf;

use serde::Deserialize;
use serde::de::{Deserializer, MapAccess, Visitor, value::MapAccessDeserializer};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawManifest {
    #[serde(default)]
    pub(crate) package: Option<RawPackage>,
    /// `[target.<NAME>]` buildable targets and `[target.'cfg(...)']`
    /// conditional tables share this key space.  Kept as a
    /// `toml::Table` so the parser can choose the schema per key
    /// (`RawTarget` or the conditional-table schema) and reject
    /// dependency sub-tables under a buildable target name with a
    /// dedicated error.
    #[serde(default)]
    pub(crate) target: toml::Table,
    #[serde(default)]
    pub(crate) dependencies: BTreeMap<String, RawDependency>,
    /// `[dev-dependencies]` - Cabin package dependencies for
    /// `test` / `example` targets.  Declaration-only for
    /// ordinary commands; `cabin test` activates them for the
    /// selected primary packages.
    #[serde(default, rename = "dev-dependencies")]
    pub(crate) dev_dependencies: BTreeMap<String, RawDependency>,
    /// `[build-dependencies]` - Cabin packages built for the build
    /// machine (the build context) instead of linked into this one.
    #[serde(default, rename = "build-dependencies")]
    pub(crate) build_dependencies: BTreeMap<String, RawDependency>,
    #[serde(default)]
    pub(crate) workspace: Option<RawWorkspace>,
    /// `[features]`.  The `default` key, when present, is the
    /// list of features enabled when the user does not pass
    /// `--no-default-features`.  Other keys declare individual features
    /// and their implication arrows.
    #[serde(default)]
    pub(crate) features: BTreeMap<String, Vec<String>>,
    /// `[profile]` carries the unconditional per-package build
    /// flags (defines, include dirs, language-specific flag
    /// lists); each `[profile.<name>]` adds per-profile
    /// knobs (`opt-level`, `debug`, …) plus flag lists appended
    /// after the base and inherited layers.  The parser converts each
    /// `[profile.<name>]` to a `cabin_core::ProfileDefinition`.
    /// Only the workspace root manifest may declare
    /// `[profile.<name>]` tables; the workspace loader rejects them on
    /// member and path-dependency manifests so a single workspace key
    /// cannot mean different things in different members.
    #[serde(default)]
    pub(crate) profile: Option<RawProfileTable>,
    /// `[build]` - workspace-root compiler execution settings.
    #[serde(default)]
    pub(crate) build: Option<RawBuild>,
    /// `[toolchain]` - explicit C/C++ tool selection.  Honored
    /// only on the workspace root manifest; rejected on member
    /// manifests so a single build invocation cannot silently use
    /// different compilers in different packages.
    #[serde(default)]
    pub(crate) toolchain: Option<RawToolchain>,
    /// `[patch]` - local patch / override declarations.
    /// Workspace-root only - the workspace loader rejects
    /// patches on member manifests.  Patches are local
    /// development policy and never enter published metadata.
    #[serde(default)]
    pub(crate) patch: BTreeMap<String, RawPatch>,
    /// `[system-provided]` - packages the build takes from the system.
    /// Workspace-root only, like `[patch]`.
    #[serde(default, rename = "system-provided")]
    pub(crate) system_provided: BTreeMap<String, RawSystemProvider>,
}

/// `[toolchain]` table.  Adding new fields here requires a matching
/// extension in `cabin_core::toolchain`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawToolchain {
    #[serde(default)]
    pub(crate) cc: Option<String>,
    #[serde(default)]
    pub(crate) cxx: Option<String>,
    #[serde(default)]
    pub(crate) ar: Option<String>,
}

/// `[build]` table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawBuild {
    /// Executable name or path used to prefix C and C++ compile commands.
    #[serde(default, rename = "compiler-wrapper")]
    pub(crate) compiler_wrapper: Option<String>,
}

/// `[profile]` parent table.  Holds the unconditional per-package
/// build-flag fields directly plus a flatten-captured map of named
/// variants (one per `[profile.<name>]` sub-table).
///
/// `#[serde(deny_unknown_fields)]` is not set here:
/// the flattened variants map captures any extra keys serde would
/// otherwise reject.  Unknown *fixed* fields surface as a type
/// mismatch when the captured value cannot be deserialised as a
/// `RawProfile`.
#[derive(Debug, Deserialize)]
pub(crate) struct RawProfileTable {
    #[serde(default)]
    pub(crate) defines: Vec<String>,
    #[serde(default, rename = "include-dirs")]
    pub(crate) include_dirs: Vec<Utf8PathBuf>,
    #[serde(default)]
    pub(crate) cflags: Vec<String>,
    #[serde(default)]
    pub(crate) cxxflags: Vec<String>,
    #[serde(default)]
    pub(crate) ldflags: Vec<String>,
    #[serde(default, rename = "link-libs")]
    pub(crate) link_libs: Vec<String>,
    #[serde(default)]
    pub(crate) frameworks: Vec<String>,
    /// Named profile variants (`[profile.dev]`, `[profile.release]`,
    /// any custom name).  Captured as a map so the parser can
    /// validate inheritance chains and merge against the base
    /// flags above.
    #[serde(flatten)]
    pub(crate) variants: BTreeMap<String, RawProfile>,
}

/// Conditional general or named `[target.'cfg(...)'.profile...]`
/// flag-bag.  Same shape as the per-package base flags on `[profile]`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawProfileFlags {
    #[serde(default)]
    pub(crate) defines: Vec<String>,
    #[serde(default, rename = "include-dirs")]
    pub(crate) include_dirs: Vec<Utf8PathBuf>,
    #[serde(default)]
    pub(crate) cflags: Vec<String>,
    #[serde(default)]
    pub(crate) cxxflags: Vec<String>,
    #[serde(default)]
    pub(crate) ldflags: Vec<String>,
    #[serde(default, rename = "link-libs")]
    pub(crate) link_libs: Vec<String>,
    #[serde(default)]
    pub(crate) frameworks: Vec<String>,
}

/// One row in the `[patch]` table.  The only supported source
/// kind is `path = "..."`; every other key is rejected by
/// `deny_unknown_fields` as an unknown field.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPatch {
    #[serde(default)]
    pub(crate) path: Option<String>,
}

/// One row in the `[system-provided]` table: `pkg-config` for a sole
/// library target, or `targets` with one module per library target.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSystemProvider {
    #[serde(default, rename = "pkg-config")]
    pub(crate) pkg_config: Option<String>,
    #[serde(default)]
    pub(crate) targets: Option<BTreeMap<String, RawSystemTargetProvider>>,
}

/// One `targets.<name>` row of a `[system-provided]` entry.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSystemTargetProvider {
    #[serde(rename = "pkg-config")]
    pub(crate) pkg_config: String,
}

/// One `[profile.<name>]` table.
///
/// `opt-level` accepts either an integer (`0`-`3`) or a string
/// (`"s"` / `"z"`); `cabin_core::OptLevel`'s deserialiser handles
/// both shapes.
///
/// Flag fields (`cflags`, `cxxflags`, `ldflags`, `defines`,
/// `include-dirs`, `link-libs`, `frameworks`) are `Option<Vec<...>>`
/// so the parser can tell whether the table declares any of them;
/// with none, `ProfileDefinition::build` is `None`.  Declared lists
/// are appended after the inherited layers by
/// `cabin_core::resolve_build_flags`; an empty list adds nothing and
/// does not clear inherited values.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawProfile {
    #[serde(default)]
    pub(crate) inherits: Option<String>,
    #[serde(default)]
    pub(crate) debug: Option<bool>,
    #[serde(default, rename = "opt-level")]
    pub(crate) opt_level: Option<cabin_core::OptLevel>,
    #[serde(default)]
    pub(crate) assertions: Option<bool>,
    #[serde(default)]
    pub(crate) defines: Option<Vec<String>>,
    #[serde(default, rename = "include-dirs")]
    pub(crate) include_dirs: Option<Vec<Utf8PathBuf>>,
    #[serde(default)]
    pub(crate) cflags: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) cxxflags: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) ldflags: Option<Vec<String>>,
    #[serde(default, rename = "link-libs")]
    pub(crate) link_libs: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) frameworks: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPackage {
    pub(crate) name: String,
    pub(crate) version: String,
    /// `[package]`-level language standard defaults, validated by the
    /// parser.  Each field accepts a literal standard string or the
    /// `{ workspace = true }` marker that opts into the matching
    /// `[workspace]` default.  The interface fields are defaults for
    /// library-like targets only and also accept `"none"` and a
    /// `{ min, max }` table.
    #[serde(default, rename = "c-standard")]
    pub(crate) c_standard: Option<RawStandardField>,
    #[serde(default, rename = "cxx-standard")]
    pub(crate) cxx_standard: Option<RawStandardField>,
    #[serde(default, rename = "interface-c-standard")]
    pub(crate) interface_c_standard: Option<RawStandardField>,
    #[serde(default, rename = "interface-cxx-standard")]
    pub(crate) interface_cxx_standard: Option<RawStandardField>,
    /// `[package]`-level default for the per-target `gnu-extensions`
    /// boolean.  Plain boolean only - no `{ workspace = true }`
    /// marker form.
    #[serde(default, rename = "gnu-extensions")]
    pub(crate) gnu_extensions: Option<bool>,
    /// `[package.upstream]` - optional machine-verifiable upstream
    /// provenance.  Validated into a typed
    /// `cabin_core::UpstreamProvenance` by the parser.
    #[serde(default)]
    pub(crate) upstream: Option<RawUpstream>,
    /// Archive allowlist; each entry is checked with
    /// `cabin_core::upstream::is_safe_archive_path` by the parser.
    #[serde(default)]
    pub(crate) include: Vec<String>,
}

/// `[package.upstream]` table.  Field values are validated by
/// `cabin_core::UpstreamProvenance::new`; unknown fields are
/// rejected here.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawUpstream {
    pub(crate) url: String,
    pub(crate) checksum: String,
    pub(crate) format: String,
    #[serde(default, rename = "strip-prefix")]
    pub(crate) strip_prefix: Option<String>,
    /// Declared unified-diff patch files, applied after the copy
    /// steps.
    #[serde(default)]
    pub(crate) patches: Vec<String>,
    /// `[[package.upstream.copy]]` file placements.
    #[serde(default)]
    pub(crate) copy: Vec<RawUpstreamCopy>,
}

/// One `[[package.upstream.copy]]` step.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawUpstreamCopy {
    pub(crate) from: String,
    pub(crate) to: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawTarget {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) sources: Vec<Utf8PathBuf>,
    #[serde(default, rename = "include-dirs")]
    pub(crate) include_dirs: Vec<Utf8PathBuf>,
    #[serde(default)]
    pub(crate) defines: Vec<String>,
    #[serde(default, rename = "interface-defines")]
    pub(crate) interface_defines: Vec<String>,
    /// Entries accept the string shorthand (a private edge) or the
    /// `{ name = "...", public = <bool> }` table form;
    /// `cabin_core::TargetDep`'s deserialiser handles both shapes.
    #[serde(default)]
    pub(crate) deps: Vec<cabin_core::TargetDep>,
    /// Package features that must be enabled for this target to be
    /// built or used.  Validated against the package's `[features]`
    /// table by `cabin_core::Package::with_config`.
    #[serde(default, rename = "required-features")]
    pub(crate) required_features: Vec<String>,
    /// Per-target language standard overrides.  Interface fields are
    /// rejected on executable-like kinds by the parser.
    #[serde(default, rename = "c-standard")]
    pub(crate) c_standard: Option<RawStandardField>,
    #[serde(default, rename = "cxx-standard")]
    pub(crate) cxx_standard: Option<RawStandardField>,
    #[serde(default, rename = "interface-c-standard")]
    pub(crate) interface_c_standard: Option<RawStandardField>,
    #[serde(default, rename = "interface-cxx-standard")]
    pub(crate) interface_cxx_standard: Option<RawStandardField>,
    /// Per-target `gnu-extensions` override (target ▶ package ▶
    /// `false`).
    #[serde(default, rename = "gnu-extensions")]
    pub(crate) gnu_extensions: Option<bool>,
    /// Native-library identity claim; validated (grammar, kind,
    /// per-package uniqueness) by `cabin_core::Package`.
    #[serde(default)]
    pub(crate) links: Option<String>,
    /// Source path -> instruction sets that source's compile enables;
    /// `cabin_core::Package` checks each key names one of the target's
    /// declared C or C++ sources.
    #[serde(default, rename = "instruction-sets")]
    pub(crate) instruction_sets:
        std::collections::BTreeMap<Utf8PathBuf, Vec<cabin_core::InstructionSet>>,
}

/// One row of `[dependencies]`, `[dev-dependencies]`, or
/// `[build-dependencies]`, unconditional or under
/// `[target.'cfg(...)']`.  The value is either a bare string (a
/// version requirement) or a `RawDependencyTable`.
#[derive(Debug, Clone)]
pub(crate) enum RawDependency {
    String(String),
    Table(RawDependencyTable),
}

// Hand-rolled Deserialize so a table-shaped value reports the
// table's own typed error (including `deny_unknown_fields`
// "unknown field `<name>`" messages).  The default
// `#[serde(untagged)]` derive collapses every failure to
// "data did not match any variant", which hides the offending
// field name from the diagnostic.
impl<'de> Deserialize<'de> for RawDependency {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RawDependencyVisitor;

        impl<'de> Visitor<'de> for RawDependencyVisitor {
            type Value = RawDependency;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a version requirement string or a dependency table")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(RawDependency::String(v.to_owned()))
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(RawDependency::String(v))
            }

            fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                RawDependencyTable::deserialize(MapAccessDeserializer::new(map))
                    .map(RawDependency::Table)
            }
        }

        deserializer.deserialize_any(RawDependencyVisitor)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawDependencyTable {
    #[serde(default)]
    pub(crate) path: Option<String>,
    #[serde(default)]
    pub(crate) version: Option<String>,
    /// `{ workspace = true }` opts the package into the
    /// workspace-level dependency declared under the matching
    /// `[workspace.<kind>-dependencies]` table (or
    /// `[workspace.dependencies]` for normal deps).
    #[serde(default)]
    pub(crate) workspace: Option<bool>,
    /// `{ system = true }` marks this dependency as
    /// system-sourced: Cabin probes it via `pkg-config` at
    /// build time instead of resolving it through the registry
    /// or workspace tables.  Mutually exclusive with `path`,
    /// `workspace`, `features`, `default-features`, and
    /// `optional` (the parser rejects each combination
    /// individually).
    #[serde(default)]
    pub(crate) system: bool,
    /// `optional = true` marks a normal-kind dependency as
    /// inactive until a feature implication enables it.
    #[serde(default)]
    pub(crate) optional: Option<bool>,
    /// Explicit list of features to enable on the dependency.
    #[serde(default)]
    pub(crate) features: Option<Vec<String>>,
    /// `default-features = false` disables the dependency's
    /// declared default features.
    #[serde(default, rename = "default-features")]
    pub(crate) default_features: Option<bool>,
    /// `ignore-interface-standard = true` exempts exactly this
    /// dependency edge from the standard-compatibility check (the
    /// edge is reported as unchecked instead).  Mutually exclusive
    /// with `system` (system deps never enter the check).
    #[serde(default, rename = "ignore-interface-standard")]
    pub(crate) ignore_interface_standard: Option<bool>,
}

/// One language-standard field value in `[package]`,
/// `[target.<name>]`, or `[workspace]`: a literal standard string,
/// the `{ workspace = true }` opt-in marker that inherits the
/// matching `[workspace]` default (accepted in `[package]` only), or
/// a range table.
#[derive(Debug, Clone)]
pub(crate) enum RawStandardField {
    Value(String),
    Marker(RawStandardMarker),
    /// The `{ min = "...", max = "..." }` bounded-requirement table,
    /// valid on interface fields only (the parse layer rejects it on
    /// implementation fields).
    Range(RawStandardRange),
}

/// The `{ workspace = <bool> }` marker table.
#[derive(Debug, Clone)]
pub(crate) struct RawStandardMarker {
    pub(crate) workspace: bool,
}

/// The bounded-requirement table: `min` is required (validated at
/// the parse layer for a field-specific diagnostic), `max`
/// optional.
#[derive(Debug, Clone)]
pub(crate) struct RawStandardRange {
    pub(crate) min: Option<String>,
    pub(crate) max: Option<String>,
}

// Hand-rolled Deserialize so a table-shaped value reports the
// table's own typed error (the `#[serde(untagged)]` derive would
// collapse every failure to "data did not match any variant").
// One table shape carries all recognized keys: `workspace` selects
// the opt-in marker, `min` / `max` a bounded requirement, and
// mixing the two families is rejected here where the key names are
// still on hand.
impl<'de> Deserialize<'de> for RawStandardField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RawStandardFieldVisitor;

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawStandardTable {
            workspace: Option<bool>,
            min: Option<String>,
            max: Option<String>,
        }

        impl<'de> Visitor<'de> for RawStandardFieldVisitor {
            type Value = RawStandardField;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "a language standard string, `{ workspace = true }`, or `{ min = \"...\", max = \"...\" }`",
                )
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(RawStandardField::Value(v.to_owned()))
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(RawStandardField::Value(v))
            }

            fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let table = RawStandardTable::deserialize(MapAccessDeserializer::new(map))?;
                match (table.workspace, &table.min, &table.max) {
                    (Some(_), None, None) => Ok(RawStandardField::Marker(RawStandardMarker {
                        workspace: table.workspace.unwrap_or(false),
                    })),
                    (None, _, _) => Ok(RawStandardField::Range(RawStandardRange {
                        min: table.min,
                        max: table.max,
                    })),
                    (Some(_), _, _) => Err(serde::de::Error::custom(
                        "`workspace = true` cannot be combined with `min` / `max`; opt into the workspace default or declare the bounds, not both",
                    )),
                }
            }
        }

        deserializer.deserialize_any(RawStandardFieldVisitor)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawWorkspace {
    #[serde(default)]
    pub(crate) members: Vec<String>,
    /// Paths or `pattern/*` globs that are *not* workspace
    /// members even when matched by `members`.
    #[serde(default)]
    pub(crate) exclude: Vec<String>,
    /// Subset of `members` that are operated on by default
    /// when the user passes no package-selection flags at the
    /// workspace root.
    #[serde(default, rename = "default-members")]
    pub(crate) default_members: Vec<String>,
    /// Shared `[workspace.dependencies]` (normal-kind workspace
    /// dependencies).
    #[serde(default)]
    pub(crate) dependencies: BTreeMap<String, String>,
    /// Shared `[workspace.dev-dependencies]`.
    #[serde(default, rename = "dev-dependencies")]
    pub(crate) dev_dependencies: BTreeMap<String, String>,
    /// Shared `[workspace.build-dependencies]`.
    #[serde(default, rename = "build-dependencies")]
    pub(crate) build_dependencies: BTreeMap<String, String>,
    /// `[workspace]`-level language-standard defaults members opt
    /// into per field with `<field> = { workspace = true }`.  The
    /// marker form is not accepted here; the implementation fields
    /// are plain strings, and the interface fields take the same
    /// remaining shapes as their `[package]` counterparts (`"none"`,
    /// a minimum string, or a `{ min, max }` table).
    #[serde(default, rename = "c-standard")]
    pub(crate) c_standard: Option<String>,
    #[serde(default, rename = "cxx-standard")]
    pub(crate) cxx_standard: Option<String>,
    #[serde(default, rename = "interface-c-standard")]
    pub(crate) interface_c_standard: Option<RawStandardField>,
    #[serde(default, rename = "interface-cxx-standard")]
    pub(crate) interface_cxx_standard: Option<RawStandardField>,
}
