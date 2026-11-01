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

use camino::{Utf8Path, Utf8PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ConfigValueSource;

/// Stable, typed identifier for one supported source/index.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SourceLocator {
    /// Local filesystem index.  A `[source-replacement]` target
    /// carries the path as written until
    /// [`SourceReplacementSettings::resolve`] joins it onto its
    /// declaring config file's directory.
    IndexPath { path: Utf8PathBuf },
    /// Sparse-HTTP index.  Carries the URL verbatim; `cabin-config`
    /// rejects credential-bearing URLs at parse time so credentials
    /// never leak into the effective configuration.
    IndexUrl { url: String },
}

impl SourceLocator {
    /// Stable lower-case label used in metadata output.  Matches
    /// the serde `kind` tag.
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

/// One source-replacement declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceReplacementEntry {
    pub original: SourceLocator,
    pub replacement: SourceLocator,
    /// Provenance label used by `cabin metadata`.  Always a
    /// config-flavor variant - source replacements live in the
    /// config layer.
    pub provenance: ConfigValueSource,
    /// Directory of the config file that declared this entry.
    pub base: Utf8PathBuf,
}

/// Collection of source-replacement entries plus typed
/// resolution / cycle detection.
///
/// Built by `cabin-config`'s merger: for each `original`, the
/// highest-precedence config file's entry wins, as for every other
/// config setting.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceReplacementSettings {
    /// `(original -> entry)` keyed by the source being
    /// replaced.  `BTreeMap` keeps iteration deterministic for
    /// metadata + lockfile serialization.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entries: BTreeMap<SourceLocator, SourceReplacementEntry>,
}

impl SourceReplacementSettings {
    /// Whether the table carries no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Resolve `initial` through the replacement chain.  Returns
    /// the terminal source plus every `original` walked through,
    /// in walk order.  Hops match replacements as written; a
    /// terminal `IndexPath` replacement then joins its entry's `base`.
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
        let mut base: Option<&Utf8Path> = None;
        loop {
            if !visited.insert(current.clone()) {
                hops.push(current);
                return Err(SourceReplacementError::Cycle { hops });
            }
            let Some(entry) = self.entries.get(&current) else {
                if let (Some(base), SourceLocator::IndexPath { path }) = (base, &mut current) {
                    *path = base.join(&*path);
                }
                return Ok(SourceReplacementResolution {
                    resolved: current,
                    hops,
                });
            };
            hops.push(entry.original.clone());
            current = entry.replacement.clone();
            base = Some(entry.base.as_path());
        }
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
    /// The entry declares neither `index-path` nor `index-url`, or
    /// its only replacement key is empty or whitespace-only.
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
            base: Utf8PathBuf::from("/ws/.cabin"),
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
        assert_eq!(res.resolved, path("/ws/.cabin/../mirror"));
        assert_eq!(res.hops, vec![original]);
    }

    #[test]
    fn resolve_walks_a_chain_until_terminal() {
        let mut settings = SourceReplacementSettings::default();
        let a = url("https://example.com/a");
        let b = url("https://example.com/b");
        let c = path("../local");
        // The first hop comes from another config file: only the
        // terminal hop's directory anchors the path.
        let first = SourceReplacementEntry {
            base: Utf8PathBuf::from("/home/.config/cabin"),
            ..entry(a.clone(), b.clone())
        };
        settings.entries.insert(a.clone(), first);
        settings
            .entries
            .insert(b.clone(), entry(b.clone(), c.clone()));
        let res = settings.resolve(&a).unwrap();
        assert_eq!(res.resolved, path("/ws/.cabin/../local"));
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
