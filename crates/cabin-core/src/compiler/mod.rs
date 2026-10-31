//! Typed compiler / tool identity and capability model.
//!
//! The `ResolvedToolchain` (see [`crate::toolchain`]) says *which*
//! tools the user picked; this module says *what those tools are*
//! and *what they can do*.  The detector in `cabin-toolchain::detect`
//! runs `--version` against each resolved tool, hands the output to
//! the pure parsers in this module, and assembles a typed
//! [`ToolchainDetectionReport`].
//!
//! This module is data and pure logic only.  Process spawning,
//! filesystem traversal, and CLI dispatch live elsewhere.

mod capabilities;
mod identity;
mod parsing;
mod report;
#[cfg(test)]
mod tests;
mod validation;

pub use capabilities::{
    ArchiverCapabilities, Capability, CapabilitySource, CompilerCapabilities,
    c_standard_capability, cxx_standard_capability, derive_ar_capabilities,
    derive_cxx_capabilities, standard_support_detail,
};
pub use identity::{
    ArchiverIdentity, ArchiverKind, CompilerIdentity, CompilerKind, CompilerVersion,
};
pub use parsing::{parse_ar_version_output, parse_cxx_version_output};
pub use report::{ToolDetection, ToolchainDetectionReport};
pub use validation::{
    ToolDetectionError, validate_ar_for_backend, validate_c_standards, validate_cc_for_backend,
    validate_cxx_for_backend, validate_cxx_standards,
};
