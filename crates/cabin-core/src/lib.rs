//! Stable internal data model for Cabin.

pub mod build_flags;
pub mod build_jobs;
pub mod compiler;
pub mod compiler_wrapper;
pub mod condition;
pub mod config_source;
pub mod coverage;
pub mod lint;
pub mod profile;
pub mod source_language;
pub mod term_color;
pub mod term_verbosity;
pub mod toolchain;
pub mod version_req;

pub use build_flags::{
    resolve_build_flags, BuildFlagsDecl, BuildFlagsSettings, BuildFlagsValidationError,
    ConditionalBuildFlagsDecl, ResolvedBuildFlags,
};
pub use build_jobs::{BuildJobs, BuildJobsParseError};
pub use compiler::{
    derive_ar_capabilities, derive_cxx_capabilities, parse_ar_version_output,
    parse_cxx_version_output, validate_ar_for_backend, validate_cc_for_backend,
    validate_cxx_for_backend, ArchiverCapabilities, ArchiverIdentity, ArchiverKind, Capability,
    CapabilitySource, CompilerCapabilities, CompilerIdentity, CompilerKind, CompilerVersion,
    ToolDetection, ToolDetectionError, ToolchainDetectionReport,
};
pub use compiler_wrapper::{
    CompilerWrapperIdentity, CompilerWrapperKind, CompilerWrapperManifestSettings,
    CompilerWrapperParseError, CompilerWrapperRequest, CompilerWrapperSource,
    CompilerWrapperSummary, ConditionalCompilerWrapperDecl, ResolvedCompilerWrapper,
};
pub use condition::{Condition, ConditionKey, ConditionParseError, TargetPlatform};
pub use config_source::ConfigValueSource;
pub use coverage::{coverage_flags_for_compiler, CoverageFlags, CoverageMode, CoverageSupport};
pub use lint::{CpplintLintSettings, LintSettings};
pub use profile::{
    available_profile_names, resolve_profile, BuiltinProfile,
    InvalidProfileName, OptLevel, ProfileDefaults, ProfileDefinition, ProfileName,
    ProfileResolutionError, ProfileSelection, ProfileSource, ResolvedProfile,
};
pub use source_language::{classify_source, link_driver_language, SourceLanguage};
pub use term_color::{ColorChoice, ColorEnvError, InvalidColorChoice};
pub use term_verbosity::{InvalidVerbosityCombination, Verbosity, VerbosityEnvError};
pub use toolchain::{
    ConditionalToolchainDecl, ResolvedTool, ResolvedToolchain, ToolKind,
    ToolSelection, ToolSource, ToolSpec, ToolchainDecl, ToolchainResolutionError,
    ToolchainSelection, ToolchainSettings,
};
