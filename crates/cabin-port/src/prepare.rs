//! Placeholder while the crate is wired up; pipeline lands in Task 5.

#![allow(dead_code)]

use std::path::PathBuf;

use cabin_core::PackageName;
use semver::Version;
use url::Url;

use crate::cache::PortCache;
use crate::error::PortError;
use crate::model::PortDescriptor;

#[derive(Debug, Clone)]
pub enum PortFetchSource {
    LocalArchive(PathBuf),
    InMemoryArchive(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct PortEntry {
    pub descriptor: PortDescriptor,
    pub port_dir: PathBuf,
    pub source: PortFetchSource,
}

#[derive(Debug, Clone, Default)]
pub struct PortPlan {
    pub entries: Vec<PortEntry>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PortPrepareOptions {
    pub frozen: bool,
}

#[derive(Debug, Clone)]
pub struct PortPrepareResult {
    pub ports: Vec<PreparedPort>,
}

#[derive(Debug, Clone)]
pub struct PreparedPort {
    pub name: PackageName,
    pub version: Version,
    pub source_dir: PathBuf,
    pub port_dir: PathBuf,
    pub provenance: PortProvenance,
}

#[derive(Debug, Clone)]
pub struct PortProvenance {
    pub url: Url,
    pub sha256_hex: String,
    pub strip_prefix: Option<String>,
    /// Absolute path to the overlay manifest inside the port
    /// directory (i.e. `port_dir.join(overlay.relative_path)`),
    /// kept absolute so it pairs uniformly with the absolute
    /// `port_dir` / `source_dir` on `PreparedPort`.
    pub overlay_manifest: PathBuf,
}

/// Stub: real implementation arrives in Task 5.
pub fn prepare(
    plan: &PortPlan,
    cache: &PortCache,
    options: PortPrepareOptions,
) -> Result<PortPrepareResult, PortError> {
    let _ = (plan, cache, options);
    unimplemented!("cabin-port: prepare.rs is filled in by Task 5")
}
