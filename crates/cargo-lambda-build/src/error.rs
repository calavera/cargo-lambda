use miette::Diagnostic;
use object::Architecture;
use thiserror::Error;

#[derive(Debug, Diagnostic, Error)]
pub(crate) enum BuildError {
    #[error(
        "invalid options: --arm64, --x86-64, and --target cannot be specified at the same time"
    )]
    InvalidTargetOptions,
    #[error("invalid options: --compiler=cargo is only allowed on Linux")]
    InvalidCompilerOption,
    #[error("install Zig and run cargo-lambda again")]
    ZigMissing,
    #[error("binary target is missing from this project: {0}")]
    FunctionBinaryMissing(String),
    #[error("binary file for {0} not found, use `cargo lambda {1}` to create it")]
    BinaryMissing(String, String),
    #[error("invalid binary architecture: {0:?}")]
    InvalidBinaryArchitecture(Architecture),
    #[error("invalid or unsupported target for AWS Lambda: {0}")]
    UnsupportedTarget(String),
}
