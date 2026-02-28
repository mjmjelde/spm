//! DEB package builder for spm.
//!
//! Builds valid DEB packages from a `PackagePlan` and `Config`.
//! Supports zstd, gzip, and uncompressed data/control tar members.

pub mod ar;
pub mod builder;
pub mod control;
pub mod error;
pub mod reader;
