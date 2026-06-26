//! Where an effective configuration value came from, across the
//! full precedence chain that combines CLI flags, environment
//! variables, config files, manifest declarations, and built-in
//! defaults.
//!
//! `cabin metadata` reports every effective setting paired with one
//! of these labels so users can audit a build without re-deriving
//! the precedence by hand.  Crates that produce effective values
//! (cabin's orchestration layer; cabin-config's merge layer;
//! the toolchain / wrapper resolvers) populate the matching variant
//! and the metadata serializer renders the stable kebab-case form.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable, ordered enum describing every layer Cabin's effective
/// configuration can come from.
///
/// The `cli`, `env`, and `*-config` variants are the only ones a
/// caller assigns directly today; manifest-derived values stay on
/// their dedicated tool/wrapper enums to preserve the existing
/// metadata shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigValueSource {
    /// Hard-coded in Cabin (e.g., the default `dev` profile).
    BuiltinDefault,
    /// Declared in the package manifest (e.g.,
    /// `[profile.<name>]`).
    Manifest,
    /// Declared in the user-level config file.
    UserConfig,
    /// Declared in the workspace-level config file.
    WorkspaceConfig,
    /// Declared in the package-local config file (non-workspace
    /// projects).
    PackageConfig,
    /// Declared in the file pointed at by `CABIN_CONFIG`.
    ExplicitConfig,
    /// Provided through an environment variable (e.g., `CC`,
    /// `CABIN_COMPILER_WRAPPER`).
    Env,
    /// Provided through a CLI flag (e.g., `--profile`,
    /// `--cxx`).
    Cli,
}

impl ConfigValueSource {
    /// Stable lower-case identifier used in JSON output and error
    /// messages.
    pub const fn as_key(self) -> &'static str {
        match self {
            ConfigValueSource::BuiltinDefault => "builtin-default",
            ConfigValueSource::Manifest => "manifest",
            ConfigValueSource::UserConfig => "user-config",
            ConfigValueSource::WorkspaceConfig => "workspace-config",
            ConfigValueSource::PackageConfig => "package-config",
            ConfigValueSource::ExplicitConfig => "explicit-config",
            ConfigValueSource::Env => "env",
            ConfigValueSource::Cli => "cli",
        }
    }
}

impl fmt::Display for ConfigValueSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_with_serde() {
        for source in [
            ConfigValueSource::BuiltinDefault,
            ConfigValueSource::Manifest,
            ConfigValueSource::UserConfig,
            ConfigValueSource::WorkspaceConfig,
            ConfigValueSource::PackageConfig,
            ConfigValueSource::ExplicitConfig,
            ConfigValueSource::Env,
            ConfigValueSource::Cli,
        ] {
            let json = serde_json::to_string(&source).unwrap();
            let echoed: ConfigValueSource = serde_json::from_str(&json).unwrap();
            assert_eq!(echoed, source);
            assert_eq!(json.trim_matches('"'), source.as_key());
        }
    }
}
