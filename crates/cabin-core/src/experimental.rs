//! Typed model for Cabin's experimental (unstable) feature gates.
//!
//! Experimental features are opt-in behaviors that may change or
//! disappear between releases.  The user enables one per invocation
//! with the global `-Z <feature>` CLI flag; nothing here persists
//! into manifests, lockfiles, or config files.  The enum lives in
//! `cabin-core` so the CLI parser and every crate that gates a pass
//! on a feature share one name list and one error wording.
//!
//! The registry is currently empty - no feature is gated behind
//! `-Z` today, so every `-Z <value>` is rejected as unknown.  Adding
//! a feature means adding a variant, its `as_str` spelling, and an
//! `ALL` entry; the parser and its error message follow from those.

use std::fmt;

/// One recognized experimental feature, as accepted by `-Z`.
///
/// Uninhabited while no experimental feature is registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExperimentalFeature {}

impl ExperimentalFeature {
    /// Every recognized feature, in the order the parse error
    /// lists them.
    pub const ALL: [Self; 0] = [];

    /// Stable kebab-case spelling, exactly what `-Z` accepts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {}
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
/// wording; with no feature registered it reads:
///
/// ```text
/// unknown experimental feature 'frobnicate'; no experimental features are currently recognized
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
        if recognized.is_empty() {
            write!(
                f,
                "unknown experimental feature '{}'; no experimental features are currently \
                 recognized",
                self.value,
            )
        } else {
            write!(
                f,
                "unknown experimental feature '{}'; expected one of: {recognized}",
                self.value,
            )
        }
    }
}

impl std::error::Error for UnknownExperimentalFeature {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_value_reports_no_recognized_features() {
        // The registry is empty, so every value - including the
        // removed `standard-compat` name - is unknown and reports
        // the same no-features wording.
        for value in ["frobnicate", "standard-compat"] {
            let err = value.parse::<ExperimentalFeature>().unwrap_err();
            assert_eq!(
                err.to_string(),
                format!(
                    "unknown experimental feature '{value}'; no experimental features are \
                     currently recognized"
                ),
            );
        }
    }
}
