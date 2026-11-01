//! The workspace root's `[system-provided]` policy: packages the build
//! takes from the system instead of fetching and building them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The workspace-root manifest's `[system-provided]` table, keyed by
/// package name.  Member manifests cannot declare it - the workspace
/// loader rejects them - so reading it off the root is sufficient.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemProvidedSettings {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entries: BTreeMap<crate::PackageName, SystemProvider>,
}

impl SystemProvidedSettings {
    /// Whether the table carries no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Where the system supplies one package from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SystemProvider {
    /// One pkg-config module supplies the package's sole library
    /// target.
    Module {
        #[serde(rename = "pkg-config")]
        pkg_config: String,
    },
    /// One pkg-config module per library target, keyed by target name;
    /// never empty.
    Targets {
        targets: BTreeMap<crate::TargetName, SystemTargetProvider>,
    },
}

impl SystemProvider {
    /// Validate one `[system-provided]` row: exactly one of a sole
    /// `pkg-config` module or a non-empty `targets` table of them.
    /// Every module is checked as a system-dependency name, since it
    /// reaches pkg-config's argv.
    ///
    /// # Errors
    /// [`SystemProviderError::Shape`] for both or neither;
    /// [`SystemProviderError::Invalid`] for an invalid module or
    /// target name.
    pub fn new(
        pkg_config: Option<String>,
        targets: Option<BTreeMap<String, String>>,
    ) -> Result<Self, SystemProviderError> {
        match (pkg_config, targets) {
            (Some(pkg_config), None) => {
                crate::validate_system_dependency_name(&pkg_config)?;
                Ok(SystemProvider::Module { pkg_config })
            }
            (None, Some(rows)) if !rows.is_empty() => {
                let mut targets = BTreeMap::new();
                for (target, pkg_config) in rows {
                    crate::validate_system_dependency_name(&pkg_config)?;
                    targets.insert(
                        crate::TargetName::new(target)?,
                        SystemTargetProvider { pkg_config },
                    );
                }
                Ok(SystemProvider::Targets { targets })
            }
            _ => Err(SystemProviderError::Shape),
        }
    }

    /// The pkg-config module that supplies the library target `target`.
    pub fn module_for(&self, target: &str) -> Option<&str> {
        match self {
            SystemProvider::Module { pkg_config } => Some(pkg_config),
            SystemProvider::Targets { targets } => targets
                .iter()
                .find(|(name, _)| name.as_str() == target)
                .map(|(_, provider)| provider.pkg_config.as_str()),
        }
    }
}

/// The pkg-config module that supplies one library target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemTargetProvider {
    #[serde(rename = "pkg-config")]
    pub pkg_config: String,
}

/// Why a `[system-provided]` row is invalid.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SystemProviderError {
    #[error(transparent)]
    Invalid(#[from] crate::ValidationError),
    #[error("needs either `pkg-config` or a non-empty `targets` table, not both")]
    Shape,
}
