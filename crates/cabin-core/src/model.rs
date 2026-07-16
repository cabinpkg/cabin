use std::collections::{BTreeMap, HashSet};

use camino::Utf8PathBuf;

use serde::{Deserialize, Serialize};

use crate::build_flags::ProfileSettings;
use crate::compiler_wrapper::CompilerWrapperRequest;
use crate::config::Features;
use crate::error::ValidationError;
use crate::language_standard::LanguageStandardSettings;
use crate::patch::PatchManifestSettings;
use crate::profile::{ProfileDefinition, ProfileName};
use crate::toolchain::ToolchainSettings;

/// Validated package name: either a bare `name` or a scoped
/// `<scope>/<name>`.
///
/// Bare names exist only in local manifests (path dependencies,
/// unpublished packages); registry packages are always scoped, and
/// the publish flow rejects bare names.  There is no alias or short
/// name mechanism: the full string is the one canonical identity
/// everywhere (manifest, index, lockfile, resolver), so `as_str` /
/// `Display` always yield it verbatim.
///
/// A scoped name contains exactly one `/`.  The scope must satisfy
/// [`is_valid_package_scope`]; the package part (and a bare name)
/// must satisfy [`is_path_safe_package_name`].  Because `/` is a
/// path separator, the full string must never be used as a single
/// filesystem path component - path sinks go through
/// [`PackageName::path_components`] instead of `as_str`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PackageName(String);

impl PackageName {
    /// Construct a [`PackageName`] after running validation rules.
    ///
    /// The per-component grammar covers filesystem path components,
    /// sparse-HTTP path segments, package archive filenames, and
    /// Windows-reserved filename characters in a single rule.  See
    /// [`is_path_safe_package_name`] (bare names and the package part)
    /// and [`is_valid_package_scope`] (the scope part) for the full
    /// predicates.
    ///
    /// # Errors
    /// Returns [`ValidationError::EmptyPackageName`] for an empty name,
    /// [`ValidationError::PackageNameContainsWhitespace`] when the name
    /// contains whitespace,
    /// [`ValidationError::PackageNameTooManySlashes`] when the name
    /// contains more than one `/`,
    /// [`ValidationError::InvalidPackageScope`] when the scope part fails
    /// [`is_valid_package_scope`], and
    /// [`ValidationError::UnsafePackageName`] when a bare name or the
    /// package part fails the [`is_path_safe_package_name`] predicate.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ValidationError::EmptyPackageName);
        }
        if value.chars().any(char::is_whitespace) {
            return Err(ValidationError::PackageNameContainsWhitespace(value));
        }
        let Some((scope, base)) = value.split_once('/') else {
            if !is_path_safe_package_name(&value) {
                return Err(ValidationError::UnsafePackageName(value));
            }
            return Ok(Self(value));
        };
        if base.contains('/') {
            return Err(ValidationError::PackageNameTooManySlashes(value));
        }
        if !is_valid_package_scope(scope) {
            return Err(ValidationError::InvalidPackageScope {
                scope: scope.to_owned(),
                name: value,
            });
        }
        if !is_path_safe_package_name(base) {
            return Err(ValidationError::UnsafePackageName(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The scope part of a scoped name, `None` for a bare name.
    pub fn scope(&self) -> Option<&str> {
        self.0.split_once('/').map(|(scope, _)| scope)
    }

    /// The package part: everything after the `/` for a scoped name,
    /// the whole name for a bare one.  This is the value to derive
    /// default target names, artifact/library file names, and
    /// pkg-config module names from - those grammars have no `/`.
    pub fn base_name(&self) -> &str {
        self.0
            .split_once('/')
            .map_or(self.0.as_str(), |(_, base)| base)
    }

    pub fn is_scoped(&self) -> bool {
        self.0.contains('/')
    }

    /// Filesystem path components for this package's directory
    /// subtree: `[scope, name]` for a scoped name, `[name]` for a
    /// bare one.  Every name-to-path sink must fold these into the
    /// base directory instead of joining `as_str`, which would embed
    /// a `/` into what callers treat as one component.  Each yielded
    /// component satisfies [`is_path_safe_package_name`] by
    /// construction: the package part is validated with exactly that
    /// predicate and the scope grammar is a strict subset of it.
    pub fn path_components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// Filename fragment identifying this package inside artifact and
    /// staging filenames (`<stem>-<version>.tar.gz` /
    /// `<stem>-<version>.json`): `<scope>-<name>` for a scoped name,
    /// the bare name otherwise.  The scope is embedded on purpose so a
    /// downloaded tarball stays self-identifying outside the registry
    /// directory tree.
    pub fn artifact_stem(&self) -> String {
        self.0.replace('/', "-")
    }
}

/// Shared single-component name validity predicate.
///
/// A name passes when it is safe to use **simultaneously** as
/// (a) a single filesystem path component on every supported
/// host OS, (b) a single sparse-HTTP URL path segment, and
/// (c) a fragment of a package archive filename.  The grammar is
/// deliberately strict so the same `PackageName` value can flow
/// from manifest parsing through the workspace loader, the
/// resolver, the lockfile, the artifact cache, and the registry
/// (file or sparse HTTP) without any per-stage re-encoding.
///
/// This predicate is single-component on purpose: it guards bare
/// package names, the package part of a scoped name, every
/// [`PackageName::path_components`] element, [`TargetName`], and the
/// URL boundaries in `cabin-index-http` / `cabin-registry-api`.  A
/// scoped `<scope>/<name>` string fails it (the `/` is outside the
/// alphabet), which is what keeps scoped names out of the remote
/// registry protocol until the scoped routes land.
///
/// A name is valid iff:
///
/// - it is non-empty;
/// - it consists only of ASCII letters (`A-Z`, `a-z`), ASCII
///   digits (`0-9`), `_`, `-`, and `.`;
/// - it is not equal to `.` or `..`;
/// - it does not start with `.` or `-`.
///
/// Consequences worth calling out:
///
/// - `foo..bar` is **accepted**: it's not a parent reference
///   because the name is not equal to `..` and does not start
///   with a dot.  Path resolvers do not interpret the embedded
///   `..` substring as a navigation.  This is intentional so that
///   common library names like `boost..hana` (hypothetical) stay
///   legal under the registry grammar.
/// - A leading `-` is rejected so the name cannot be mistaken
///   for a flag when it reaches an argv-driven tool (e.g.,
///   `pkg-config`, the linker), or for the start of a CLI
///   short-option block.  An embedded `-` (like `foo-bar`) is
///   still fine.
/// - URL-reserved characters (`?`, `#`, `%`, `:`), Windows-
///   reserved filename characters (`< > : " | ? *`), and path
///   separators (`/`, `\`) are all outside the allowed alphabet,
///   so they are rejected without needing a separate enumeration.
/// - Control characters and non-ASCII characters are also outside
///   the alphabet, so they fall under the same rule.
///
/// The shared helper keeps `cabin-package`, `cabin-registry-file`,
/// and `cabin-index-http` from drifting on this rule.
pub fn is_path_safe_package_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    if name == "." || name == ".." {
        return false;
    }
    if name.starts_with('.') || name.starts_with('-') {
        return false;
    }
    name.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// Scope validity predicate for scoped package names.
///
/// Mirrors the registry's scope grammar (`registry/src/routes.rs`,
/// `is_valid_scope`): `[a-z0-9]([a-z0-9-]*[a-z0-9])?`, at most 39
/// bytes.  The grammar is GitHub-login-compatible on purpose: a scope
/// is granted by proving control of the same-named GitHub account
/// (logins are lowercased at claim time), so every claimable login
/// must fit.  Every scope passing this predicate also passes
/// [`is_path_safe_package_name`], so a scope directory component
/// never needs a second guard.
pub fn is_valid_package_scope(scope: &str) -> bool {
    !scope.is_empty()
        && scope.len() <= 39
        && scope
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !scope.starts_with('-')
        && !scope.ends_with('-')
}

/// Run the shared three-step validation behind a path-safe name
/// newtype: reject an empty value, reject any whitespace, then enforce
/// [`is_path_safe_package_name`].  [`TargetName`] routes through this;
/// [`PackageName`] validates its components directly (its scoped form
/// is not a single path-safe component) but shares the same predicate
/// per component, keeping the two from drifting on the rule.
fn validate_path_safe_name(
    value: String,
    empty: ValidationError,
    whitespace: impl FnOnce(String) -> ValidationError,
    unsafe_name: impl FnOnce(String) -> ValidationError,
) -> Result<String, ValidationError> {
    if value.is_empty() {
        return Err(empty);
    }
    if value.chars().any(char::is_whitespace) {
        return Err(whitespace(value));
    }
    if !is_path_safe_package_name(&value) {
        return Err(unsafe_name(value));
    }
    Ok(value)
}

impl AsRef<str> for PackageName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PackageName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for PackageName {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        PackageName::new(value)
    }
}

impl From<PackageName> for String {
    fn from(value: PackageName) -> Self {
        value.0
    }
}

/// Validated target name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TargetName(String);

impl TargetName {
    /// Construct a [`TargetName`] after running validation.
    ///
    /// Target names are joined into filesystem paths by the build
    /// planner (object directories, executable paths, Cargo target
    /// directories), so they share the path-component grammar with
    /// [`PackageName`]: a name like `[target."../escape"]` would
    /// otherwise let a malicious manifest write artifacts outside
    /// the selected `--build-dir`.  The grammar is enforced through
    /// [`is_path_safe_package_name`], which already covers path
    /// separators, `..` / `.`, leading `.` or `-`, control characters,
    /// non-ASCII bytes, and Windows-reserved filename characters in a
    /// single rule.
    ///
    /// # Errors
    /// Returns [`ValidationError::EmptyTargetName`] for an empty name,
    /// [`ValidationError::TargetNameContainsWhitespace`] when the name contains
    /// whitespace, and [`ValidationError::UnsafeTargetName`] when it fails the
    /// [`is_path_safe_package_name`] predicate.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        validate_path_safe_name(
            value.into(),
            ValidationError::EmptyTargetName,
            ValidationError::TargetNameContainsWhitespace,
            ValidationError::UnsafeTargetName,
        )
        .map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for TargetName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TargetName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for TargetName {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        TargetName::new(value)
    }
}

impl From<TargetName> for String {
    fn from(value: TargetName) -> Self {
        value.0
    }
}

/// What kind of artifact a target produces.
///
/// Target kinds describe artifact role only.  Source-language
/// classification is per-file, based on source extension: `.c`
/// compiles as C, `.cc` / `.cpp` / `.cxx` / `.c++` / `.C` compile
/// as C++.  A single target may freely mix C/C++ sources; the
/// planner selects the compiler per source and selects the link
/// driver from the direct and transitive source-language closure
/// (C++ if any object is C++, otherwise C).
///
/// The string representations are stable: they are written by the manifest
/// parser, surfaced by `cabin metadata`, and consumed by the build graph
/// planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TargetKind {
    /// Static-archive library (`lib<name>.a`).
    #[serde(rename = "library")]
    Library,
    /// A header-only library.  Has no translation units of its own;
    /// the planner emits no compile or archive actions, and consumers
    /// pick up its `include_dirs` through the dependency graph.
    #[serde(rename = "header-only")]
    HeaderOnly,
    /// A linked executable.  Built by default by `cabin build`.
    #[serde(rename = "executable")]
    Executable,
    /// A test executable.  Built and run by `cabin test`.  Excluded
    /// from the default `cabin build` selection.
    #[serde(rename = "test")]
    Test,
    /// An example executable.  Excluded from the default
    /// `cabin build` selection.  The only way an example
    /// reaches the build graph is as a transitive dep of another
    /// selected target.
    #[serde(rename = "example")]
    Example,
}

impl TargetKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::HeaderOnly => "header-only",
            Self::Executable => "executable",
            Self::Test => "test",
            Self::Example => "example",
        }
    }

    /// All kinds, in declaration order.  Useful for error messages that list
    /// the supported types.
    pub const fn all() -> &'static [TargetKind] {
        &[
            Self::Library,
            Self::HeaderOnly,
            Self::Executable,
            Self::Test,
            Self::Example,
        ]
    }

    /// Whether this kind produces an executable (linked binary).
    /// Library kinds return `false`.
    pub const fn produces_executable(self) -> bool {
        matches!(self, Self::Executable | Self::Test | Self::Example)
    }

    /// Whether this kind produces a static-archive library (`lib<name>.a`).
    pub const fn produces_archive(self) -> bool {
        matches!(self, Self::Library)
    }

    /// Whether this kind is a header-only library (no compile/
    /// archive actions; consumers pick up `include_dirs`).
    pub const fn is_header_only(self) -> bool {
        matches!(self, Self::HeaderOnly)
    }

    /// Whether this kind is "library-like" - a static-archive
    /// library or a header-only library.  These are the kinds that
    /// carry a public interface (include dirs, interface standards)
    /// to their consumers, as opposed to executable-like kinds.
    pub const fn is_library_like(self) -> bool {
        self.produces_archive() || self.is_header_only()
    }

    /// Whether ordinary `cabin build` selects this kind by default.
    /// Dev-only kinds (`test` / `example`) are excluded
    /// from the default set: tests are built by `cabin test`,
    /// and examples only reach the build graph as a
    /// transitive dep of another selected target.
    ///
    /// Header-only libraries are included so the dependency
    /// closure walk reaches them; the planner emits no compile or
    /// archive actions for them, so saying "yes, this is part of
    /// the default selection" is a no-op on Ninja's side.
    pub const fn is_default_buildable(self) -> bool {
        matches!(self, Self::Library | Self::HeaderOnly | Self::Executable)
    }

    /// Whether this kind is a *development-only* target - a target
    /// that exists to support workspace development but is not part
    /// of the package's public surface.  Production callers use this
    /// to decide whether dev-dependencies should be activated and
    /// whether the target may be run by `cabin test`.
    pub const fn is_dev_only(self) -> bool {
        matches!(self, Self::Test | Self::Example)
    }

    /// Whether `cabin test` runs this kind after building it.  Today
    /// only `test` runs; `example` is build-only.
    pub const fn is_test(self) -> bool {
        matches!(self, Self::Test)
    }
}

impl std::fmt::Display for TargetKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One declared entry of a target's `deps` array.
///
/// The manifest accepts two spellings: a bare reference string
/// (`"foo"`, `"pkg:target"`), which declares a *private* edge, and
/// the table form (`{ name = "foo", public = true }`), which
/// additionally sets the per-edge visibility.  Both forms keep the
/// reference exactly as written; alias resolution (`foo` ->
/// `foo:foo`) happens in `cabin-build` against a concrete package
/// graph, and the resolved edge carries this declaration's
/// visibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetDep {
    /// Raw target reference exactly as written in the manifest -
    /// a bare name or a qualified `package:target`.  See
    /// [`Target::deps`] for why this is a `String`, not a
    /// [`TargetName`].
    pub reference: String,
    /// Whether this edge re-exports the dependency's public
    /// headers to the target's own consumers.  Declarative only
    /// today: recorded on the resolved dependency graph, consumed
    /// by nothing yet.
    pub public: bool,
}

impl TargetDep {
    /// A private edge to `reference` - the meaning of the string
    /// shorthand in manifests.
    pub fn private(reference: impl Into<String>) -> Self {
        Self {
            reference: reference.into(),
            public: false,
        }
    }
}

impl From<&str> for TargetDep {
    fn from(reference: &str) -> Self {
        Self::private(reference)
    }
}

// The serialized shape mirrors the manifest surface: a private
// edge stays a bare string (so existing manifests and the
// `cabin metadata` JSON view keep their previous shape), and a
// public edge serializes as the `{ name, public }` table.
impl Serialize for TargetDep {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if self.public {
            use serde::ser::SerializeMap;
            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("name", &self.reference)?;
            map.serialize_entry("public", &self.public)?;
            map.end()
        } else {
            serializer.serialize_str(&self.reference)
        }
    }
}

// Hand-rolled Deserialize so the table form reports its own typed
// errors (including the `deny_unknown_fields` "unknown field
// `<name>`" message); an untagged derive would collapse every
// failure to "data did not match any variant".
impl<'de> Deserialize<'de> for TargetDep {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct TargetDepTable {
            name: String,
            #[serde(default)]
            public: bool,
        }

        struct TargetDepVisitor;

        impl<'de> serde::de::Visitor<'de> for TargetDepVisitor {
            type Value = TargetDep;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a target reference string or a `{ name, public }` table")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(TargetDep::private(v))
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(TargetDep::private(v))
            }

            fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
            where
                M: serde::de::MapAccess<'de>,
            {
                let table =
                    TargetDepTable::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(TargetDep {
                    reference: table.name,
                    public: table.public,
                })
            }
        }

        deserializer.deserialize_any(TargetDepVisitor)
    }
}

/// A buildable unit within a package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub name: TargetName,
    pub kind: TargetKind,
    #[serde(default)]
    pub sources: Vec<Utf8PathBuf>,
    #[serde(default)]
    pub include_dirs: Vec<Utf8PathBuf>,
    #[serde(default)]
    pub defines: Vec<String>,
    /// Explicit references to the linked targets.  A bare name
    /// resolves to a same-package target first, then as the
    /// same-name shorthand on a dependency package (`foo` means
    /// `foo:foo`, matching the dependency's library / header-only
    /// targets only); every other cross-package reference is the
    /// qualified `package:target` form.  A package dependency only
    /// makes the package available - it never exports a *default*
    /// target, so a bare name that matches neither a local target
    /// nor a same-named linkable dependency target is a hard error.
    /// Resolution against a concrete package graph lives in
    /// `cabin-build`, not here.
    ///
    /// References are stored as raw strings, not [`TargetName`], because
    /// the qualified `package:target` form contains a `:` that the
    /// path-safe target-name grammar rejects.  Validation happens at
    /// resolution time against the already-validated package / target
    /// graph; dep strings never flow directly into a filesystem path.
    /// Each entry also carries the declared per-edge visibility - see
    /// [`TargetDep`].
    #[serde(default)]
    pub deps: Vec<TargetDep>,
    /// Package features that must all be enabled for this target
    /// to be built or used.  Entries name features declared in the
    /// owning package's `[features]` table;
    /// [`Package::with_config`] rejects unknown names.  Default
    /// target enumeration skips a target whose required features
    /// are not enabled; naming one explicitly (a `deps` entry, a
    /// manifest-target selector, `cabin test --test`) is a hard
    /// error instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_features: Vec<String>,
    /// Per-target `c-standard` / `cxx-standard` /
    /// `interface-c-standard` / `interface-cxx-standard` overrides.
    /// Interface fields are only meaningful on `library` /
    /// `header-only` kinds; the manifest parser rejects them on
    /// executable-like targets.
    #[serde(default, skip_serializing_if = "LanguageStandardSettings::is_empty")]
    pub language: LanguageStandardSettings,
}

impl Target {
    /// The subset of this target's `required-features` that is not
    /// in `enabled`, in declaration order.  Empty when the target
    /// is buildable under the given feature set.
    pub fn missing_required_features(
        &self,
        enabled: &std::collections::BTreeSet<String>,
    ) -> Vec<String> {
        self.required_features
            .iter()
            .filter(|f| !enabled.contains(*f))
            .cloned()
            .collect()
    }
}

fn default_true() -> bool {
    true
}

/// A package-level Cabin dependency declared in
/// `[dependencies]` or `[dev-dependencies]`.
///
/// System dependencies (`system = true` entries) are *not*
/// represented here - they live in [`SystemDependency`] because
/// they have a different schema and never enter Cabin
/// resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    /// The dependency alias used in the manifest.  The alias must
    /// equal the depended-on package's `[package].name`.
    pub name: PackageName,
    pub source: DependencySource,
    /// Which manifest section the dependency was declared in.
    /// Defaults to [`DependencyKind::Normal`] so manifests that
    /// only use `[dependencies]` keep their previous serialized
    /// shape.
    #[serde(default, skip_serializing_if = "DependencyKind::is_normal")]
    pub kind: DependencyKind,
    /// Whether the dependency is optional.  Optional dependencies
    /// only enter ordinary resolution / fetch / build when a
    /// feature enables them via `dep:<name>` or
    /// `<name>/<feature>`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub optional: bool,
    /// Features requested on the dependency package by this edge.
    /// Stored as the raw manifest strings; the feature resolver
    /// validates them against the depended-on package's
    /// `[features]` table.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Whether this edge requests the dependency package's
    /// `default` feature.  Defaults to `true`. `default-features =
    /// false` only narrows *this* edge - if another edge requests
    /// defaults for the same package, the unified result still
    /// includes them.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub default_features: bool,
    /// Optional target condition.  `Some` when the dependency was
    /// declared inside a `[target.'cfg(...)'.<kind>]` table;
    /// `None` for unconditional declarations.  Conditional
    /// dependencies whose condition does not match the
    /// evaluation [`crate::TargetPlatform`] are filtered out by
    /// `cabin-workspace` / `cabin-feature` / `cabin-build`
    /// before reaching the resolver or the build planner, but they
    /// stay on `Package::dependencies` for metadata round-trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<crate::Condition>,
    /// `ignore-interface-standard = true`: exempt exactly this
    /// dependency edge from the standard-compatibility check.  The
    /// check still reports the edge as unchecked; the field is
    /// deliberately per-edge only (no package-wide or global
    /// variant).
    #[serde(default, skip_serializing_if = "is_false")]
    pub ignore_interface_standard: bool,
}

fn is_false<T>(value: &T) -> bool
where
    T: PartialEq + Default,
{
    *value == T::default()
}

fn is_true<T>(value: &T) -> bool
where
    T: PartialEq + Default + std::ops::Not<Output = T>,
{
    *value == !T::default()
}

impl Dependency {
    /// Whether this declaration is active for the given
    /// [`crate::TargetPlatform`].  Unconditional declarations
    /// are always active; conditional declarations are active
    /// iff their condition evaluates to `true`.
    pub fn matches_platform(&self, platform: &crate::TargetPlatform) -> bool {
        match &self.condition {
            None => true,
            // Dependency gating is platform-only: a feature- or
            // compiler-referencing `cfg` is rejected on dependency
            // tables at manifest load, so the platform-only context is
            // correct-by-construction here (any such leaf would
            // already have been refused).
            Some(cond) => cond.evaluate(&crate::ConditionContext::platform_only(platform)),
        }
    }
}

/// Which kind of dependency is declared.
///
/// Cabin distinguishes package dependency kinds (`Normal`, `Dev`)
/// - both of which are sourced from other Cabin packages - from
///   system dependencies, which are externally provided and never
///   enter Cabin resolution.  System declarations live alongside the
///   package kinds as a separate `system = true` flag on a regular
///   `[dependencies]` / `[dev-dependencies]` entry and are modeled
///   by [`SystemDependency`].
///
/// The wire format mirrors the manifest section names: `"normal"`,
/// `"dev"`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum DependencyKind {
    /// `[dependencies]`.  Linked into ordinary builds.
    #[default]
    Normal,
    /// `[dev-dependencies]`.  Declaration-only for ordinary
    /// commands; activated for the selected primary packages by
    /// `cabin test`.
    Dev,
}

impl DependencyKind {
    /// Stable lowercase label, matching the manifest section name.
    pub const fn as_str(self) -> &'static str {
        match self {
            DependencyKind::Normal => "normal",
            DependencyKind::Dev => "dev",
        }
    }

    /// All kinds in canonical order. `cabin metadata` and the
    /// canonical package metadata both iterate kinds in this order
    /// so output stays deterministic.
    pub const fn all() -> &'static [DependencyKind] {
        &[DependencyKind::Normal, DependencyKind::Dev]
    }

    /// Whether this kind is included in the resolver / fetch /
    /// build pipeline by default.  Dev dependencies are excluded.
    pub const fn is_resolved_by_default(self) -> bool {
        matches!(self, DependencyKind::Normal)
    }

    /// Helper for `#[serde(skip_serializing_if = ...)]` so
    /// existing on-disk metadata that omits the `kind` field
    /// stays byte-identical for `[dependencies]`-only manifests.
    pub fn is_normal(&self) -> bool {
        matches!(self, DependencyKind::Normal)
    }

    /// The manifest section name (`[dependencies]`,
    /// `[dev-dependencies]`) corresponding to this kind.
    /// Used in error messages.
    pub const fn manifest_section(self) -> &'static str {
        match self {
            DependencyKind::Normal => "[dependencies]",
            DependencyKind::Dev => "[dev-dependencies]",
        }
    }
}

impl std::fmt::Display for DependencyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Raw requirement strings from the workspace root's
/// `[workspace.<kind>-dependencies]` tables, keyed by kind then
/// dependency name.  Carried for publish-time archive
/// normalization, which writes the author's original spelling -
/// the parsed [`semver::VersionReq`] would respell it (`"0.2"`
/// renders as `"^0.2"`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceDepRequirements {
    entries: BTreeMap<DependencyKind, BTreeMap<String, String>>,
}

impl WorkspaceDepRequirements {
    /// Record the raw requirement string for `(kind, name)`.
    pub fn insert(&mut self, kind: DependencyKind, name: String, requirement: String) {
        self.entries
            .entry(kind)
            .or_default()
            .insert(name, requirement);
    }

    /// The raw requirement string for `(kind, name)`.  The lookup is
    /// strictly kind-specific, mirroring the loader's rule.
    #[must_use]
    pub fn requirement(&self, kind: DependencyKind, name: &str) -> Option<&str> {
        self.entries.get(&kind)?.get(name).map(String::as_str)
    }
}

/// A system dependency declared with `system = true` on a
/// `[dependencies]` / `[dev-dependencies]` entry.
///
/// System dependencies are externally provided (system libraries,
/// SDKs, installed tools).  Cabin never resolves, fetches,
/// downloads, or installs them - `cabin-system-deps` probes them
/// via `pkg-config` at build time, and the resulting cflags /
/// ldflags are merged into the per-package build flags before
/// the planner runs.  The typed value round-trips through
/// `cabin metadata`, the canonical package metadata, and the
/// index metadata so external tooling sees the system-dep set
/// alongside the Cabin-package deps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemDependency {
    /// The dependency name as written in the manifest.
    pub name: PackageName,
    /// Version requirement string for `pkg-config`.  Cabin does
    /// not interpret it as a `SemVer` constraint; the system-deps
    /// layer translates the supported comparators for
    /// `pkg-config` and reports unsupported forms as errors.
    pub version: String,
    /// Which dependency table the entry was declared in
    /// (`[dependencies]` or `[dev-dependencies]`).  Drives per-kind
    /// activation: a dev-kind system dep is only probed when
    /// `cabin test` is running, mirroring the Cabin-package
    /// dev-dep rule.
    #[serde(default)]
    pub kind: DependencyKind,
    /// Optional target condition.  `Some` when the system
    /// dependency was declared inside a
    /// `[target.'cfg(...)'.<kind>-dependencies]` table.  The
    /// condition is preserved so package / index metadata stays
    /// portable across platforms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<crate::Condition>,
}

/// Where a foundation-port dependency's recipe comes from.
///
/// Constructed by the manifest parser from one of the two
/// recipe-locator fields:
///
/// - `{ port = true, version = "..." }` → `Builtin { name, version_req }`.  The recipe
///   is resolved from `cabin_port::builtin::BUILTIN` by the discovery layer using the
///   consumer-supplied `version_req`.
/// - `{ port-path = "..." }` → `Path(PathBuf)`.  The recipe lives
///   on disk at the given path, interpreted relative to the
///   manifest directory that declared it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortDepSource {
    /// Bundled curated recipe. `version_req` is the consumer-supplied requirement,
    /// resolved against `cabin_port::builtin::BUILTIN` by the discovery layer.
    Builtin {
        name: PackageName,
        version_req: semver::VersionReq,
    },
    Path(Utf8PathBuf),
}

/// Where a dependency is sourced from.
///
/// Covers [`DependencySource::Path`] for local path dependencies,
/// [`DependencySource::Version`] for registry-resolved versioned
/// dependencies, [`DependencySource::Port`] for foundation-port
/// dependencies (curated recipes under `crates/cabin-port/ports/`), and
/// [`DependencySource::Workspace`] for the `{ workspace = true }`
/// opt-in into the workspace's shared dependency table.  The
/// `Workspace` variant is an unresolved marker -
/// `cabin-workspace::load_workspace` rewrites it into the
/// matching `Path` / `Version` / `Port` source from
/// `[workspace.dependencies]` before any consumer sees a
/// [`crate::Package`] returned from the workspace loader.  If a
/// `Workspace` source ever reaches a planner or resolver it
/// indicates the package was loaded outside of
/// `cabin-workspace`, which is a workspace invariant violation
/// worth surfacing as a clear error in the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DependencySource {
    /// Local path dependency.  The path is interpreted relative to the
    /// manifest directory of the package that declared the dependency.
    #[serde(rename = "path")]
    Path(Utf8PathBuf),
    /// Versioned registry dependency.  The requirement is matched against
    /// candidate versions during dependency resolution.
    #[serde(rename = "version")]
    Version(semver::VersionReq),
    /// Foundation-port dependency.  The recipe source is one of two
    /// shapes (see [`PortDepSource`]): a relative path to a port
    /// directory on disk (`Path`), or a bundled curated recipe keyed
    /// by the dependency name (`Builtin`).  The CLI orchestration
    /// layer prepares the port (download → verify → safe-extract
    /// with `strip_prefix` → overlay copy) before the workspace
    /// loader resolves the dependency to the prepared directory.
    #[serde(rename = "port")]
    Port(PortDepSource),
    /// `dep = { workspace = true }`.  An unresolved opt-in into
    /// the workspace's `[workspace.dependencies]` table, resolved
    /// by `load_workspace` before the `PackageGraph` is produced
    /// (see the enum docs).
    #[serde(rename = "workspace")]
    Workspace,
}

/// Top-level validated package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub name: PackageName,
    pub version: semver::Version,
    pub targets: Vec<Target>,
    /// Cabin package dependencies declared under
    /// `[dependencies]` or `[dev-dependencies]`.  Each entry
    /// carries its [`DependencyKind`]; iteration order is sorted
    /// by `(kind, name)` so callers see deterministic output.
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    /// `system = true` declarations.  Empty if not
    /// declared.  System dependencies never enter the resolver,
    /// the lockfile, or the artifact cache; they are
    /// declaration-only and round-trip through metadata.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub system_dependencies: Vec<SystemDependency>,
    /// `[features]` declarations.  Empty if the manifest has
    /// no `[features]` table.
    #[serde(default, skip_serializing_if = "is_empty_features")]
    pub features: Features,
    /// `[profile.<name>]` declarations from the manifest, keyed
    /// by profile name.  Built-in profiles do not need to appear
    /// here; entries that match a built-in name override those
    /// defaults.  Empty for manifests with no profile tables, so
    /// older manifests stay byte-identical through round-tripping.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<ProfileName, ProfileDefinition>,
    /// `[toolchain]` plus any `[target.'cfg(...)'.toolchain]`
    /// overrides declared on this manifest.  Only the workspace
    /// root manifest's settings are honored; member manifests
    /// that declare a `[toolchain]` table are rejected by the
    /// workspace loader.
    #[serde(default, skip_serializing_if = "ToolchainSettings::is_empty")]
    pub toolchain: ToolchainSettings,
    /// `[profile]` plus any general or named
    /// `[target.'cfg(...)'.profile...]`
    /// declarations for this package.  Per-package by design - each
    /// package may add its own defines / include dirs / extra args.
    ///
    /// The raw compiler / linker flag arrays (`cflags` / `cxxflags`
    /// / `ldflags`) are honored only for local packages - the
    /// workspace root, its members, and `path` dependencies.  They
    /// are dropped for registry dependencies during flag resolution
    /// (see `resolve_build_flags`), because they are unvalidated and
    /// could otherwise smuggle build-time code-execution options
    /// such as `-fplugin=`. `defines` and `include_dirs` are
    /// validated and kept for every package.
    #[serde(default, skip_serializing_if = "ProfileSettings::is_empty")]
    pub build: ProfileSettings,
    /// `[package]`-level `c-standard` / `cxx-standard` /
    /// `interface-c-standard` / `interface-cxx-standard`
    /// declarations.  Honored for every package kind - unlike the
    /// raw flag escape hatches, a typed standard is a bounded
    /// correctness requirement, so registry packages keep theirs.
    #[serde(default, skip_serializing_if = "LanguageStandardSettings::is_empty")]
    pub language: LanguageStandardSettings,
    /// Workspace-root `[build] compiler-wrapper` declaration.
    /// Member manifests cannot declare build execution settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compiler_wrapper: Option<CompilerWrapperRequest>,
    /// `[patch]` declarations on the workspace-root manifest.
    /// Member manifests cannot declare patches - the workspace
    /// loader rejects them - and `cabin package` refuses to
    /// archive a manifest with a non-empty `[patch]` table.
    /// Patches are *local development policy*, not package
    /// metadata.
    #[serde(default, skip_serializing_if = "PatchManifestSettings::is_empty")]
    pub patches: PatchManifestSettings,
}

fn is_empty_features(f: &Features) -> bool {
    f.default.is_empty() && f.features.is_empty()
}

impl Package {
    /// Build a validated [`Package`].
    ///
    /// Validation:
    /// - target names are unique
    /// - dependency names are unique within each kind (the same
    ///   name may legitimately appear under multiple kinds)
    /// - system dependency names are unique within the
    ///   collected `system = true` declarations
    /// - feature declarations are well-formed
    ///
    /// Target-dep references (same-package, cross-package, or
    /// qualified `package:target`) are resolved by `cabin-build`
    /// against the full package graph, not here.
    ///
    /// # Errors
    /// Returns a [`ValidationError`] when validation fails: see
    /// [`Package::with_config`], which performs the checks
    /// ([`ValidationError::DuplicateTargetName`],
    /// [`ValidationError::DuplicateDependency`], and feature-table errors).
    pub fn new(
        name: PackageName,
        version: semver::Version,
        targets: Vec<Target>,
        dependencies: Vec<Dependency>,
    ) -> Result<Self, ValidationError> {
        Self::with_config(PackageConfigInput {
            name,
            version,
            targets,
            dependencies,
            system_dependencies: Vec::new(),
            features: Features::default(),
        })
    }

    /// Build a validated [`Package`] with `[features]` declarations
    /// attached. `cabin-manifest` calls this after parsing the
    /// `[features]` table.
    ///
    /// # Errors
    /// Returns [`ValidationError::DuplicateTargetName`] for repeated target
    /// names, [`ValidationError::DuplicateDependency`] for a duplicate
    /// dependency within a kind, [`ValidationError::DuplicateSystemDependency`]
    /// for a duplicate system dependency, and propagates any
    /// [`ValidationError`] from validating the `[features]` table.
    pub fn with_config(input: PackageConfigInput) -> Result<Self, ValidationError> {
        let PackageConfigInput {
            name,
            version,
            targets,
            dependencies,
            system_dependencies,
            features,
        } = input;
        Self::validate_targets(&targets)?;
        Self::validate_dependencies(&dependencies)?;
        Self::validate_system_dependencies(&system_dependencies)?;
        features.validate()?;
        Self::validate_required_features(&targets, &features)?;
        Ok(Self {
            name,
            version,
            targets,
            dependencies,
            system_dependencies,
            features,
            profiles: BTreeMap::new(),
            toolchain: ToolchainSettings::default(),
            build: ProfileSettings::default(),
            language: LanguageStandardSettings::default(),
            compiler_wrapper: None,
            patches: PatchManifestSettings::default(),
        })
    }

    /// Attach manifest-declared `[profile.*]` definitions to this
    /// package.  Returns the same package so callers can chain it
    /// after [`Package::with_config`] without exploding the
    /// constructor signature for every new optional table.
    #[must_use]
    pub fn with_profiles(mut self, profiles: BTreeMap<ProfileName, ProfileDefinition>) -> Self {
        self.profiles = profiles;
        self
    }
}

/// Bundled inputs for [`Package::with_config`].
///
/// `cabin-manifest` builds this from the parsed `cabin.toml` and hands
/// it to [`Package::with_config`].  Threading inputs through one struct
/// keeps `with_config` callable across the workspace without a fixed
/// positional argument order.
#[derive(Debug, Clone)]
pub struct PackageConfigInput {
    /// `package.name` from the manifest.
    pub name: PackageName,
    /// `package.version` from the manifest.
    pub version: semver::Version,
    /// Parsed `[target.*]` definitions.
    pub targets: Vec<Target>,
    /// Parsed `[dependencies]` / `[dev-dependencies]`.
    pub dependencies: Vec<Dependency>,
    /// Parsed `[system-dependencies]`.
    pub system_dependencies: Vec<SystemDependency>,
    /// Parsed `[features]`.
    pub features: Features,
}

impl Package {
    /// Attach the manifest-declared `[toolchain]` /
    /// `[target.'cfg(...)'.toolchain]` block.  Workspace loaders
    /// reject these declarations on member / path-dep manifests
    /// so only the entry-point manifest's value reaches downstream
    /// crates.
    #[must_use]
    pub fn with_toolchain(mut self, toolchain: ToolchainSettings) -> Self {
        self.toolchain = toolchain;
        self
    }

    /// Attach the manifest-declared `[profile]` and general or named
    /// `[target.'cfg(...)'.profile...]` blocks.  Per-package by design.
    #[must_use]
    pub fn with_build(mut self, build: ProfileSettings) -> Self {
        self.build = build;
        self
    }

    /// Attach the manifest-declared `[package]`-level language
    /// standard fields.  Per-package by design: registry packages'
    /// standard declarations are honored, unlike their raw flag
    /// escape hatches.
    #[must_use]
    pub fn with_language(mut self, language: LanguageStandardSettings) -> Self {
        self.language = language;
        self
    }

    /// Attach the manifest-declared `[build] compiler-wrapper`.
    /// Workspace loaders reject this declaration on member / path-dep
    /// manifests.
    #[must_use]
    pub fn with_compiler_wrapper(mut self, request: Option<CompilerWrapperRequest>) -> Self {
        self.compiler_wrapper = request;
        self
    }

    /// Attach the manifest-declared `[patch]` block.  Workspace
    /// loaders reject these declarations on member / path-dep
    /// manifests so only the entry-point manifest's value
    /// reaches downstream crates.
    #[must_use]
    pub fn with_patches(mut self, patches: PatchManifestSettings) -> Self {
        self.patches = patches;
        self
    }

    fn validate_targets(targets: &[Target]) -> Result<(), ValidationError> {
        let mut seen: HashSet<&str> = HashSet::with_capacity(targets.len());
        for target in targets {
            if !seen.insert(target.name.as_str()) {
                return Err(ValidationError::DuplicateTargetName(
                    target.name.as_str().to_owned(),
                ));
            }
        }
        Ok(())
    }

    /// Every `required-features` entry must satisfy the feature
    /// identifier grammar and name a feature declared in this
    /// package's `[features]` table.  The reserved `default` key is
    /// not a declared feature, so requiring it is rejected too.
    fn validate_required_features(
        targets: &[Target],
        features: &Features,
    ) -> Result<(), ValidationError> {
        for target in targets {
            for name in &target.required_features {
                crate::config::validate_feature_identifier(name)?;
                if !features.features.contains_key(name) {
                    return Err(ValidationError::UnknownRequiredFeature {
                        target: target.name.as_str().to_owned(),
                        feature: name.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    fn validate_dependencies(deps: &[Dependency]) -> Result<(), ValidationError> {
        let mut seen: HashSet<(DependencyKind, &str)> = HashSet::with_capacity(deps.len());
        for dep in deps {
            if !seen.insert((dep.kind, dep.name.as_str())) {
                return Err(ValidationError::DuplicateDependency {
                    name: dep.name.as_str().to_owned(),
                    kind: dep.kind,
                });
            }
        }
        Ok(())
    }

    fn validate_system_dependencies(deps: &[SystemDependency]) -> Result<(), ValidationError> {
        let mut seen: HashSet<&str> = HashSet::with_capacity(deps.len());
        for dep in deps {
            if !seen.insert(dep.name.as_str()) {
                return Err(ValidationError::DuplicateSystemDependency(
                    dep.name.as_str().to_owned(),
                ));
            }
        }
        Ok(())
    }

    /// Iterator over dependencies of a specific kind.  Order is
    /// the same as `dependencies` (sorted by `(kind, name)`).
    pub fn dependencies_of_kind(&self, kind: DependencyKind) -> impl Iterator<Item = &Dependency> {
        self.dependencies.iter().filter(move |d| d.kind == kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version() -> semver::Version {
        semver::Version::parse("0.1.0").unwrap()
    }

    fn pkg(name: &str) -> PackageName {
        PackageName::new(name).unwrap()
    }

    fn tgt(name: &str) -> TargetName {
        TargetName::new(name).unwrap()
    }

    fn target(name: &str, kind: TargetKind, deps: &[&str]) -> Target {
        Target {
            name: tgt(name),
            kind,
            sources: Vec::new(),
            include_dirs: Vec::new(),
            defines: Vec::new(),
            deps: deps.iter().map(|d| TargetDep::from(*d)).collect(),
            required_features: Vec::new(),
            language: LanguageStandardSettings::default(),
        }
    }

    #[test]
    fn package_name_rejects_empty() {
        assert_eq!(
            PackageName::new("").unwrap_err(),
            ValidationError::EmptyPackageName
        );
    }

    #[test]
    fn package_name_rejects_whitespace() {
        let err = PackageName::new("hello world").unwrap_err();
        assert!(matches!(
            err,
            ValidationError::PackageNameContainsWhitespace(_)
        ));
    }

    /// The displayed error must describe the actual grammar so a
    /// user reading the message can fix their manifest without
    /// reading the source.  Pin the exact phrasing so the wording
    /// can only change deliberately.
    #[test]
    fn package_name_error_describes_grammar() {
        let err = PackageName::new("foo?bar").unwrap_err();
        let displayed = err.to_string();
        assert!(
            displayed.contains("\"foo?bar\""),
            "error must echo the offending name: {displayed}"
        );
        assert!(
            displayed.contains("ASCII letters")
                && displayed.contains("ASCII digits")
                && displayed.contains("`_`")
                && displayed.contains("`-`")
                && displayed.contains("`.`"),
            "error must describe the allowed alphabet: {displayed}"
        );
        assert!(
            displayed.contains("must not start with `.` or `-`")
                && displayed.contains("must not be `.` or `..`"),
            "error must describe the structural restrictions: {displayed}"
        );
    }

    // -----------------------------------------------------------------
    // PackageName grammar covers filesystem, URL, and
    // windows-filename safety simultaneously.
    // -----------------------------------------------------------------

    #[test]
    fn package_name_accepts_simple_alphanumeric() {
        assert!(PackageName::new("fmt").is_ok());
    }

    #[test]
    fn package_name_accepts_hyphen_and_underscore() {
        assert!(PackageName::new("foo-bar").is_ok());
        assert!(PackageName::new("foo_bar").is_ok());
        assert!(PackageName::new("foo-bar-baz").is_ok());
    }

    #[test]
    fn package_name_accepts_dot_in_middle() {
        // Dots in the middle of a name are allowed; only literal
        // `.` / `..` and a leading dot are rejected.
        assert!(PackageName::new("foo.bar").is_ok());
        assert!(PackageName::new("foo..bar").is_ok());
    }

    #[test]
    fn package_name_rejects_path_traversal() {
        for raw in [".", "..", ".hidden", "foo\\bar"] {
            assert!(
                matches!(
                    PackageName::new(raw).unwrap_err(),
                    ValidationError::UnsafePackageName(_)
                ),
                "{raw:?} should be rejected as unsafe"
            );
        }
        // `../evil` parses as scope `..` + package `evil`; the dot is
        // outside the scope alphabet, so the traversal is rejected as
        // an invalid scope rather than an unsafe bare name.
        assert!(matches!(
            PackageName::new("../evil").unwrap_err(),
            ValidationError::InvalidPackageScope { .. }
        ));
    }

    // -----------------------------------------------------------------
    // Scoped names: `<scope>/<name>` with exactly one `/`.
    // -----------------------------------------------------------------

    #[test]
    fn package_name_accepts_scoped_form() {
        for raw in [
            "fmtlib/fmt",
            "a/b",
            "boost-org/boost",
            "s0me-org/pkg_name",
            "gabime/spdlog.core",
        ] {
            assert!(PackageName::new(raw).is_ok(), "{raw:?} should be accepted");
        }
    }

    #[test]
    fn package_name_rejects_more_than_one_slash() {
        for raw in ["a/b/c", "a//b", "fmtlib/fmt/extra", "a/b/"] {
            assert!(
                matches!(
                    PackageName::new(raw).unwrap_err(),
                    ValidationError::PackageNameTooManySlashes(_)
                ),
                "{raw:?} should be rejected: more than one `/`"
            );
        }
    }

    #[test]
    fn package_name_rejects_invalid_scopes() {
        // Uppercase, `_`, `.`, leading/trailing `-`, empty, and
        // over-long scopes are all outside the GitHub-login-compatible
        // scope grammar.
        let too_long = format!("{}/fmt", "a".repeat(40));
        for raw in [
            "Fmtlib/fmt",
            "fmt_lib/fmt",
            "fmt.lib/fmt",
            "-fmtlib/fmt",
            "fmtlib-/fmt",
            "/fmt",
            "../fmt",
            too_long.as_str(),
        ] {
            assert!(
                matches!(
                    PackageName::new(raw).unwrap_err(),
                    ValidationError::InvalidPackageScope { .. }
                ),
                "{raw:?} should be rejected: invalid scope"
            );
        }
        // The 39-character maximum itself is accepted.
        assert!(PackageName::new(format!("{}/fmt", "a".repeat(39))).is_ok());
    }

    #[test]
    fn package_name_rejects_unsafe_scoped_package_part() {
        for raw in ["fmtlib/", "fmtlib/..", "fmtlib/.hidden", "fmtlib/-flag"] {
            assert!(
                matches!(
                    PackageName::new(raw).unwrap_err(),
                    ValidationError::UnsafePackageName(_)
                ),
                "{raw:?} should be rejected: unsafe package part"
            );
        }
    }

    #[test]
    fn package_name_scoped_accessors() {
        let scoped = PackageName::new("fmtlib/fmt").unwrap();
        assert!(scoped.is_scoped());
        assert_eq!(scoped.scope(), Some("fmtlib"));
        assert_eq!(scoped.base_name(), "fmt");
        assert_eq!(scoped.as_str(), "fmtlib/fmt");
        assert_eq!(scoped.artifact_stem(), "fmtlib-fmt");

        let bare = PackageName::new("fmt").unwrap();
        assert!(!bare.is_scoped());
        assert_eq!(bare.scope(), None);
        assert_eq!(bare.base_name(), "fmt");
        assert_eq!(bare.artifact_stem(), "fmt");
    }

    /// The full scoped string must never act as one filesystem path
    /// component: `path_components` is the only sanctioned name-to-path
    /// mapping, and it yields one slash-free, path-safe component per
    /// part.
    #[test]
    fn package_name_path_components_never_yield_a_slash() {
        let scoped = PackageName::new("fmtlib/fmt").unwrap();
        let components: Vec<&str> = scoped.path_components().collect();
        assert_eq!(components, ["fmtlib", "fmt"]);

        let bare = PackageName::new("fmt").unwrap();
        let components: Vec<&str> = bare.path_components().collect();
        assert_eq!(components, ["fmt"]);

        for name in ["fmtlib/fmt", "a/b", "plain", "foo-bar"] {
            for component in PackageName::new(name).unwrap().path_components() {
                assert!(
                    is_path_safe_package_name(component),
                    "component {component:?} of {name:?} must be path-safe"
                );
                assert!(!component.contains('/'));
            }
        }
    }

    /// The scoped form is preserved verbatim through serde: identity is
    /// the full string, with no normalization on either direction.
    #[test]
    fn package_name_scoped_serde_round_trip() {
        let scoped: PackageName = serde_json::from_str("\"fmtlib/fmt\"").unwrap();
        assert_eq!(scoped.as_str(), "fmtlib/fmt");
        assert_eq!(serde_json::to_string(&scoped).unwrap(), "\"fmtlib/fmt\"");
        assert!(serde_json::from_str::<PackageName>("\"a/b/c\"").is_err());
    }

    #[test]
    fn valid_package_scope_grammar() {
        for scope in ["a", "a-b", "a0", "0a", "fmtlib", &"a".repeat(39)] {
            assert!(is_valid_package_scope(scope), "{scope:?} should be valid");
        }
        for scope in ["", "-a", "a-", "A", "a_b", "a.b", &"a".repeat(40)] {
            assert!(
                !is_valid_package_scope(scope),
                "{scope:?} should be invalid"
            );
        }
    }

    /// A leading `-` is rejected so the name cannot be parsed as
    /// a flag when it reaches an argv-driven tool (e.g.,
    /// `pkg-config` for `system = true` deps, the linker, or
    /// `clap` short-option splitting).
    #[test]
    fn package_name_rejects_leading_dash() {
        for raw in ["-foo", "--list-all", "-Lfoo", "-"] {
            assert!(
                matches!(
                    PackageName::new(raw).unwrap_err(),
                    ValidationError::UnsafePackageName(_)
                ),
                "{raw:?} must be rejected because of the leading `-`"
            );
        }
        // Embedded `-` is still fine.
        assert!(PackageName::new("foo-bar").is_ok());
        assert!(PackageName::new("foo--bar").is_ok());
    }

    #[test]
    fn package_name_rejects_url_reserved() {
        for raw in [
            "foo?bar",
            "foo#bar",
            "foo%2Fbar",
            "foo:bar",
            "foo&bar",
            "foo=bar",
            "foo+bar",
            "foo@bar",
        ] {
            assert!(
                matches!(
                    PackageName::new(raw).unwrap_err(),
                    ValidationError::UnsafePackageName(_)
                ),
                "{raw:?} should be rejected as URL-reserved / outside grammar"
            );
        }
    }

    #[test]
    fn package_name_rejects_windows_reserved_filename_chars() {
        for raw in [
            "foo<bar", "foo>bar", "foo|bar", "foo\"bar", "foo*bar", "foo:bar",
        ] {
            assert!(
                matches!(
                    PackageName::new(raw).unwrap_err(),
                    ValidationError::UnsafePackageName(_)
                ),
                "{raw:?} should be rejected as Windows-reserved filename char"
            );
        }
    }

    #[test]
    fn package_name_rejects_non_ascii() {
        // A grammar limited to ASCII alphanumerics + `_-.` keeps
        // the encoding in URLs and tar archives unambiguous.
        for raw in ["foo\u{00E9}bar", "\u{4E2D}\u{6587}", "emoji\u{1F600}"] {
            assert!(
                matches!(
                    PackageName::new(raw).unwrap_err(),
                    ValidationError::UnsafePackageName(_)
                ),
                "{raw:?} should be rejected as non-ASCII"
            );
        }
    }

    #[test]
    fn package_name_rejects_control_chars() {
        for raw in ["foo\u{0000}bar", "foo\u{0007}bar", "foo\u{007F}bar"] {
            assert!(PackageName::new(raw).is_err(), "{raw:?} should be rejected");
        }
    }

    #[test]
    fn target_name_rejects_empty() {
        assert_eq!(
            TargetName::new("").unwrap_err(),
            ValidationError::EmptyTargetName
        );
    }

    #[test]
    fn target_name_rejects_whitespace() {
        let err = TargetName::new("a b").unwrap_err();
        assert!(matches!(
            err,
            ValidationError::TargetNameContainsWhitespace(_)
        ));
    }

    /// Symmetric with `package_name_rejects_leading_dash`.  Target
    /// names eventually thread into argv (cargo flags, archiver
    /// inputs); a leading `-` would be ambiguous with a flag.
    /// Post-tightening this case is reported as `UnsafeTargetName`
    /// because the path-safe predicate rejects leading dashes as
    /// part of the same rule that excludes path separators.
    #[test]
    fn target_name_rejects_leading_dash() {
        for raw in ["-foo", "--release", "-"] {
            assert!(
                matches!(
                    TargetName::new(raw).unwrap_err(),
                    ValidationError::UnsafeTargetName(_)
                ),
                "{raw:?} must be rejected because of the leading `-`"
            );
        }
        // Embedded `-` is still fine.
        assert!(TargetName::new("foo-bar").is_ok());
    }

    /// Target names are joined into object, executable, and Cargo
    /// target directory paths by the build planner.  A manifest like
    /// `[target."/tmp/out"]` would otherwise let an attacker write
    /// build artifacts outside the selected `--build-dir`.  Reject
    /// the full path-component grammar: path separators, parent
    /// references, leading dots, absolute paths, drive letters,
    /// and non-ASCII bytes.
    #[test]
    fn target_name_rejects_path_unsafe_values() {
        for raw in [
            "/foo",
            "foo/bar",
            "\\foo",
            "foo\\bar",
            "..",
            "../evil",
            ".",
            ".hidden",
            "/tmp/out",
            "C:foo",
            "foo\u{00E9}bar",
            "foo\u{0000}bar",
        ] {
            assert!(
                matches!(
                    TargetName::new(raw).unwrap_err(),
                    ValidationError::UnsafeTargetName(_)
                ),
                "{raw:?} should be rejected as path-unsafe"
            );
        }
    }

    #[test]
    fn target_name_accepts_path_safe_values() {
        for raw in ["foo", "foo-bar", "foo_bar", "foo.bar", "lib1", "a"] {
            assert!(TargetName::new(raw).is_ok(), "{raw:?} should be accepted");
        }
    }

    #[test]
    fn project_accepts_valid_targets() {
        let package = Package::new(
            pkg("hello"),
            version(),
            vec![
                target("lib", TargetKind::Library, &[]),
                target("exe", TargetKind::Executable, &["lib"]),
            ],
            Vec::new(),
        )
        .unwrap();
        assert_eq!(package.targets.len(), 2);
        assert!(package.dependencies.is_empty());
    }

    #[test]
    fn project_rejects_duplicate_targets() {
        let err = Package::new(
            pkg("hello"),
            version(),
            vec![
                target("a", TargetKind::Library, &[]),
                target("a", TargetKind::Executable, &[]),
            ],
            Vec::new(),
        )
        .unwrap_err();
        assert_eq!(err, ValidationError::DuplicateTargetName("a".into()));
    }

    #[test]
    fn project_accepts_unknown_target_dep_for_planner_resolution() {
        // target-dep existence is resolved by cabin-build against
        // the full package graph, so cabin-core no longer rejects unknown
        // names here.
        let package = Package::new(
            pkg("hello"),
            version(),
            vec![target("exe", TargetKind::Executable, &["external"])],
            Vec::new(),
        )
        .unwrap();
        assert_eq!(package.targets[0].deps[0], TargetDep::private("external"));
    }

    #[test]
    fn target_dep_serde_round_trips_both_shapes() {
        // A private edge keeps the bare-string shape (existing
        // manifests and the `cabin metadata` JSON view are
        // unchanged); a public edge serializes as the table form.
        let private = TargetDep::private("fmt");
        assert_eq!(serde_json::to_string(&private).unwrap(), "\"fmt\"");
        let public = TargetDep {
            reference: "fmt:core".to_owned(),
            public: true,
        };
        assert_eq!(
            serde_json::to_string(&public).unwrap(),
            r#"{"name":"fmt:core","public":true}"#
        );
        for dep in [private, public] {
            let json = serde_json::to_string(&dep).unwrap();
            assert_eq!(serde_json::from_str::<TargetDep>(&json).unwrap(), dep);
        }
    }

    #[test]
    fn project_rejects_required_feature_not_declared() {
        let mut gated = target("tls", TargetKind::Library, &[]);
        gated.required_features = vec!["ssl".into()];
        let err = Package::with_config(PackageConfigInput {
            name: pkg("hello"),
            version: version(),
            targets: vec![gated],
            dependencies: Vec::new(),
            system_dependencies: Vec::new(),
            features: Features::default(),
        })
        .unwrap_err();
        assert_eq!(
            err,
            ValidationError::UnknownRequiredFeature {
                target: "tls".into(),
                feature: "ssl".into(),
            }
        );
    }

    #[test]
    fn project_accepts_required_feature_declared_in_features_table() {
        let mut gated = target("tls", TargetKind::Library, &[]);
        gated.required_features = vec!["ssl".into()];
        let features = Features::new(
            Vec::new(),
            [("ssl".to_owned(), Vec::new())].into_iter().collect(),
        )
        .unwrap();
        let package = Package::with_config(PackageConfigInput {
            name: pkg("hello"),
            version: version(),
            targets: vec![gated],
            dependencies: Vec::new(),
            system_dependencies: Vec::new(),
            features,
        })
        .unwrap();
        assert_eq!(package.targets[0].required_features, vec!["ssl"]);
    }

    #[test]
    fn project_rejects_required_feature_with_invalid_grammar() {
        // `dep:` / `pkg/feature` entry forms are feature-list
        // syntax, not feature names; `required-features` only
        // accepts local feature identifiers.
        let mut gated = target("tls", TargetKind::Library, &[]);
        gated.required_features = vec!["dep:openssl".into()];
        let err = Package::with_config(PackageConfigInput {
            name: pkg("hello"),
            version: version(),
            targets: vec![gated],
            dependencies: Vec::new(),
            system_dependencies: Vec::new(),
            features: Features::default(),
        })
        .unwrap_err();
        assert_eq!(
            err,
            ValidationError::InvalidConfigName {
                kind: "feature",
                value: "dep:openssl".into(),
            }
        );
    }

    #[test]
    fn missing_required_features_reports_unmet_subset_in_order() {
        let mut gated = target("tls", TargetKind::Library, &[]);
        gated.required_features = vec!["ssl".into(), "net".into()];
        let enabled: std::collections::BTreeSet<String> = ["net".to_owned()].into();
        assert_eq!(gated.missing_required_features(&enabled), vec!["ssl"]);
        let both: std::collections::BTreeSet<String> = ["net".to_owned(), "ssl".to_owned()].into();
        assert!(gated.missing_required_features(&both).is_empty());
    }

    fn dep(name: &str, kind: DependencyKind) -> Dependency {
        Dependency {
            name: pkg(name),
            source: DependencySource::Path(Utf8PathBuf::from("../somewhere")),
            kind,
            optional: false,
            features: Vec::new(),
            default_features: true,
            condition: None,
            ignore_interface_standard: false,
        }
    }

    #[test]
    fn project_rejects_duplicate_dependencies_within_a_kind() {
        let err = Package::new(
            pkg("hello"),
            version(),
            Vec::new(),
            vec![
                dep("greet", DependencyKind::Normal),
                dep("greet", DependencyKind::Normal),
            ],
        )
        .unwrap_err();
        assert_eq!(
            err,
            ValidationError::DuplicateDependency {
                name: "greet".into(),
                kind: DependencyKind::Normal,
            }
        );
    }

    #[test]
    fn project_accepts_same_name_across_different_kinds() {
        // The same package may appear under multiple dependency
        // kind sections - that is the documented duplicate policy.
        let package = Package::new(
            pkg("hello"),
            version(),
            Vec::new(),
            vec![
                dep("fmt", DependencyKind::Normal),
                dep("fmt", DependencyKind::Dev),
            ],
        )
        .expect("same name across distinct kinds is allowed");
        assert_eq!(package.dependencies.len(), 2);
    }

    #[test]
    fn project_rejects_duplicate_system_dependencies() {
        let sys = |n: &str| SystemDependency {
            name: pkg(n),
            version: ">=1".into(),
            kind: DependencyKind::Normal,
            condition: None,
        };
        let err = Package::with_config(PackageConfigInput {
            name: pkg("hello"),
            version: version(),
            targets: Vec::new(),
            dependencies: Vec::new(),
            system_dependencies: vec![sys("zlib"), sys("zlib")],
            features: Features::default(),
        })
        .unwrap_err();
        assert_eq!(
            err,
            ValidationError::DuplicateSystemDependency("zlib".into())
        );
    }

    #[test]
    fn dependency_kind_lists_are_consistent() {
        // `all()` covers every variant.
        let all = DependencyKind::all();
        assert_eq!(all.len(), 2);
        // Resolution policy: dev is excluded by default.
        assert!(DependencyKind::Normal.is_resolved_by_default());
        assert!(!DependencyKind::Dev.is_resolved_by_default());
    }

    #[test]
    fn target_kind_str_round_trip() {
        for kind in TargetKind::all() {
            assert_eq!(kind.to_string(), kind.as_str());
        }
    }

    #[test]
    fn target_kind_classification_matches_documented_policy() {
        // `library` / `executable` are the production surface
        // that `cabin build` enumerates by default.
        for kind in [TargetKind::Library, TargetKind::Executable] {
            assert!(
                kind.is_default_buildable(),
                "{kind} must be default-buildable"
            );
            assert!(!kind.is_dev_only(), "{kind} must not be dev-only");
            assert!(!kind.is_test(), "{kind} must not be classed as a test");
        }
        // The dev-only kinds: `cabin build` ignores them; `cabin
        // test` runs `test` only.
        for kind in [TargetKind::Test, TargetKind::Example] {
            assert!(
                !kind.is_default_buildable(),
                "{kind} must NOT be default-buildable"
            );
            assert!(kind.is_dev_only(), "{kind} must be dev-only");
            assert!(kind.produces_executable(), "{kind} produces an executable");
        }
        assert!(TargetKind::Test.is_test());
        assert!(!TargetKind::Example.is_test());
    }

    #[test]
    fn produces_executable_matches_kind_intent() {
        assert!(!TargetKind::Library.produces_executable());
        assert!(!TargetKind::HeaderOnly.produces_executable());
        assert!(TargetKind::Executable.produces_executable());
        assert!(TargetKind::Test.produces_executable());
        assert!(TargetKind::Example.produces_executable());
    }

    #[test]
    fn header_only_is_default_buildable_but_produces_nothing() {
        // Header-only is included in the default selection so the
        // dep-closure walk reaches it, but the planner emits no
        // compile / archive / link actions for it.
        assert!(TargetKind::HeaderOnly.is_default_buildable());
        assert!(TargetKind::HeaderOnly.is_header_only());
        assert!(!TargetKind::HeaderOnly.produces_archive());
        assert!(!TargetKind::HeaderOnly.produces_executable());
    }
}
