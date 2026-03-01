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
                    } else {
                        // Split needed. Calculate number of even parts, then
                        // divide total uncompressed size equally.
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
                    let max_size_str = config.splitting.max_size.as_deref().unwrap_or("4GiB");
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
                    } else {
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
                    let max_size_str = config.splitting.max_size.as_deref().unwrap_or("4GiB");
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
        })
    }
}

/// Build warnings about packages that are close to format limits.
fn build_warnings(
    is_split: bool,
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
    } else if !is_split && pct > 60.0 {
        // Not split, but getting close to the limit.
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

    // Inject required directory entries into each part.
    // Each part gets directory entries for all ancestor paths of its files.
    for (part_files, _) in &mut raw_parts {
        let mut needed_dirs: std::collections::BTreeSet<&Path> =
            std::collections::BTreeSet::new();
        for f in part_files.iter() {
            let mut p = f.install_path.parent();
            while let Some(ancestor) = p {
                if ancestor == Path::new("") || ancestor == Path::new("/") {
                    break;
                }
                if !needed_dirs.insert(ancestor) {
                    break; // already seen this and all its ancestors
                }
                p = ancestor.parent();
            }
        }

        let mut dir_entries: Vec<FileEntry> = Vec::new();
        for dir in &dirs {
            if needed_dirs.contains(dir.install_path.as_path()) {
                dir_entries.push(dir.clone());
            }
        }
        // Sort dirs before files so directory entries precede their children.
        dir_entries.append(part_files);
        *part_files = dir_entries;
    }

    raw_parts
}

/// Split files into parts by directory path boundaries.
fn split_by_directory(
    files: Vec<FileEntry>,
    parts_config: &[crate::config::SplitPart],
    _pkg_name: &str,
) -> Vec<(Vec<FileEntry>, u64)> {
    let mut parts: Vec<(Vec<FileEntry>, u64)> =
        parts_config.iter().map(|_| (Vec::new(), 0u64)).collect();
    let mut remainder: Vec<FileEntry> = Vec::new();
    let mut remainder_size: u64 = 0;

    for entry in files {
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

    parts
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

        assert!(plan.is_split);
        assert!(plan.sub_packages.len() > 2); // Meta + at least 2 parts

        // First sub-package should be the meta-package.
        assert_eq!(plan.sub_packages[0].role, SubPackageRole::Meta);
        assert!(plan.sub_packages[0].files.is_empty());

        // Remaining should be parts.
        for (i, sp) in plan.sub_packages.iter().skip(1).enumerate() {
            assert_eq!(sp.role, SubPackageRole::Part((i + 1) as u32));
            assert!(!sp.files.is_empty());
        }
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
        // 160 MiB / 100 MiB limit = 2 parts.
        let part_count = plan
            .sub_packages
            .iter()
            .filter(|sp| matches!(sp.role, SubPackageRole::Part(_)))
            .count();
        assert!(part_count >= 2);
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

        // Should have: meta + core part + libs part + remainder part.
        let parts: Vec<_> = plan
            .sub_packages
            .iter()
            .filter(|sp| matches!(sp.role, SubPackageRole::Part(_)))
            .collect();
        assert!(parts.len() >= 2);
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
        // 8.75 GiB > 8.0 GiB → should trigger splitting.
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

        assert!(plan.is_split, "expected splitting for borderline package");
        // Should have a warning about safety margin since estimated < raw limit.
        assert!(
            plan.warnings.iter().any(|w| w.contains("safety margin")),
            "expected safety margin warning, got: {:?}",
            plan.warnings
        );
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
}
