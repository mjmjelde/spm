//! RPM package builder for spm.
//!
//! Builds valid RPM v4 packages from a `PackagePlan` and `Config`.
//! Supports both standard cpio (070701) and extended cpio (07070X)
//! payload formats for large file packages.

pub mod builder;
pub mod error;
pub mod header;
pub mod lead;
pub mod signature;
pub mod tags;
