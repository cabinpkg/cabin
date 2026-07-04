//! Stable internal data model for Cabin.
//!
//! This crate defines the validated, format-agnostic types that the rest of
//! the workspace builds on.  Manifest parsing, the CLI, and (later) the build
//! graph all consume the same `Package` value, so changes here ripple
//! everywhere - keep this surface small.
//!
//! Crate boundaries:
//! - this crate must not depend on `clap`, `toml`, or any raw manifest
//!   Structs;
//! - manifest-shaped serde structs live in `cabin-manifest`;
//! - CLI dispatch lives in `cabin`.

pub mod build_flags;
pub mod build_jobs;
pub mod compiler;
pub mod compiler_wrapper;
pub mod condition;
pub mod config;
pub mod config_source;
pub mod error;
pub mod hash;
pub mod language_standard;
pub mod model;
pub mod patch;
pub mod process;
pub mod profile;
pub mod registry;
pub mod source_language;
pub mod source_replacement;
pub mod standard_compatibility;
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
    ToolDetectionError, ToolchainDetectionReport, c_standard_capability, cxx_standard_capability,
    derive_ar_capabilities, derive_cxx_capabilities, parse_ar_version_output,
    parse_cxx_version_output, standard_support_detail, validate_ar_for_backend,
    validate_c_standards, validate_cc_for_backend, validate_cxx_for_backend,
    validate_cxx_standards,
};
pub use compiler_wrapper::{
    CompilerWrapperIdentity, CompilerWrapperKind, CompilerWrapperParseError,
    CompilerWrapperRequest, CompilerWrapperSource, CompilerWrapperSummary, ResolvedCompilerWrapper,
};
pub use condition::{
    CompilerSlot, Condition, ConditionContext, ConditionKey, ConditionParseError, TargetPlatform,
};
pub use config::{
    BuildConfiguration, BuildConfigurationInput, DEFAULT_FEATURE_KEY, FeatureEntry, Features,
    InvalidFeatureEntryKind, SelectionRequest, ToolchainSummary,
};
pub use config_source::ConfigValueSource;
pub use error::ValidationError;
pub use language_standard::{
    CStandard, CxxStandard, InterfaceRequirement, InterfaceStandard,
    InterfaceStandardContradiction, InterfaceStandardSource, LanguageStandard,
    LanguageStandardParseError, LanguageStandardSettings, LanguageStandardSource,
    LanguageStandardsSummary, ResolvedLanguageStandards, ResolvedStandard, STANDARD_FLAG_PREFIXES,
    StandardDeclaration, StandardFlagConflict, StandardRequirement, TargetStandardsSummary,
    WorkspaceStandardDefaults, effective_c, effective_cxx, effective_gnu_extensions,
    find_interface_standard_contradictions, find_standard_flag_conflicts, imposes_requirement,
    interface_c, interface_cxx, parse_interface_c, parse_interface_cxx, resolve_language_standards,
};
pub use model::{
    Dependency, DependencyKind, DependencySource, Package, PackageConfigInput, PackageName,
    PortDepSource, SystemDependency, Target, TargetDep, TargetKind, TargetName,
    WorkspaceDepRequirements, is_path_safe_package_name,
};
pub use patch::{
    DeclaredPatch, PatchManifestSettings, PatchProvenance, PatchSource, PatchSourceKind,
    PatchValidationError,
};
pub use process::{ExitStatusKind, exit_status_kind};
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
