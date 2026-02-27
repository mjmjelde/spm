use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur when loading or validating a config file.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The config file was not found at the specified path.
    #[error("config file not found: {0}")]
    NotFound(PathBuf),

    /// The YAML content could not be parsed.
    #[error("failed to parse YAML: {0}")]
    ParseError(#[from] serde_yaml::Error),

    /// The config failed semantic validation.
    #[error("validation error: {0}")]
    Validation(String),

    /// An environment variable referenced in the config is not set.
    #[error("environment variable not set: {0}")]
    EnvVar(String),

    /// An I/O error occurred while reading the config file.
    #[error("I/O error reading {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}
