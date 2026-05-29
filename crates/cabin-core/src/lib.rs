//! Stable internal data model for Cabin.
//!
//! This crate defines the validated, format-agnostic types that the rest of
//! the workspace builds on. Manifest parsing, the CLI, and (later) the build
//! graph all consume the same `Package` value, so changes here ripple
//! everywhere — keep this surface small.
//!
//! Crate boundaries:
//! - this crate must not depend on `clap`, `toml`, or any raw manifest
//!   Structs;
//! - manifest-shaped serde structs live in `cabin-manifest`;
//! - CLI dispatch lives in `cabin`.

#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::return_self_not_must_use,
    clippy::doc_markdown,
    clippy::redundant_closure_for_method_calls,
    clippy::match_wildcard_for_single_variants,
    clippy::map_unwrap_or,
    clippy::uninlined_format_args,
    clippy::too_many_lines,
    clippy::elidable_lifetime_names,
    clippy::ignored_unit_patterns,
    clippy::manual_string_new,
    clippy::needless_raw_string_hashes,
    clippy::semicolon_if_nothing_returned,
    clippy::match_same_arms
)]

pub mod build_flags;
pub mod build_jobs;
pub mod compiler;
pub mod compiler_wrapper;
pub mod condition;
pub mod config;
pub mod config_source;
pub mod error;
pub mod model;
pub mod patch;
pub mod profile;
pub mod source_language;
pub mod source_replacement;
pub mod term_color;
pub mod term_verbosity;
pub mod toolchain;
pub mod version_req;

pub use build_flags::{
    BuildFlagsValidationError, ConditionalProfileFlags, ProfileFlags, ProfileSettings,
    ResolvedProfileFlags, resolve_build_flags,
};
pub use build_jobs::{BuildJobs, BuildJobsParseError};
pub use compiler::{
    ArchiverCapabilities, ArchiverIdentity, ArchiverKind, Capability, CapabilitySource,
    CompilerCapabilities, CompilerIdentity, CompilerKind, CompilerVersion, ToolDetection,
    ToolDetectionError, ToolchainDetectionReport, derive_ar_capabilities, derive_cxx_capabilities,
    parse_ar_version_output, parse_cxx_version_output, validate_ar_for_backend,
    validate_cc_for_backend, validate_cxx_for_backend,
};
pub use compiler_wrapper::{
    CompilerWrapperIdentity, CompilerWrapperKind, CompilerWrapperManifestSettings,
    CompilerWrapperParseError, CompilerWrapperRequest, CompilerWrapperSource,
    CompilerWrapperSummary, ConditionalCompilerWrapperDecl, ResolvedCompilerWrapper,
};
pub use condition::{Condition, ConditionKey, ConditionParseError, TargetPlatform};
pub use config::{
    BuildConfiguration, BuildConfigurationInput, DEFAULT_FEATURE_KEY, FeatureEntry, Features,
    InvalidFeatureEntryKind, SelectionRequest, ToolchainSummary,
};
pub use config_source::ConfigValueSource;
pub use error::ValidationError;
pub use model::{
    Dependency, DependencyKind, DependencySource, Package, PackageConfigInput, PackageName,
    PortDepSource, SystemDependency, Target, TargetKind, TargetName, is_path_safe_package_name,
};
pub use patch::{
    DeclaredPatch, PatchManifestSettings, PatchProvenance, PatchSource, PatchSourceKind,
    PatchValidationError,
};
pub use profile::{
    BuiltinProfile, InvalidProfileName, OptLevel, ProfileDefaults, ProfileDefinition, ProfileName,
    ProfileResolutionError, ProfileSelection, ProfileSource, ResolvedProfile,
    available_profile_names, resolve_profile,
};
pub use source_language::{SourceLanguage, classify_source, link_driver_language};
pub use source_replacement::{
    SourceLocator, SourceReplacementEntry, SourceReplacementError, SourceReplacementResolution,
    SourceReplacementSettings,
};
pub use term_color::{ColorChoice, ColorEnvError, InvalidColorChoice};
pub use term_verbosity::{InvalidVerbosityCombination, Verbosity, VerbosityEnvError};
pub use toolchain::{
    ConditionalToolchainDecl, ResolvedTool, ResolvedToolchain, ToolKind, ToolSelection, ToolSource,
    ToolSpec, ToolchainDecl, ToolchainResolutionError, ToolchainSelection, ToolchainSettings,
};
