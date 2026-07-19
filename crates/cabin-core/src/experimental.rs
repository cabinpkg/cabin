//! Typed model for Cabin's experimental (unstable) feature gates.
//!
//! Experimental features are opt-in behaviors that may change or
//! disappear between releases.  The user enables one per invocation
//! with the global `-Z <feature>` CLI flag; nothing here persists
//! into manifests, lockfiles, or config files.  The enum lives in
//! `cabin-core` so the CLI parser and every crate that gates a pass
//! on a feature share one name list and one error wording.
//!
//! Adding a feature means adding a variant, its `as_str` spelling,
//! and an `ALL` entry; the parser and its error message follow from
//! those.

use std::collections::BTreeSet;
use std::fmt;

/// One recognized experimental feature, as accepted by `-Z`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExperimentalFeature {
    /// `-Z remote-registry`: the experimental remote-registry
    /// client.  Gates the `auth-required` / `api` registry
    /// `config.json` fields; see `docs/remote-registry.md`.
    RemoteRegistry,
}

impl ExperimentalFeature {
    /// Every recognized feature, in the order the parse error
    /// lists them.
    pub const ALL: [Self; 1] = [Self::RemoteRegistry];

    /// Stable kebab-case spelling, exactly what `-Z` accepts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RemoteRegistry => "remote-registry",
        }
    }
}

/// The set of experimental features enabled for one CLI invocation.
///
/// Built once from the parsed `-Z` occurrences and threaded through
/// command contexts so downstream crates can ask "is this feature
/// enabled" without re-parsing argv.  The default value has every
/// feature disabled - the fail-closed baseline for callers with no
/// CLI surface (tests, library use).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExperimentalFeatures(BTreeSet<ExperimentalFeature>);

impl ExperimentalFeatures {
    /// Whether `feature` was enabled for this invocation.
    #[must_use]
    pub fn is_enabled(&self, feature: ExperimentalFeature) -> bool {
        self.0.contains(&feature)
    }
}

impl FromIterator<ExperimentalFeature> for ExperimentalFeatures {
    fn from_iter<I: IntoIterator<Item = ExperimentalFeature>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl fmt::Display for ExperimentalFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ExperimentalFeature {
    type Err = UnknownExperimentalFeature;

    // Exact-match parsing, mirroring the other typed CLI/env value
    // parsers in this crate: no case folding, no trimming.
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|feature| feature.as_str() == raw)
            .ok_or_else(|| UnknownExperimentalFeature {
                value: raw.to_owned(),
            })
    }
}

/// Error returned when a `-Z` value names no recognized
/// experimental feature.  The `Display` impl is the user-visible
/// wording:
///
/// ```text
/// unknown experimental feature 'frobnicate'; expected one of: remote-registry
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownExperimentalFeature {
    /// The raw, invalid value as the user provided it.
    pub value: String,
}

impl fmt::Display for UnknownExperimentalFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let recognized = ExperimentalFeature::ALL
            .map(ExperimentalFeature::as_str)
            .join(", ");
        write!(
            f,
            "unknown experimental feature '{}'; expected one of: {recognized}",
            self.value,
        )
    }
}

impl std::error::Error for UnknownExperimentalFeature {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_feature_round_trips_through_from_str() {
        for feature in ExperimentalFeature::ALL {
            assert_eq!(feature.as_str().parse::<ExperimentalFeature>(), Ok(feature));
        }
    }

    #[test]
    fn unknown_value_lists_recognized_features() {
        // Every unknown value - including the removed
        // `standard-compat` name - reports the same wording, naming
        // the full recognized list.
        for value in ["frobnicate", "standard-compat"] {
            let err = value.parse::<ExperimentalFeature>().unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("unknown experimental feature '{value}'; expected one of: remote-registry"),
            );
        }
    }

    #[test]
    fn feature_set_defaults_to_disabled() {
        let none = ExperimentalFeatures::default();
        assert!(!none.is_enabled(ExperimentalFeature::RemoteRegistry));
        let set: ExperimentalFeatures = [ExperimentalFeature::RemoteRegistry].into_iter().collect();
        assert!(set.is_enabled(ExperimentalFeature::RemoteRegistry));
    }
}
