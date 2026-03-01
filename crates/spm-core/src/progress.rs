//! Progress reporting trait for package build pipelines.
//!
//! Library crates call methods on this trait during data-intensive stages.
//! The CLI layer provides an indicatif-backed implementation. Tests and
//! library consumers can use [`NoopProgress`].

/// Identifies a discrete build stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStage {
    /// Hashing source files to compute digests (RPM only).
    HashingFiles,
    /// Writing file data into the payload archive (CPIO for RPM, tar for DEB).
    WritingPayload,
    /// Building metadata headers.
    BuildingMetadata,
    /// Computing signature hashes over the assembled header + payload (RPM only).
    ComputingSignature,
    /// Assembling the final package file on disk.
    Assembling,
    /// Writing the control tar (DEB only).
    WritingControl,
}

impl BuildStage {
    /// Human-readable label for display.
    pub fn label(&self) -> &'static str {
        match self {
            Self::HashingFiles => "Hashing files",
            Self::WritingPayload => "Compressing payload",
            Self::BuildingMetadata => "Building metadata",
            Self::ComputingSignature => "Computing signature",
            Self::Assembling => "Assembling package",
            Self::WritingControl => "Writing control",
        }
    }
}

/// Trait for receiving build progress updates.
///
/// Uses `&self` so implementations can use interior mutability (e.g.
/// indicatif's `ProgressBar` which is internally thread-safe).
pub trait BuildProgress {
    /// A new build stage is starting.
    ///
    /// `total_items` is the number of files to process (0 if not applicable).
    /// `total_bytes` is the total uncompressed byte count (0 if not applicable).
    fn stage_start(&self, stage: BuildStage, total_items: u64, total_bytes: u64);

    /// One item (file) has been processed in the current stage.
    ///
    /// `bytes` is the size of this specific item.
    fn item_completed(&self, bytes: u64);

    /// The current stage has finished.
    fn stage_finish(&self, stage: BuildStage);

    /// A split part has been finalized during streaming split.
    ///
    /// `part` is the 1-based part number. `compressed_size` is the
    /// compressed data.tar size for this part.
    fn part_completed(&self, _part: u32, _compressed_size: u64) {}
}

/// A no-op implementation that discards all progress updates.
pub struct NoopProgress;

impl BuildProgress for NoopProgress {
    fn stage_start(&self, _stage: BuildStage, _total_items: u64, _total_bytes: u64) {}
    fn item_completed(&self, _bytes: u64) {}
    fn stage_finish(&self, _stage: BuildStage) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_progress_does_not_panic() {
        let p = NoopProgress;
        p.stage_start(BuildStage::HashingFiles, 100, 1024);
        p.item_completed(512);
        p.stage_finish(BuildStage::HashingFiles);
    }

    #[test]
    fn stage_labels_are_nonempty() {
        let stages = [
            BuildStage::HashingFiles,
            BuildStage::WritingPayload,
            BuildStage::BuildingMetadata,
            BuildStage::ComputingSignature,
            BuildStage::Assembling,
            BuildStage::WritingControl,
        ];
        for stage in &stages {
            assert!(!stage.label().is_empty(), "{:?} has empty label", stage);
        }
    }
}
