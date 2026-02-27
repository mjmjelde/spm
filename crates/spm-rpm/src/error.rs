//! Error types for RPM package building.

use std::path::PathBuf;

use thiserror::Error;

/// Errors that can occur during RPM package building.
#[derive(Debug, Error)]
pub enum RpmError {
    /// An I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A CPIO archive error occurred.
    #[error("CPIO archive error: {0}")]
    Cpio(#[from] spm_cpio::CpioError),

    /// A compression error occurred.
    #[error("compression error: {0}")]
    Compress(#[from] spm_compress::CompressError),

    /// Failed to open a source file referenced in the package plan.
    #[error("failed to open source file '{}': {source}", path.display())]
    SourceFile {
        /// Path to the source file.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// Error constructing the RPM header.
    #[error("RPM header error: {0}")]
    Header(String),
}
