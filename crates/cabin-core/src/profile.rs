//! Build profiles.
//!
//! A profile is a named preset of build settings that affect how
//! Cabin compiles a package — debug information, optimisation
//! level, assertions. Two profiles are built in:
//!
//! - `dev` — local development. Debug info on, no optimisation,
//!   assertions on.
//! - `release` — optimised builds. Debug info off, full
//!   optimisation, assertions off.
//!
//! Manifests may declare additional `[profile.<name>]` tables to
//! override the built-in defaults or to add custom presets that
//! `inherit` from one of the built-ins. Resolution merges
//! parents-first and is fully typed; the rest of Cabin
//! (`cabin-build`, `cabin-cli`, `cabin-package`) consumes a
//! [`ResolvedProfile`] directly and never sees raw TOML.
//!
//! This module owns the *model and the resolver*. Manifest parsing
//! lives in `cabin-manifest`; CLI flag handling lives in
//! `cabin-cli`.

use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One of the two profiles Cabin always provides without any
/// manifest declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BuiltinProfile {
    Dev,
    Release,
}

impl BuiltinProfile {
    /// Iterate over every built-in in a stable order.
    pub fn all() -> [BuiltinProfile; 2] {
        [BuiltinProfile::Dev, BuiltinProfile::Release]
    }

    /// Public name as it appears in `[profile.<name>]` and on the
    /// CLI.
    pub fn as_str(self) -> &'static str {
        match self {
            BuiltinProfile::Dev => "dev",
            BuiltinProfile::Release => "release",
        }
    }

    /// Default field values for this built-in.
    pub fn defaults(self) -> ProfileDefaults {
        match self {
            BuiltinProfile::Dev => ProfileDefaults {
                debug: true,
                opt_level: OptLevel::O0,
                assertions: true,
            },
            BuiltinProfile::Release => ProfileDefaults {
                debug: false,
                opt_level: OptLevel::O3,
                assertions: false,
            },
        }
    }

    /// Look up a built-in by name (case-sensitive).
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "dev" => Some(BuiltinProfile::Dev),
            "release" => Some(BuiltinProfile::Release),
            _ => None,
        }
    }
}

impl fmt::Display for BuiltinProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Concrete defaults for one profile. Used to seed inheritance
/// before any manifest overrides apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileDefaults {
    pub debug: bool,
    pub opt_level: OptLevel,
    pub assertions: bool,
}

/// Semantic optimisation level. Mirrors the GCC / Clang `-O`
/// family without exposing raw flag strings; the build planner
/// translates each value into the toolchain's flag at command
/// construction time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OptLevel {
    /// `-O0`. No optimisation; the dev profile default.
    O0,
    /// `-O1`. Lightweight optimisation.
    O1,
    /// `-O2`. Standard optimisation.
    O2,
    /// `-O3`. Aggressive optimisation; the release profile default.
    O3,
    /// `-Os`. Optimise for size.
    S,
    /// `-Oz`. Optimise harder for size where the toolchain supports
    /// it; falls back to `-Os` semantics for toolchains that do
    /// not.
    Z,
}

impl OptLevel {
    /// Compiler flag for this level. The string form is stable and
    /// is what the build planner appends to C and C++ compile
    /// commands.
    pub fn as_flag(self) -> &'static str {
        match self {
            OptLevel::O0 => "-O0",
            OptLevel::O1 => "-O1",
            OptLevel::O2 => "-O2",
            OptLevel::O3 => "-O3",
            OptLevel::S => "-Os",
            OptLevel::Z => "-Oz",
        }
    }

    /// Value used in JSON / metadata serialisation. Mirrors the
    /// public manifest key (`opt-level`).
    pub fn as_str(self) -> &'static str {
        match self {
            OptLevel::O0 => "0",
            OptLevel::O1 => "1",
            OptLevel::O2 => "2",
            OptLevel::O3 => "3",
            OptLevel::S => "s",
            OptLevel::Z => "z",
        }
    }
}

impl fmt::Display for OptLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for OptLevel {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for OptLevel {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        // `opt-level` accepts numeric integers (0..=3) and the
        // string aliases `"s"` / `"z"`. The TOML deserialiser hands
        // the parsed value through one of these channels; both must
        // reach the same `OptLevel`.
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = OptLevel;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("0, 1, 2, 3, \"s\", or \"z\"")
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<OptLevel, E> {
                OptLevel::parse(s).map_err(serde::de::Error::custom)
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<OptLevel, E> {
                OptLevel::parse(&v.to_string()).map_err(serde::de::Error::custom)
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<OptLevel, E> {
                OptLevel::parse(&v.to_string()).map_err(serde::de::Error::custom)
            }
        }
        de.deserialize_any(V)
    }
}

impl OptLevel {
    /// Parse the public manifest form. Accepts integers `0..=3`
    /// and the lowercase letters `"s"` / `"z"` exactly. Anything
    /// else returns a stable, user-facing error string.
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "0" => Ok(OptLevel::O0),
            "1" => Ok(OptLevel::O1),
            "2" => Ok(OptLevel::O2),
            "3" => Ok(OptLevel::O3),
            "s" => Ok(OptLevel::S),
            "z" => Ok(OptLevel::Z),
            other => Err(format!(
                "invalid opt-level {other:?}; expected 0, 1, 2, 3, \"s\", or \"z\""
            )),
        }
    }
}

/// Validated profile name.
///
/// Profile names appear in three places: the manifest TOML key
/// (`[profile.<name>]`), the CLI flag (`--profile <name>`), and
/// the on-disk build directory layout
/// (`<build_dir>/<profile>/...`). The grammar below is the
/// intersection of those three constraints so a single value can
/// flow through all of them without per-stage re-validation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProfileName(String);

impl ProfileName {
    /// Construct a [`ProfileName`] after running validation.
    ///
    /// A name is valid iff:
    ///
    /// - it is non-empty;
    /// - it consists only of ASCII alphanumerics, `_`, `-`, `.`;
    /// - it does not start with `.`;
    /// - it is not literally `.` or `..`.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidProfileName> {
        let value = value.into();
        if !is_path_safe_profile_name(&value) {
            return Err(InvalidProfileName(value));
        }
        Ok(Self(value))
    }

    /// Construct a [`ProfileName`] for one of Cabin's two built-ins.
    /// Built-in names are guaranteed valid so this never fails.
    pub fn builtin(profile: BuiltinProfile) -> Self {
        Self(profile.as_str().to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the matching [`BuiltinProfile`] when this name
    /// refers to a built-in.
    pub fn as_builtin(&self) -> Option<BuiltinProfile> {
        BuiltinProfile::from_name(&self.0)
    }
}

impl fmt::Display for ProfileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ProfileName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for ProfileName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<ProfileName> for String {
    fn from(name: ProfileName) -> Self {
        name.0
    }
}

impl TryFrom<String> for ProfileName {
    type Error = InvalidProfileName;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        ProfileName::new(value)
    }
}

/// Returns whether `name` matches the [`ProfileName`] grammar.
pub(crate) fn is_path_safe_profile_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    if name == "." || name == ".." {
        return false;
    }
    if name.starts_with('.') {
        return false;
    }
    name.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// `cabin.toml`'s public grammar limits which characters profile
/// names may contain. The constructor surfaces this error type so
/// callers (CLI, manifest parser) can format a clear diagnostic
/// without duplicating the rule.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error(
    "invalid profile name {0:?}; profile names must be non-empty, must not start with `.`, must not be `.` or `..`, and may only contain ASCII alphanumerics, `_`, `-`, or `.`"
)]
pub struct InvalidProfileName(pub String);

/// One `[profile.<name>]` declaration as it appeared in
/// `cabin.toml`, after manifest-level validation but before
/// inheritance resolution. Every field except `name` is `Option`
/// so the resolver can tell "user did not set this" from "user
/// set this to a value".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileDefinition {
    pub name: ProfileName,
    /// Profile this one inherits from. Required for custom
    /// profiles; rejected on built-in profiles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherits: Option<ProfileName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<bool>,
    #[serde(default, rename = "opt-level", skip_serializing_if = "Option::is_none")]
    pub opt_level: Option<OptLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assertions: Option<bool>,
    /// Per-profile flag overrides for `[profile.<name>]` — defines,
    /// include directories, and extra compile / link arguments that
    /// apply when this profile is selected. `None` when the profile
    /// has no flag overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<crate::build_flags::ProfileFlags>,
}

/// User-facing profile selection (one CLI invocation picks at
/// most one profile). The resolver expands this into a full
/// [`ResolvedProfile`] against a definition table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSelection {
    pub name: ProfileName,
}

impl ProfileSelection {
    /// Default selection when neither `--profile` nor `--release`
    /// is supplied — the `dev` built-in.
    pub fn default_dev() -> Self {
        Self {
            name: ProfileName::builtin(BuiltinProfile::Dev),
        }
    }

    /// Selection produced by the legacy `--release` flag, kept as
    /// a compatibility alias for `--profile release`.
    pub fn release_alias() -> Self {
        Self {
            name: ProfileName::builtin(BuiltinProfile::Release),
        }
    }

    /// Selection from a user-supplied `--profile <name>` argument.
    pub fn from_name(name: ProfileName) -> Self {
        Self { name }
    }
}

/// Where a [`ResolvedProfile`] originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileSource {
    /// One of `dev` / `release` with no manifest entry.
    Builtin,
    /// One of `dev` / `release` with a `[profile.dev]` /
    /// `[profile.release]` manifest override.
    BuiltinOverridden,
    /// A user-defined `[profile.<name>]` inheriting from a
    /// built-in (directly or transitively).
    Custom,
}

/// Fully resolved profile.
///
/// Scalar fields are typed and concrete; downstream consumers
/// (build planner, CLI) read this struct directly. `build` is
/// the per-profile flag overlay merged root → selected across
/// `inherits_chain` — see the field docstring.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedProfile {
    pub name: ProfileName,
    pub debug: bool,
    pub opt_level: OptLevel,
    pub assertions: bool,
    pub source: ProfileSource,
    /// Chain of profile names walked by inheritance, root first.
    /// For built-ins this is `[name]`; for a custom profile that
    /// inherits from `release` it is `["release", <custom>]`.
    /// The chain is also the order used to **append** each
    /// step's `ProfileDefinition.build` into [`Self::build`].
    pub inherits_chain: Vec<ProfileName>,
    /// `[profile.<name>]` per-profile flag overlay, merged
    /// root-first across `inherits_chain` via
    /// `ProfileFlags::append_layer`.
    ///
    /// `None` means **no** profile in the chain declared
    /// `build = Some(_)`. `Some(_)` means at least one step
    /// contributed profile flags, even if the resulting
    /// accumulator happens to be empty (uniform shape for
    /// consumers).
    ///
    /// `#[serde(skip)]` because the merged value is computed
    /// from the inherits-chain walk inside
    /// [`resolve_profile`] — it isn't part of the on-disk JSON
    /// schema. `cabin metadata`'s JSON view of a profile comes
    /// from [`Self::as_json`], which lists fields explicitly;
    /// the resolved build flags surface under
    /// `BuildConfiguration.build_flags` in metadata output, not
    /// here.
    #[serde(skip)]
    pub build: Option<crate::build_flags::ProfileFlags>,
}

impl ResolvedProfile {
    /// Compact JSON view used by `cabin metadata` and by
    /// `CABIN_BUILD_CONFIGURATION_JSON`. Field order matches the
    /// struct declaration order so the on-disk shape is stable.
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name.as_str(),
            "debug": self.debug,
            "opt_level": self.opt_level.as_str(),
            "assertions": self.assertions,
            "source": match self.source {
                ProfileSource::Builtin => "builtin",
                ProfileSource::BuiltinOverridden => "builtin-overridden",
                ProfileSource::Custom => "custom",
            },
            "inherits_chain": self
                .inherits_chain
                .iter()
                .map(|n| n.as_str())
                .collect::<Vec<_>>(),
        })
    }

    /// Compute the language-neutral compile flags this profile
    /// contributes.
    /// The order is fixed: `-O<level>` first, then `-g` when
    /// debug info is requested, then `-DNDEBUG` when assertions
    /// are off. Determinism matters here because the result lands
    /// in `compile_commands.json`.
    pub fn compile_flags(&self) -> Vec<&'static str> {
        let mut out = Vec::with_capacity(3);
        out.push(self.opt_level.as_flag());
        if self.debug {
            out.push("-g");
        }
        if !self.assertions {
            out.push("-DNDEBUG");
        }
        out
    }
}

/// Errors produced by [`resolve_profile`].
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProfileResolutionError {
    /// The user selected a profile that neither matches a built-in
    /// nor a manifest entry.
    #[error("unknown profile `{name}`")]
    UnknownProfile { name: String },

    /// A custom profile's `inherits =` points at a name that does
    /// not exist.
    #[error("profile `{profile}` inherits from unknown profile `{parent}`")]
    UnknownInheritedProfile { profile: String, parent: String },

    /// The inheritance graph contains a cycle. The chain is
    /// rendered with `->` separators so the diagnostic is
    /// scannable in CI logs.
    #[error("profile inheritance cycle detected: {}", display_chain(.chain))]
    InheritanceCycle { chain: Vec<String> },

    /// `[profile.dev]` or `[profile.release]` declared
    /// `inherits =`, which is not allowed because built-in
    /// profiles already have implicit defaults.
    #[error("built-in profile `{name}` cannot declare `inherits`; only custom profiles inherit")]
    BuiltinCannotInherit { name: String },

    /// A custom profile omitted `inherits =`. Cabin requires the
    /// field on every custom profile so the inheritance closure is
    /// explicit.
    #[error(
        "custom profile `{name}` must declare `inherits = \"dev\"` or `inherits = \"release\"` (or another custom profile)"
    )]
    CustomMissingInherits { name: String },
}

fn display_chain(chain: &[String]) -> String {
    chain.join(" -> ")
}

/// Resolve a [`ProfileSelection`] against a set of manifest
/// [`ProfileDefinition`]s.
///
/// `definitions` is the workspace-root manifest's
/// `[profile.<name>]` table set. Built-in profiles (`dev`,
/// `release`) do not need to appear in the table; if they do, the
/// values override the built-in defaults.
///
/// Resolution rules:
///
/// - if the selection names a built-in and no override exists,
///   return [`ProfileSource::Builtin`] with the built-in defaults;
/// - if the selection names a built-in *with* an override, apply
///   the override on top of the defaults, mark the result as
///   [`ProfileSource::BuiltinOverridden`];
/// - if the selection names a custom profile, walk the
///   `inherits` chain to a built-in root, merge fields
///   parents-first, mark the result as [`ProfileSource::Custom`];
/// - the chain is checked for cycles and unknown parents up
///   front so the merge step never panics.
///
/// Merge semantics across the inherits chain:
///
/// - **Scalar fields** (`opt-level`, `debug`, `assertions`) use
///   **replacement** — root first, child later, later wins.
/// - **Array fields** in
///   [`ProfileDefinition::build`] (`cflags`, `cxxflags`,
///   `ldflags`, `defines`, `include-dirs`) use **append**:
///   each chain step's
///   layer is folded into the accumulator via
///   `ProfileFlags::append_layer` in
///   root → selected order. The merged result lands on
///   [`ResolvedProfile::build`] and is passed to
///   [`crate::build_flags::resolve_build_flags`] downstream so
///   the package's `[profile]` / `[target.'cfg(...)'.profile]`
///   layers sit beneath the chain-merged profile flags.
pub fn resolve_profile(
    selection: &ProfileSelection,
    definitions: &BTreeMap<ProfileName, ProfileDefinition>,
) -> Result<ResolvedProfile, ProfileResolutionError> {
    validate_definitions(definitions)?;

    let mut chain: Vec<ProfileName> = Vec::new();
    let mut seen: BTreeSet<ProfileName> = BTreeSet::new();
    let mut cursor = selection.name.clone();

    // Walk inheritance up to a built-in root. The chain ends as
    // soon as either (a) `cursor` names a built-in or (b) `cursor`
    // names a manifest definition that has no `inherits` (which is
    // only legal for built-in overrides).
    loop {
        if !seen.insert(cursor.clone()) {
            // Cycle: render the chain ending at the offending name.
            let mut display: Vec<String> = chain.iter().map(|n| n.as_str().to_owned()).collect();
            display.push(cursor.as_str().to_owned());
            return Err(ProfileResolutionError::InheritanceCycle { chain: display });
        }
        chain.push(cursor.clone());

        if let Some(def) = definitions.get(&cursor) {
            match (cursor.as_builtin(), &def.inherits) {
                (Some(_), None) => break,
                (None, Some(parent)) => {
                    if !definitions.contains_key(parent) && parent.as_builtin().is_none() {
                        return Err(ProfileResolutionError::UnknownInheritedProfile {
                            profile: cursor.as_str().to_owned(),
                            parent: parent.as_str().to_owned(),
                        });
                    }
                    cursor = parent.clone();
                    continue;
                }
                (Some(_), Some(_)) => {
                    unreachable!("validate_definitions rejects `inherits` on built-ins")
                }
                (None, None) => {
                    unreachable!("validate_definitions rejects custom profiles without `inherits`")
                }
            }
        }

        if cursor.as_builtin().is_some() {
            break;
        }

        return Err(ProfileResolutionError::UnknownProfile {
            name: cursor.as_str().to_owned(),
        });
    }

    // `chain` is selected -> ... -> root. Reverse so we merge
    // root-first.
    chain.reverse();

    let root_name = chain.first().expect("chain is non-empty after walk");
    let builtin = root_name
        .as_builtin()
        .ok_or_else(|| ProfileResolutionError::UnknownProfile {
            name: root_name.as_str().to_owned(),
        })?;
    let defaults = builtin.defaults();

    let mut debug = defaults.debug;
    let mut opt_level = defaults.opt_level;
    let mut assertions = defaults.assertions;
    // Per-profile flag arrays merge with **append** semantics
    // across the inherits chain — root → selected. Scalars
    // above use replacement (later wins); arrays here use
    // accumulation. The merge stays out of `as_json` so the
    // cabin-metadata schema is unchanged.
    let mut merged_build: Option<crate::build_flags::ProfileFlags> = None;
    for step in &chain {
        if let Some(def) = definitions.get(step) {
            if let Some(d) = def.debug {
                debug = d;
            }
            if let Some(o) = def.opt_level {
                opt_level = o;
            }
            if let Some(a) = def.assertions {
                assertions = a;
            }
            if let Some(layer) = def.build.as_ref() {
                let acc =
                    merged_build.get_or_insert_with(crate::build_flags::ProfileFlags::default);
                acc.append_layer(layer);
            }
        }
    }

    let final_name = selection.name.clone();
    let source = match (
        final_name.as_builtin(),
        definitions.contains_key(&final_name),
    ) {
        (Some(_), true) => ProfileSource::BuiltinOverridden,
        (Some(_), false) => ProfileSource::Builtin,
        (None, _) => ProfileSource::Custom,
    };

    Ok(ResolvedProfile {
        name: final_name,
        debug,
        opt_level,
        assertions,
        source,
        inherits_chain: chain,
        build: merged_build,
    })
}

/// Whole-table validation: every custom profile declares
/// `inherits`, no built-in declares it, and inherits-targets are
/// known. Cycles are caught in [`resolve_profile`] when the chain
/// is walked.
fn validate_definitions(
    definitions: &BTreeMap<ProfileName, ProfileDefinition>,
) -> Result<(), ProfileResolutionError> {
    for (name, def) in definitions {
        match (name.as_builtin(), &def.inherits) {
            (Some(_), Some(_)) => {
                return Err(ProfileResolutionError::BuiltinCannotInherit {
                    name: name.as_str().to_owned(),
                });
            }
            (None, None) => {
                return Err(ProfileResolutionError::CustomMissingInherits {
                    name: name.as_str().to_owned(),
                });
            }
            (None, Some(parent)) => {
                if !definitions.contains_key(parent) && parent.as_builtin().is_none() {
                    return Err(ProfileResolutionError::UnknownInheritedProfile {
                        profile: name.as_str().to_owned(),
                        parent: parent.as_str().to_owned(),
                    });
                }
            }
            (Some(_), None) => {}
        }
    }
    Ok(())
}

/// Enumerate every profile name reachable for a given definition
/// set: the two built-ins plus every manifest-declared name.
/// Useful for `cabin metadata --profile-list`-style consumers
/// without forcing each caller to special-case built-ins.
pub fn available_profile_names(
    definitions: &BTreeMap<ProfileName, ProfileDefinition>,
) -> Vec<ProfileName> {
    let mut names: BTreeSet<ProfileName> = BTreeSet::new();
    for builtin in BuiltinProfile::all() {
        names.insert(ProfileName::builtin(builtin));
    }
    for k in definitions.keys() {
        names.insert(k.clone());
    }
    names.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(s: &str) -> ProfileName {
        ProfileName::new(s).unwrap()
    }

    fn def(
        n: &str,
        inherits: Option<&str>,
        debug: Option<bool>,
        opt: Option<OptLevel>,
        assertions: Option<bool>,
    ) -> (ProfileName, ProfileDefinition) {
        let pn = name(n);
        let def = ProfileDefinition {
            name: pn.clone(),
            inherits: inherits.map(name),
            debug,
            opt_level: opt,
            assertions,
            build: None,
        };
        (pn, def)
    }

    fn defs(
        items: Vec<(ProfileName, ProfileDefinition)>,
    ) -> BTreeMap<ProfileName, ProfileDefinition> {
        items.into_iter().collect()
    }

    #[test]
    fn dev_default_is_built_in_and_unmodified() {
        let r = resolve_profile(&ProfileSelection::default_dev(), &BTreeMap::new()).unwrap();
        assert_eq!(r.name.as_str(), "dev");
        assert!(r.debug);
        assert_eq!(r.opt_level, OptLevel::O0);
        assert!(r.assertions);
        assert_eq!(r.source, ProfileSource::Builtin);
        assert_eq!(r.inherits_chain.len(), 1);
    }

    #[test]
    fn release_default_is_built_in_and_unmodified() {
        let r = resolve_profile(&ProfileSelection::release_alias(), &BTreeMap::new()).unwrap();
        assert_eq!(r.name.as_str(), "release");
        assert!(!r.debug);
        assert_eq!(r.opt_level, OptLevel::O3);
        assert!(!r.assertions);
        assert_eq!(r.source, ProfileSource::Builtin);
    }

    #[test]
    fn dev_override_marks_source_builtin_overridden() {
        let d = defs(vec![def(
            "dev",
            None,
            Some(false),
            Some(OptLevel::O2),
            None,
        )]);
        let r = resolve_profile(&ProfileSelection::default_dev(), &d).unwrap();
        assert_eq!(r.opt_level, OptLevel::O2);
        assert!(!r.debug);
        // Assertions inherits from the built-in default.
        assert!(r.assertions);
        assert_eq!(r.source, ProfileSource::BuiltinOverridden);
    }

    #[test]
    fn release_override_keeps_unaffected_fields() {
        let d = defs(vec![def("release", None, Some(true), None, None)]);
        let r = resolve_profile(&ProfileSelection::release_alias(), &d).unwrap();
        assert!(r.debug);
        assert_eq!(r.opt_level, OptLevel::O3);
        assert!(!r.assertions);
        assert_eq!(r.source, ProfileSource::BuiltinOverridden);
    }

    #[test]
    fn custom_profile_inherits_from_release_then_overrides_debug() {
        let d = defs(vec![def(
            "relwithdebinfo",
            Some("release"),
            Some(true),
            None,
            None,
        )]);
        let r = resolve_profile(&ProfileSelection::from_name(name("relwithdebinfo")), &d).unwrap();
        assert!(r.debug);
        assert_eq!(r.opt_level, OptLevel::O3);
        assert!(!r.assertions);
        assert_eq!(r.source, ProfileSource::Custom);
        let chain: Vec<&str> = r.inherits_chain.iter().map(|n| n.as_str()).collect();
        assert_eq!(chain, vec!["release", "relwithdebinfo"]);
    }

    #[test]
    fn custom_chain_through_another_custom_resolves_deterministically() {
        let d = defs(vec![
            def(
                "intermediate",
                Some("release"),
                None,
                Some(OptLevel::O2),
                None,
            ),
            def("ci", Some("intermediate"), Some(true), None, Some(true)),
        ]);
        let r = resolve_profile(&ProfileSelection::from_name(name("ci")), &d).unwrap();
        assert!(r.debug);
        assert_eq!(r.opt_level, OptLevel::O2);
        assert!(r.assertions);
        let chain: Vec<&str> = r.inherits_chain.iter().map(|n| n.as_str()).collect();
        assert_eq!(chain, vec!["release", "intermediate", "ci"]);
    }

    fn def_full(
        n: &str,
        inherits: Option<&str>,
        debug: Option<bool>,
        opt: Option<OptLevel>,
        assertions: Option<bool>,
        build: Option<crate::build_flags::ProfileFlags>,
    ) -> (ProfileName, ProfileDefinition) {
        let pn = name(n);
        let def = ProfileDefinition {
            name: pn.clone(),
            inherits: inherits.map(name),
            debug,
            opt_level: opt,
            assertions,
            build,
        };
        (pn, def)
    }

    fn flags_cxx(values: &[&str]) -> crate::build_flags::ProfileFlags {
        crate::build_flags::ProfileFlags {
            cxxflags: values.iter().map(|s| (*s).to_owned()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn cxxflags_append_across_inheritance() {
        let d = defs(vec![
            def_full("release", None, None, None, None, Some(flags_cxx(&["-O3"]))),
            def_full(
                "bench",
                Some("release"),
                None,
                None,
                None,
                Some(flags_cxx(&["-pg"])),
            ),
        ]);
        let r = resolve_profile(&ProfileSelection::from_name(name("bench")), &d).unwrap();
        let build = r
            .build
            .expect("merged build is some when chain contributes");
        assert_eq!(build.cxxflags, vec!["-O3".to_owned(), "-pg".to_owned()]);
    }

    #[test]
    fn parent_build_inherited_when_leaf_has_no_build() {
        let d = defs(vec![
            def_full("release", None, None, None, None, Some(flags_cxx(&["-O3"]))),
            def_full("bench", Some("release"), None, None, None, None),
        ]);
        let r = resolve_profile(&ProfileSelection::from_name(name("bench")), &d).unwrap();
        let build = r.build.expect("parent build survives leaf having no build");
        assert_eq!(build.cxxflags, vec!["-O3".to_owned()]);
    }

    #[test]
    fn include_dirs_dedup_across_inheritance() {
        use std::path::PathBuf;
        let parent_flags = crate::build_flags::ProfileFlags {
            include_dirs: vec![PathBuf::from("include"), PathBuf::from("vendor/include")],
            ..Default::default()
        };
        let leaf_flags = crate::build_flags::ProfileFlags {
            include_dirs: vec![PathBuf::from("include"), PathBuf::from("third_party")],
            ..Default::default()
        };
        let d = defs(vec![
            def_full("release", None, None, None, None, Some(parent_flags)),
            def_full("bench", Some("release"), None, None, None, Some(leaf_flags)),
        ]);
        let r = resolve_profile(&ProfileSelection::from_name(name("bench")), &d).unwrap();
        let build = r.build.expect("merged build is some");
        assert_eq!(
            build.include_dirs,
            vec![
                PathBuf::from("include"),
                PathBuf::from("vendor/include"),
                PathBuf::from("third_party"),
            ],
        );
    }

    #[test]
    fn scalar_fields_replace_across_inheritance() {
        let d = defs(vec![
            def_full(
                "release",
                None,
                Some(false),
                Some(OptLevel::O3),
                Some(false),
                Some(flags_cxx(&["-O3"])),
            ),
            def_full(
                "bench",
                Some("release"),
                Some(true),
                Some(OptLevel::O2),
                Some(true),
                Some(flags_cxx(&["-pg"])),
            ),
        ]);
        let r = resolve_profile(&ProfileSelection::from_name(name("bench")), &d).unwrap();
        assert!(r.debug, "leaf debug=true replaces parent debug=false");
        assert_eq!(r.opt_level, OptLevel::O2, "leaf opt-level replaces parent");
        assert!(r.assertions, "leaf assertions replaces parent");
        let build = r.build.expect("merged build is some");
        assert_eq!(
            build.cxxflags,
            vec!["-O3".to_owned(), "-pg".to_owned()],
            "arrays still append even though scalars replace",
        );
    }

    #[test]
    fn build_is_none_when_no_chain_step_sets_build() {
        let d = defs(vec![
            def_full("ci", Some("release"), Some(true), None, None, None),
            def_full(
                "ci-strict",
                Some("ci"),
                None,
                Some(OptLevel::O2),
                None,
                None,
            ),
        ]);
        let r = resolve_profile(&ProfileSelection::from_name(name("ci-strict")), &d).unwrap();
        assert!(
            r.build.is_none(),
            "build stays None when no chain step contributed flags",
        );
    }

    #[test]
    fn unknown_profile_selection_errors() {
        let err = resolve_profile(
            &ProfileSelection::from_name(name("fastdebug")),
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ProfileResolutionError::UnknownProfile { ref name } if name == "fastdebug"
        ));
    }

    #[test]
    fn custom_without_inherits_is_rejected() {
        let d = defs(vec![def("ci", None, Some(true), None, None)]);
        let err = resolve_profile(&ProfileSelection::from_name(name("ci")), &d).unwrap_err();
        assert!(matches!(
            err,
            ProfileResolutionError::CustomMissingInherits { ref name } if name == "ci"
        ));
    }

    #[test]
    fn builtin_with_inherits_is_rejected() {
        let d = defs(vec![def("dev", Some("release"), None, None, None)]);
        let err = resolve_profile(&ProfileSelection::default_dev(), &d).unwrap_err();
        assert!(matches!(
            err,
            ProfileResolutionError::BuiltinCannotInherit { ref name } if name == "dev"
        ));
    }

    #[test]
    fn unknown_inherited_profile_errors() {
        let d = defs(vec![def("ci", Some("fast"), None, None, None)]);
        let err = resolve_profile(&ProfileSelection::from_name(name("ci")), &d).unwrap_err();
        match err {
            ProfileResolutionError::UnknownInheritedProfile { profile, parent } => {
                assert_eq!(profile, "ci");
                assert_eq!(parent, "fast");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn inheritance_cycle_is_detected() {
        let d = defs(vec![
            def("a", Some("b"), None, None, None),
            def("b", Some("a"), None, None, None),
        ]);
        let err = resolve_profile(&ProfileSelection::from_name(name("a")), &d).unwrap_err();
        match err {
            ProfileResolutionError::InheritanceCycle { chain } => {
                assert!(chain.contains(&"a".to_owned()));
                assert!(chain.contains(&"b".to_owned()));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn invalid_profile_name_is_rejected_at_construction() {
        for bad in [
            ".release",
            "..",
            "",
            "release/x",
            "release\\x",
            "release ",
            "rel?",
        ] {
            assert!(ProfileName::new(bad).is_err(), "{bad:?} should be invalid");
        }
        for good in [
            "dev",
            "release",
            "rel-with-debug-info",
            "ci.fast",
            "0",
            "ci_2",
        ] {
            assert!(ProfileName::new(good).is_ok(), "{good:?} should be valid");
        }
    }

    #[test]
    fn opt_level_parse_round_trips_and_rejects_unknown() {
        for (raw, expected) in [
            ("0", OptLevel::O0),
            ("1", OptLevel::O1),
            ("2", OptLevel::O2),
            ("3", OptLevel::O3),
            ("s", OptLevel::S),
            ("z", OptLevel::Z),
        ] {
            assert_eq!(OptLevel::parse(raw).unwrap(), expected);
            assert_eq!(expected.as_str(), raw);
        }
        let err = OptLevel::parse("fast").unwrap_err();
        assert!(err.contains("invalid opt-level"));
        assert!(err.contains("\"fast\""));
    }

    #[test]
    fn compile_flags_are_deterministic_and_drop_ndebug_when_assertions_on() {
        let r = ResolvedProfile {
            name: name("dev"),
            debug: true,
            opt_level: OptLevel::O0,
            assertions: true,
            source: ProfileSource::Builtin,
            inherits_chain: vec![name("dev")],
            build: None,
        };
        assert_eq!(r.compile_flags(), vec!["-O0", "-g"]);

        let r = ResolvedProfile {
            name: name("release"),
            debug: false,
            opt_level: OptLevel::O3,
            assertions: false,
            source: ProfileSource::Builtin,
            inherits_chain: vec![name("release")],
            build: None,
        };
        assert_eq!(r.compile_flags(), vec!["-O3", "-DNDEBUG"]);
    }

    #[test]
    fn compile_flags_are_language_neutral_profile_flags() {
        let r = ResolvedProfile {
            name: name("dev"),
            debug: true,
            opt_level: OptLevel::O2,
            assertions: false,
            source: ProfileSource::Builtin,
            inherits_chain: vec![name("dev")],
            build: None,
        };
        assert_eq!(r.compile_flags(), vec!["-O2", "-g", "-DNDEBUG"]);
    }

    #[test]
    fn available_profile_names_includes_built_ins_and_custom() {
        let d = defs(vec![def("ci", Some("release"), None, None, None)]);
        let names: Vec<String> = available_profile_names(&d)
            .into_iter()
            .map(|n| n.as_str().to_owned())
            .collect();
        assert_eq!(
            names,
            vec!["ci".to_owned(), "dev".to_owned(), "release".to_owned()]
        );
    }
}
