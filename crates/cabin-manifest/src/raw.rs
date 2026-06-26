//! Private serde structures that mirror `cabin.toml` shape.
//!
//! These types deliberately stay inside `cabin-manifest`.  The conversion in
//! `parse.rs` immediately turns them into validated `cabin_core` values so
//! the rest of the workspace never sees raw manifest layout.

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
    /// `[target.<NAME>]` declares a buildable C/C++ target.
    /// We deserialise lazily (`toml::Value`) so we can cleanly
    /// reject the unsupported `[target.'cfg(...)'.dependencies]`
    /// platform-specific dependency syntax with a clear error
    /// before flowing into `RawTarget`.
    #[serde(default)]
    pub(crate) target: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub(crate) dependencies: BTreeMap<String, RawDependency>,
    /// `[dev-dependencies]` - Cabin package dependencies for
    /// `test` / `example` targets.  Declaration-only for
    /// ordinary commands; `cabin test` activates them for the
    /// selected primary packages.
    #[serde(default, rename = "dev-dependencies")]
    pub(crate) dev_dependencies: BTreeMap<String, RawDependency>,
    #[serde(default)]
    pub(crate) workspace: Option<RawWorkspace>,
    /// `[features]`.  The `default` key, when present, is the
    /// list of features enabled when the user does not pass
    /// `--no-default-features`.  Other keys declare individual features
    /// and their implication arrows.
    #[serde(default)]
    pub(crate) features: BTreeMap<String, Vec<String>>,
    /// `[profile.<name>]` tables.  Validated by the parser and
    /// converted to typed `cabin_core::ProfileDefinition` values.
    /// Only the workspace root manifest is permitted to declare
    /// profile tables; member manifests that do are rejected with
    /// a clear error so a single workspace key cannot silently
    /// mean different things in different members.
    ///
    /// `[profile]` carries the unconditional per-package build
    /// flags (defines, include dirs, language-specific flag
    /// lists, compiler cache); `[profile.<name>]` adds per-profile
    /// knobs (`opt-level`, `debug`, …) plus optional per-profile
    /// overrides of the base flag lists.
    #[serde(default)]
    pub(crate) profile: Option<RawProfileTable>,
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
}

/// `[toolchain]` table.  Adding new fields here requires a matching
/// extension in `cabin_core::toolchain` and a new error variant in
/// `ManifestError`.  Anything unknown is rejected.
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

/// `[profile]` parent table.  Holds the unconditional per-package
/// build-flag fields directly (replaces the v1 `[profile]` table)
/// plus a flatten-captured map of named variants (one per
/// `[profile.<name>]` sub-table).
///
/// `#[serde(deny_unknown_fields)]` is intentionally not set here:
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
    /// `[profile.cache]` sub-table.  Holds compiler-cache wrapper
    /// settings (`ccache`, `sccache`).  Workspace-root only - the
    /// loader rejects member manifests that declare it.
    #[serde(default)]
    pub(crate) cache: Option<RawProfileCache>,
    /// Named profile variants (`[profile.dev]`, `[profile.release]`,
    /// any custom name).  Captured as a map so the parser can
    /// validate inheritance chains and merge against the base
    /// flags above.
    #[serde(flatten)]
    pub(crate) variants: BTreeMap<String, RawProfile>,
}

/// Conditional `[target.'cfg(...)'.profile]` flag-bag.  Same shape
/// as the per-package base flags on `[profile]`.
#[derive(Debug, Clone, Deserialize)]
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
    /// `[target.'cfg(...)'.profile.cache]` sub-table for the
    /// conditional case.
    #[serde(default)]
    pub(crate) cache: Option<RawProfileCache>,
}

/// `[profile.cache]` sub-table.  The compiler-cache wrapper applied to
/// every C++ compile command in this build invocation.  Field names
/// are kebab-cased to match the rest of the manifest grammar.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawProfileCache {
    /// `compiler-wrapper = "ccache" | "sccache" | "none"`.  The
    /// special value `"none"` represents an explicit opt-out.
    /// Anything else is rejected by the parser.
    #[serde(default, rename = "compiler-wrapper")]
    pub(crate) compiler_wrapper: Option<String>,
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

/// One `[profile.<name>]` table.
///
/// `opt-level` accepts either an integer (`0`-`3`) or a string
/// (`"s"` / `"z"`); `cabin_core::OptLevel`'s deserialiser handles
/// both shapes.  Unknown fields are rejected so unsupported future
/// keys do not silently slip through.
///
/// Flag fields (`cflags`, `cxxflags`, `ldflags`, `defines`,
/// `include-dirs`) are `Option<Vec<...>>` so the resolver can
/// distinguish "the user did not override this layer" (`None`,
/// inherit the base / chained value) from "the user set it to
/// an empty list" (`Some(vec![])`, replace the base with empty).
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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPackage {
    pub(crate) name: String,
    pub(crate) version: String,
    /// `[package]`-level language standard defaults.  Validated into
    /// typed `cabin_core::CStandard` / `CxxStandard` values by the
    /// parser; the interface fields are defaults for library-like
    /// targets only.  Each field accepts either a literal standard
    /// string or the `{ workspace = true }` marker that opts into
    /// the matching `[workspace]` default.
    #[serde(default, rename = "c-standard")]
    pub(crate) c_standard: Option<RawStandardField>,
    #[serde(default, rename = "cxx-standard")]
    pub(crate) cxx_standard: Option<RawStandardField>,
    #[serde(default, rename = "interface-c-standard")]
    pub(crate) interface_c_standard: Option<RawStandardField>,
    #[serde(default, rename = "interface-cxx-standard")]
    pub(crate) interface_cxx_standard: Option<RawStandardField>,
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
    #[serde(default)]
    pub(crate) deps: Vec<String>,
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
}

/// Cabin package dependency entry, e.g. one row of
/// `[dependencies]` or `[dev-dependencies]`.  The value may be
/// either a bare
/// string (interpreted as a version requirement) or a table with
/// `path = "..."` / `version = "..."` / `workspace = true`.
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
    /// `{ port = true }` declares a foundation-port dependency
    /// resolved by the dep's name against the bundled set in
    /// `cabin_port::builtin`. `port = false` is treated as if
    /// the field were absent so it never collides with another
    /// source.  Mutually exclusive with `port-path`, `path`,
    /// `version`, `workspace`, and `system`; does not support
    /// `features`, `default-features`, or `optional`.
    #[serde(default)]
    pub(crate) port: Option<bool>,
    /// `{ port-path = "..." }` declares a foundation-port
    /// dependency resolved by filesystem path to a recipe
    /// directory containing `port.toml` and an overlay
    /// `cabin.toml`.  Mutually exclusive with `port`, `path`,
    /// `version`, `workspace`, and `system`.
    #[serde(default, rename = "port-path")]
    pub(crate) port_path: Option<String>,
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
    /// individually).  System dependencies are unconditionally
    /// required; there is no `required` field.
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
}

/// One `[package]`-level standard field value: a literal standard
/// string or the `{ workspace = true }` opt-in marker that
/// inherits the matching `[workspace]` default.
#[derive(Debug, Clone)]
pub(crate) enum RawStandardField {
    Value(String),
    Marker(RawStandardMarker),
}

/// The `{ workspace = <bool> }` marker table.  Any other key is
/// rejected by `deny_unknown_fields`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawStandardMarker {
    pub(crate) workspace: bool,
}

// Hand-rolled Deserialize so a table-shaped value reports the
// table's own typed error (the `#[serde(untagged)]` derive would
// collapse every failure to "data did not match any variant").
impl<'de> Deserialize<'de> for RawStandardField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RawStandardFieldVisitor;

        impl<'de> Visitor<'de> for RawStandardFieldVisitor {
            type Value = RawStandardField;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a language standard string or `{ workspace = true }`")
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
                RawStandardMarker::deserialize(MapAccessDeserializer::new(map))
                    .map(RawStandardField::Marker)
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
    /// Workspace root.  Rendered hyphenated to match common
    /// package-manager conventions.
    #[serde(default, rename = "default-members")]
    pub(crate) default_members: Vec<String>,
    /// Shared `[workspace.dependencies]` (normal-kind workspace
    /// dependencies).  Stored as strings so the existing
    /// version-requirement parser can be reused.
    #[serde(default)]
    pub(crate) dependencies: BTreeMap<String, String>,
    /// Shared `[workspace.dev-dependencies]`.
    #[serde(default, rename = "dev-dependencies")]
    pub(crate) dev_dependencies: BTreeMap<String, String>,
    /// `[workspace]`-level language-standard defaults members opt
    /// into per field with `<field> = { workspace = true }`.  Plain
    /// strings - the marker form is not accepted here.
    #[serde(default, rename = "c-standard")]
    pub(crate) c_standard: Option<String>,
    #[serde(default, rename = "cxx-standard")]
    pub(crate) cxx_standard: Option<String>,
    #[serde(default, rename = "interface-c-standard")]
    pub(crate) interface_c_standard: Option<String>,
    #[serde(default, rename = "interface-cxx-standard")]
    pub(crate) interface_cxx_standard: Option<String>,
}
