//! Streaming compression and decompression abstraction for spm.
//!
//! Provides a unified interface for compressing and decompressing data streams
//! using zstd (with multi-threading support), gzip, xz (with multi-threading
//! support), and a no-op passthrough.
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

use std::io::{Read, Write};

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
    /// XZ/LZMA2 compression (multi-threaded via liblzma).
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

/// A compressing writer with explicit finalization.
///
/// Unlike a plain `Box<dyn Write>`, this type exposes a [`finish()`](FinishableWriter::finish)
/// method that properly finalizes the compression stream and propagates any errors.
/// This avoids the silent error swallowing that occurs when relying on `Drop` for
/// gzip and xz encoders.
pub struct FinishableWriter<'a> {
    inner: FinishableInner<'a>,
}

enum FinishableInner<'a> {
    Zstd(Box<dyn Write + 'a>),
    Gzip(flate2::write::GzEncoder<Box<dyn Write + 'a>>),
    Xz(xz2::write::XzEncoder<Box<dyn Write + 'a>>),
    None(Box<dyn Write + 'a>),
}

impl<'a> Write for FinishableWriter<'a> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match &mut self.inner {
            FinishableInner::Zstd(w) => w.write(buf),
            FinishableInner::Gzip(w) => w.write(buf),
            FinishableInner::Xz(w) => w.write(buf),
            FinishableInner::None(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match &mut self.inner {
            FinishableInner::Zstd(w) => w.flush(),
            FinishableInner::Gzip(w) => w.flush(),
            FinishableInner::Xz(w) => w.flush(),
            FinishableInner::None(w) => w.flush(),
        }
    }
}

impl<'a> FinishableWriter<'a> {
    /// Finalize the compression stream, flushing all buffered data.
    ///
    /// This must be called before measuring the output size. Unlike `drop()`,
    /// this method propagates any errors that occur during finalization.
    pub fn finish(self) -> std::io::Result<()> {
        match self.inner {
            FinishableInner::Zstd(mut w) => w.flush(),
            FinishableInner::Gzip(w) => {
                w.finish()?;
                Ok(())
            }
            FinishableInner::Xz(w) => {
                w.finish()?;
                Ok(())
            }
            FinishableInner::None(mut w) => w.flush(),
        }
    }
}

/// Create a compressing writer that wraps an output writer.
///
/// Data written to the returned writer is compressed using the configured
/// algorithm. The caller **must** call [`FinishableWriter::finish()`] to
/// finalize the compression stream and propagate any errors. Relying on
/// `drop()` alone may silently discard finalization errors for gzip and xz.
///
/// # Errors
///
/// Returns [`CompressError::Io`] if the compression encoder fails to initialize.
pub fn compress_writer<'a, W: Write + 'a>(
    config: &CompressorConfig,
    output: W,
) -> Result<FinishableWriter<'a>, CompressError> {
    let boxed: Box<dyn Write + 'a> = Box::new(output);
    match config.algorithm {
        Algorithm::Zstd => {
            let mut encoder = zstd::stream::Encoder::new(boxed, config.effective_level())?;
            encoder.multithread(config.effective_threads() as u32)?;
            Ok(FinishableWriter {
                inner: FinishableInner::Zstd(Box::new(encoder.auto_finish())),
            })
        }
        Algorithm::Gzip => {
            let encoder = flate2::write::GzEncoder::new(
                boxed,
                flate2::Compression::new(config.effective_level() as u32),
            );
            Ok(FinishableWriter {
                inner: FinishableInner::Gzip(encoder),
            })
        }
        Algorithm::Xz => {
            let level = config.effective_level() as u32;
            let threads = config.effective_threads() as u32;
            if threads > 1 {
                let stream = xz2::stream::MtStreamBuilder::new()
                    .threads(threads)
                    .preset(level)
                    .encoder()
                    .map_err(|e| {
                        CompressError::Io(std::io::Error::other(e))
                    })?;
                Ok(FinishableWriter {
                    inner: FinishableInner::Xz(xz2::write::XzEncoder::new_stream(boxed, stream)),
                })
            } else {
                Ok(FinishableWriter {
                    inner: FinishableInner::Xz(xz2::write::XzEncoder::new(boxed, level)),
                })
            }
        }
        Algorithm::None => Ok(FinishableWriter {
            inner: FinishableInner::None(boxed),
        }),
    }
}

/// Create a decompressing reader that wraps an input reader.
///
/// Data read from the returned reader is decompressed using the specified
/// algorithm from the underlying `input` reader.
///
/// # Errors
///
/// Returns [`CompressError::Io`] if the decompression decoder fails to initialize.
pub fn decompress_reader<'a, R: Read + 'a>(
    algorithm: Algorithm,
    input: R,
) -> Result<Box<dyn Read + 'a>, CompressError> {
    match algorithm {
        Algorithm::Zstd => {
            let decoder = zstd::stream::Decoder::new(input)?;
            Ok(Box::new(decoder))
        }
        Algorithm::Gzip => {
            let decoder = flate2::read::GzDecoder::new(input);
            Ok(Box::new(decoder))
        }
        Algorithm::Xz => {
            let decoder = xz2::read::XzDecoder::new(input);
            Ok(Box::new(decoder))
        }
        Algorithm::None => Ok(Box::new(input)),
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
    fn test_xz_roundtrip() {
        let original = vec![42u8; 1_000_000];
        let mut compressed = Vec::new();
        {
            let config = CompressorConfig {
                algorithm: Algorithm::Xz,
                level: Some(6),
                threads: 1,
            };
            let mut writer = compress_writer(&config, &mut compressed).unwrap();
            writer.write_all(&original).unwrap();
        }
        assert!(compressed.len() < original.len() / 10);
        let mut decompressed = Vec::new();
        let mut decoder = xz2::read::XzDecoder::new(&compressed[..]);
        std::io::copy(&mut decoder, &mut decompressed).unwrap();
        assert_eq!(original, decompressed);
    }

    #[test]
    fn test_xz_multithreaded() {
        let original = vec![42u8; 1_000_000];
        let mut compressed = Vec::new();
        {
            let config = CompressorConfig {
                algorithm: Algorithm::Xz,
                level: Some(6),
                threads: 4,
            };
            let mut writer = compress_writer(&config, &mut compressed).unwrap();
            writer.write_all(&original).unwrap();
        }
        let mut decompressed = Vec::new();
        let mut decoder = xz2::read::XzDecoder::new(&compressed[..]);
        std::io::copy(&mut decoder, &mut decompressed).unwrap();
        assert_eq!(original, decompressed);
    }

    #[test]
    fn test_xz_empty_input() {
        let mut compressed = Vec::new();
        {
            let config = CompressorConfig {
                algorithm: Algorithm::Xz,
                level: None,
                threads: 1,
            };
            let mut writer = compress_writer(&config, &mut compressed).unwrap();
            writer.write_all(&[]).unwrap();
        }
        assert!(!compressed.is_empty());
        let mut decompressed = Vec::new();
        let mut decoder = xz2::read::XzDecoder::new(&compressed[..]);
        std::io::copy(&mut decoder, &mut decompressed).unwrap();
        assert!(decompressed.is_empty());
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
    fn test_decompress_zstd_roundtrip() {
        let original = vec![42u8; 100_000];
        let mut compressed = Vec::new();
        {
            let config = CompressorConfig {
                algorithm: Algorithm::Zstd,
                level: Some(3),
                threads: 1,
            };
            let mut writer = compress_writer(&config, &mut compressed).unwrap();
            writer.write_all(&original).unwrap();
        }
        let mut decompressed = Vec::new();
        let mut reader = decompress_reader(Algorithm::Zstd, &compressed[..]).unwrap();
        std::io::copy(&mut reader, &mut decompressed).unwrap();
        assert_eq!(original, decompressed);
    }

    #[test]
    fn test_decompress_gzip_roundtrip() {
        let original = vec![42u8; 100_000];
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
        let mut decompressed = Vec::new();
        let mut reader = decompress_reader(Algorithm::Gzip, &compressed[..]).unwrap();
        std::io::copy(&mut reader, &mut decompressed).unwrap();
        assert_eq!(original, decompressed);
    }

    #[test]
    fn test_decompress_xz_roundtrip() {
        let original = vec![42u8; 100_000];
        let mut compressed = Vec::new();
        {
            let config = CompressorConfig {
                algorithm: Algorithm::Xz,
                level: Some(6),
                threads: 1,
            };
            let mut writer = compress_writer(&config, &mut compressed).unwrap();
            writer.write_all(&original).unwrap();
        }
        let mut decompressed = Vec::new();
        let mut reader = decompress_reader(Algorithm::Xz, &compressed[..]).unwrap();
        std::io::copy(&mut reader, &mut decompressed).unwrap();
        assert_eq!(original, decompressed);
    }

    #[test]
    fn test_decompress_none_passthrough() {
        let original = vec![42u8; 1000];
        let mut decompressed = Vec::new();
        let mut reader = decompress_reader(Algorithm::None, &original[..]).unwrap();
        std::io::copy(&mut reader, &mut decompressed).unwrap();
        assert_eq!(original, decompressed);
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
