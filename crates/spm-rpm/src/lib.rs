//! RPM package builder and reader for spm.
//!
//! Builds valid RPM v4 packages from a `PackagePlan` and `Config`.
//! Supports both standard cpio (070701) and extended cpio (07070X)
//! payload formats for large file packages.
//!
//! Also provides a reader for extracting metadata from existing RPM files.

pub mod builder;
pub mod error;
pub mod header;
pub mod lead;
pub mod reader;
pub mod signature;
pub mod tags;
