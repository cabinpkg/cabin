//! Stable internal data model for Cabin.

pub mod build_jobs;
pub mod condition;
pub mod config_source;
pub mod lint;
pub mod source_language;
pub mod term_color;
pub mod term_verbosity;
pub mod version_req;

pub use build_jobs::{BuildJobs, BuildJobsParseError};
pub use condition::{Condition, ConditionKey, ConditionParseError, TargetPlatform};
pub use config_source::ConfigValueSource;
pub use lint::{CpplintLintSettings, LintSettings};
pub use source_language::{classify_source, link_driver_language, SourceLanguage};
pub use term_color::{ColorChoice, ColorEnvError, InvalidColorChoice};
pub use term_verbosity::{InvalidVerbosityCombination, Verbosity, VerbosityEnvError};
