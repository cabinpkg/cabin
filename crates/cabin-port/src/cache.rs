//! Placeholder while the crate is wired up; cache lands in Task 3.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Stub: real type arrives in Task 3.
#[derive(Debug, Clone)]
pub struct PortCache {
    root: PathBuf,
}

impl PortCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
