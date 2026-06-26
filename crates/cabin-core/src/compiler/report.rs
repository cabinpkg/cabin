//! Whole-toolchain detection report and its deterministic JSON view.

use serde::{Deserialize, Serialize};

use super::capabilities::{ArchiverCapabilities, Capability, CompilerCapabilities};
use super::identity::{ArchiverIdentity, CompilerIdentity};

/// Whole-toolchain detection report.  The CLI builds one per
/// invocation that needs detection (build / metadata) and threads
/// it into the planner and the metadata view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainDetectionReport {
    pub cxx: ToolDetection<CompilerIdentity, CompilerCapabilities>,
    /// Optional because `ResolvedToolchain.cc` is itself optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<ToolDetection<CompilerIdentity, CompilerCapabilities>>,
    pub ar: ToolDetection<ArchiverIdentity, ArchiverCapabilities>,
}

impl ToolchainDetectionReport {
    /// Compact, deterministic JSON view used by `cabin metadata`
    /// and any tooling that wants to inspect detection results
    /// without re-deriving them.  Each tool block carries
    /// `path` / `identity` / `capabilities`; absent tools (a
    /// missing C compiler) are omitted entirely so the JSON
    /// shape stays stable.
    pub fn as_json(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "cxx".to_owned(),
            serde_json::json!({
                "path": self.cxx.path.as_str().to_owned(),
                "identity": self.cxx.identity.as_json(),
                "capabilities": cxx_capabilities_as_json(&self.cxx.capabilities),
            }),
        );
        if let Some(cc) = &self.cc {
            obj.insert(
                "cc".to_owned(),
                serde_json::json!({
                    "path": cc.path.as_str().to_owned(),
                    "identity": cc.identity.as_json(),
                    "capabilities": cxx_capabilities_as_json(&cc.capabilities),
                }),
            );
        }
        obj.insert(
            "ar".to_owned(),
            serde_json::json!({
                "path": self.ar.path.as_str().to_owned(),
                "identity": self.ar.identity.as_json(),
                "capabilities": ar_capabilities_as_json(&self.ar.capabilities),
            }),
        );
        serde_json::Value::Object(obj)
    }
}

/// One tool's detection outcome plus the path it was invoked at.
/// `path` is the resolved absolute path from
/// [`crate::ResolvedToolchain`]; it is preserved here so error
/// messages can mention the exact executable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDetection<I, C> {
    pub path: camino::Utf8PathBuf,
    pub identity: I,
    pub capabilities: C,
}

/// Render a [`CompilerCapabilities`] as a deterministic JSON map
/// keyed by the public capability name, in alphabetical order.
pub(crate) fn cxx_capabilities_as_json(caps: &CompilerCapabilities) -> serde_json::Value {
    // Exhaustive destructure (no `..`) so adding a capability field
    // is a compile error here until it is wired into the JSON, rather
    // than being silently dropped from `cabin metadata`.
    let CompilerCapabilities {
        gcc_style_flags,
        msvc_style_flags,
        depfile_mmd_mf,
        external_include_dirs,
    } = caps;
    let mut entries: [(&'static str, &Capability); 4] = [
        ("gcc_style_flags", gcc_style_flags),
        ("msvc_style_flags", msvc_style_flags),
        ("depfile_mmd_mf", depfile_mmd_mf),
        ("external_include_dirs", external_include_dirs),
    ];
    capabilities_to_json(&mut entries)
}

pub(crate) fn ar_capabilities_as_json(caps: &ArchiverCapabilities) -> serde_json::Value {
    let ArchiverCapabilities {
        ar_crs,
        static_library_output,
    } = caps;
    let mut entries: [(&'static str, &Capability); 2] = [
        ("ar_crs", ar_crs),
        ("static_library_output", static_library_output),
    ];
    capabilities_to_json(&mut entries)
}

/// Render `(key, capability)` pairs into an alphabetically-keyed JSON
/// object - `{ "<key>": { "supported": <bool>, "source": <kebab> } }`.
/// Sorting here keeps the output independent of the caller's field
/// order, matching the historical BTreeSet-keyed rendering.
fn capabilities_to_json(entries: &mut [(&'static str, &Capability)]) -> serde_json::Value {
    entries.sort_by_key(|(key, _)| *key);
    let mut obj = serde_json::Map::new();
    for (key, cap) in entries {
        obj.insert(
            (*key).to_owned(),
            serde_json::json!({
                "supported": cap.supported,
                "source": cap.source.as_key(),
            }),
        );
    }
    serde_json::Value::Object(obj)
}
