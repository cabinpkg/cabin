use thiserror::Error;

/// Errors produced while locating the Ninja build runner.
#[derive(Debug, Error)]
pub enum ToolchainError {
    #[error("ninja was not found. Install Ninja or set the NINJA environment variable.")]
    NoNinja,

    #[error(
        "environment variable {var} is set to {value:?} but no executable can be found at that location"
    )]
    BadEnvOverride { var: String, value: String },
}
