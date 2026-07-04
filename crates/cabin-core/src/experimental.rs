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

use std::fmt;

/// One recognized experimental feature, as accepted by `-Z`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExperimentalFeature {
    /// Post-resolution language-standard compatibility checking:
    /// evaluate the edge-compatibility model of
    /// `docs/design/standard-compatibility/spec.md` over the
    /// resolved target graph and report violated edges as errors
    /// that fail the command (warnings under the temporary
    /// `[build] standard-compat-errors = false` config migration
    /// switch; unchecked-edge notes under a per-edge
    /// `ignore-interface-standard = true` override).  Never
    /// influences version selection.
    StandardCompat,
}

impl ExperimentalFeature {
    /// Every recognized feature, in the order the parse error
    /// lists them.
    pub const ALL: [Self; 1] = [Self::StandardCompat];

    /// Stable kebab-case spelling, exactly what `-Z` accepts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StandardCompat => "standard-compat",
        }
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
/// unknown experimental feature 'frobnicate'; expected one of: standard-compat
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownExperimentalFeature {
    /// The raw, invalid value as the user provided it.
    pub value: String,
}

impl fmt::Display for UnknownExperimentalFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown experimental feature '{}'; expected one of: {}",
            self.value,
            ExperimentalFeature::ALL
                .map(ExperimentalFeature::as_str)
                .join(", ")
        )
    }
}

impl std::error::Error for UnknownExperimentalFeature {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_round_trips_through_from_str() {
        for feature in ExperimentalFeature::ALL {
            assert_eq!(
                feature.as_str().parse::<ExperimentalFeature>().unwrap(),
                feature,
                "{feature:?} did not round-trip"
            );
        }
    }

    #[test]
    fn unknown_value_lists_recognized_features() {
        let err = "frobnicate".parse::<ExperimentalFeature>().unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown experimental feature 'frobnicate'; expected one of: standard-compat"
        );
    }

    #[test]
    fn parsing_does_not_normalize_case() {
        assert!("Standard-Compat".parse::<ExperimentalFeature>().is_err());
        assert!("STANDARD-COMPAT".parse::<ExperimentalFeature>().is_err());
    }

    #[test]
    fn display_matches_as_str() {
        assert_eq!(
            ExperimentalFeature::StandardCompat.to_string(),
            "standard-compat"
        );
    }
}
