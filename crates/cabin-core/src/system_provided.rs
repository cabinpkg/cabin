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
#[serde(rename_all = "kebab-case")]
pub struct SystemProvider {
    /// The pkg-config module that supplies the package's sole library
    /// target.
    pub pkg_config: String,
}
