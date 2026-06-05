//! Features — public, additive, named-boolean capabilities used
//! to gate optional dependencies and per-edge feature requests.
//!
//! A Feature may be enabled by the package's user or by a
//! downstream consumer. Feature implication arrows form a directed
//! graph; the resolver expands defaults plus user requests by
//! transitive closure. Feature entries can enable optional
//! dependencies (`dep:foo`) and request features on dependency
//! packages (`crate/feature`).
//!
//! All declarations live on `cabin_core::Package`. Selection happens
//! through [`BuildConfiguration::resolve`], which consumes the
//! declarations plus a [`SelectionRequest`] (typically built from CLI
//! flags by `cabin`).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::build_flags::ResolvedProfileFlags;
use crate::compiler_wrapper::{CompilerWrapperSummary, ResolvedCompilerWrapper};
use crate::error::ValidationError;
use crate::profile::ResolvedProfile;
use crate::toolchain::ResolvedToolchain;

/// The reserved feature group name. The list of names mapped to this
/// key in `[features]` is the package's "default" feature set: the
/// Features Cabin enables when the user does not pass
/// `--no-default-features`.
pub const DEFAULT_FEATURE_KEY: &str = "default";

/// `[features]` declarations for a package.
///
/// Feature names are stable identifiers. The `default` group lists
/// which features are enabled by default; other entries declare
/// individual features and what enabling them implies.
///
/// Each entry on the right-hand side of a feature is a string in
/// one of three documented forms (parsed lazily into
/// [`FeatureEntry`] by the feature resolver):
///
/// - `"feature_name"` — enables another local feature on the same
///   package (transitive feature implication).
/// - `"dep:dependency_name"` — enables an optional Cabin package
///   dependency declared by this package's `[dependencies]`
///   table.
/// - `"dependency_name/feature_name"` — requests a specific
///   feature on a Cabin package dependency. If the dependency is
///   optional, this form also enables it.
///
/// The on-disk shape stays a flat list of strings so older
/// readers and the canonical metadata format remain
/// byte-identical for packages that only use the local-feature
/// form.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Features {
    /// Default features. Empty when there is no `default` entry in
    /// `[features]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default: Vec<String>,
    /// Declared features and their implication lists. Stored as a
    /// `BTreeMap` so iteration is deterministic and output is stable.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub features: BTreeMap<String, Vec<String>>,
}

impl Features {
    /// Convenience constructor for `Package::new`-style call sites.
    ///
    /// # Errors
    /// Returns a [`ValidationError`] when the resulting feature set fails
    /// [`Features::validate`] (see that method for the specific conditions).
    pub fn new(
        default: Vec<String>,
        features: BTreeMap<String, Vec<String>>,
    ) -> Result<Self, ValidationError> {
        let me = Self { default, features };
        me.validate()?;
        Ok(me)
    }

    /// Validate identifier grammar, the reserved `default` key,
    /// internal references between local features, and local
    /// cycles. `dep:` / `dep/feature` entries are validated for
    /// grammar only; the *feature resolver* checks that the
    /// referenced dependency exists, that it is optional when
    /// `dep:` is used, and that the requested feature exists on
    /// the dependency package — those checks need the package
    /// graph and therefore happen one layer up.
    ///
    /// # Errors
    /// Returns [`ValidationError::ReservedFeatureName`] when `default` is used
    /// as a declared feature, [`ValidationError::UnknownFeatureReference`] for a
    /// default or implication pointing at an undeclared local feature,
    /// [`ValidationError::InvalidFeatureEntry`] for a malformed implication
    /// entry, a cycle error from `Self::detect_cycles`, and any identifier
    /// grammar error from validating a feature name.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.features.contains_key(DEFAULT_FEATURE_KEY) {
            return Err(ValidationError::ReservedFeatureName(
                DEFAULT_FEATURE_KEY.to_owned(),
            ));
        }
        for name in self.features.keys() {
            validate_identifier(name)?;
        }
        for name in &self.default {
            validate_identifier(name)?;
            if !self.features.contains_key(name) {
                return Err(ValidationError::UnknownFeatureReference {
                    referrer: DEFAULT_FEATURE_KEY.to_owned(),
                    referenced: name.to_owned(),
                });
            }
        }
        for (name, implies) in &self.features {
            for raw in implies {
                let entry = FeatureEntry::parse(raw).map_err(|kind| {
                    ValidationError::InvalidFeatureEntry {
                        referrer: name.clone(),
                        entry: raw.clone(),
                        reason: kind,
                    }
                })?;
                match entry {
                    FeatureEntry::Local(local) => {
                        if !self.features.contains_key(&local) {
                            return Err(ValidationError::UnknownFeatureReference {
                                referrer: name.clone(),
                                referenced: local,
                            });
                        }
                    }
                    FeatureEntry::OptionalDep(_) | FeatureEntry::DepFeature { .. } => {
                        // Dependency-shaped entries are validated by
                        // the feature resolver, which has access to
                        // the dep list.
                    }
                }
            }
        }
        self.detect_cycles()?;
        Ok(())
    }

    fn detect_cycles(&self) -> Result<(), ValidationError> {
        #[derive(Clone, Copy)]
        enum Color {
            Visiting,
            Done,
        }
        fn visit<'a>(
            node: &'a str,
            features: &'a BTreeMap<String, Vec<String>>,
            state: &mut std::collections::HashMap<&'a str, Color>,
            path: &mut Vec<&'a str>,
        ) -> Result<(), ValidationError> {
            match state.get(node) {
                Some(Color::Done) => return Ok(()),
                Some(Color::Visiting) => {
                    let start = path.iter().position(|n| *n == node).unwrap_or(0);
                    let mut cycle: Vec<String> =
                        path[start..].iter().map(|s| (*s).to_owned()).collect();
                    cycle.push(node.to_owned());
                    return Err(ValidationError::FeatureCycle(cycle));
                }
                None => {}
            }
            state.insert(node, Color::Visiting);
            path.push(node);
            if let Some(implies) = features.get(node) {
                for r in implies {
                    // Cycle detection only follows local-feature
                    // edges. `dep:` / `dep/feature` entries are
                    // not part of this package's local feature
                    // graph and never trigger a local cycle here.
                    // Look up the referenced name in the existing
                    // `features` keys (so the borrowed slice
                    // outlives this call) instead of creating a
                    // new `String` from the parsed entry.
                    if let Ok(FeatureEntry::Local(local)) = FeatureEntry::parse(r)
                        && let Some((stored, _)) = features.get_key_value(local.as_str())
                    {
                        visit(stored.as_str(), features, state, path)?;
                    }
                }
            }
            path.pop();
            state.insert(node, Color::Done);
            Ok(())
        }
        let mut state = std::collections::HashMap::new();
        let mut path: Vec<&str> = Vec::new();
        for name in self.features.keys() {
            visit(name.as_str(), &self.features, &mut state, &mut path)?;
        }
        Ok(())
    }

    /// Expand a set of root feature names by transitive closure
    /// over the *local* `features` map. Caller is responsible for
    /// ensuring every root is a declared feature.
    ///
    /// Entries that take the form `dep:<name>` or `<dep>/<feature>`
    /// are skipped: they are package-level effects, not local
    /// features, and are owned by the cross-package feature
    /// resolver.
    pub fn expand(&self, roots: &BTreeSet<String>) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut stack: Vec<String> = roots.iter().cloned().collect();
        while let Some(name) = stack.pop() {
            if !out.insert(name.clone()) {
                continue;
            }
            if let Some(implies) = self.features.get(&name) {
                for raw in implies {
                    if let Ok(FeatureEntry::Local(local)) = FeatureEntry::parse(raw) {
                        stack.push(local);
                    }
                }
            }
        }
        out
    }
}

/// Typed view of a single right-hand-side entry in a `[features]`
/// list (`feature_name`, `dep:dependency_name`, or
/// `dependency_name/feature_name`).
///
/// `cabin-core` parses the form lazily — the on-disk shape stays
/// the original string — so older readers are unaffected. The
/// feature resolver consumes the typed view to decide which
/// effects an entry has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureEntry {
    /// Enables another local feature on the same package.
    Local(String),
    /// Enables an optional Cabin package dependency declared by
    /// this package. Spelled `dep:<name>` in the manifest.
    OptionalDep(String),
    /// Requests `feature` on `dep`. If `dep` is optional, this
    /// also enables it. Spelled `<dep>/<feature>` in the
    /// manifest.
    DepFeature { dep: String, feature: String },
}

/// Why parsing a feature-list entry failed. Carried inside
/// [`ValidationError::InvalidFeatureEntry`] so user errors keep
/// the original string and the structural reason it was
/// rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidFeatureEntryKind {
    /// The entry was empty.
    Empty,
    /// The entry started with `dep:` but the name was empty.
    EmptyDepName,
    /// The entry contained a `/` but either side was empty.
    EmptyDepOrFeature,
    /// The entry contained more than one `/` separator.
    MultiplePathSeparators,
    /// The entry contained a character outside the supported
    /// alphabet (`A-Z a-z 0-9 _ - .` plus the leading `dep:` or
    /// single `/` separator).
    UnsupportedCharacter(char),
}

impl InvalidFeatureEntryKind {
    pub fn message(self) -> &'static str {
        match self {
            InvalidFeatureEntryKind::Empty => "feature entries must not be empty",
            InvalidFeatureEntryKind::EmptyDepName => {
                "`dep:` entries require a non-empty dependency name"
            }
            InvalidFeatureEntryKind::EmptyDepOrFeature => {
                "`<dep>/<feature>` entries require both a dependency name and a feature name"
            }
            InvalidFeatureEntryKind::MultiplePathSeparators => {
                "feature entries may contain at most one `/`"
            }
            InvalidFeatureEntryKind::UnsupportedCharacter(_) => {
                "feature entries may only use ASCII letters, digits, `_`, `-`, `.`, plus the leading `dep:` or single `/` separator"
            }
        }
    }
}

impl FeatureEntry {
    /// Parse a single `[features]` value into a typed entry.
    ///
    /// # Errors
    /// Returns [`InvalidFeatureEntryKind::Empty`] for an empty input,
    /// [`InvalidFeatureEntryKind::EmptyDepName`] for a bare `dep:`,
    /// [`InvalidFeatureEntryKind::MultiplePathSeparators`] for more than one
    /// `/`, [`InvalidFeatureEntryKind::EmptyDepOrFeature`] when either side of
    /// `<dep>/<feature>` is empty, and
    /// [`InvalidFeatureEntryKind::UnsupportedCharacter`] for a name containing a
    /// character outside the allowed identifier set.
    pub fn parse(input: &str) -> Result<Self, InvalidFeatureEntryKind> {
        if input.is_empty() {
            return Err(InvalidFeatureEntryKind::Empty);
        }
        if let Some(rest) = input.strip_prefix("dep:") {
            if rest.is_empty() {
                return Err(InvalidFeatureEntryKind::EmptyDepName);
            }
            check_identifier_chars(rest)?;
            return Ok(FeatureEntry::OptionalDep(rest.to_owned()));
        }
        if let Some((dep, feature)) = input.split_once('/') {
            if feature.contains('/') {
                return Err(InvalidFeatureEntryKind::MultiplePathSeparators);
            }
            if dep.is_empty() || feature.is_empty() {
                return Err(InvalidFeatureEntryKind::EmptyDepOrFeature);
            }
            check_identifier_chars(dep)?;
            check_identifier_chars(feature)?;
            return Ok(FeatureEntry::DepFeature {
                dep: dep.to_owned(),
                feature: feature.to_owned(),
            });
        }
        check_identifier_chars(input)?;
        Ok(FeatureEntry::Local(input.to_owned()))
    }
}

fn check_identifier_chars(s: &str) -> Result<(), InvalidFeatureEntryKind> {
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '-' | '.' => {}
            other => return Err(InvalidFeatureEntryKind::UnsupportedCharacter(other)),
        }
    }
    Ok(())
}

/// User-supplied flag inputs that select features.
/// Built by `cabin` from `--features`, `--all-features`, and
/// `--no-default-features`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionRequest {
    /// Explicit `--features a,b` entries. Order does not matter; the
    /// Resolver normalizes them.
    pub features: BTreeSet<String>,
    pub all_features: bool,
    pub no_default_features: bool,
}

/// Resolved, validated build configuration. Drives:
/// - which features are enabled;
/// - which profile its compile / link flags come from;
/// - which toolchain compiled it;
/// - which semantic build flags applied;
/// - the deterministic fingerprint that future cache logic can hash on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildConfiguration {
    pub enabled_features: BTreeSet<String>,
    /// Resolved profile (e.g. `dev`, `release`, or a custom
    /// profile inheriting from a built-in). Always populated:
    /// every build configuration is associated with exactly one
    /// profile.
    pub profile: ResolvedProfile,
    /// Toolchain summary used for fingerprinting and metadata.
    /// Recorded as the requested spec + tool source per kind so
    /// the fingerprint is stable across machines that resolve
    /// `clang++` to different absolute paths.
    pub toolchain: ToolchainSummary,
    /// Resolved per-package build flags. The metadata view
    /// reports this directly; the fingerprint includes a
    /// deterministic digest of every field.
    pub build_flags: ResolvedProfileFlags,
    pub fingerprint: String,
}

/// Lightweight, non-machine-specific summary of the resolved
/// toolchain. Stored on every [`BuildConfiguration`] so the
/// fingerprint reflects "which compiler did this build use" without
/// pinning the local absolute path.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainSummary {
    /// `(kind -> "<spec>") `, sorted alphabetically by tool key.
    /// Each entry records the user-visible spelling (`clang++`,
    /// `/opt/llvm/bin/clang++`, …); absolute resolved paths from
    /// PATH discovery are deliberately omitted.
    pub tools: BTreeMap<String, String>,
    /// `(kind -> source label) ` parallel to `tools`. Source
    /// labels are stable strings (`cli`, `env`, `manifest`,
    /// `manifest-conditional`, `default`).
    pub sources: BTreeMap<String, String>,
    /// Optional compiler-cache wrapper (e.g. `ccache`, `sccache`)
    /// applied on top of the C++ compiler. `None` when no wrapper
    /// is selected; otherwise the kind/spec/source/version are
    /// folded into the configuration fingerprint so a build with a
    /// different wrapper choice reuses neither cache layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compiler_wrapper: Option<CompilerWrapperSummary>,
}

impl ToolchainSummary {
    /// Build a summary from a `ResolvedToolchain`. Storage is
    /// deterministic: tools iterate in sorted [`crate::ToolKind`]
    /// order via [`ResolvedToolchain::iter`].
    pub fn from_resolved(toolchain: &ResolvedToolchain) -> Self {
        Self::from_resolved_parts(toolchain, None)
    }

    /// Build a summary from a `ResolvedToolchain` plus an optional
    /// compiler-cache wrapper. The wrapper is normalized into a
    /// [`CompilerWrapperSummary`] so the fingerprint captures the
    /// requested wrapper without leaking the local absolute path.
    pub fn from_resolved_parts(
        toolchain: &ResolvedToolchain,
        wrapper: Option<&ResolvedCompilerWrapper>,
    ) -> Self {
        let mut tools = BTreeMap::new();
        let mut sources = BTreeMap::new();
        for tool in toolchain.iter() {
            let key = tool.kind.as_key().to_owned();
            tools.insert(key.clone(), tool.spec.display());
            sources.insert(
                key,
                crate::toolchain::tool_source_label(tool.source).to_owned(),
            );
        }
        Self {
            tools,
            sources,
            compiler_wrapper: wrapper.map(CompilerWrapperSummary::from_resolved),
        }
    }
}

/// Bundled inputs for [`BuildConfiguration::resolve`].
///
/// `BuildConfiguration` ties together every per-package input the
/// resolver evaluates (declared features, the requested selection,
/// and the already-resolved profile / toolchain / build flags).
/// Threading them through one struct keeps the call signature stable
/// as new inputs land and stops `cabin metadata` orchestration from
/// needing to remember a fixed positional order.
#[derive(Debug)]
pub struct BuildConfigurationInput<'a> {
    /// Package name. Used only to render clear validation errors.
    pub package: &'a str,
    /// Declared `[features]` table for the package.
    pub features: &'a Features,
    /// CLI / config selection request (features).
    pub request: &'a SelectionRequest,
    /// Already-resolved profile.
    pub profile: ResolvedProfile,
    /// Already-resolved toolchain summary.
    pub toolchain: ToolchainSummary,
    /// Already-resolved per-profile build flags.
    pub build_flags: ResolvedProfileFlags,
}

impl BuildConfiguration {
    /// Resolve a [`SelectionRequest`] against a set of declarations.
    /// `input.package` is used only to make error messages clear.
    ///
    /// # Errors
    /// Returns [`ValidationError::UnknownFeature`] when the request names a
    /// feature not declared in `input.features`.
    pub fn resolve(input: BuildConfigurationInput<'_>) -> Result<Self, ValidationError> {
        let BuildConfigurationInput {
            package,
            features,
            request,
            profile,
            toolchain,
            build_flags,
        } = input;
        let enabled_features = resolve_features(package, features, request)?;
        let fingerprint =
            compute_fingerprint(&enabled_features, &profile, &toolchain, &build_flags);
        Ok(Self {
            enabled_features,
            profile,
            toolchain,
            build_flags,
            fingerprint,
        })
    }

    /// Combined JSON view used to populate the `cabin metadata`
    /// Configuration block.
    pub fn as_json(&self) -> serde_json::Value {
        let compiler_wrapper =
            self.toolchain
                .compiler_wrapper
                .as_ref()
                .map_or(serde_json::Value::Null, |w| {
                    let mut obj = serde_json::Map::new();
                    obj.insert("kind".to_owned(), serde_json::Value::String(w.kind.clone()));
                    obj.insert("spec".to_owned(), serde_json::Value::String(w.spec.clone()));
                    obj.insert(
                        "source".to_owned(),
                        serde_json::Value::String(w.source.clone()),
                    );
                    if let Some(v) = &w.version {
                        obj.insert("version".to_owned(), serde_json::Value::String(v.clone()));
                    }
                    serde_json::Value::Object(obj)
                });
        serde_json::json!({
            "features": self.enabled_features.iter().collect::<Vec<_>>(),
            "profile": self.profile.as_json(),
            "toolchain": {
                "tools": &self.toolchain.tools,
                "sources": &self.toolchain.sources,
                "compiler_wrapper": compiler_wrapper,
            },
            "build_flags": self.build_flags.as_json(),
            "fingerprint": self.fingerprint,
        })
    }
}

fn resolve_features(
    package: &str,
    features: &Features,
    request: &SelectionRequest,
) -> Result<BTreeSet<String>, ValidationError> {
    // Validate every requested name exists.
    for name in &request.features {
        if !features.features.contains_key(name) {
            return Err(ValidationError::UnknownFeature {
                package: package.to_owned(),
                feature: name.clone(),
            });
        }
    }

    let mut roots: BTreeSet<String> = BTreeSet::new();
    if request.all_features {
        for name in features.features.keys() {
            roots.insert(name.clone());
        }
    } else {
        if !request.no_default_features {
            for name in &features.default {
                roots.insert(name.clone());
            }
        }
        for name in &request.features {
            roots.insert(name.clone());
        }
    }
    Ok(features.expand(&roots))
}

fn bool_bytes(b: bool) -> &'static [u8] {
    if b { b"true" } else { b"false" }
}

fn compute_fingerprint(
    features: &BTreeSet<String>,
    profile: &ResolvedProfile,
    toolchain: &ToolchainSummary,
    build_flags: &ResolvedProfileFlags,
) -> String {
    // Hash a stable, line-based serialization rather than JSON so the
    // fingerprint is independent of serialiser whitespace choices.
    let mut hasher = Sha256::new();
    hasher.update(b"features\n");
    for f in features {
        hasher.update(f.as_bytes());
        hasher.update(b"\n");
    }
    hasher.update(b"profile\n");
    hasher.update(b"name=");
    hasher.update(profile.name.as_str().as_bytes());
    hasher.update(b"\n");
    hasher.update(b"debug=");
    hasher.update(bool_bytes(profile.debug));
    hasher.update(b"\n");
    hasher.update(b"opt-level=");
    hasher.update(profile.opt_level.as_str().as_bytes());
    hasher.update(b"\n");
    hasher.update(b"assertions=");
    hasher.update(bool_bytes(profile.assertions));
    hasher.update(b"\n");
    hasher.update(b"toolchain\n");
    for (kind, spec) in &toolchain.tools {
        hasher.update(kind.as_bytes());
        hasher.update(b"=");
        hasher.update(spec.as_bytes());
        hasher.update(b"\n");
    }
    hasher.update(b"compiler-wrapper\n");
    match &toolchain.compiler_wrapper {
        Some(wrapper) => {
            hasher.update(b"kind=");
            hasher.update(wrapper.kind.as_bytes());
            hasher.update(b"\n");
            hasher.update(b"spec=");
            hasher.update(wrapper.spec.as_bytes());
            hasher.update(b"\n");
            if let Some(version) = wrapper.version.as_deref() {
                hasher.update(b"version=");
                hasher.update(version.as_bytes());
                hasher.update(b"\n");
            }
        }
        None => {
            hasher.update(b"kind=none\n");
        }
    }
    hasher.update(b"build-flags\n");
    hasher.update(b"defines\n");
    for d in &build_flags.defines {
        hasher.update(d.as_bytes());
        hasher.update(b"\n");
    }
    hasher.update(b"include-dirs\n");
    for inc in &build_flags.include_dirs {
        hasher.update(inc.as_str().as_bytes());
        hasher.update(b"\n");
    }
    hasher.update(b"language-neutral-compile-args\n");
    for a in &build_flags.extra_compile_args {
        hasher.update(a.as_bytes());
        hasher.update(b"\n");
    }
    // The C-only and C++-only escape hatches change the
    // generated compile commands and the resulting object
    // contents, so they must move the fingerprint. Each section
    // is anchored by a labeled header so a future addition
    // (e.g. extra-asm-compile-args) cannot accidentally collide
    // with one of the existing buckets and produce the same
    // fingerprint as a different input.
    hasher.update(b"cflags\n");
    for a in &build_flags.cflags {
        hasher.update(a.as_bytes());
        hasher.update(b"\n");
    }
    hasher.update(b"cxxflags\n");
    for a in &build_flags.cxxflags {
        hasher.update(a.as_bytes());
        hasher.update(b"\n");
    }
    hasher.update(b"ldflags\n");
    for a in &build_flags.ldflags {
        hasher.update(a.as_bytes());
        hasher.update(b"\n");
    }
    crate::hash::hex_digest(&hasher.finalize())
}

/// Identifier grammar for feature names.
fn validate_identifier(name: &str) -> Result<(), ValidationError> {
    if name.is_empty() {
        return Err(ValidationError::EmptyConfigName("feature"));
    }
    let bad = name.chars().any(|c| {
        !(c.is_ascii_alphanumeric() || c == '_' || c == '-')
            || c.is_whitespace()
            || matches!(c, '/' | '.' | ':')
    });
    if bad {
        return Err(ValidationError::InvalidConfigName {
            kind: "feature",
            value: name.to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{
        ProfileDefinition, ProfileName, ProfileSelection, ResolvedProfile, resolve_profile,
    };
    use camino::Utf8PathBuf;

    fn dev() -> ResolvedProfile {
        resolve_profile(
            &ProfileSelection::default_dev(),
            &BTreeMap::<ProfileName, ProfileDefinition>::new(),
        )
        .expect("built-in dev resolves")
    }

    fn feats(default: &[&str], pairs: &[(&str, &[&str])]) -> Features {
        let mut features = BTreeMap::new();
        for (k, vs) in pairs {
            features.insert(
                (*k).to_owned(),
                vs.iter().map(|s| (*s).to_owned()).collect(),
            );
        }
        Features {
            default: default.iter().map(|s| (*s).to_owned()).collect(),
            features,
        }
    }

    #[test]
    fn features_validate_ok_for_simple_decls() {
        feats(&["simd"], &[("simd", &[]), ("ssl", &[])])
            .validate()
            .unwrap();
    }

    #[test]
    fn features_reject_reserved_default_key() {
        let mut f = feats(&[], &[]);
        f.features.insert("default".into(), vec![]);
        match f.validate().unwrap_err() {
            ValidationError::ReservedFeatureName(n) => assert_eq!(n, "default"),
            other => panic!("expected ReservedFeatureName, got {other:?}"),
        }
    }

    #[test]
    fn features_reject_unknown_default_reference() {
        match feats(&["nope"], &[("simd", &[])]).validate().unwrap_err() {
            ValidationError::UnknownFeatureReference { referenced, .. } => {
                assert_eq!(referenced, "nope");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn features_reject_internal_unknown_reference() {
        match feats(&[], &[("full", &["ssl"])]).validate().unwrap_err() {
            ValidationError::UnknownFeatureReference {
                referrer,
                referenced,
            } => {
                assert_eq!(referrer, "full");
                assert_eq!(referenced, "ssl");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn features_reject_cycles() {
        let f = feats(&[], &[("a", &["b"]), ("b", &["a"])]);
        match f.validate().unwrap_err() {
            ValidationError::FeatureCycle(cycle) => {
                assert!(cycle.iter().any(|n| n == "a"));
                assert!(cycle.iter().any(|n| n == "b"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn features_reject_invalid_name() {
        let f = feats(&[], &[("foo/bar", &[])]);
        match f.validate().unwrap_err() {
            ValidationError::InvalidConfigName { kind, value } => {
                assert_eq!(kind, "feature");
                assert_eq!(value, "foo/bar");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn features_expand_default_set() {
        let f = feats(
            &["full"],
            &[("simd", &[]), ("ssl", &[]), ("full", &["simd", "ssl"])],
        );
        f.validate().unwrap();
        let cfg = BuildConfiguration::resolve(BuildConfigurationInput {
            package: "demo",
            features: &f,
            request: &SelectionRequest::default(),
            profile: dev(),
            toolchain: ToolchainSummary::default(),
            build_flags: ResolvedProfileFlags::default(),
        })
        .unwrap();
        let v: Vec<&str> = cfg.enabled_features.iter().map(String::as_str).collect();
        assert_eq!(v, vec!["full", "simd", "ssl"]);
    }

    #[test]
    fn no_default_features_drops_defaults() {
        let f = feats(&["simd"], &[("simd", &[]), ("ssl", &[])]);
        f.validate().unwrap();
        let cfg = BuildConfiguration::resolve(BuildConfigurationInput {
            package: "demo",
            features: &f,
            request: &SelectionRequest {
                no_default_features: true,
                ..Default::default()
            },
            profile: dev(),
            toolchain: ToolchainSummary::default(),
            build_flags: ResolvedProfileFlags::default(),
        })
        .unwrap();
        assert!(cfg.enabled_features.is_empty());
    }

    #[test]
    fn explicit_features_are_added() {
        let f = feats(&[], &[("simd", &[]), ("ssl", &[])]);
        f.validate().unwrap();
        let mut req = SelectionRequest::default();
        req.features.insert("ssl".into());
        let cfg = BuildConfiguration::resolve(BuildConfigurationInput {
            package: "demo",
            features: &f,
            request: &req,
            profile: dev(),
            toolchain: ToolchainSummary::default(),
            build_flags: ResolvedProfileFlags::default(),
        })
        .unwrap();
        let v: Vec<&str> = cfg.enabled_features.iter().map(String::as_str).collect();
        assert_eq!(v, vec!["ssl"]);
    }

    #[test]
    fn all_features_enables_every_declared_feature() {
        let f = feats(&[], &[("simd", &[]), ("ssl", &[])]);
        f.validate().unwrap();
        let cfg = BuildConfiguration::resolve(BuildConfigurationInput {
            package: "demo",
            features: &f,
            request: &SelectionRequest {
                all_features: true,
                ..Default::default()
            },
            profile: dev(),
            toolchain: ToolchainSummary::default(),
            build_flags: ResolvedProfileFlags::default(),
        })
        .unwrap();
        let v: Vec<&str> = cfg.enabled_features.iter().map(String::as_str).collect();
        assert_eq!(v, vec!["simd", "ssl"]);
    }

    #[test]
    fn unknown_feature_in_request_errors() {
        let f = feats(&[], &[("simd", &[])]);
        let mut req = SelectionRequest::default();
        req.features.insert("missing".into());
        match BuildConfiguration::resolve(BuildConfigurationInput {
            package: "demo",
            features: &f,
            request: &req,
            profile: dev(),
            toolchain: ToolchainSummary::default(),
            build_flags: ResolvedProfileFlags::default(),
        })
        .unwrap_err()
        {
            ValidationError::UnknownFeature { feature, .. } => assert_eq!(feature, "missing"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn fingerprint_is_stable_for_same_inputs() {
        let f = feats(&["simd"], &[("simd", &[]), ("ssl", &[])]);
        f.validate().unwrap();
        let cfg1 = BuildConfiguration::resolve(BuildConfigurationInput {
            package: "demo",
            features: &f,
            request: &SelectionRequest::default(),
            profile: dev(),
            toolchain: ToolchainSummary::default(),
            build_flags: ResolvedProfileFlags::default(),
        })
        .unwrap();
        let cfg2 = BuildConfiguration::resolve(BuildConfigurationInput {
            package: "demo",
            features: &f,
            request: &SelectionRequest::default(),
            profile: dev(),
            toolchain: ToolchainSummary::default(),
            build_flags: ResolvedProfileFlags::default(),
        })
        .unwrap();
        assert_eq!(cfg1.fingerprint, cfg2.fingerprint);
        assert_eq!(cfg1.fingerprint.len(), 64);
    }

    #[test]
    fn fingerprint_differs_when_features_change() {
        let f = feats(&[], &[("simd", &[]), ("ssl", &[])]);
        f.validate().unwrap();
        let mut req = SelectionRequest::default();
        let cfg_empty = BuildConfiguration::resolve(BuildConfigurationInput {
            package: "demo",
            features: &f,
            request: &req,
            profile: dev(),
            toolchain: ToolchainSummary::default(),
            build_flags: ResolvedProfileFlags::default(),
        })
        .unwrap();
        req.features.insert("simd".into());
        let cfg_simd = BuildConfiguration::resolve(BuildConfigurationInput {
            package: "demo",
            features: &f,
            request: &req,
            profile: dev(),
            toolchain: ToolchainSummary::default(),
            build_flags: ResolvedProfileFlags::default(),
        })
        .unwrap();
        assert_ne!(cfg_empty.fingerprint, cfg_simd.fingerprint);
    }
    /// Helper: resolve a `BuildConfiguration` with the supplied
    /// build flags. Every other input is the boring default so
    /// the only difference between two calls is the `flags` arg
    /// — used for the fingerprint-input regression tests below.
    fn resolve_with_flags(flags: ResolvedProfileFlags) -> BuildConfiguration {
        BuildConfiguration::resolve(BuildConfigurationInput {
            package: "demo",
            features: &Features::default(),
            request: &SelectionRequest::default(),
            profile: dev(),
            toolchain: ToolchainSummary::default(),
            build_flags: flags,
        })
        .unwrap()
    }

    #[test]
    fn fingerprint_differs_when_defines_change() {
        let baseline = resolve_with_flags(ResolvedProfileFlags::default());
        let added = resolve_with_flags(ResolvedProfileFlags {
            defines: vec!["FOO=1".to_owned()],
            ..ResolvedProfileFlags::default()
        });
        assert_ne!(baseline.fingerprint, added.fingerprint);
    }

    #[test]
    fn fingerprint_differs_when_include_dirs_change() {
        let baseline = resolve_with_flags(ResolvedProfileFlags::default());
        let added = resolve_with_flags(ResolvedProfileFlags {
            include_dirs: vec![Utf8PathBuf::from("include")],
            ..ResolvedProfileFlags::default()
        });
        assert_ne!(baseline.fingerprint, added.fingerprint);
    }

    #[test]
    fn fingerprint_differs_when_extra_compile_args_change() {
        let baseline = resolve_with_flags(ResolvedProfileFlags::default());
        let added = resolve_with_flags(ResolvedProfileFlags {
            extra_compile_args: vec!["-Wall".to_owned()],
            ..ResolvedProfileFlags::default()
        });
        assert_ne!(baseline.fingerprint, added.fingerprint);
    }

    #[test]
    fn fingerprint_differs_when_cflags_change() {
        // The per-language escape hatches must each contribute
        // their own fingerprint bucket. A C compile command's
        // argv changes when this slot changes, which means the
        // resulting `.o` bytes can change too — a future on-disk
        // artifact cache *must* see a different fingerprint or it
        // would silently reuse a stale object. The fingerprint
        // must move.
        let baseline = resolve_with_flags(ResolvedProfileFlags::default());
        let added = resolve_with_flags(ResolvedProfileFlags {
            cflags: vec!["-std=c99".to_owned()],
            ..ResolvedProfileFlags::default()
        });
        assert_ne!(baseline.fingerprint, added.fingerprint);
    }

    #[test]
    fn fingerprint_differs_when_cxxflags_change() {
        // Mirror of the C-only test: a C++ compile command's
        // argv must move the fingerprint too.
        let baseline = resolve_with_flags(ResolvedProfileFlags::default());
        let added = resolve_with_flags(ResolvedProfileFlags {
            cxxflags: vec!["-fno-rtti".to_owned()],
            ..ResolvedProfileFlags::default()
        });
        assert_ne!(baseline.fingerprint, added.fingerprint);
    }

    #[test]
    fn fingerprint_distinguishes_c_only_from_cxx_only_extra_args() {
        // Belt-and-suspenders: putting the *same* flag string in
        // the C-only slot vs. the C++-only slot must produce
        // different fingerprints because the two slots route to
        // different compile commands. Without this guarantee,
        // future cache logic could accidentally serve a C-only
        // object for a C++-only request that happens to share an
        // argv string.
        let c_only = resolve_with_flags(ResolvedProfileFlags {
            cflags: vec!["-Wsome-warning".to_owned()],
            ..ResolvedProfileFlags::default()
        });
        let cxx_only = resolve_with_flags(ResolvedProfileFlags {
            cxxflags: vec!["-Wsome-warning".to_owned()],
            ..ResolvedProfileFlags::default()
        });
        assert_ne!(c_only.fingerprint, cxx_only.fingerprint);
    }

    #[test]
    fn fingerprint_differs_when_ldflags_change() {
        let baseline = resolve_with_flags(ResolvedProfileFlags::default());
        let added = resolve_with_flags(ResolvedProfileFlags {
            ldflags: vec!["-Wl,--as-needed".to_owned()],
            ..ResolvedProfileFlags::default()
        });
        assert_ne!(baseline.fingerprint, added.fingerprint);
    }

    #[test]
    fn fingerprint_is_stable_for_same_build_flags() {
        // Determinism: identical inputs produce identical
        // fingerprints. The fingerprint serialiser sorts every
        // map / set; this test pins that contract.
        let flags = ResolvedProfileFlags {
            defines: vec!["FOO=1".to_owned(), "BAR=2".to_owned()],
            include_dirs: vec![
                Utf8PathBuf::from("include"),
                Utf8PathBuf::from("vendor/include"),
            ],
            extra_compile_args: vec!["-Wall".to_owned()],
            cflags: vec!["-std=c99".to_owned()],
            cxxflags: vec!["-fno-rtti".to_owned()],
            ldflags: vec!["-Wl,--as-needed".to_owned()],
        };
        let a = resolve_with_flags(flags.clone());
        let b = resolve_with_flags(flags);
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.fingerprint.len(), 64, "sha256 hex digest is 64 chars");
    }

    fn release() -> ResolvedProfile {
        use crate::profile::{ProfileDefinition, ProfileName, ProfileSelection, resolve_profile};
        resolve_profile(
            &ProfileSelection::release_alias(),
            &BTreeMap::<ProfileName, ProfileDefinition>::new(),
        )
        .expect("built-in release resolves")
    }

    #[test]
    fn fingerprint_differs_when_profile_changes() {
        let dev_cfg = BuildConfiguration::resolve(BuildConfigurationInput {
            package: "demo",
            features: &Features::default(),
            request: &SelectionRequest::default(),
            profile: dev(),
            toolchain: ToolchainSummary::default(),
            build_flags: ResolvedProfileFlags::default(),
        })
        .unwrap();
        let release_cfg = BuildConfiguration::resolve(BuildConfigurationInput {
            package: "demo",
            features: &Features::default(),
            request: &SelectionRequest::default(),
            profile: release(),
            toolchain: ToolchainSummary::default(),
            build_flags: ResolvedProfileFlags::default(),
        })
        .unwrap();
        // Built-in dev and release differ in opt-level, debug,
        // assertions, and name — every field participates in
        // the fingerprint, so the digest must move.
        assert_ne!(dev_cfg.fingerprint, release_cfg.fingerprint);
    }

    #[test]
    fn fingerprint_differs_when_toolchain_summary_changes() {
        let mut tc_a = ToolchainSummary::default();
        tc_a.tools.insert("cxx".to_owned(), "g++".to_owned());
        let mut tc_b = ToolchainSummary::default();
        tc_b.tools.insert("cxx".to_owned(), "clang++".to_owned());
        let cfg_a = BuildConfiguration::resolve(BuildConfigurationInput {
            package: "demo",
            features: &Features::default(),
            request: &SelectionRequest::default(),
            profile: dev(),
            toolchain: tc_a,
            build_flags: ResolvedProfileFlags::default(),
        })
        .unwrap();
        let cfg_b = BuildConfiguration::resolve(BuildConfigurationInput {
            package: "demo",
            features: &Features::default(),
            request: &SelectionRequest::default(),
            profile: dev(),
            toolchain: tc_b,
            build_flags: ResolvedProfileFlags::default(),
        })
        .unwrap();
        assert_ne!(cfg_a.fingerprint, cfg_b.fingerprint);
    }

    #[test]
    fn fingerprint_differs_when_compiler_wrapper_changes() {
        let no_wrapper = ToolchainSummary::default();
        let with_wrapper = ToolchainSummary {
            compiler_wrapper: Some(CompilerWrapperSummary {
                kind: "ccache".into(),
                spec: "ccache".into(),
                source: "cli".into(),
                version: Some("4.8.0".into()),
            }),
            ..ToolchainSummary::default()
        };
        let cfg_a = BuildConfiguration::resolve(BuildConfigurationInput {
            package: "demo",
            features: &Features::default(),
            request: &SelectionRequest::default(),
            profile: dev(),
            toolchain: no_wrapper,
            build_flags: ResolvedProfileFlags::default(),
        })
        .unwrap();
        let cfg_b = BuildConfiguration::resolve(BuildConfigurationInput {
            package: "demo",
            features: &Features::default(),
            request: &SelectionRequest::default(),
            profile: dev(),
            toolchain: with_wrapper,
            build_flags: ResolvedProfileFlags::default(),
        })
        .unwrap();
        assert_ne!(cfg_a.fingerprint, cfg_b.fingerprint);
    }
}
