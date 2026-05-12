//! Where a config file or an effective config value came from.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Discovery origin of a config file.
///
/// Recorded on every [`LoadedConfigFile`] so `cabin metadata` can
/// report exactly which files contributed to the merged
/// [`crate::EffectiveConfig`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigSource {
    /// Per-user config file under the user config directory.
    User,
    /// Workspace-level config file at `<workspace-root>/.cabin/config.toml`.
    Workspace,
    /// Package-local config file at `<package-root>/.cabin/config.toml`
    /// for non-workspace projects.
    Package,
    /// Config file pointed at explicitly by the `CABIN_CONFIG`
    /// environment variable. Highest precedence among config files.
    Explicit,
}

impl ConfigSource {
    /// Stable lower-case identifier used in JSON output and error
    /// messages.
    pub const fn as_key(self) -> &'static str {
        match self {
            ConfigSource::User => "user",
            ConfigSource::Workspace => "workspace",
            ConfigSource::Package => "package",
            ConfigSource::Explicit => "explicit",
        }
    }
}

impl fmt::Display for ConfigSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_key())
    }
}

/// One config file discovered on disk plus its parsed contents.
///
/// `loaded_files` on [`crate::EffectiveConfig`] returns these in
/// the deterministic order the merge consumed them: lower-priority
/// files come first, higher-priority files come last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedConfigFile {
    pub source: ConfigSource,
    pub path: PathBuf,
    pub parsed: crate::parse::ParsedConfig,
}

/// One config-derived value plus the file it came from.
///
/// Used inside [`crate::EffectiveConfig`] for every per-key value
/// so consumers can tell which file (user / workspace / package /
/// explicit) was responsible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcedValue<T> {
    pub value: T,
    pub source: ConfigSource,
}

impl<T> SourcedValue<T> {
    pub fn new(value: T, source: ConfigSource) -> Self {
        Self { value, source }
    }

    /// Borrow the inner value while keeping the source available.
    pub fn as_ref(&self) -> SourcedValue<&T> {
        SourcedValue {
            value: &self.value,
            source: self.source,
        }
    }
}
