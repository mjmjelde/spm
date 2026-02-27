/// Package planning: file tree analysis, split detection, and plan generation.
///
/// The planner takes a parsed config and format limits, walks the source directory,
/// determines whether splitting is needed, resolves scripts (including alternatives
/// injection), and produces a `PackagePlan` that later phases use to build packages.
use crate::alternatives::{resolve_scripts, ResolvedScripts};
use crate::config::Config;
use crate::error::PlanError;
use crate::filetree::{EntryType, FileEntry, FileTree};
use crate::types::{estimated_compression_ratio, parse_size, FormatLimits};

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
    pub fn plan(config: &Config, limits: &FormatLimits) -> Result<PackagePlan, PlanError> {
        let source_dir = &config.content.source_dir;

        // Walk the file tree.
        let files = FileTree::walk(source_dir, &config.content)?;

        // Calculate total size and detect extended cpio need.
        let total_size: u64 = files.iter().map(|f| f.size).sum();
        let needs_extended_cpio = files.iter().any(|f| {
            matches!(f.entry_type, EntryType::RegularFile) && f.size > limits.max_file_size_standard
        });

        // Resolve scripts with alternatives injection.
        // Config dir is the parent of the config file, but since we don't have
        // the config file path here, we use the source_dir's parent or cwd.
        // In practice, the CLI passes the config dir separately.
        let config_dir = std::env::current_dir().unwrap_or_default();
        let scripts = resolve_scripts(&config.scripts, &config.content.alternatives, &config_dir)?;

        let pkg_name = &config.package.name;

        // Determine if splitting is needed.
        let sub_packages = if !config.splitting.enabled {
            // Splitting disabled — check if we're within limits.
            let ratio = estimated_compression_ratio(&config.compression.algorithm);
            let estimated_compressed = (total_size as f64 * ratio) as u64;
            if estimated_compressed > limits.max_compressed_payload {
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
                    let ratio = estimated_compression_ratio(&config.compression.algorithm);
                    let estimated_compressed = (total_size as f64 * ratio) as u64;

                    if estimated_compressed <= limits.max_compressed_payload {
                        // No split needed.
                        vec![SubPackage {
                            name: pkg_name.clone(),
                            role: SubPackageRole::Standalone,
                            files,
                            total_size,
                            scripts,
                        }]
                    } else {
                        // Split needed. Calculate max uncompressed size per part.
                        let safety_factor = 0.90;
                        let max_uncompressed_per_part =
                            (limits.max_compressed_payload as f64 * safety_factor / ratio) as u64;
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

        Ok(PackagePlan {
            name: pkg_name.clone(),
            version: config.package.version.clone(),
            release: config.package.release.clone(),
            arch: config.package.arch.clone(),
            sub_packages,
            is_split,
            needs_extended_cpio,
            total_size,
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

        let sub_packages = if !config.splitting.enabled {
            let ratio = estimated_compression_ratio(&config.compression.algorithm);
            let estimated_compressed = (total_size as f64 * ratio) as u64;
            if estimated_compressed > limits.max_compressed_payload {
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
                    let ratio = estimated_compression_ratio(&config.compression.algorithm);
                    let estimated_compressed = (total_size as f64 * ratio) as u64;

                    if estimated_compressed <= limits.max_compressed_payload {
                        vec![SubPackage {
                            name: pkg_name.clone(),
                            role: SubPackageRole::Standalone,
                            files,
                            total_size,
                            scripts,
                        }]
                    } else {
                        let safety_factor = 0.90;
                        let max_uncompressed_per_part =
                            (limits.max_compressed_payload as f64 * safety_factor / ratio) as u64;
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

        Ok(PackagePlan {
            name: pkg_name.clone(),
            version: config.package.version.clone(),
            release: config.package.release.clone(),
            arch: config.package.arch.clone(),
            sub_packages,
            is_split,
            needs_extended_cpio,
            total_size,
        })
    }
}

/// Split files into parts by accumulated size.
/// Files are assumed to already be sorted by install_path.
fn split_by_size(
    files: Vec<FileEntry>,
    max_size_per_part: u64,
    _pkg_name: &str,
) -> Vec<(Vec<FileEntry>, u64)> {
    let mut parts: Vec<(Vec<FileEntry>, u64)> = Vec::new();
    let mut current_files: Vec<FileEntry> = Vec::new();
    let mut current_size: u64 = 0;

    for entry in files {
        // Directories go in whatever part their first file is in.
        // Don't count them toward the size limit.
        if matches!(entry.entry_type, EntryType::Directory) {
            current_files.push(entry);
            continue;
        }

        if current_size + entry.size > max_size_per_part && !current_files.is_empty() {
            parts.push((std::mem::take(&mut current_files), current_size));
            current_size = 0;
        }
        current_size += entry.size;
        current_files.push(entry);
    }

    if !current_files.is_empty() {
        parts.push((current_files, current_size));
    }

    parts
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
        let install_str = entry.install_path.to_string_lossy();
        let mut assigned = false;

        for (i, part_cfg) in parts_config.iter().enumerate() {
            for prefix in &part_cfg.paths {
                if install_str.starts_with(prefix) {
                    parts[i].1 += entry.size;
                    parts[i].0.push(entry.clone());
                    assigned = true;
                    break;
                }
            }
            if assigned {
                break;
            }
        }

        if !assigned {
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
                    // We need to restore the actual file size.
                    // Since the hardlink entry has size 0, we read the size from
                    // the source path metadata if available.
                    let actual_size = std::fs::metadata(&entry.source_path)
                        .map(|m| m.len())
                        .unwrap_or(0);
                    entry.entry_type = EntryType::RegularFile;
                    entry.size = actual_size;
                    *total_size += actual_size;
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
                source_dir: PathBuf::from("/tmp"),
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
}
