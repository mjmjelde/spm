/// Package planning: file tree analysis, split detection, and plan generation.
///
/// The planner takes a parsed config and format limits, walks the source directory,
/// determines whether splitting is needed, resolves scripts (including alternatives
/// injection), and produces a `PackagePlan` that later phases use to build packages.
use std::path::Path;

use crate::alternatives::{resolve_scripts, ResolvedScripts};
use crate::config::Config;
use crate::error::PlanError;
use crate::filetree::{EntryType, FileEntry, FileTree};
use crate::types::{estimated_compression_ratio, format_size, parse_size, FormatLimits};

/// Safety factor for auto-split decisions. Compression ratio estimates can be
/// off by 20%+ depending on content type (text vs binary vs pre-compressed),
/// so we trigger splitting when estimated compressed size exceeds 80% of the
/// format limit. This prevents producing corrupt packages when the actual
/// ratio is worse than predicted.
const AUTO_SPLIT_HEADROOM: f64 = 0.80;

/// Fraction of per-part size below which a trailing part is merged back into
/// the previous part. Greedy bin-packing can leave a tiny remainder when
/// integer division slightly underestimates the per-part budget; merging
/// avoids producing a trivially small extra package.
const TRAILING_PART_MERGE_THRESHOLD: f64 = 0.02;

/// The complete output of the planning phase.
#[derive(Debug)]
pub struct PackagePlan {
    /// Package name.
    pub name: String,
    /// Package version.
    pub version: String,
    /// Release number.
    pub release: String,
    /// Target architecture.
    pub arch: String,
    /// Sub-packages to build.
    pub sub_packages: Vec<SubPackage>,
    /// Whether the package is split into multiple parts.
    pub is_split: bool,
    /// Whether any file exceeds the standard cpio 4 GiB limit (RPM-specific).
    pub needs_extended_cpio: bool,
    /// Total uncompressed size across all sub-packages.
    pub total_size: u64,
    /// Non-fatal warnings generated during planning.
    pub warnings: Vec<String>,
    /// When true, the DEB builder should perform monitored streaming split
    /// instead of relying on pre-computed SubPackage boundaries. The plan
    /// will contain a single SubPackage with all files.
    pub deferred_split: bool,
}

/// A single buildable package (standalone, meta-package, or part of a split).
#[derive(Debug)]
pub struct SubPackage {
    /// Package name (e.g., "matlab-2025a" or "matlab-2025a-part1").
    pub name: String,
    /// Role of this sub-package.
    pub role: SubPackageRole,
    /// Files included in this sub-package.
    pub files: Vec<FileEntry>,
    /// Total uncompressed size of files in this sub-package.
    pub total_size: u64,
    /// Resolved scripts (only populated for standalone or meta packages).
    pub scripts: ResolvedScripts,
}

/// The role of a sub-package within a package plan.
#[derive(Debug, PartialEq)]
pub enum SubPackageRole {
    /// Single package (no splitting).
    Standalone,
    /// Meta-package: no files, depends on all parts.
    Meta,
    /// Part N of a split package.
    Part(u32),
}

/// Produces a `PackagePlan` from a config and format limits.
pub struct Planner;

impl Planner {
    /// Create a package plan from config and format-specific limits.
    ///
    /// This walks the source directory, calculates sizes, resolves scripts,
    /// and determines whether splitting is needed.
    pub fn plan(
        config: &Config,
        limits: &FormatLimits,
        config_dir: &Path,
    ) -> Result<PackagePlan, PlanError> {
        // Walk the file tree.
        let files = FileTree::walk(&config.content)?;

        // Calculate total size and detect extended cpio need.
        let total_size: u64 = files.iter().map(|f| f.size).sum();
        let needs_extended_cpio = files.iter().any(|f| {
            matches!(f.entry_type, EntryType::RegularFile) && f.size > limits.max_file_size_standard
        });

        // Resolve scripts with alternatives injection.
        let scripts = resolve_scripts(&config.scripts, &config.content.alternatives, config_dir)?;

        let pkg_name = &config.package.name;

        // Determine if splitting is needed.
        let ratio = estimated_compression_ratio(&config.compression.algorithm);
        let estimated_compressed = (total_size as f64 * ratio) as u64;
        let split_threshold = (limits.max_compressed_payload as f64 * AUTO_SPLIT_HEADROOM) as u64;

        let mut deferred_split = false;

        let sub_packages = if !config.splitting.enabled {
            // Splitting disabled — check if we're within limits (with safety margin).
            if estimated_compressed > split_threshold {
                return Err(PlanError::ExceedsLimits {
                    format: limits.format_name.to_string(),
                    total_size: estimated_compressed,
                });
            }
            vec![SubPackage {
                name: pkg_name.clone(),
                role: SubPackageRole::Standalone,
                files,
                total_size,
                scripts,
            }]
        } else {
            match config.splitting.strategy.as_str() {
                "auto" => {
                    if estimated_compressed <= split_threshold {
                        // No split needed.
                        vec![SubPackage {
                            name: pkg_name.clone(),
                            role: SubPackageRole::Standalone,
                            files,
                            total_size,
                            scripts,
                        }]
                    } else if limits.max_compressed_payload < u64::MAX {
                        // Format has a finite payload limit (DEB): defer splitting
                        // to the builder which monitors actual compressed size.
                        deferred_split = true;
                        vec![SubPackage {
                            name: pkg_name.clone(),
                            role: SubPackageRole::Standalone,
                            files,
                            total_size,
                            scripts,
                        }]
                    } else {
                        // Unlimited format (RPM): use estimation-based split.
                        let safe_limit =
                            limits.max_compressed_payload as f64 * AUTO_SPLIT_HEADROOM;
                        let num_parts =
                            (estimated_compressed as f64 / safe_limit).ceil() as u64;
                        let max_uncompressed_per_part = total_size / num_parts;
                        let mut parts = split_by_size(files, max_uncompressed_per_part, pkg_name);
                        fixup_hardlinks_across_parts(&mut parts);
                        build_split_packages(parts, pkg_name, scripts)
                    }
                }
                "size" => {
                    let max_size_str = config.splitting.max_size.as_deref()
                        .expect("max_size required for size strategy (validated by config)");
                    let max_size =
                        parse_size(max_size_str).map_err(|reason| PlanError::InvalidSize {
                            value: max_size_str.to_string(),
                            reason,
                        })?;

                    if total_size <= max_size {
                        vec![SubPackage {
                            name: pkg_name.clone(),
                            role: SubPackageRole::Standalone,
                            files,
                            total_size,
                            scripts,
                        }]
                    } else {
                        let mut parts = split_by_size(files, max_size, pkg_name);
                        fixup_hardlinks_across_parts(&mut parts);
                        build_split_packages(parts, pkg_name, scripts)
                    }
                }
                "directory" => {
                    if config.splitting.parts.is_empty() {
                        // No parts defined — treat as standalone.
                        vec![SubPackage {
                            name: pkg_name.clone(),
                            role: SubPackageRole::Standalone,
                            files,
                            total_size,
                            scripts,
                        }]
                    } else {
                        let mut parts =
                            split_by_directory(files, &config.splitting.parts, pkg_name);
                        fixup_hardlinks_across_parts(&mut parts);
                        build_split_packages(parts, pkg_name, scripts)
                    }
                }
                _ => {
                    // Unknown strategy — should have been caught by config validation.
                    vec![SubPackage {
                        name: pkg_name.clone(),
                        role: SubPackageRole::Standalone,
                        files,
                        total_size,
                        scripts,
                    }]
                }
            }
        };

        let is_split = sub_packages
            .iter()
            .any(|sp| sp.role == SubPackageRole::Meta);

        let warnings = build_warnings(
            is_split,
            deferred_split,
            estimated_compressed,
            limits,
        );

        Ok(PackagePlan {
            name: pkg_name.clone(),
            version: config.package.version.clone(),
            release: config.package.release.clone(),
            arch: config.package.arch.clone(),
            sub_packages,
            is_split,
            needs_extended_cpio,
            total_size,
            warnings,
            deferred_split,
        })
    }

    /// Create a plan from pre-built file entries (for testing without a real filesystem).
    pub fn plan_from_entries(
        config: &Config,
        limits: &FormatLimits,
        files: Vec<FileEntry>,
        scripts: ResolvedScripts,
    ) -> Result<PackagePlan, PlanError> {
        let total_size: u64 = files.iter().map(|f| f.size).sum();
        let needs_extended_cpio = files.iter().any(|f| {
            matches!(f.entry_type, EntryType::RegularFile) && f.size > limits.max_file_size_standard
        });

        let pkg_name = &config.package.name;

        let ratio = estimated_compression_ratio(&config.compression.algorithm);
        let estimated_compressed = (total_size as f64 * ratio) as u64;
        let split_threshold = (limits.max_compressed_payload as f64 * AUTO_SPLIT_HEADROOM) as u64;

        let mut deferred_split = false;

        let sub_packages = if !config.splitting.enabled {
            if estimated_compressed > split_threshold {
                return Err(PlanError::ExceedsLimits {
                    format: limits.format_name.to_string(),
                    total_size: estimated_compressed,
                });
            }
            vec![SubPackage {
                name: pkg_name.clone(),
                role: SubPackageRole::Standalone,
                files,
                total_size,
                scripts,
            }]
        } else {
            match config.splitting.strategy.as_str() {
                "auto" => {
                    if estimated_compressed <= split_threshold {
                        vec![SubPackage {
                            name: pkg_name.clone(),
                            role: SubPackageRole::Standalone,
                            files,
                            total_size,
                            scripts,
                        }]
                    } else if limits.max_compressed_payload < u64::MAX {
                        // Format has a finite payload limit (DEB): defer splitting
                        // to the builder which monitors actual compressed size.
                        deferred_split = true;
                        vec![SubPackage {
                            name: pkg_name.clone(),
                            role: SubPackageRole::Standalone,
                            files,
                            total_size,
                            scripts,
                        }]
                    } else {
                        // Unlimited format (RPM): use estimation-based split.
                        let safe_limit =
                            limits.max_compressed_payload as f64 * AUTO_SPLIT_HEADROOM;
                        let num_parts =
                            (estimated_compressed as f64 / safe_limit).ceil() as u64;
                        let max_uncompressed_per_part = total_size / num_parts;
                        let mut parts = split_by_size(files, max_uncompressed_per_part, pkg_name);
                        fixup_hardlinks_across_parts(&mut parts);
                        build_split_packages(parts, pkg_name, scripts)
                    }
                }
                "size" => {
                    let max_size_str = config.splitting.max_size.as_deref()
                        .expect("max_size required for size strategy (validated by config)");
                    let max_size =
                        parse_size(max_size_str).map_err(|reason| PlanError::InvalidSize {
                            value: max_size_str.to_string(),
                            reason,
                        })?;

                    if total_size <= max_size {
                        vec![SubPackage {
                            name: pkg_name.clone(),
                            role: SubPackageRole::Standalone,
                            files,
                            total_size,
                            scripts,
                        }]
                    } else {
                        let mut parts = split_by_size(files, max_size, pkg_name);
                        fixup_hardlinks_across_parts(&mut parts);
                        build_split_packages(parts, pkg_name, scripts)
                    }
                }
                "directory" => {
                    if config.splitting.parts.is_empty() {
                        vec![SubPackage {
                            name: pkg_name.clone(),
                            role: SubPackageRole::Standalone,
                            files,
                            total_size,
                            scripts,
                        }]
                    } else {
                        let mut parts =
                            split_by_directory(files, &config.splitting.parts, pkg_name);
                        fixup_hardlinks_across_parts(&mut parts);
                        build_split_packages(parts, pkg_name, scripts)
                    }
                }
                _ => vec![SubPackage {
                    name: pkg_name.clone(),
                    role: SubPackageRole::Standalone,
                    files,
                    total_size,
                    scripts,
                }],
            }
        };

        let is_split = sub_packages
            .iter()
            .any(|sp| sp.role == SubPackageRole::Meta);

        let warnings = build_warnings(
            is_split,
            deferred_split,
            estimated_compressed,
            limits,
        );

        Ok(PackagePlan {
            name: pkg_name.clone(),
            version: config.package.version.clone(),
            release: config.package.release.clone(),
            arch: config.package.arch.clone(),
            sub_packages,
            is_split,
            needs_extended_cpio,
            total_size,
            warnings,
            deferred_split,
        })
    }
}

/// Build warnings about packages that are close to format limits.
fn build_warnings(
    is_split: bool,
    deferred_split: bool,
    estimated_compressed: u64,
    limits: &FormatLimits,
) -> Vec<String> {
    let mut warnings = Vec::new();

    // Only relevant for formats with a finite payload limit (DEB).
    if limits.max_compressed_payload == u64::MAX {
        return warnings;
    }

    let pct = estimated_compressed as f64 / limits.max_compressed_payload as f64 * 100.0;

    if is_split && estimated_compressed <= limits.max_compressed_payload {
        // Split was triggered by the safety margin, not by clearly exceeding the limit.
        warnings.push(format!(
            "splitting triggered by safety margin: estimated compressed size ({}) is \
             {pct:.0}% of the {} format limit ({}); actual compression may vary ±20%",
            format_size(estimated_compressed),
            limits.format_name,
            format_size(limits.max_compressed_payload),
        ));
    } else if !is_split && !deferred_split && pct > 60.0 {
        // Not split (and not deferred), but getting close to the limit.
        warnings.push(format!(
            "estimated compressed size ({}) is {pct:.0}% of the {} format limit ({}); \
             consider enabling splitting for safety",
            format_size(estimated_compressed),
            limits.format_name,
            format_size(limits.max_compressed_payload),
        ));
    }

    warnings
}

/// Split files into parts by accumulated size.
/// Files are assumed to already be sorted by install_path.
fn split_by_size(
    files: Vec<FileEntry>,
    max_size_per_part: u64,
    _pkg_name: &str,
) -> Vec<(Vec<FileEntry>, u64)> {
    // Separate directories from non-directory entries.
    let mut dirs: Vec<FileEntry> = Vec::new();
    let mut non_dirs: Vec<FileEntry> = Vec::new();
    for entry in files {
        if matches!(entry.entry_type, EntryType::Directory) {
            dirs.push(entry);
        } else {
            non_dirs.push(entry);
        }
    }

    // Split non-directory entries by size.
    let mut raw_parts: Vec<(Vec<FileEntry>, u64)> = Vec::new();
    let mut current_files: Vec<FileEntry> = Vec::new();
    let mut current_size: u64 = 0;

    for entry in non_dirs {
        if current_size + entry.size > max_size_per_part && !current_files.is_empty() {
            raw_parts.push((std::mem::take(&mut current_files), current_size));
            current_size = 0;
        }
        current_size += entry.size;
        current_files.push(entry);
    }
    if !current_files.is_empty() {
        raw_parts.push((current_files, current_size));
    }

    // Merge a trivially small trailing part into the previous part.
    // Greedy bin-packing with floor-divided budgets can leave a tiny
    // remainder (e.g. a single 495-byte file) that isn't worth its own
    // package.
    if raw_parts.len() >= 2 {
        let threshold = (max_size_per_part as f64 * TRAILING_PART_MERGE_THRESHOLD) as u64;
        let last_idx = raw_parts.len() - 1;
        if raw_parts[last_idx].1 <= threshold {
            let (trailing_files, trailing_size) = raw_parts.pop().unwrap();
            let prev = raw_parts.last_mut().unwrap();
            prev.0.extend(trailing_files);
            prev.1 += trailing_size;
        }
    }

    inject_ancestor_dirs(&mut raw_parts, &dirs);

    raw_parts
}

/// Inject ancestor directory entries into each split part.
///
/// Each part gets directory entries for all ancestor paths of its non-directory
/// files. Directories are placed before files so that directory entries precede
/// their children in archive order.
fn inject_ancestor_dirs(parts: &mut [(Vec<FileEntry>, u64)], dirs: &[FileEntry]) {
    for (part_files, _) in parts.iter_mut() {
        let mut needed_dirs: std::collections::BTreeSet<std::path::PathBuf> =
            std::collections::BTreeSet::new();
        for f in part_files.iter() {
            if matches!(f.entry_type, EntryType::Directory) {
                // Track existing directory entries so they're preserved.
                needed_dirs.insert(f.install_path.clone());
                continue;
            }
            let mut p = f.install_path.parent();
            while let Some(ancestor) = p {
                if ancestor == Path::new("") || ancestor == Path::new("/") {
                    break;
                }
                if !needed_dirs.insert(ancestor.to_path_buf()) {
                    break; // already seen this and all its ancestors
                }
                p = ancestor.parent();
            }
        }

        // Remove existing directory entries from part_files (we'll re-inject from dirs pool).
        part_files.retain(|f| !matches!(f.entry_type, EntryType::Directory));

        let mut dir_entries: Vec<FileEntry> = Vec::new();
        for dir in dirs {
            if needed_dirs.contains(&dir.install_path) {
                dir_entries.push(dir.clone());
            }
        }
        // Dirs before files so directory entries precede their children.
        dir_entries.append(part_files);
        *part_files = dir_entries;
    }
}

/// Split files into parts by directory path boundaries.
fn split_by_directory(
    files: Vec<FileEntry>,
    parts_config: &[crate::config::SplitPart],
    _pkg_name: &str,
) -> Vec<(Vec<FileEntry>, u64)> {
    // Separate directories from non-directory entries.
    let mut dirs: Vec<FileEntry> = Vec::new();
    let mut non_dirs: Vec<FileEntry> = Vec::new();
    for entry in files {
        if matches!(entry.entry_type, EntryType::Directory) {
            dirs.push(entry);
        } else {
            non_dirs.push(entry);
        }
    }

    let mut parts: Vec<(Vec<FileEntry>, u64)> =
        parts_config.iter().map(|_| (Vec::new(), 0u64)).collect();
    let mut remainder: Vec<FileEntry> = Vec::new();
    let mut remainder_size: u64 = 0;

    for entry in non_dirs {
        // Find which part this entry belongs to (if any).
        let target_part = parts_config.iter().enumerate().find_map(|(i, part_cfg)| {
            part_cfg
                .paths
                .iter()
                // Use Path::starts_with for component-aware matching so that
                // e.g. "/opt/pkg" does NOT match "/opt/pkg2/file".
                .any(|prefix| entry.install_path.starts_with(prefix))
                .then_some(i)
        });

        if let Some(i) = target_part {
            parts[i].1 += entry.size;
            parts[i].0.push(entry);
        } else {
            remainder_size += entry.size;
            remainder.push(entry);
        }
    }

    // Add remainder as an extra part if non-empty.
    if !remainder.is_empty() {
        parts.push((remainder, remainder_size));
    }

    // Filter out empty parts (configured paths that matched no files).
    parts.retain(|(files, _)| !files.is_empty());

    // Inject ancestor directory entries into each part.
    inject_ancestor_dirs(&mut parts, &dirs);

    parts
}

/// Pre-scanned hardlink family map for streaming split.
///
/// Maps each hardlink target to the indices of its link entries in the file list.
/// Used by the DEB streaming split builder to keep hardlink families together
/// in the same package part.
pub struct HardlinkFamilies {
    /// hardlink-target install_path → indices of link entries pointing to it.
    target_to_links: std::collections::HashMap<std::path::PathBuf, Vec<usize>>,
    /// Set of indices that are hardlink entries (to skip in the main loop).
    link_indices: std::collections::HashSet<usize>,
}

impl HardlinkFamilies {
    /// Pre-scan a file list and build the family map.
    pub fn scan(files: &[FileEntry]) -> Self {
        let mut target_to_links: std::collections::HashMap<std::path::PathBuf, Vec<usize>> =
            std::collections::HashMap::new();
        let mut link_indices = std::collections::HashSet::new();

        for (i, entry) in files.iter().enumerate() {
            if let EntryType::Hardlink { ref target } = entry.entry_type {
                target_to_links
                    .entry(target.clone())
                    .or_default()
                    .push(i);
                link_indices.insert(i);
            }
        }

        Self {
            target_to_links,
            link_indices,
        }
    }

    /// Returns true if this index is a hardlink entry that should be skipped
    /// in the main iteration (it will be pulled in when its target is processed).
    pub fn is_link(&self, index: usize) -> bool {
        self.link_indices.contains(&index)
    }

    /// If this file is a hardlink target, return the indices of all its link entries.
    pub fn links_for_target(&self, target_path: &std::path::Path) -> Option<&[usize]> {
        self.target_to_links.get(target_path).map(|v| v.as_slice())
    }

    /// Returns true if there are no hardlinks in the file list.
    pub fn is_empty(&self) -> bool {
        self.target_to_links.is_empty()
    }
}

/// When files are split across parts, hardlinks whose target is in a different
/// part must be converted to regular files with their actual size restored.
///
/// Note: `total_size` per part is adjusted upward when a hardlink (which
/// contributed 0 bytes during splitting) is promoted to a regular file.
/// This means the part's total_size may exceed the original split target,
/// which is acceptable — the alternative is a broken package.
fn fixup_hardlinks_across_parts(parts: &mut [(Vec<FileEntry>, u64)]) {
    use std::collections::HashSet;

    // Collect all install_paths per part.
    let part_paths: Vec<HashSet<std::path::PathBuf>> = parts
        .iter()
        .map(|(files, _)| files.iter().map(|f| f.install_path.clone()).collect())
        .collect();

    for (part_idx, (files, total_size)) in parts.iter_mut().enumerate() {
        for entry in files.iter_mut() {
            if let EntryType::Hardlink { ref target } = entry.entry_type {
                // Check if the target is in this same part.
                if !part_paths[part_idx].contains(target) {
                    // Target is in a different part — convert to regular file.
                    // Read actual size from source path; skip conversion if
                    // source_path is empty (synthetic entry) to avoid zero-size files.
                    if !entry.source_path.as_os_str().is_empty() {
                        let actual_size = std::fs::metadata(&entry.source_path)
                            .map(|m| m.len())
                            .unwrap_or(entry.size);
                        entry.entry_type = EntryType::RegularFile;
                        entry.size = actual_size;
                        *total_size += actual_size;
                    }
                }
            }
        }
    }
}

/// Construct the final sub-packages from split parts, adding a meta-package.
fn build_split_packages(
    parts: Vec<(Vec<FileEntry>, u64)>,
    pkg_name: &str,
    scripts: ResolvedScripts,
) -> Vec<SubPackage> {
    let mut sub_packages = Vec::new();

    // Meta-package (no files, has scripts and deps).
    sub_packages.push(SubPackage {
        name: pkg_name.to_string(),
        role: SubPackageRole::Meta,
        files: Vec::new(),
        total_size: 0,
        scripts,
    });

    // Part packages.
    for (i, (files, total_size)) in parts.into_iter().enumerate() {
        let part_num = (i + 1) as u32;
        sub_packages.push(SubPackage {
            name: format!("{pkg_name}-part{part_num}"),
            role: SubPackageRole::Part(part_num),
            files,
            total_size,
            scripts: ResolvedScripts::default(),
        });
    }

    sub_packages
}

/// Count only regular files, symlinks, and hardlinks (not directories) in a file list.
pub fn count_files(files: &[FileEntry]) -> usize {
    files
        .iter()
        .filter(|f| !matches!(f.entry_type, EntryType::Directory))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use std::path::PathBuf;

    /// Create a minimal config for planner tests.
    fn test_config(name: &str, strategy: &str) -> Config {
        Config {
            package: PackageConfig {
                name: name.to_string(),
                version: "1.0".to_string(),
                release: "1".to_string(),
                arch: "x86_64".to_string(),
                license: "MIT".to_string(),
                maintainer: "test".to_string(),
                description: "test".to_string(),
                url: None,
                vendor: None,
                dependencies: DependencyConfig::default(),
            },
            content: ContentConfig {
                defaults: ContentDefaults::default(),
                files: vec![],
                symlinks: vec![],
                directories: vec![],
                alternatives: vec![],
            },
            scripts: ScriptsConfig::default(),
            compression: CompressionConfig::default(),
            splitting: SplittingConfig {
                enabled: true,
                strategy: strategy.to_string(),
                max_size: None,
                parts: vec![],
            },
            signing: None,
            rpm: None,
            deb: None,
            build: None,
        }
    }

    /// Create a synthetic FileEntry for testing.
    fn test_file(install_path: &str, size: u64) -> FileEntry {
        FileEntry {
            install_path: PathBuf::from(install_path),
            source_path: PathBuf::from("/dev/null"),
            entry_type: EntryType::RegularFile,
            size,
            mode: 0o644,
            user: "root".to_string(),
            group: "root".to_string(),
            is_config: false,
        }
    }

    fn test_dir(install_path: &str) -> FileEntry {
        FileEntry {
            install_path: PathBuf::from(install_path),
            source_path: PathBuf::new(),
            entry_type: EntryType::Directory,
            size: 0,
            mode: 0o755,
            user: "root".to_string(),
            group: "root".to_string(),
            is_config: false,
        }
    }

    #[test]
    fn test_no_split_small_package() {
        let config = test_config("testpkg", "auto");
        let limits = FormatLimits::rpm();
        let files = vec![
            test_dir("/opt/testpkg"),
            test_file("/opt/testpkg/bin/hello", 1024),
            test_file("/opt/testpkg/lib/libfoo.so", 4096),
        ];

        let plan = Planner::plan_from_entries(&config, &limits, files, ResolvedScripts::default())
            .unwrap();

        assert!(!plan.is_split);
        assert_eq!(plan.sub_packages.len(), 1);
        assert_eq!(plan.sub_packages[0].role, SubPackageRole::Standalone);
        assert_eq!(plan.total_size, 5120); // 1024 + 4096
    }

    #[test]
    fn test_auto_split_deb_over_limit() {
        let config = test_config("bigpkg", "auto");
        let limits = FormatLimits::deb();

        // Create files that total ~30 GiB uncompressed.
        // With 0.35 ratio, compressed ~10.5 GiB > 9.3 GiB DEB limit.
        let file_size = 5 * 1024 * 1024 * 1024u64; // 5 GiB each
        let files = vec![
            test_file("/opt/bigpkg/data1.bin", file_size),
            test_file("/opt/bigpkg/data2.bin", file_size),
            test_file("/opt/bigpkg/data3.bin", file_size),
            test_file("/opt/bigpkg/data4.bin", file_size),
            test_file("/opt/bigpkg/data5.bin", file_size),
            test_file("/opt/bigpkg/data6.bin", file_size),
        ];

        let plan = Planner::plan_from_entries(&config, &limits, files, ResolvedScripts::default())
            .unwrap();

        // DEB auto-split now defers to the builder for monitored streaming split.
        assert!(plan.deferred_split, "DEB over-limit should set deferred_split");
        assert_eq!(plan.sub_packages.len(), 1, "deferred split produces single SubPackage");
        assert_eq!(plan.sub_packages[0].role, SubPackageRole::Standalone);
        assert!(!plan.sub_packages[0].files.is_empty());
    }

    #[test]
    fn test_size_based_split() {
        let mut config = test_config("sizepkg", "size");
        config.splitting.max_size = Some("100MiB".to_string());

        let limits = FormatLimits::rpm();
        let mib = 1024 * 1024u64;
        let files = vec![
            test_file("/opt/pkg/a.bin", 40 * mib),
            test_file("/opt/pkg/b.bin", 40 * mib),
            test_file("/opt/pkg/c.bin", 40 * mib),
            test_file("/opt/pkg/d.bin", 40 * mib),
        ];

        let plan = Planner::plan_from_entries(&config, &limits, files, ResolvedScripts::default())
            .unwrap();

        assert!(plan.is_split);
        // 160 MiB / 100 MiB limit = 2 parts (a+b in part1, c+d in part2).
        let part_count = plan
            .sub_packages
            .iter()
            .filter(|sp| matches!(sp.role, SubPackageRole::Part(_)))
            .count();
        assert_eq!(part_count, 2);
    }

    #[test]
    fn test_directory_based_split() {
        let mut config = test_config("dirpkg", "directory");
        config.splitting.parts = vec![
            SplitPart {
                name: "core".to_string(),
                paths: vec!["/opt/pkg/bin".to_string()],
            },
            SplitPart {
                name: "libs".to_string(),
                paths: vec!["/opt/pkg/lib".to_string()],
            },
        ];

        let limits = FormatLimits::rpm();
        let files = vec![
            test_file("/opt/pkg/bin/tool", 1024),
            test_file("/opt/pkg/lib/libfoo.so", 2048),
            test_file("/opt/pkg/share/docs.txt", 512),
        ];

        let plan = Planner::plan_from_entries(&config, &limits, files, ResolvedScripts::default())
            .unwrap();

        assert!(plan.is_split);

        // Should have: meta + core part + libs part + remainder part = 4 total.
        let parts: Vec<_> = plan
            .sub_packages
            .iter()
            .filter(|sp| matches!(sp.role, SubPackageRole::Part(_)))
            .collect();
        assert_eq!(parts.len(), 3, "expected core + libs + remainder parts");
    }

    #[test]
    fn test_extended_cpio_detection() {
        let config = test_config("largefile", "auto");
        let limits = FormatLimits::rpm();

        // File larger than 4 GiB.
        let files = vec![test_file(
            "/opt/pkg/huge.bin",
            0xFFFF_FFFF + 1, // Just over 4 GiB
        )];

        let plan = Planner::plan_from_entries(&config, &limits, files, ResolvedScripts::default())
            .unwrap();

        assert!(plan.needs_extended_cpio);
    }

    #[test]
    fn test_no_extended_cpio_when_all_small() {
        let config = test_config("smallpkg", "auto");
        let limits = FormatLimits::rpm();

        let files = vec![
            test_file("/opt/pkg/a.bin", 1024),
            test_file("/opt/pkg/b.bin", 2048),
        ];

        let plan = Planner::plan_from_entries(&config, &limits, files, ResolvedScripts::default())
            .unwrap();

        assert!(!plan.needs_extended_cpio);
    }

    #[test]
    fn test_extended_cpio_not_triggered_for_deb() {
        let config = test_config("debpkg", "auto");
        let limits = FormatLimits::deb();

        // DEB's max_file_size_standard is u64::MAX, so this should never trigger.
        let files = vec![test_file("/opt/pkg/huge.bin", 5 * 1024 * 1024 * 1024)];

        let plan = Planner::plan_from_entries(&config, &limits, files, ResolvedScripts::default())
            .unwrap();

        assert!(!plan.needs_extended_cpio);
    }

    #[test]
    fn test_splitting_disabled_within_limits() {
        let mut config = test_config("smallpkg", "auto");
        config.splitting.enabled = false;

        let limits = FormatLimits::rpm();
        let files = vec![test_file("/opt/pkg/hello", 1024)];

        let plan = Planner::plan_from_entries(&config, &limits, files, ResolvedScripts::default())
            .unwrap();

        assert!(!plan.is_split);
        assert_eq!(plan.sub_packages[0].role, SubPackageRole::Standalone);
    }

    #[test]
    fn test_splitting_disabled_exceeds_limits() {
        let mut config = test_config("hugepkg", "auto");
        config.splitting.enabled = false;

        let limits = FormatLimits::deb();

        // Create enough data that compressed estimate exceeds DEB limit.
        // 30 GiB * 0.35 = ~10.5 GiB > 9.3 GiB.
        let files = vec![test_file("/opt/pkg/huge.bin", 30 * 1024 * 1024 * 1024)];

        let result =
            Planner::plan_from_entries(&config, &limits, files, ResolvedScripts::default());

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            PlanError::ExceedsLimits { .. }
        ));
    }

    #[test]
    fn test_total_size_calculation() {
        let config = test_config("testpkg", "auto");
        let limits = FormatLimits::rpm();
        let files = vec![
            test_file("/opt/pkg/a", 100),
            test_file("/opt/pkg/b", 200),
            test_file("/opt/pkg/c", 300),
            test_dir("/opt/pkg"),
        ];

        let plan = Planner::plan_from_entries(&config, &limits, files, ResolvedScripts::default())
            .unwrap();

        assert_eq!(plan.total_size, 600);
    }

    #[test]
    fn test_meta_package_has_scripts() {
        let mut config = test_config("splitpkg", "size");
        config.splitting.max_size = Some("50MiB".to_string());

        let limits = FormatLimits::rpm();
        let mib = 1024 * 1024u64;
        let files = vec![
            test_file("/opt/pkg/a.bin", 40 * mib),
            test_file("/opt/pkg/b.bin", 40 * mib),
        ];

        let scripts = ResolvedScripts {
            post_install: Some("echo installed".to_string()),
            ..Default::default()
        };

        let plan = Planner::plan_from_entries(&config, &limits, files, scripts).unwrap();

        assert!(plan.is_split);

        // Meta-package should have the scripts.
        let meta = plan
            .sub_packages
            .iter()
            .find(|sp| sp.role == SubPackageRole::Meta)
            .unwrap();
        assert!(meta.scripts.post_install.is_some());

        // Parts should have empty scripts.
        for sp in plan
            .sub_packages
            .iter()
            .filter(|sp| matches!(sp.role, SubPackageRole::Part(_)))
        {
            assert!(sp.scripts.post_install.is_none());
        }
    }

    #[test]
    fn test_auto_split_deb_borderline() {
        // 25 GiB uncompressed × 0.35 = 8.75 GiB estimated.
        // Raw DEB limit is 9.999 GiB, but with 0.80 headroom the threshold is ~8.0 GiB.
        // 8.75 GiB > 8.0 GiB → should trigger deferred split.
        let config = test_config("borderline", "auto");
        let limits = FormatLimits::deb();

        let gib = 1024 * 1024 * 1024u64;
        let files = vec![
            test_file("/opt/pkg/data1.bin", 5 * gib),
            test_file("/opt/pkg/data2.bin", 5 * gib),
            test_file("/opt/pkg/data3.bin", 5 * gib),
            test_file("/opt/pkg/data4.bin", 5 * gib),
            test_file("/opt/pkg/data5.bin", 5 * gib),
        ];

        let plan = Planner::plan_from_entries(&config, &limits, files, ResolvedScripts::default())
            .unwrap();

        // DEB borderline now defers to the builder.
        assert!(plan.deferred_split, "expected deferred_split for borderline DEB");
        assert_eq!(plan.sub_packages.len(), 1);
        assert_eq!(plan.sub_packages[0].role, SubPackageRole::Standalone);
    }

    #[test]
    fn test_splitting_disabled_borderline_errors() {
        // With splitting disabled, a borderline package should error out.
        let mut config = test_config("borderline", "auto");
        config.splitting.enabled = false;

        let limits = FormatLimits::deb();
        let gib = 1024 * 1024 * 1024u64;
        // 25 GiB × 0.35 = 8.75 GiB > 8.0 GiB threshold
        let files = vec![test_file("/opt/pkg/huge.bin", 25 * gib)];

        let result =
            Planner::plan_from_entries(&config, &limits, files, ResolvedScripts::default());

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            PlanError::ExceedsLimits { .. }
        ));
    }

    #[test]
    fn test_plan_warning_near_limit() {
        // A package that doesn't need splitting but is >60% of the limit.
        // 18 GiB × 0.35 = 6.3 GiB → 63% of 9.999 GiB → should warn.
        let config = test_config("nearlimit", "auto");
        let limits = FormatLimits::deb();

        let gib = 1024 * 1024 * 1024u64;
        let files = vec![
            test_file("/opt/pkg/data1.bin", 9 * gib),
            test_file("/opt/pkg/data2.bin", 9 * gib),
        ];

        let plan = Planner::plan_from_entries(&config, &limits, files, ResolvedScripts::default())
            .unwrap();

        assert!(!plan.is_split, "should not split at 63%");
        assert!(
            plan.warnings.iter().any(|w| w.contains("consider enabling splitting")),
            "expected near-limit warning, got: {:?}",
            plan.warnings
        );
    }

    #[test]
    fn test_no_warning_for_small_package() {
        // Small package should have no warnings.
        let config = test_config("smallpkg", "auto");
        let limits = FormatLimits::deb();

        let files = vec![test_file("/opt/pkg/small.bin", 1024)];

        let plan = Planner::plan_from_entries(&config, &limits, files, ResolvedScripts::default())
            .unwrap();

        assert!(!plan.is_split);
        assert!(plan.warnings.is_empty(), "expected no warnings for small package");
    }

    #[test]
    fn test_no_warning_for_rpm() {
        // RPM has no practical limit (u64::MAX), so no warnings should be generated.
        let config = test_config("rpmpkg", "auto");
        let limits = FormatLimits::rpm();

        let gib = 1024 * 1024 * 1024u64;
        let files = vec![test_file("/opt/pkg/data.bin", 20 * gib)];

        let plan = Planner::plan_from_entries(&config, &limits, files, ResolvedScripts::default())
            .unwrap();

        assert!(plan.warnings.is_empty(), "expected no warnings for RPM");
    }

    #[test]
    fn test_count_files() {
        let files = vec![
            test_dir("/opt/pkg"),
            test_file("/opt/pkg/a", 100),
            test_file("/opt/pkg/b", 200),
            FileEntry {
                install_path: PathBuf::from("/usr/bin/link"),
                source_path: PathBuf::new(),
                entry_type: EntryType::Symlink {
                    target: PathBuf::from("/opt/pkg/a"),
                },
                size: 0,
                mode: 0o120777,
                user: "root".to_string(),
                group: "root".to_string(),
                is_config: false,
            },
        ];

        assert_eq!(count_files(&files), 3); // 2 regular + 1 symlink, no dirs
    }

    #[test]
    fn test_split_by_size_directories_in_all_parts() {
        let mib = 1024 * 1024u64;
        // Sorted input: directories first, then files.
        let files = vec![
            test_dir("/opt"),
            test_dir("/opt/pkg"),
            test_dir("/opt/pkg/bin"),
            test_file("/opt/pkg/bin/a.bin", 50 * mib),
            test_file("/opt/pkg/bin/b.bin", 50 * mib),
            test_file("/opt/pkg/bin/c.bin", 50 * mib),
        ];

        let parts = split_by_size(files, 80 * mib, "testpkg");

        // Should split into at least 2 parts.
        assert!(parts.len() >= 2, "expected >= 2 parts, got {}", parts.len());

        // Every part should contain the directory entries for /opt, /opt/pkg, /opt/pkg/bin.
        for (i, (part_files, _)) in parts.iter().enumerate() {
            let dir_paths: Vec<&Path> = part_files
                .iter()
                .filter(|e| matches!(e.entry_type, EntryType::Directory))
                .map(|e| e.install_path.as_path())
                .collect();
            assert!(
                dir_paths.contains(&Path::new("/opt")),
                "part {i} missing /opt directory"
            );
            assert!(
                dir_paths.contains(&Path::new("/opt/pkg")),
                "part {i} missing /opt/pkg directory"
            );
            assert!(
                dir_paths.contains(&Path::new("/opt/pkg/bin")),
                "part {i} missing /opt/pkg/bin directory"
            );
        }

        // Directories should come before files in each part.
        for (i, (part_files, _)) in parts.iter().enumerate() {
            let first_file_idx = part_files
                .iter()
                .position(|e| !matches!(e.entry_type, EntryType::Directory));
            let last_dir_idx = part_files
                .iter()
                .rposition(|e| matches!(e.entry_type, EntryType::Directory));
            if let (Some(first_file), Some(last_dir)) = (first_file_idx, last_dir_idx) {
                assert!(
                    last_dir < first_file,
                    "part {i}: directories should precede files"
                );
            }
        }
    }

    #[test]
    fn test_hardlink_fixup_cross_part_promotion() {
        // When a hardlink's target is in a different part, it should be
        // promoted to a regular file.
        let mib = 1024 * 1024u64;
        let files = vec![
            test_dir("/opt"),
            test_dir("/opt/pkg"),
            test_file("/opt/pkg/original.bin", 50 * mib),
            // Extra file ensures part 2 is above the trailing merge threshold.
            test_file("/opt/pkg/data.bin", 30 * mib),
            FileEntry {
                install_path: PathBuf::from("/opt/pkg/link.bin"),
                source_path: PathBuf::from("/dev/null"), // exists on disk
                entry_type: EntryType::Hardlink {
                    target: PathBuf::from("/opt/pkg/original.bin"),
                },
                size: 0,
                mode: 0o644,
                user: "root".to_string(),
                group: "root".to_string(),
                is_config: false,
            },
        ];

        // Split at 40 MiB. original (50 MiB) goes first (exceeds 40 but is first
        // file so no split yet). Then data (30 MiB): 50+30=80 > 40 && !empty → split.
        // Part 2: data (30 MiB) + link (0 bytes). 30 MiB is 75% of 40 MiB → not merged.
        // original is in part 1, link is in part 2 → cross-part.
        let mut parts = split_by_size(files, 40 * mib, "testpkg");
        assert!(parts.len() >= 2, "should have at least 2 parts");

        fixup_hardlinks_across_parts(&mut parts);

        // Link should now be a regular file.
        let link = parts
            .iter()
            .flat_map(|(f, _)| f)
            .find(|e| e.install_path == Path::new("/opt/pkg/link.bin"))
            .unwrap();
        assert!(
            matches!(link.entry_type, EntryType::RegularFile),
            "cross-part hardlink should be promoted to RegularFile"
        );
    }

    #[test]
    fn test_hardlink_fixup_same_part_preserved() {
        // When a hardlink's target is in the same part, it should stay as a hardlink.
        let mib = 1024 * 1024u64;
        let files = vec![
            test_dir("/opt"),
            test_dir("/opt/pkg"),
            test_file("/opt/pkg/original.bin", 10 * mib),
            FileEntry {
                install_path: PathBuf::from("/opt/pkg/link.bin"),
                source_path: PathBuf::from("/dev/null"),
                entry_type: EntryType::Hardlink {
                    target: PathBuf::from("/opt/pkg/original.bin"),
                },
                size: 0,
                mode: 0o644,
                user: "root".to_string(),
                group: "root".to_string(),
                is_config: false,
            },
        ];

        // Large limit ensures both end up in the same part.
        let mut parts = split_by_size(files, 100 * mib, "testpkg");
        assert_eq!(parts.len(), 1, "should be a single part");

        fixup_hardlinks_across_parts(&mut parts);

        let link = parts
            .iter()
            .flat_map(|(f, _)| f)
            .find(|e| e.install_path == Path::new("/opt/pkg/link.bin"))
            .unwrap();
        assert!(
            matches!(link.entry_type, EntryType::Hardlink { .. }),
            "same-part hardlink should stay as Hardlink"
        );
    }

    #[test]
    fn test_file_distribution_across_size_split() {
        // Verify that no files are lost or duplicated across parts.
        let mib = 1024 * 1024u64;
        let files = vec![
            test_dir("/opt"),
            test_dir("/opt/pkg"),
            test_file("/opt/pkg/a.bin", 30 * mib),
            test_file("/opt/pkg/b.bin", 30 * mib),
            test_file("/opt/pkg/c.bin", 30 * mib),
            test_file("/opt/pkg/d.bin", 30 * mib),
            test_file("/opt/pkg/e.bin", 30 * mib),
        ];

        let original_non_dirs: Vec<PathBuf> = files
            .iter()
            .filter(|f| !matches!(f.entry_type, EntryType::Directory))
            .map(|f| f.install_path.clone())
            .collect();

        let parts = split_by_size(files, 50 * mib, "testpkg");

        // Collect all non-directory files across all parts.
        let mut distributed: Vec<PathBuf> = parts
            .iter()
            .flat_map(|(f, _)| f)
            .filter(|e| !matches!(e.entry_type, EntryType::Directory))
            .map(|e| e.install_path.clone())
            .collect();
        distributed.sort();
        let mut expected = original_non_dirs;
        expected.sort();

        assert_eq!(
            distributed, expected,
            "all non-directory files must appear exactly once across parts"
        );
    }

    #[test]
    fn test_directory_split_ancestor_dirs_in_all_parts() {
        // Verify that directory-based split injects ancestor directories.
        let files = vec![
            test_dir("/opt"),
            test_dir("/opt/pkg"),
            test_dir("/opt/pkg/bin"),
            test_dir("/opt/pkg/lib"),
            test_dir("/opt/pkg/share"),
            test_file("/opt/pkg/bin/tool", 1024),
            test_file("/opt/pkg/lib/libfoo.so", 2048),
            test_file("/opt/pkg/share/docs.txt", 512),
        ];

        let parts_config = vec![
            SplitPart {
                name: "core".to_string(),
                paths: vec!["/opt/pkg/bin".to_string()],
            },
            SplitPart {
                name: "libs".to_string(),
                paths: vec!["/opt/pkg/lib".to_string()],
            },
        ];

        let parts = split_by_directory(files, &parts_config, "testpkg");

        // Should have 3 parts: core, libs, remainder.
        assert_eq!(parts.len(), 3, "expected core + libs + remainder");

        // Each part should have ancestor directories /opt and /opt/pkg.
        for (i, (part_files, _)) in parts.iter().enumerate() {
            let dir_paths: Vec<&Path> = part_files
                .iter()
                .filter(|e| matches!(e.entry_type, EntryType::Directory))
                .map(|e| e.install_path.as_path())
                .collect();
            assert!(
                dir_paths.contains(&Path::new("/opt")),
                "part {i} missing /opt ancestor directory"
            );
            assert!(
                dir_paths.contains(&Path::new("/opt/pkg")),
                "part {i} missing /opt/pkg ancestor directory"
            );
        }

        // Core part should have /opt/pkg/bin directory.
        let core_dirs: Vec<&Path> = parts[0]
            .0
            .iter()
            .filter(|e| matches!(e.entry_type, EntryType::Directory))
            .map(|e| e.install_path.as_path())
            .collect();
        assert!(core_dirs.contains(&Path::new("/opt/pkg/bin")));

        // Libs part should have /opt/pkg/lib directory.
        let libs_dirs: Vec<&Path> = parts[1]
            .0
            .iter()
            .filter(|e| matches!(e.entry_type, EntryType::Directory))
            .map(|e| e.install_path.as_path())
            .collect();
        assert!(libs_dirs.contains(&Path::new("/opt/pkg/lib")));
    }

    #[test]
    fn test_directory_split_empty_part_filtered() {
        // A configured part whose paths match no files should be filtered out.
        let files = vec![
            test_dir("/opt"),
            test_dir("/opt/pkg"),
            test_dir("/opt/pkg/bin"),
            test_file("/opt/pkg/bin/tool", 1024),
        ];

        let parts_config = vec![
            SplitPart {
                name: "core".to_string(),
                paths: vec!["/opt/pkg/bin".to_string()],
            },
            SplitPart {
                name: "empty".to_string(),
                paths: vec!["/opt/nonexistent".to_string()],
            },
        ];

        let parts = split_by_directory(files, &parts_config, "testpkg");

        // Empty part should be filtered out — only core part remains.
        assert_eq!(parts.len(), 1, "empty part should be filtered out");
        let has_tool = parts[0]
            .0
            .iter()
            .any(|e| e.install_path == Path::new("/opt/pkg/bin/tool"));
        assert!(has_tool, "core part should contain the tool file");
    }

    #[test]
    fn test_config_validates_size_strategy_requires_max_size() {
        let mut config = test_config("testpkg", "size");
        // max_size is None by default in test_config.
        assert!(config.splitting.max_size.is_none());
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("max_size is required"),
            "expected max_size required error, got: {err}"
        );

        // With max_size set, should pass.
        config.splitting.max_size = Some("1GiB".to_string());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validates_directory_strategy_requires_parts() {
        let config = test_config("testpkg", "directory");
        // parts is empty by default in test_config.
        assert!(config.splitting.parts.is_empty());
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("parts must be non-empty"),
            "expected parts required error, got: {err}"
        );
    }

    #[test]
    fn test_config_validates_max_size_format() {
        let mut config = test_config("testpkg", "auto");
        config.splitting.max_size = Some("invalid_size".to_string());
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("max_size") && err.contains("invalid"),
            "expected max_size format error, got: {err}"
        );
    }

    // --- Trailing runt merge tests ---

    #[test]
    fn test_split_by_size_merges_trailing_runt() {
        // With max_size_per_part = 100, two files of 99 each fill parts 1
        // and 2 just under the limit, and a 1-byte file would spill into a
        // would-be part 3. The merge logic should fold it back into part 2.
        let files = vec![
            test_dir("/opt"),
            test_dir("/opt/pkg"),
            test_file("/opt/pkg/a.bin", 99),
            test_file("/opt/pkg/b.bin", 99),
            test_file("/opt/pkg/tiny.txt", 1),
        ];

        let parts = split_by_size(files, 100, "testpkg");

        assert_eq!(
            parts.len(),
            2,
            "trailing runt part should be merged; got {} parts",
            parts.len()
        );

        // The tiny file should be in the last part.
        let last_non_dirs: Vec<&std::path::Path> = parts
            .last()
            .unwrap()
            .0
            .iter()
            .filter(|e| !matches!(e.entry_type, EntryType::Directory))
            .map(|e| e.install_path.as_path())
            .collect();
        assert!(
            last_non_dirs
                .iter()
                .any(|p| p.ends_with("tiny.txt")),
            "tiny file should be in the last (merged) part"
        );
    }

    #[test]
    fn test_split_by_size_keeps_large_last_part() {
        // When the last part is substantial (well above 2% threshold), it
        // should NOT be merged.
        let mib = 1024 * 1024u64;
        let files = vec![
            test_dir("/opt"),
            test_dir("/opt/pkg"),
            test_file("/opt/pkg/a.bin", 80 * mib),
            test_file("/opt/pkg/b.bin", 80 * mib),
            test_file("/opt/pkg/c.bin", 60 * mib),
        ];

        // max = 100 MiB. Part 1: a (80 MiB). Part 2: b (80 MiB). Part 3: c (60 MiB).
        // 60 MiB is 60% of 100 MiB — well above the 2% threshold.
        let parts = split_by_size(files, 100 * mib, "testpkg");

        assert_eq!(
            parts.len(),
            3,
            "large last part should NOT be merged; got {} parts",
            parts.len()
        );
    }

    #[test]
    fn test_split_by_size_merge_preserves_all_files() {
        // Verify that merging the trailing part does not lose any files.
        let files = vec![
            test_dir("/opt"),
            test_dir("/opt/pkg"),
            test_file("/opt/pkg/a.bin", 90),
            test_file("/opt/pkg/b.bin", 90),
            test_file("/opt/pkg/c.bin", 1),
            test_file("/opt/pkg/d.bin", 1),
        ];

        let original_non_dirs: Vec<std::path::PathBuf> = files
            .iter()
            .filter(|f| !matches!(f.entry_type, EntryType::Directory))
            .map(|f| f.install_path.clone())
            .collect();

        let parts = split_by_size(files, 100, "testpkg");

        let mut distributed: Vec<std::path::PathBuf> = parts
            .iter()
            .flat_map(|(f, _)| f)
            .filter(|e| !matches!(e.entry_type, EntryType::Directory))
            .map(|e| e.install_path.clone())
            .collect();
        distributed.sort();
        let mut expected = original_non_dirs;
        expected.sort();

        assert_eq!(
            distributed, expected,
            "all files must be preserved after trailing merge"
        );
    }

    #[test]
    fn test_auto_split_deb_defers_instead_of_splitting() {
        // Previously this tested the trailing runt merge for DEB.
        // Now DEB auto-split defers to the builder, so verify deferred_split is set
        // and all files are in a single SubPackage.
        let config = test_config("matlab", "auto");
        let limits = FormatLimits::deb();

        let gib = 1024 * 1024 * 1024u64;
        let chunk = 3 * gib;
        let files = vec![
            test_dir("/opt"),
            test_dir("/opt/matlab"),
            test_file("/opt/matlab/part_a1", chunk),
            test_file("/opt/matlab/part_a2", chunk),
            test_file("/opt/matlab/part_a3", chunk),
            test_file("/opt/matlab/part_a4", chunk),
            test_file("/opt/matlab/part_b1", chunk),
            test_file("/opt/matlab/part_b2", chunk),
            test_file("/opt/matlab/part_b3", chunk),
            test_file("/opt/matlab/part_b4", chunk),
            test_file("/opt/matlab/tiny_leftover.txt", 495),
        ];

        let plan = Planner::plan_from_entries(
            &config,
            &limits,
            files,
            ResolvedScripts::default(),
        )
        .unwrap();

        assert!(plan.deferred_split, "DEB auto-split should defer to builder");
        assert_eq!(plan.sub_packages.len(), 1);
        assert_eq!(plan.sub_packages[0].role, SubPackageRole::Standalone);

        // All files should be present in the single SubPackage.
        let has_tiny = plan.sub_packages[0]
            .files
            .iter()
            .any(|f| f.install_path.ends_with("tiny_leftover.txt"));
        assert!(has_tiny, "tiny file must not be lost");
    }

    // --- HardlinkFamilies tests ---

    #[test]
    fn test_hardlink_families_no_links() {
        let files = vec![test_file("/opt/a", 100), test_file("/opt/b", 200)];
        let families = HardlinkFamilies::scan(&files);
        assert!(families.is_empty());
        assert!(!families.is_link(0));
        assert!(!families.is_link(1));
        assert!(families.links_for_target(Path::new("/opt/a")).is_none());
    }

    #[test]
    fn test_hardlink_families_basic() {
        let files = vec![
            test_file("/opt/target", 100),
            FileEntry {
                install_path: PathBuf::from("/opt/link1"),
                source_path: PathBuf::from("/src/target"),
                entry_type: EntryType::Hardlink {
                    target: PathBuf::from("/opt/target"),
                },
                size: 0,
                mode: 0o644,
                user: "root".to_string(),
                group: "root".to_string(),
                is_config: false,
            },
            FileEntry {
                install_path: PathBuf::from("/opt/link2"),
                source_path: PathBuf::from("/src/target"),
                entry_type: EntryType::Hardlink {
                    target: PathBuf::from("/opt/target"),
                },
                size: 0,
                mode: 0o644,
                user: "root".to_string(),
                group: "root".to_string(),
                is_config: false,
            },
        ];
        let families = HardlinkFamilies::scan(&files);
        assert!(!families.is_empty());
        assert!(!families.is_link(0)); // target is not a link
        assert!(families.is_link(1)); // link1 is a link
        assert!(families.is_link(2)); // link2 is a link

        let links = families
            .links_for_target(Path::new("/opt/target"))
            .unwrap();
        assert_eq!(links, &[1, 2]);
    }

    // --- Deferred split tests ---

    #[test]
    fn test_deferred_split_for_deb_auto() {
        use crate::types::FormatLimits;

        // Create a config with auto-split enabled and large enough data to trigger it.
        let mut config = test_config("bigpkg", "auto");
        config.splitting.enabled = true;

        let limits = FormatLimits::deb();

        // Create files whose estimated compressed size exceeds the DEB limit.
        // DEB limit = 9_999_999_999, zstd ratio = 0.35, headroom = 0.80.
        // Threshold = 9_999_999_999 * 0.80 = 7_999_999_999.
        // Need estimated > threshold: total * 0.35 > 7_999_999_999
        // total > 22_857_142_854
        let files = vec![test_file("/opt/bigfile", 25_000_000_000)];
        let scripts = ResolvedScripts::default();

        let plan = Planner::plan_from_entries(&config, &limits, files, scripts).unwrap();
        assert!(plan.deferred_split, "DEB auto-split should set deferred_split");
        assert_eq!(plan.sub_packages.len(), 1, "should produce single SubPackage");
        assert_eq!(plan.sub_packages[0].role, SubPackageRole::Standalone);
        assert!(!plan.is_split, "is_split should be false (no Meta subpackage)");
    }

    #[test]
    fn test_no_deferred_split_for_rpm_auto() {
        use crate::types::FormatLimits;

        let mut config = test_config("bigpkg", "auto");
        config.splitting.enabled = true;

        let limits = FormatLimits::rpm();

        // RPM has u64::MAX limit, so even with huge files estimation-based split is used.
        // But RPM auto-split threshold is u64::MAX * 0.80 which is still enormous.
        // With RPM, auto-split should never trigger deferred_split.
        let files = vec![test_file("/opt/bigfile", 100)];
        let scripts = ResolvedScripts::default();

        let plan = Planner::plan_from_entries(&config, &limits, files, scripts).unwrap();
        assert!(!plan.deferred_split, "RPM should not set deferred_split");
    }
}
