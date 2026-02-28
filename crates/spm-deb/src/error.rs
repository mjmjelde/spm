//! Error types for DEB package building.

use std::path::PathBuf;

use thiserror::Error;

/// Errors that can occur during DEB package building.
#[derive(Debug, Error)]
pub enum DebError {
    /// An I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A compression error occurred.
    #[error("compression error: {0}")]
    Compress(#[from] spm_compress::CompressError),

    /// Failed to open a source file referenced in the package plan.
    #[error("failed to open source file '{}': {source}", path.display())]
    SourceFile {
        path: PathBuf,
        source: std::io::Error,
    },

    /// Error writing the ar archive.
    #[error("ar archive error: {0}")]
    Archive(String),

    /// Error writing the tar archive.
    #[error("tar archive error: {0}")]
    Tar(String),

    /// Error generating control data.
    #[error("control file error: {0}")]
    Control(String),

    /// Invalid DEB file structure.
    #[error("invalid DEB: {0}")]
    InvalidDeb(String),
}
