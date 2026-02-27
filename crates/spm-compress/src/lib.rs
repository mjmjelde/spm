//! Streaming compression abstraction for spm.
//!
//! Provides a unified interface for compressing data streams using zstd
//! (with multi-threading support), gzip, and a no-op passthrough. Xz
//! support is stubbed and will be added in a later phase.
//!
//! # Example
//!
//! ```
//! use std::io::Write;
//! use spm_compress::{Algorithm, CompressorConfig, compress_writer};
//!
//! let mut output = Vec::new();
//! let config = CompressorConfig {
//!     algorithm: Algorithm::Zstd,
//!     level: Some(3),
//!     threads: 1,
//! };
//! {
//!     let mut writer = compress_writer(&config, &mut output).unwrap();
//!     writer.write_all(b"hello world").unwrap();
//! }
//! assert!(!output.is_empty());
//! ```

use std::io::Write;

use thiserror::Error;

/// Errors that can occur during compression operations.
#[derive(Debug, Error)]
pub enum CompressError {
    /// An I/O error occurred during compression.
    #[error("compression I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The requested compression algorithm is not supported.
    #[error("unsupported algorithm: {0}")]
    Unsupported(String),
}

/// Supported compression algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    /// Zstandard compression (multi-threaded).
    Zstd,
    /// Gzip compression.
    Gzip,
    /// XZ/LZMA2 compression (not yet implemented).
    Xz,
    /// No compression (passthrough).
    None,
}

impl Algorithm {
    /// Parse a compression algorithm name from a string.
    ///
    /// Accepts `"zstd"`, `"gzip"`, `"xz"`, and `"none"`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, CompressError> {
        match s {
            "zstd" => Ok(Self::Zstd),
            "gzip" => Ok(Self::Gzip),
            "xz" => Ok(Self::Xz),
            "none" => Ok(Self::None),
            other => Err(CompressError::Unsupported(other.into())),
        }
    }

    /// File extension for archive members (e.g., `"zst"`, `"gz"`, `"xz"`).
    ///
    /// Returns an empty string for [`Algorithm::None`].
    pub fn extension(&self) -> &str {
        match self {
            Self::Zstd => "zst",
            Self::Gzip => "gz",
            Self::Xz => "xz",
            Self::None => "",
        }
    }

    /// RPM `PAYLOADCOMPRESSOR` tag value.
    pub fn rpm_tag(&self) -> &str {
        match self {
            Self::Zstd => "zstd",
            Self::Gzip => "gzip",
            Self::Xz => "xz",
            Self::None => "identity",
        }
    }

    /// Estimated compression ratio for planning purposes.
    ///
    /// Returns the expected `compressed_size / uncompressed_size` ratio.
    pub fn estimated_ratio(&self) -> f64 {
        match self {
            Self::Zstd => 0.35,
            Self::Gzip => 0.40,
            Self::Xz => 0.30,
            Self::None => 1.0,
        }
    }
}

/// Configuration for a compression operation.
pub struct CompressorConfig {
    /// Which algorithm to use.
    pub algorithm: Algorithm,
    /// Algorithm-specific compression level. `None` uses the algorithm default.
    pub level: Option<i32>,
    /// Thread count for algorithms that support it. `0` means auto-detect.
    pub threads: usize,
}

impl CompressorConfig {
    /// Returns the effective thread count, resolving `0` to the number of logical CPUs.
    fn effective_threads(&self) -> usize {
        if self.threads == 0 {
            num_cpus::get()
        } else {
            self.threads
        }
    }

    /// Returns the effective compression level, using algorithm defaults when unset.
    fn effective_level(&self) -> i32 {
        match (self.algorithm, self.level) {
            (Algorithm::Zstd, Some(l)) => l,
            (Algorithm::Zstd, None) => 3,
            (Algorithm::Gzip, Some(l)) => l,
            (Algorithm::Gzip, None) => 6,
            (Algorithm::Xz, Some(l)) => l,
            (Algorithm::Xz, None) => 6,
            (Algorithm::None, _) => 0,
        }
    }
}

/// Create a compressing writer that wraps an output writer.
///
/// Data written to the returned writer is compressed using the configured
/// algorithm and flushed to the underlying `output` writer. The caller must
/// drop the returned writer (or let it go out of scope) to flush all
/// buffered compression state.
///
/// # Errors
///
/// Returns [`CompressError::Unsupported`] for [`Algorithm::Xz`] (not yet implemented).
/// Returns [`CompressError::Io`] if the compression encoder fails to initialize.
pub fn compress_writer<'a, W: Write + 'a>(
    config: &CompressorConfig,
    output: W,
) -> Result<Box<dyn Write + 'a>, CompressError> {
    match config.algorithm {
        Algorithm::Zstd => {
            let mut encoder = zstd::stream::Encoder::new(output, config.effective_level())?;
            encoder.multithread(config.effective_threads() as u32)?;
            Ok(Box::new(encoder.auto_finish()))
        }
        Algorithm::Gzip => {
            let encoder = flate2::write::GzEncoder::new(
                output,
                flate2::Compression::new(config.effective_level() as u32),
            );
            Ok(Box::new(encoder))
        }
        Algorithm::Xz => {
            // xz support added in Phase 5 (requires liblzma bindings)
            Err(CompressError::Unsupported("xz not yet implemented".into()))
        }
        Algorithm::None => Ok(Box::new(output)),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn test_zstd_roundtrip() {
        let original = vec![42u8; 1_000_000]; // 1 MB of repeated data
        let mut compressed = Vec::new();
        {
            let config = CompressorConfig {
                algorithm: Algorithm::Zstd,
                level: Some(3),
                threads: 1,
            };
            let mut writer = compress_writer(&config, &mut compressed).unwrap();
            writer.write_all(&original).unwrap();
            // writer dropped here, flushing
        }
        // Compressed should be much smaller than original
        assert!(compressed.len() < original.len() / 10);
        // Decompress and verify
        let mut decompressed = Vec::new();
        zstd::stream::copy_decode(&compressed[..], &mut decompressed).unwrap();
        assert_eq!(original, decompressed);
    }

    #[test]
    fn test_gzip_roundtrip() {
        let original = vec![42u8; 1_000_000];
        let mut compressed = Vec::new();
        {
            let config = CompressorConfig {
                algorithm: Algorithm::Gzip,
                level: Some(6),
                threads: 0,
            };
            let mut writer = compress_writer(&config, &mut compressed).unwrap();
            writer.write_all(&original).unwrap();
        }
        assert!(compressed.len() < original.len() / 10);
        let mut decompressed = Vec::new();
        let mut decoder = flate2::read::GzDecoder::new(&compressed[..]);
        std::io::copy(&mut decoder, &mut decompressed).unwrap();
        assert_eq!(original, decompressed);
    }

    #[test]
    fn test_none_passthrough() {
        let original = vec![42u8; 1000];
        let mut output = Vec::new();
        {
            let config = CompressorConfig {
                algorithm: Algorithm::None,
                level: None,
                threads: 0,
            };
            let mut writer = compress_writer(&config, &mut output).unwrap();
            writer.write_all(&original).unwrap();
        }
        assert_eq!(original, output);
    }

    #[test]
    fn test_zstd_multithreaded() {
        let config = CompressorConfig {
            algorithm: Algorithm::Zstd,
            level: Some(3),
            threads: 4,
        };
        let mut writer = compress_writer(&config, std::io::sink()).unwrap();
        // Should not panic — verifies multithread setup works
        writer.write_all(&vec![0u8; 10_000_000]).unwrap();
    }

    #[test]
    fn test_zstd_auto_threads() {
        // threads=0 means auto-detect CPU count
        let config = CompressorConfig {
            algorithm: Algorithm::Zstd,
            level: None,
            threads: 0,
        };
        let mut writer = compress_writer(&config, std::io::sink()).unwrap();
        writer.write_all(&vec![0u8; 1_000_000]).unwrap();
    }

    #[test]
    fn test_empty_input_zstd() {
        let mut compressed = Vec::new();
        {
            let config = CompressorConfig {
                algorithm: Algorithm::Zstd,
                level: None,
                threads: 1,
            };
            let mut writer = compress_writer(&config, &mut compressed).unwrap();
            writer.write_all(&[]).unwrap();
        }
        // Compressed output should be non-empty (stream header + trailer)
        assert!(!compressed.is_empty());
        let mut decompressed = Vec::new();
        zstd::stream::copy_decode(&compressed[..], &mut decompressed).unwrap();
        assert!(decompressed.is_empty());
    }

    #[test]
    fn test_empty_input_gzip() {
        let mut compressed = Vec::new();
        {
            let config = CompressorConfig {
                algorithm: Algorithm::Gzip,
                level: None,
                threads: 0,
            };
            let mut writer = compress_writer(&config, &mut compressed).unwrap();
            writer.write_all(&[]).unwrap();
        }
        assert!(!compressed.is_empty());
        let mut decompressed = Vec::new();
        let mut decoder = flate2::read::GzDecoder::new(&compressed[..]);
        std::io::copy(&mut decoder, &mut decompressed).unwrap();
        assert!(decompressed.is_empty());
    }

    #[test]
    fn test_xz_returns_unsupported() {
        let config = CompressorConfig {
            algorithm: Algorithm::Xz,
            level: None,
            threads: 0,
        };
        let result = compress_writer(&config, std::io::sink());
        match result {
            Err(CompressError::Unsupported(_)) => {}
            Err(other) => panic!("expected Unsupported, got: {other}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[test]
    fn test_algorithm_from_str() {
        assert_eq!(Algorithm::from_str("zstd").unwrap(), Algorithm::Zstd);
        assert_eq!(Algorithm::from_str("gzip").unwrap(), Algorithm::Gzip);
        assert_eq!(Algorithm::from_str("xz").unwrap(), Algorithm::Xz);
        assert_eq!(Algorithm::from_str("none").unwrap(), Algorithm::None);
        assert!(Algorithm::from_str("brotli").is_err());
        assert!(Algorithm::from_str("").is_err());
    }

    #[test]
    fn test_algorithm_extension() {
        assert_eq!(Algorithm::Zstd.extension(), "zst");
        assert_eq!(Algorithm::Gzip.extension(), "gz");
        assert_eq!(Algorithm::Xz.extension(), "xz");
        assert_eq!(Algorithm::None.extension(), "");
    }

    #[test]
    fn test_algorithm_rpm_tag() {
        assert_eq!(Algorithm::Zstd.rpm_tag(), "zstd");
        assert_eq!(Algorithm::Gzip.rpm_tag(), "gzip");
        assert_eq!(Algorithm::Xz.rpm_tag(), "xz");
        assert_eq!(Algorithm::None.rpm_tag(), "identity");
    }

    #[test]
    fn test_zstd_roundtrip_varied_data() {
        // Compress data that isn't trivially compressible
        let mut original = Vec::with_capacity(100_000);
        for i in 0u32..25_000 {
            original.extend_from_slice(&i.to_le_bytes());
        }
        let mut compressed = Vec::new();
        {
            let config = CompressorConfig {
                algorithm: Algorithm::Zstd,
                level: Some(3),
                threads: 2,
            };
            let mut writer = compress_writer(&config, &mut compressed).unwrap();
            writer.write_all(&original).unwrap();
        }
        let mut decompressed = Vec::new();
        zstd::stream::copy_decode(&compressed[..], &mut decompressed).unwrap();
        assert_eq!(original, decompressed);
    }
}
