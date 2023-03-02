use std::num::ParseIntError;

use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Diagnostic, Error)]
pub enum MetadataError {
    #[error("invalid memory value `{0}`")]
    #[diagnostic()]
    InvalidMemory(i32),
    #[error("invalid lambda metadata in Cargo.toml file: {0}")]
    #[diagnostic()]
    InvalidCargoMetadata(#[from] serde_json::Error),
    #[error("invalid timeout value")]
    #[diagnostic()]
    InvalidTimeout(#[from] ParseIntError),
    #[error("invalid tracing option `{0}`")]
    #[diagnostic()]
    InvalidTracing(String),
    #[error("there are more than one binary in the project, you must specify a binary name")]
    #[diagnostic()]
    MultipleBinariesInProject,
    #[error("there are no binaries in this project")]
    #[diagnostic()]
    MissingBinaryInProject,
    #[error("invalid environment variable `{0}`")]
    #[diagnostic()]
    InvalidEnvVar(String),
}
