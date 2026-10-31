//! Typed model for build-backend parallelism.
//!
//! The job count for the build backend (currently Ninja) comes from
//! `-j` / `--jobs <N>`, the `CABIN_BUILD_JOBS` environment variable, or
//! the `[build] jobs` config key.  Every layer's raw input parses
//! through this module, so every consumer sees the same validated value.
//!
//! Crate boundaries: the type lives in `cabin-core` because several
//! crates carry it (config, CLI, `cabin-tidy`).  Conversion of
//! [`BuildJobs`] into a tool's argv fragment belongs to the call site
//! that spawns that tool.

use std::num::NonZeroU32;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Validated build-parallelism setting.
///
/// Wraps a `NonZeroU32`, so no consumer observes a zero count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildJobs(NonZeroU32);

impl BuildJobs {
    /// Build a [`BuildJobs`] from a raw `u32`.
    ///
    /// # Errors
    /// Returns [`BuildJobsParseError::Zero`] when `value` is `0`.
    pub fn new(value: u32) -> Result<Self, BuildJobsParseError> {
        NonZeroU32::new(value)
            .map(Self)
            .ok_or(BuildJobsParseError::Zero)
    }

    /// Underlying job count.
    pub fn get(self) -> u32 {
        self.0.get()
    }
}

impl FromStr for BuildJobs {
    type Err = BuildJobsParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(BuildJobsParseError::Empty);
        }
        // `u32::from_str` rejects negative and non-numeric values.
        // The error carries the untrimmed input so the diagnostic
        // quotes what the user wrote.
        let parsed: u32 = trimmed.parse().map_err(|_| BuildJobsParseError::Invalid {
            value: s.to_owned(),
        })?;
        Self::new(parsed)
    }
}

impl std::fmt::Display for BuildJobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Reasons [`BuildJobs::from_str`] / [`BuildJobs::new`] reject
/// an input.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BuildJobsParseError {
    /// Empty / whitespace-only string.
    #[error("expected a positive integer, got an empty value")]
    Empty,

    /// Numeric `0`.
    #[error("expected a positive integer, got 0")]
    Zero,

    /// Non-numeric or out-of-range value.
    #[error("invalid jobs value {value:?}; expected a positive integer")]
    Invalid {
        /// The offending input as the user wrote it.
        value: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_positive_integers() {
        let one = BuildJobs::from_str("1").unwrap();
        assert_eq!(one.get(), 1);
        let many = BuildJobs::from_str("64").unwrap();
        assert_eq!(many.get(), 64);
    }

    #[test]
    fn rejects_zero() {
        assert_eq!(BuildJobs::from_str("0"), Err(BuildJobsParseError::Zero));
        assert_eq!(BuildJobs::new(0), Err(BuildJobsParseError::Zero));
    }

    #[test]
    fn rejects_non_numeric() {
        match BuildJobs::from_str("many") {
            Err(BuildJobsParseError::Invalid { value }) => assert_eq!(value, "many"),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(BuildJobs::from_str(""), Err(BuildJobsParseError::Empty));
        assert_eq!(BuildJobs::from_str("   "), Err(BuildJobsParseError::Empty));
    }

    #[test]
    fn trims_surrounding_whitespace() {
        let parsed = BuildJobs::from_str(" 4 ").unwrap();
        assert_eq!(parsed.get(), 4);
    }
}
