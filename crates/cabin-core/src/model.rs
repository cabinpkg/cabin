use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::build_flags::ProfileSettings;
use crate::compiler_wrapper::CompilerWrapperManifestSettings;
use crate::config::{Features, OptionDecl, VariantDecl};
use crate::error::ValidationError;
use crate::lint::LintSettings;
use crate::patch::PatchManifestSettings;
use crate::profile::{ProfileDefinition, ProfileName};
use crate::toolchain::ToolchainSettings;

/// Validated package name.
///
/// Newtype wrapper so future versions can centralise package-name syntax
/// rules (e.g. registry-specific patterns) without touching every callsite.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PackageName(String);

impl PackageName {
    /// Construct a [`PackageName`] after running validation rules.
    ///
    /// The grammar enforced here covers filesystem path
    /// components, sparse-HTTP path segments, package archive
    /// filenames, and Windows-reserved filename characters in a
    /// single rule. See [`is_path_safe_package_name`] for the
    /// full predicate.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ValidationError::EmptyPackageName);
        }
        if value.chars().any(char::is_whitespace) {
            return Err(ValidationError::PackageNameContainsWhitespace(value));
        }
        if !is_path_safe_package_name(&value) {
            return Err(ValidationError::UnsafePackageName(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Shared package-name validity predicate.
///
/// A name passes when it is safe to use **simultaneously** as
/// (a) a single filesystem path component on every supported
/// host OS, (b) a single sparse-HTTP URL path segment, and
/// (c) a fragment of a package archive filename. The grammar is
/// deliberately strict so the same `PackageName` value can flow
/// from manifest parsing through the workspace loader, the
/// resolver, the lockfile, the artifact cache, and the registry
/// (file or sparse HTTP) without any per-stage re-encoding.
///
/// A name is valid iff:
///
/// - it is non-empty;
/// - it consists only of ASCII letters (`A-Z`, `a-z`), ASCII
///   digits (`0-9`), `_`, `-`, and `.`;
/// - it is not literally `.` or `..`;
/// - it does not start with `.` or `-`.
///
/// Consequences worth calling out:
///
/// - `foo..bar` is **accepted**: it's not a parent reference
///   because the name is not literally `..` and does not start
///   with a dot. Path resolvers do not interpret the embedded
///   `..` substring as a navigation. This is intentional so that
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
    /// otherwise let a malicious manifest write artefacts outside
    /// the selected `--build-dir`. The grammar is enforced through
    /// [`is_path_safe_package_name`], which already covers path
    /// separators, `..` / `.`, leading `.` or `-`, control characters,
    /// non-ASCII bytes, and Windows-reserved filename characters in a
    /// single rule.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ValidationError::EmptyTargetName);
        }
        if value.chars().any(char::is_whitespace) {
            return Err(ValidationError::TargetNameContainsWhitespace(value));
        }
        if !is_path_safe_package_name(&value) {
            return Err(ValidationError::UnsafeTargetName(value));
        }
        Ok(Self(value))
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

/// What kind of artefact a target produces.
///
/// The string representations are stable: they are written by the manifest
/// parser, surfaced by `cabin metadata`, and consumed by the build graph
/// planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TargetKind {
    #[serde(rename = "cpp_library")]
    CppLibrary,
    /// A header-only C/C++ library. Has no translation units of its own;
    /// the planner emits no compile or archive actions, and consumers
    /// pick up its `include_dirs` through the dependency graph.
    #[serde(rename = "cpp_header_only")]
    CppHeaderOnly,
    #[serde(rename = "cpp_executable")]
    CppExecutable,
    /// A C/C++ test executable. Built and run by `cabin test`. Excluded
    /// from the default `cabin build` selection.
    #[serde(rename = "cpp_test")]
    CppTest,
    /// A C/C++ example executable. Excluded from the default
    /// `cabin build` selection. Today the only way an example
    /// reaches the build graph is as a transitive dep of another
    /// selected target; a dedicated explicit-kind selector flag
    /// is reserved for future work (the historic `--target` name
    /// is reserved for platform/toolchain target selection).
    #[serde(rename = "cpp_example")]
    CppExample,
    #[serde(rename = "rust_library")]
    RustLibrary,
    #[serde(rename = "rust_executable")]
    RustExecutable,
}

impl TargetKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CppLibrary => "cpp_library",
            Self::CppHeaderOnly => "cpp_header_only",
            Self::CppExecutable => "cpp_executable",
            Self::CppTest => "cpp_test",
            Self::CppExample => "cpp_example",
            Self::RustLibrary => "rust_library",
            Self::RustExecutable => "rust_executable",
        }
    }

    /// All kinds, in declaration order. Useful for error messages that list
    /// the supported types.
    pub const fn all() -> &'static [TargetKind] {
        &[
            Self::CppLibrary,
            Self::CppHeaderOnly,
            Self::CppExecutable,
            Self::CppTest,
            Self::CppExample,
            Self::RustLibrary,
            Self::RustExecutable,
        ]
    }

    /// Whether this kind produces an executable (linked binary).
    /// Library kinds return `false`.
    pub const fn produces_executable(self) -> bool {
        matches!(self, Self::CppExecutable | Self::CppTest | Self::CppExample)
    }

    /// Whether ordinary `cabin build` selects this kind by default.
    /// Dev-only kinds (`cpp_test` / `cpp_example`) are excluded
    /// from the default set: `cpp_test` is built by `cabin test`,
    /// and `cpp_example` only reaches the build graph as a
    /// transitive dep of another selected target.
    ///
    /// `cpp_header_only` libraries are included so the dependency
    /// closure walk reaches them; the planner emits no compile or
    /// archive actions for them, so saying "yes, this is part of
    /// the default selection" is a no-op on Ninja's side.
    pub const fn is_default_buildable(self) -> bool {
        matches!(
            self,
            Self::CppLibrary | Self::CppHeaderOnly | Self::CppExecutable
        )
    }

    /// Whether this kind is a *development-only* target — a target
    /// that exists to support workspace development but is not part
    /// of the package's public surface. Production callers use this
    /// to decide whether dev-dependencies should be activated and
    /// whether the target may be run by `cabin test`.
    pub const fn is_dev_only(self) -> bool {
        matches!(self, Self::CppTest | Self::CppExample)
    }

    /// Whether this kind is a C/C++ target (any of library /
    /// header-only / executable / test / example). Useful for
    /// cross-language guards where `cabin-rust` paths must not
    /// depend on C/C++ outputs.
    pub const fn is_cpp(self) -> bool {
        matches!(
            self,
            Self::CppLibrary
                | Self::CppHeaderOnly
                | Self::CppExecutable
                | Self::CppTest
                | Self::CppExample
        )
    }

    /// Whether `cabin test` runs this kind after building it. Today
    /// only `cpp_test` runs; `cpp_example` is build-only.
    pub const fn is_test(self) -> bool {
        matches!(self, Self::CppTest)
    }
}

impl std::fmt::Display for TargetKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A buildable unit within a package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub name: TargetName,
    pub kind: TargetKind,
    #[serde(default)]
    pub sources: Vec<PathBuf>,
    #[serde(default)]
    pub include_dirs: Vec<PathBuf>,
    #[serde(default)]
    pub defines: Vec<String>,
    /// Same-package target names or cross-package references. Cross-package
    /// references take the form `package` (resolves to the package's default
    /// library target) or `package:target` (qualified). Resolution against a
    /// concrete package graph lives in `cabin-build`, not here.
    ///
    /// Stored as raw strings, not [`TargetName`], because the qualified
    /// `package:target` form contains a `:` that the path-safe target-name
    /// grammar rejects. Validation happens at resolution time against the
    /// already-validated package / target graph; dep strings never flow
    /// directly into a filesystem path.
    #[serde(default)]
    pub deps: Vec<String>,
    /// Rust-target metadata. `Some` only for `kind = TargetKind::RustLibrary`
    /// (or future Rust kinds); the planner is responsible for asserting the
    /// invariant. Validation of `crate_type` is deferred to `cabin-rust` so
    /// `cabin-core` does not need to know what string values are accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust: Option<RustTarget>,
}

/// Rust-target manifest fields, kept separate from the generic
/// [`Target`] so C/C++ targets do not pay a memory or serialisation
/// cost. Parsed by `cabin-manifest`; validated and consumed by
/// `cabin-rust` and `cabin-build`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustTarget {
    /// `manifest_path = "..."` relative to the Cabin package root.
    pub manifest_path: PathBuf,
    /// Raw `crate_type` value (Cabin's `cabin-rust` decides whether
    /// the value is supported).
    pub crate_type: String,
    /// Optional `crate_name` override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crate_name: Option<String>,
    /// `features = [...]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// `default_features = false` maps to `--no-default-features`.
    /// Defaults to `true` to mirror Cargo's own default behaviour.
    #[serde(default = "default_true")]
    pub default_features: bool,
}

fn default_true() -> bool {
    true
}

/// A package-level Cabin dependency declared in
/// `[dependencies]`, `[build-dependencies]`, or
/// `[dev-dependencies]`.
///
/// System dependencies (`system = true` entries) are *not*
/// represented here — they live in [`SystemDependency`] because
/// they have a different schema and never enter Cabin
/// resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    /// The dependency alias used in the manifest. The alias must
    /// equal the depended-on package's `[package].name`.
    pub name: PackageName,
    pub source: DependencySource,
    /// Which manifest section the dependency was declared in.
    /// Defaults to [`DependencyKind::Normal`] so manifests that
    /// only use `[dependencies]` keep their previous serialised
    /// shape.
    #[serde(default, skip_serializing_if = "DependencyKind::is_normal")]
    pub kind: DependencyKind,
    /// Whether the dependency is optional. Optional dependencies
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
    /// `default` feature. Defaults to `true`. `default-features =
    /// false` only narrows *this* edge — if another edge requests
    /// defaults for the same package, the unified result still
    /// includes them.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub default_features: bool,
    /// Optional target condition. `Some` when the dependency was
    /// declared inside a `[target.'cfg(...)'.<kind>]` table;
    /// `None` for unconditional declarations. Conditional
    /// dependencies whose condition does not match the
    /// evaluation [`crate::TargetPlatform`] are filtered out by
    /// `cabin-workspace` / `cabin-feature` / `cabin-build`
    /// before reaching the resolver or the build planner, but they
    /// stay on `Package::dependencies` for metadata round-trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<crate::Condition>,
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
    /// [`crate::TargetPlatform`]. Unconditional declarations
    /// are always active; conditional declarations are active
    /// iff their condition evaluates to `true`.
    pub fn matches_platform(&self, platform: &crate::TargetPlatform) -> bool {
        match &self.condition {
            None => true,
            Some(cond) => cond.evaluate(platform),
        }
    }
}

/// Which kind of dependency is declared.
///
/// Cabin distinguishes package dependency kinds (`Normal`, `Build`,
/// `Dev`) — all of which are sourced from other Cabin packages —
/// from system dependencies, which are externally provided and never
/// enter Cabin resolution. System declarations live alongside the
/// package kinds as a separate `system = true` flag on a regular
/// `[dependencies]` / `[build-dependencies]` / `[dev-dependencies]`
/// entry and are modelled by [`SystemDependency`].
///
/// The wire format mirrors the manifest section names: `"normal"`,
/// `"build"`, `"dev"`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum DependencyKind {
    /// `[dependencies]`. Linked into ordinary builds.
    #[default]
    Normal,
    /// `[build-dependencies]`. Available to build preparation;
    /// not auto-linked into ordinary targets.
    Build,
    /// `[dev-dependencies]`. Declaration-only for ordinary
    /// commands; reserved for future test / example targets.
    Dev,
}

impl DependencyKind {
    /// Stable lowercase label, matching the manifest section name.
    pub const fn as_str(self) -> &'static str {
        match self {
            DependencyKind::Normal => "normal",
            DependencyKind::Build => "build",
            DependencyKind::Dev => "dev",
        }
    }

    /// All kinds in canonical order. `cabin metadata` and the
    /// canonical package metadata both iterate kinds in this order
    /// so output stays deterministic.
    pub const fn all() -> &'static [DependencyKind] {
        &[
            DependencyKind::Normal,
            DependencyKind::Build,
            DependencyKind::Dev,
        ]
    }

    /// Whether this kind is included in the resolver / fetch /
    /// build pipeline by default. Dev dependencies are excluded.
    pub const fn is_resolved_by_default(self) -> bool {
        matches!(self, DependencyKind::Normal | DependencyKind::Build)
    }

    /// Whether this kind contributes link / include edges to
    /// ordinary `cabin build` targets. Only `Normal` does.
    pub const fn affects_ordinary_build(self) -> bool {
        matches!(self, DependencyKind::Normal)
    }

    /// Helper for `#[serde(skip_serializing_if = ...)]` so
    /// existing on-disk metadata that omits the `kind` field
    /// stays byte-identical for `[dependencies]`-only manifests.
    pub fn is_normal(&self) -> bool {
        matches!(self, DependencyKind::Normal)
    }

    /// The manifest section name (`[dependencies]`,
    /// `[build-dependencies]`, etc.) corresponding to this kind.
    /// Used in error messages.
    pub const fn manifest_section(self) -> &'static str {
        match self {
            DependencyKind::Normal => "[dependencies]",
            DependencyKind::Build => "[build-dependencies]",
            DependencyKind::Dev => "[dev-dependencies]",
        }
    }
}

impl std::fmt::Display for DependencyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A system dependency declared with `system = true` on a
/// `[dependencies]` / `[build-dependencies]` /
/// `[dev-dependencies]` entry.
///
/// System dependencies are externally provided (system libraries,
/// SDKs, installed tools). Cabin never resolves, fetches,
/// downloads, or installs them — `cabin-system-deps` probes them
/// via `pkg-config` at build time, and the resulting cflags /
/// ldflags are merged into the per-package build flags before
/// the planner runs. The typed value round-trips through
/// `cabin metadata`, the canonical package metadata, and the
/// index metadata so external tooling sees the system-dep set
/// alongside the Cabin-package deps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemDependency {
    /// The dependency name as written in the manifest.
    pub name: PackageName,
    /// Version requirement string for `pkg-config`. Cabin does
    /// not interpret it as a SemVer constraint; the system-deps
    /// layer translates the supported comparators for
    /// `pkg-config` and reports unsupported forms as errors.
    pub version: String,
    /// Which dependency table the entry was declared in
    /// (`[dependencies]`, `[build-dependencies]`, or
    /// `[dev-dependencies]`). Drives per-kind activation: a
    /// dev-kind system dep is only probed when `cabin test` is
    /// running, mirroring the Cabin-package dev-dep rule.
    #[serde(default)]
    pub kind: DependencyKind,
    /// Optional target condition. `Some` when the system
    /// dependency was declared inside a
    /// `[target.'cfg(...)'.<kind>-dependencies]` table. The
    /// condition is preserved so package / index metadata stays
    /// portable across platforms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<crate::Condition>,
}

/// Where a dependency is sourced from.
///
/// Covers [`DependencySource::Path`] for local path dependencies and
/// [`DependencySource::Version`] for registry-resolved versioned
/// dependencies; [`DependencySource::Workspace`] for
/// the `{ workspace = true }` opt-in into the workspace's shared
/// dependency table. The `Workspace` variant is an unresolved
/// marker — `cabin-workspace::load_workspace` rewrites it into the
/// matching `Path` or `Version` source from `[workspace.dependencies]`
/// before any consumer sees a [`crate::Package`] in a [`crate::Package`]
/// returned from the workspace loader. If a `Workspace` source ever
/// reaches a planner or resolver it indicates the package was loaded
/// outside of `cabin-workspace`, which is a workspace invariant
/// violation worth surfacing as a clear error in the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DependencySource {
    /// Local path dependency. The path is interpreted relative to the
    /// manifest directory of the package that declared the dependency.
    #[serde(rename = "path")]
    Path(PathBuf),
    /// Versioned registry dependency. The requirement is matched against
    /// candidate versions during dependency resolution.
    #[serde(rename = "version")]
    Version(semver::VersionReq),
    /// `dep = { workspace = true }`. An unresolved opt-in
    /// into the workspace's `[workspace.dependencies]` table.
    /// `cabin-workspace::load_workspace` resolves these to a
    /// concrete [`DependencySource::Path`] or
    /// [`DependencySource::Version`] before producing a
    /// `PackageGraph`.
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
    /// `[dependencies]`, `[build-dependencies]`, or
    /// `[dev-dependencies]`. Each entry carries its
    /// [`DependencyKind`]; iteration order is sorted by
    /// `(kind, name)` so callers see deterministic output.
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    /// `system = true` declarations. Empty if not
    /// declared. System dependencies never enter the resolver,
    /// the lockfile, or the artifact cache; they are
    /// declaration-only and round-trip through metadata.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub system_dependencies: Vec<SystemDependency>,
    /// `[features]` declarations. Empty if the manifest has
    /// no `[features]` table.
    #[serde(default, skip_serializing_if = "is_empty_features")]
    pub features: Features,
    /// `[options]` declarations. Empty if not declared.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, OptionDecl>,
    /// `[variants]` declarations. Empty if not declared.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variants: BTreeMap<String, VariantDecl>,
    /// `[profile.<name>]` declarations from the manifest, keyed
    /// by profile name. Built-in profiles do not need to appear
    /// here; entries that match a built-in name override those
    /// defaults. Empty for manifests with no profile tables, so
    /// older manifests stay byte-identical through round-tripping.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<ProfileName, ProfileDefinition>,
    /// `[toolchain]` plus any `[target.'cfg(...)'.toolchain]`
    /// overrides declared on this manifest. Only the workspace
    /// root manifest's settings are honoured; member manifests
    /// that declare a `[toolchain]` table are rejected by the
    /// workspace loader.
    #[serde(default, skip_serializing_if = "ToolchainSettings::is_empty")]
    pub toolchain: ToolchainSettings,
    /// `[profile]` plus any `[target.'cfg(...)'.profile]`
    /// declarations for this package. Per-package by design — each
    /// package may add its own defines / include dirs / extra args.
    #[serde(default, skip_serializing_if = "ProfileSettings::is_empty")]
    pub build: ProfileSettings,
    /// `[profile.cache]` plus any `[target.'cfg(...)'.profile.cache]`
    /// declarations from the workspace root manifest. Member
    /// manifests cannot declare cache settings — the workspace
    /// loader rejects them — so reading off the root is sufficient.
    /// Round-trips through metadata so packaged manifests preserve
    /// a publisher's declared wrapper preferences.
    #[serde(
        default,
        skip_serializing_if = "CompilerWrapperManifestSettings::is_empty"
    )]
    pub compiler_wrapper: CompilerWrapperManifestSettings,
    /// `[patch]` declarations on the workspace-root manifest.
    /// Member manifests cannot declare patches — the workspace
    /// loader rejects them — and `cabin package` refuses to
    /// archive a manifest with a non-empty `[patch]` table.
    /// Patches are *local development policy*, not package
    /// metadata.
    #[serde(default, skip_serializing_if = "PatchManifestSettings::is_empty")]
    pub patches: PatchManifestSettings,
    /// `[lint.<tool>]` declarations.  Today only
    /// `[lint.cpplint].filters` is recognised; future lint
    /// tools land as additional sub-tables here without
    /// changing this field's name.  Empty for manifests with
    /// no lint table so older manifests round-trip
    /// byte-identically through metadata.
    #[serde(default, skip_serializing_if = "LintSettings::is_empty")]
    pub lint: LintSettings,
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
    /// - feature/option/variant declarations are well-formed
    ///
    /// Target-dep references (same-package, cross-package, or
    /// qualified `package:target`) are resolved by `cabin-build`
    /// against the full package graph, not here.
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
            options: BTreeMap::new(),
            variants: BTreeMap::new(),
        })
    }

    /// Build a validated [`Package`] with `[features]` / `[options]` /
    /// `[variants]` declarations attached. `cabin-manifest` calls this
    /// after parsing the new tables.
    pub fn with_config(input: PackageConfigInput) -> Result<Self, ValidationError> {
        let PackageConfigInput {
            name,
            version,
            targets,
            dependencies,
            system_dependencies,
            features,
            options,
            variants,
        } = input;
        Self::validate_targets(&targets)?;
        Self::validate_dependencies(&dependencies)?;
        Self::validate_system_dependencies(&system_dependencies)?;
        features.validate()?;
        for (n, decl) in &options {
            decl.validate(n)?;
        }
        for (n, decl) in &variants {
            decl.validate(n)?;
        }
        Ok(Self {
            name,
            version,
            targets,
            dependencies,
            system_dependencies,
            features,
            options,
            variants,
            profiles: BTreeMap::new(),
            toolchain: ToolchainSettings::default(),
            build: ProfileSettings::default(),
            compiler_wrapper: CompilerWrapperManifestSettings::default(),
            patches: PatchManifestSettings::default(),
            lint: LintSettings::default(),
        })
    }

    /// Attach manifest-declared `[profile.*]` definitions to this
    /// package. Returns the same package so callers can chain it
    /// after [`Package::with_config`] without exploding the
    /// constructor signature for every new optional table.
    pub fn with_profiles(mut self, profiles: BTreeMap<ProfileName, ProfileDefinition>) -> Self {
        self.profiles = profiles;
        self
    }
}

/// Bundled inputs for [`Package::with_config`].
///
/// `cabin-manifest` builds this from the parsed `cabin.toml` and hands
/// it to [`Package::with_config`]. Threading inputs through one struct
/// keeps `with_config` callable across the workspace without a fixed
/// positional argument order as new tables (features, options,
/// variants, …) land.
#[derive(Debug, Clone)]
pub struct PackageConfigInput {
    /// `package.name` from the manifest.
    pub name: PackageName,
    /// `package.version` from the manifest.
    pub version: semver::Version,
    /// Parsed `[target.*]` definitions.
    pub targets: Vec<Target>,
    /// Parsed `[dependencies]` / `[build-dependencies]` / `[dev-dependencies]`.
    pub dependencies: Vec<Dependency>,
    /// Parsed `[system-dependencies]`.
    pub system_dependencies: Vec<SystemDependency>,
    /// Parsed `[features]`.
    pub features: Features,
    /// Parsed `[options]`.
    pub options: BTreeMap<String, OptionDecl>,
    /// Parsed `[variants]`.
    pub variants: BTreeMap<String, VariantDecl>,
}

impl Package {
    /// Attach manifest-declared `[lint.<tool>]` settings.
    /// Companion to [`Package::with_profiles`] / friends so
    /// callers can layer in optional tables without exploding
    /// the constructor signature.
    pub fn with_lint(mut self, lint: LintSettings) -> Self {
        self.lint = lint;
        self
    }

    /// Attach the manifest-declared `[toolchain]` /
    /// `[target.'cfg(...)'.toolchain]` block. Workspace loaders
    /// reject these declarations on member / path-dep manifests
    /// so only the entry-point manifest's value reaches downstream
    /// crates.
    pub fn with_toolchain(mut self, toolchain: ToolchainSettings) -> Self {
        self.toolchain = toolchain;
        self
    }

    /// Attach the manifest-declared `[profile]` /
    /// `[target.'cfg(...)'.profile]` block. Per-package by design.
    pub fn with_build(mut self, build: ProfileSettings) -> Self {
        self.build = build;
        self
    }

    /// Attach the manifest-declared `[profile.cache]` /
    /// `[target.'cfg(...)'.profile.cache]` blocks. Workspace
    /// loaders reject these declarations on member / path-dep
    /// manifests so only the entry-point manifest's value reaches
    /// downstream crates.
    pub fn with_compiler_wrapper(mut self, settings: CompilerWrapperManifestSettings) -> Self {
        self.compiler_wrapper = settings;
        self
    }

    /// Attach the manifest-declared `[patch]` block. Workspace
    /// loaders reject these declarations on member / path-dep
    /// manifests so only the entry-point manifest's value
    /// reaches downstream crates.
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

    /// Iterator over the package dependencies that participate in
    /// the resolver / fetch / build pipeline by default — i.e.
    /// every Cabin package dependency except `Dev`.
    pub fn resolved_dependencies(&self) -> impl Iterator<Item = &Dependency> {
        self.dependencies
            .iter()
            .filter(|d| d.kind.is_resolved_by_default())
    }

    /// Iterator over dependencies of a specific kind. Order is
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
            deps: deps.iter().map(|d| (*d).to_owned()).collect(),
            rust: None,
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
    /// reading the source. Pin the exact phrasing so the wording
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
        for raw in [".", "..", "../evil", ".hidden", "foo/bar", "foo\\bar"] {
            assert!(
                matches!(
                    PackageName::new(raw).unwrap_err(),
                    ValidationError::UnsafePackageName(_)
                ),
                "{raw:?} should be rejected as unsafe"
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

    /// Symmetric with `package_name_rejects_leading_dash`. Target
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
    /// target directory paths by the build planner. A manifest like
    /// `[target."/tmp/out"]` would otherwise let an attacker write
    /// build artefacts outside the selected `--build-dir`. Reject
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
                target("lib", TargetKind::CppLibrary, &[]),
                target("exe", TargetKind::CppExecutable, &["lib"]),
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
                target("a", TargetKind::CppLibrary, &[]),
                target("a", TargetKind::CppExecutable, &[]),
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
            vec![target("exe", TargetKind::CppExecutable, &["external"])],
            Vec::new(),
        )
        .unwrap();
        assert_eq!(package.targets[0].deps[0].as_str(), "external");
    }

    fn dep(name: &str, kind: DependencyKind) -> Dependency {
        Dependency {
            name: pkg(name),
            source: DependencySource::Path(PathBuf::from("../somewhere")),
            kind,
            optional: false,
            features: Vec::new(),
            default_features: true,
            condition: None,
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
        // kind sections — that is the documented duplicate policy.
        let package = Package::new(
            pkg("hello"),
            version(),
            Vec::new(),
            vec![
                dep("fmt", DependencyKind::Normal),
                dep("fmt", DependencyKind::Build),
                dep("fmt", DependencyKind::Dev),
            ],
        )
        .expect("same name across distinct kinds is allowed");
        assert_eq!(package.dependencies.len(), 3);
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
            options: BTreeMap::new(),
            variants: BTreeMap::new(),
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
        assert_eq!(all.len(), 3);
        // Resolution policy: dev is excluded by default.
        assert!(DependencyKind::Normal.is_resolved_by_default());
        assert!(DependencyKind::Build.is_resolved_by_default());
        assert!(!DependencyKind::Dev.is_resolved_by_default());
        // Linkage policy: only Normal contributes to ordinary builds.
        assert!(DependencyKind::Normal.affects_ordinary_build());
        for kind in [DependencyKind::Build, DependencyKind::Dev] {
            assert!(!kind.affects_ordinary_build());
        }
    }

    #[test]
    fn target_kind_str_round_trip() {
        for kind in TargetKind::all() {
            assert_eq!(kind.to_string(), kind.as_str());
        }
    }

    #[test]
    fn target_kind_classification_matches_documented_policy() {
        // `cpp_library` / `cpp_executable` are the production C++
        // surface that `cabin build` enumerates by default. Rust
        // libraries are reachable through target deps but not
        // enumerated by the default selection, matching the
        // planner's behaviour.
        for kind in [TargetKind::CppLibrary, TargetKind::CppExecutable] {
            assert!(
                kind.is_default_buildable(),
                "{kind} must be default-buildable"
            );
            assert!(!kind.is_dev_only(), "{kind} must not be dev-only");
            assert!(!kind.is_test(), "{kind} must not be classed as a test");
        }
        for kind in [TargetKind::RustLibrary, TargetKind::RustExecutable] {
            assert!(!kind.is_default_buildable(), "{kind} not in default set");
        }
        // The dev-only kinds: `cabin build` ignores them; `cabin
        // test` runs `cpp_test` only.
        for kind in [TargetKind::CppTest, TargetKind::CppExample] {
            assert!(
                !kind.is_default_buildable(),
                "{kind} must NOT be default-buildable"
            );
            assert!(kind.is_dev_only(), "{kind} must be dev-only");
            assert!(kind.produces_executable(), "{kind} produces an executable");
        }
        assert!(TargetKind::CppTest.is_test());
        assert!(!TargetKind::CppExample.is_test());
    }

    #[test]
    fn produces_executable_matches_kind_intent() {
        assert!(!TargetKind::CppLibrary.produces_executable());
        assert!(!TargetKind::RustLibrary.produces_executable());
        assert!(TargetKind::CppExecutable.produces_executable());
        assert!(TargetKind::CppTest.produces_executable());
        assert!(TargetKind::CppExample.produces_executable());
    }
}
