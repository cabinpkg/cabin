//! Typed source-replacement model.
//!
//! A *source replacement* redirects one supported index source
//! to another supported index source for the duration of one
//! Cabin invocation.  The mapping is local config policy - it
//! never enters published package metadata, never affects the
//! resolver for downstream consumers, and only swaps existing
//! source kinds (local filesystem index, sparse-HTTP index).
//!
//! Public syntax (config-only):
//!
//! ```toml
//! [source-replacement]
//! "https://example.com/index" = { index-path = "../mirror" }
//! ```
//!
//! The parser converts the table into a [`SourceReplacementSettings`]
//! collection with stable ordering.  Resolution walks the chain
//! once with cycle detection so a misconfigured chain like
//! `A -> B -> A` surfaces a clear error before the resolver
//! ever opens an index.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use camino::Utf8PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ConfigValueSource;

/// Stable, typed identifier for one supported source/index.
///
/// Keeping this enum closed (instead of stringly-typed `(kind,
/// value)` pairs) means every consumer - resolver, lockfile,
/// metadata view - agrees on what each variant means and which
/// data it carries.  New supported kinds extend the enum
/// explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SourceLocator {
    /// Local filesystem index.  Carries the path verbatim; the
    /// orchestration layer absolutises against the declaring
    /// file's directory before consulting the index loader.
    IndexPath { path: Utf8PathBuf },
    /// Sparse-HTTP index.  Carries the URL verbatim; the
    /// orchestration layer rejects credential-bearing URLs at
    /// parse time so credentials never leak into the
    /// effective configuration.
    IndexUrl { url: String },
}

impl SourceLocator {
    /// Stable lower-case label used for metadata + lockfile
    /// output.  Matches the serde `kind` tag.
    pub fn kind_key(&self) -> &'static str {
        match self {
            SourceLocator::IndexPath { .. } => "index-path",
            SourceLocator::IndexUrl { .. } => "index-url",
        }
    }

    /// Stable display string the user can recognize in errors
    /// and metadata output.
    pub fn display(&self) -> String {
        match self {
            SourceLocator::IndexPath { path } => path.as_str().to_owned(),
            SourceLocator::IndexUrl { url } => url.clone(),
        }
    }
}

impl fmt::Display for SourceLocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

/// One source-replacement declaration.  The orchestration layer
/// folds `Vec<SourceReplacementEntry>` into a
/// [`SourceReplacementSettings`] map keyed by `original` so
/// duplicates can be rejected deterministically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceReplacementEntry {
    pub original: SourceLocator,
    pub replacement: SourceLocator,
    /// Provenance label used by `cabin metadata`.  Always a
    /// config-flavor variant - source replacements live in the
    /// config layer.
    pub provenance: ConfigValueSource,
}

/// Collection of source-replacement entries plus typed
/// resolution / cycle detection.
///
/// Built by `cabin-config`'s merger from the highest-priority
/// config file's `[source-replacement]` table; lower-priority
/// files contribute additional entries when their `original`
/// key is not already covered, so the resulting map preserves
/// the same "higher level overrides" semantics the rest of the
/// config layer uses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceReplacementSettings {
    /// `(original -> entry)` keyed by the source being
    /// replaced.  `BTreeMap` keeps iteration deterministic for
    /// metadata + lockfile serialization.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entries: BTreeMap<SourceLocator, SourceReplacementEntry>,
}

impl SourceReplacementSettings {
    /// Whether the table carries no entries.  Used by the
    /// workspace loader / metadata view to skip emitting empty
    /// blocks.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Resolve `initial` through the replacement chain.  Returns
    /// the terminal source plus the chain of intermediate
    /// originals (in walk order) so the lockfile / metadata view
    /// can record the full hop list.
    ///
    /// Cycles surface a [`SourceReplacementError::Cycle`]
    /// carrying the offending hop list so users see exactly
    /// which entries form the loop.
    ///
    /// # Errors
    /// Returns [`SourceReplacementError::Cycle`] when the replacement chain
    /// revisits a source, carrying the hop list up to and including the
    /// repeated entry.
    pub fn resolve(
        &self,
        initial: &SourceLocator,
    ) -> Result<SourceReplacementResolution, SourceReplacementError> {
        let mut current = initial.clone();
        let mut visited: BTreeSet<SourceLocator> = BTreeSet::new();
        let mut hops: Vec<SourceLocator> = Vec::new();
        loop {
            if !visited.insert(current.clone()) {
                hops.push(current);
                return Err(SourceReplacementError::Cycle { hops });
            }
            let Some(entry) = self.entries.get(&current) else {
                return Ok(SourceReplacementResolution {
                    resolved: current,
                    hops,
                });
            };
            hops.push(entry.original.clone());
            current = entry.replacement.clone();
        }
    }

    /// Whether the supplied `original` source has a replacement
    /// declared.  Useful when the orchestration layer wants to
    /// know if applying replacement changed anything (so the
    /// metadata / lockfile view can show "unchanged" cleanly).
    pub fn replaces(&self, original: &SourceLocator) -> bool {
        self.entries.contains_key(original)
    }
}

/// Result of walking the replacement chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceReplacementResolution {
    /// Terminal source (the value the caller should
    /// open).  Equals the `initial` argument when no replacement
    /// applied.
    pub resolved: SourceLocator,
    /// Every `original` Cabin walked through, in order.  Empty
    /// when `initial` was already terminal.
    pub hops: Vec<SourceLocator>,
}

/// Errors produced while parsing / resolving source
/// replacements.  Wording is stable so integration tests can
/// match substrings.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SourceReplacementError {
    /// `replace-with` (or the inline `index-path` /
    /// `index-url`) was missing - every entry must declare a
    /// replacement.
    #[error(
        "source replacement for `{original}` is missing a replacement; expected `index-path = \"...\"` or `index-url = \"...\"`"
    )]
    MissingReplacement { original: String },

    /// Both `index-path` and `index-url` were declared on the
    /// same entry.  A single replacement entry may only redirect
    /// to one source.
    #[error(
        "source replacement for `{original}` declares both `index-path` and `index-url`; pick exactly one"
    )]
    AmbiguousReplacement { original: String },

    /// A URL (either the original or the replacement) carried
    /// `userinfo` (e.g., `https://user:pass@example.com/...`).
    /// Cabin's source-replacement model does not handle
    /// credentials, so a URL with `userinfo` is rejected before
    /// it can flow into log output or the lockfile.  The `url`
    /// field is expected to be redacted (`***` in place of
    /// userinfo) by the constructor so error rendering never
    /// echoes the secret back to stderr / logs.
    #[error("source replacement URL `{url}` must not contain credentials")]
    CredentialsInUrl { url: String },

    /// The same `original` key appears in two replacement
    /// declarations at the same precedence level.
    #[error(
        "multiple source replacements for `{original}` are active at the same precedence level; remove one declaration"
    )]
    DuplicateAtSameLevel { original: String },

    /// A replacement chain looped back to a previously-visited
    /// source.
    #[error("source replacement cycle detected: {chain}", chain = format_chain(hops))]
    Cycle { hops: Vec<SourceLocator> },
}

fn format_chain(hops: &[SourceLocator]) -> String {
    let mut chain = String::new();
    for (idx, hop) in hops.iter().enumerate() {
        if idx > 0 {
            chain.push_str(" -> ");
        }
        chain.push_str(&hop.display());
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(original: SourceLocator, replacement: SourceLocator) -> SourceReplacementEntry {
        SourceReplacementEntry {
            original,
            replacement,
            provenance: ConfigValueSource::WorkspaceConfig,
        }
    }

    fn url(s: &str) -> SourceLocator {
        SourceLocator::IndexUrl { url: s.to_owned() }
    }

    fn path(s: &str) -> SourceLocator {
        SourceLocator::IndexPath {
            path: Utf8PathBuf::from(s),
        }
    }

    #[test]
    fn resolve_passes_terminal_source_through_unchanged() {
        let settings = SourceReplacementSettings::default();
        let target = url("https://example.com/index");
        let res = settings.resolve(&target).unwrap();
        assert_eq!(res.resolved, target);
        assert!(res.hops.is_empty());
    }

    #[test]
    fn resolve_walks_a_single_hop() {
        let mut settings = SourceReplacementSettings::default();
        let original = url("https://example.com/index");
        let replacement = path("../mirror");
        settings.entries.insert(
            original.clone(),
            entry(original.clone(), replacement.clone()),
        );
        let res = settings.resolve(&original).unwrap();
        assert_eq!(res.resolved, replacement);
        assert_eq!(res.hops, vec![original]);
    }

    #[test]
    fn resolve_walks_a_chain_until_terminal() {
        let mut settings = SourceReplacementSettings::default();
        let a = url("https://example.com/a");
        let b = url("https://example.com/b");
        let c = path("../local");
        settings
            .entries
            .insert(a.clone(), entry(a.clone(), b.clone()));
        settings
            .entries
            .insert(b.clone(), entry(b.clone(), c.clone()));
        let res = settings.resolve(&a).unwrap();
        assert_eq!(res.resolved, c);
        assert_eq!(res.hops, vec![a, b]);
    }

    #[test]
    fn resolve_rejects_two_hop_cycle() {
        let mut settings = SourceReplacementSettings::default();
        let a = url("https://example.com/a");
        let b = url("https://example.com/b");
        settings
            .entries
            .insert(a.clone(), entry(a.clone(), b.clone()));
        settings.entries.insert(b.clone(), entry(b, a.clone()));
        let err = settings.resolve(&a).unwrap_err();
        match err {
            SourceReplacementError::Cycle { hops } => {
                let display: Vec<String> = hops.iter().map(SourceLocator::display).collect();
                assert_eq!(
                    display,
                    vec![
                        "https://example.com/a".to_owned(),
                        "https://example.com/b".to_owned(),
                        "https://example.com/a".to_owned(),
                    ]
                );
            }
            other => panic!("expected Cycle, got {other:?}"),
        }
    }

    #[test]
    fn resolve_detects_self_loop() {
        let mut settings = SourceReplacementSettings::default();
        let a = url("https://example.com/a");
        settings
            .entries
            .insert(a.clone(), entry(a.clone(), a.clone()));
        let err = settings.resolve(&a).unwrap_err();
        assert!(matches!(err, SourceReplacementError::Cycle { .. }));
    }

    #[test]
    fn replaces_returns_true_only_for_declared_originals() {
        let mut settings = SourceReplacementSettings::default();
        let a = url("https://example.com/a");
        let b = path("/mirror");
        settings
            .entries
            .insert(a.clone(), entry(a.clone(), b.clone()));
        assert!(settings.replaces(&a));
        assert!(!settings.replaces(&b));
    }

    #[test]
    fn locator_kind_keys_round_trip_through_serde() {
        let path_locator = path("../mirror");
        let url_locator = url("https://example.com/index");
        for locator in [path_locator, url_locator] {
            let json = serde_json::to_string(&locator).unwrap();
            let echoed: SourceLocator = serde_json::from_str(&json).unwrap();
            assert_eq!(echoed, locator);
        }
    }
}
