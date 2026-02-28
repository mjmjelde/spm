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

/// Errors that can occur during file tree walking.
#[derive(Debug, Error)]
pub enum FileTreeError {
    /// A glob pattern could not be parsed.
    #[error("invalid glob pattern '{pattern}': {source}")]
    InvalidGlob {
        pattern: String,
        source: glob::PatternError,
    },

    /// Error during directory traversal.
    #[error("error walking directory: {0}")]
    WalkError(#[from] walkdir::Error),

    /// I/O error reading file metadata.
    #[error("I/O error reading metadata for {path}: {source}")]
    Metadata {
        path: PathBuf,
        source: std::io::Error,
    },

    /// A file mapping src pattern matched no files.
    #[error("glob pattern '{pattern}' matched no files")]
    NoMatches { pattern: String },

    /// A file mapping destination is invalid.
    #[error("invalid mapping: dst '{dst}' for src '{src}': {reason}")]
    InvalidMapping {
        src: String,
        dst: String,
        reason: String,
    },
}

/// Errors that can occur during package planning.
#[derive(Debug, Error)]
pub enum PlanError {
    /// Error building the file tree.
    #[error(transparent)]
    FileTree(#[from] FileTreeError),

    /// Error in config.
    #[error(transparent)]
    Config(#[from] ConfigError),

    /// Could not read a script file referenced in config.
    #[error("failed to read script '{path}': {source}")]
    ScriptRead {
        path: PathBuf,
        source: std::io::Error,
    },

    /// Invalid size string (e.g., "8GiB" parsing failure).
    #[error("invalid size '{value}': {reason}")]
    InvalidSize { value: String, reason: String },

    /// Splitting is disabled but the package exceeds format limits.
    #[error("package exceeds {format} limits ({total_size} bytes) but splitting is disabled")]
    ExceedsLimits { format: String, total_size: u64 },
}
