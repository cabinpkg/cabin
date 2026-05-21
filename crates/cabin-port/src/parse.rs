//! Placeholder while the crate is wired up; the parser lands in Task 2.
//!
//! Keeping the module file present means `lib.rs` compiles without
//! drama as we wire up `cabin-port` end-to-end task by task.

#![allow(dead_code)]

use std::path::Path;

use crate::error::PortError;
use crate::model::PortDescriptor;

/// Stub: real parser arrives in Task 2.
pub fn load_port(path: impl AsRef<Path>) -> Result<PortDescriptor, PortError> {
    let _ = path;
    unimplemented!("cabin-port: parse.rs is filled in by Task 2")
}

/// Stub: real parser arrives in Task 2.
pub fn parse_port_str(text: &str, path: &Path) -> Result<PortDescriptor, PortError> {
    let _ = (text, path);
    unimplemented!("cabin-port: parse.rs is filled in by Task 2")
}
