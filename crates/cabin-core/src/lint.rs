//! Lint-tool settings declared in `cabin.toml`.
//!
//! Today the only lint tool with manifest support is
//! `cpplint`, exposed under the `[lint.cpplint]` table.  The
//! shape is split into a top-level [`LintSettings`] holding
//! every per-tool sub-table so future lint tools can land
//! without changing the public field path of every existing
//! consumer.
//!
//! These types live in `cabin-core` so the workspace graph,
//! metadata view, and (eventually) lint planner all see one
//! validated representation.  Manifest parsing lives in
//! `cabin-manifest`; lint orchestration lives in
//! `cabin-lint` / `cabin-cli`'s lint glue.

use serde::{Deserialize, Serialize};

/// Lint-tool settings declared in a package's `cabin.toml`.
///
/// One sub-table per supported lint tool.  Adding a new tool
/// is a deliberate change — the model layer must extend
/// before the manifest parser and the orchestration layer can
/// see it — so the field set here is intentionally small.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LintSettings {
    /// `[lint.cpplint]` settings.  Defaults are "no
    /// manifest-provided cpplint configuration"; users may
    /// still rely on `CPPLINT.cfg` discovery from cpplint
    /// itself.
    #[serde(default, skip_serializing_if = "CpplintLintSettings::is_empty")]
    pub cpplint: CpplintLintSettings,
}

impl LintSettings {
    /// Whether the settings carry no manifest-provided values.
    /// Used by `Project`'s `skip_serializing_if` to keep
    /// older manifests' metadata round-trips byte-stable.
    pub fn is_empty(&self) -> bool {
        self.cpplint.is_empty()
    }
}

/// `[lint.cpplint]` settings.
///
/// Only `filters` is supported today; the field is preserved
/// in declaration order so a `-foo` entry that precedes a
/// `+foo` entry behaves the way the user wrote it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpplintLintSettings {
    /// `--filter=...` entries Cabin passes to cpplint.  The
    /// list is taken verbatim — Cabin does not normalise or
    /// dedupe entries because cpplint's filter grammar is
    /// position-sensitive.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<String>,
}

impl CpplintLintSettings {
    /// Whether the settings carry no manifest-provided values.
    pub fn is_empty(&self) -> bool {
        self.filters.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty() {
        let s = LintSettings::default();
        assert!(s.is_empty());
        assert!(s.cpplint.is_empty());
    }

    #[test]
    fn cpplint_with_filters_is_not_empty() {
        let s = LintSettings {
            cpplint: CpplintLintSettings {
                filters: vec!["-build/c++11".into()],
            },
        };
        assert!(!s.is_empty());
        assert!(!s.cpplint.is_empty());
    }

    #[test]
    fn round_trips_through_json() {
        let s = LintSettings {
            cpplint: CpplintLintSettings {
                filters: vec!["-build/c++11".into(), "-whitespace/braces".into()],
            },
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: LintSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
