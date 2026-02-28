//! DEB control file generation.
//!
//! Generates the `control`, `conffiles`, and `md5sums` files
//! that go inside `control.tar.{zst,gz}`.

use std::io::Read;
use std::path::Path;

use md5::{Digest, Md5};

use spm_core::config::Config;
use spm_core::filetree::{EntryType, FileEntry};
use spm_core::planner::{PackagePlan, SubPackage};

use crate::error::DebError;

/// Map architecture strings from spm (RPM-style) to DEB-style.
pub fn deb_arch(arch: &str) -> &str {
    match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "i686" => "i386",
        "armv7hl" => "armhf",
        "noarch" | "all" => "all",
        other => other,
    }
}

/// Generate the DEB control file content.
pub fn generate_control(
    sub_package: &SubPackage,
    plan: &PackagePlan,
    config: &Config,
    extra_depends: &[String],
) -> String {
    let mut lines = Vec::new();

    lines.push(format!("Package: {}", sub_package.name));
    lines.push(format!("Version: {}-{}", plan.version, plan.release));
    lines.push(format!("Architecture: {}", deb_arch(&plan.arch)));
    lines.push(format!("Maintainer: {}", config.package.maintainer));

    // Installed-Size in KiB (rounded up, as per Debian policy).
    let installed_size_kib = (sub_package.total_size + 1023) / 1024;
    lines.push(format!("Installed-Size: {installed_size_kib}"));

    // Section from deb overrides or default "misc".
    let section = config
        .deb
        .as_ref()
        .and_then(|d| d.section.as_deref())
        .unwrap_or("misc");
    lines.push(format!("Section: {section}"));

    // Priority from deb overrides or default "optional".
    let priority = config
        .deb
        .as_ref()
        .and_then(|d| d.priority.as_deref())
        .unwrap_or("optional");
    lines.push(format!("Priority: {priority}"));

    // Depends: merge common + deb-specific + extra (for meta-packages).
    let mut depends: Vec<&str> = Vec::new();
    for dep in &config.package.dependencies.requires {
        depends.push(dep);
    }
    for dep in &config.package.dependencies.requires_deb {
        depends.push(dep);
    }
    for dep in extra_depends {
        depends.push(dep);
    }
    if !depends.is_empty() {
        lines.push(format!("Depends: {}", depends.join(", ")));
    }

    // Conflicts.
    if !config.package.dependencies.conflicts.is_empty() {
        lines.push(format!(
            "Conflicts: {}",
            config.package.dependencies.conflicts.join(", ")
        ));
    }

    // Provides.
    if !config.package.dependencies.provides.is_empty() {
        lines.push(format!(
            "Provides: {}",
            config.package.dependencies.provides.join(", ")
        ));
    }

    // Replaces.
    if !config.package.dependencies.replaces.is_empty() {
        lines.push(format!(
            "Replaces: {}",
            config.package.dependencies.replaces.join(", ")
        ));
    }

    // Homepage.
    if let Some(url) = &config.package.url {
        lines.push(format!("Homepage: {url}"));
    }

    // Description.
    lines.push(format!("Description: {}", config.package.description));

    // Extra fields from deb.fields.
    if let Some(deb) = &config.deb {
        let mut fields: Vec<_> = deb.fields.iter().collect();
        fields.sort_by_key(|(k, _)| *k);
        for (key, value) in fields {
            lines.push(format!("{key}: {value}"));
        }
    }

    lines.join("\n") + "\n"
}

/// Generate the conffiles content (one absolute path per line for `is_config` files).
///
/// Returns `None` if there are no config files.
pub fn generate_conffiles(files: &[FileEntry]) -> Option<String> {
    let conf_paths: Vec<String> = files
        .iter()
        .filter(|f| f.is_config && matches!(f.entry_type, EntryType::RegularFile))
        .map(|f| f.install_path.to_string_lossy().to_string())
        .collect();

    if conf_paths.is_empty() {
        None
    } else {
        Some(conf_paths.join("\n") + "\n")
    }
}

/// Compute md5sums for all regular files.
///
/// Format: `<hex_md5>  <relative_path>\n` (two spaces between hash and path).
/// Paths are relative (leading `/` stripped), matching DEB convention.
pub fn generate_md5sums(files: &[FileEntry]) -> Result<String, DebError> {
    let mut lines = Vec::new();
    for entry in files {
        if !matches!(entry.entry_type, EntryType::RegularFile) {
            continue;
        }
        let md5_hex = md5_file(&entry.source_path)?;
        let path = entry.install_path.to_string_lossy();
        let rel_path = path.strip_prefix('/').unwrap_or(&path);
        lines.push(format!("{md5_hex}  {rel_path}"));
    }
    if lines.is_empty() {
        return Ok(String::new());
    }
    Ok(lines.join("\n") + "\n")
}

/// Compute the MD5 hex digest of a file.
fn md5_file(path: &Path) -> Result<String, DebError> {
    let mut file = std::fs::File::open(path).map_err(|e| DebError::SourceFile {
        path: path.to_owned(),
        source: e,
    })?;
    let mut hasher = Md5::new();
    let mut buf = [0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use spm_core::alternatives::ResolvedScripts;
    use spm_core::config::*;
    use spm_core::planner::SubPackageRole;
    use std::path::PathBuf;

    /// Helper to create a minimal Config for testing.
    fn test_config() -> Config {
        Config {
            package: PackageConfig {
                name: "testpkg".to_string(),
                version: "1.0".to_string(),
                release: "1".to_string(),
                arch: "x86_64".to_string(),
                license: "MIT".to_string(),
                maintainer: "Test User <test@example.com>".to_string(),
                description: "A test package".to_string(),
                url: None,
                vendor: None,
                dependencies: DependencyConfig::default(),
            },
            content: ContentConfig {
                files: Vec::new(),
                symlinks: Vec::new(),
                directories: Vec::new(),
                alternatives: Vec::new(),
                defaults: ContentDefaults::default(),
            },
            scripts: ScriptsConfig::default(),
            compression: CompressionConfig::default(),
            splitting: SplittingConfig::default(),
            signing: None,
            rpm: None,
            deb: None,
            build: None,
        }
    }

    /// Helper to create a minimal SubPackage for testing.
    fn test_sub_package(name: &str, role: SubPackageRole) -> SubPackage {
        SubPackage {
            name: name.to_string(),
            role,
            files: Vec::new(),
            total_size: 4096,
            scripts: ResolvedScripts::default(),
        }
    }

    /// Helper to create a minimal PackagePlan for testing.
    fn test_plan() -> PackagePlan {
        PackagePlan {
            name: "testpkg".to_string(),
            version: "1.0".to_string(),
            release: "1".to_string(),
            arch: "x86_64".to_string(),
            sub_packages: Vec::new(),
            is_split: false,
            needs_extended_cpio: false,
            total_size: 4096,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn test_deb_arch_mapping() {
        assert_eq!(deb_arch("x86_64"), "amd64");
        assert_eq!(deb_arch("aarch64"), "arm64");
        assert_eq!(deb_arch("i686"), "i386");
        assert_eq!(deb_arch("armv7hl"), "armhf");
        assert_eq!(deb_arch("noarch"), "all");
        assert_eq!(deb_arch("all"), "all");
        assert_eq!(deb_arch("s390x"), "s390x");
    }

    #[test]
    fn test_generate_control_minimal() {
        let config = test_config();
        let sub_pkg = test_sub_package("testpkg", SubPackageRole::Standalone);
        let plan = test_plan();

        let control = generate_control(&sub_pkg, &plan, &config, &[]);

        assert!(control.contains("Package: testpkg\n"));
        assert!(control.contains("Version: 1.0-1\n"));
        assert!(control.contains("Architecture: amd64\n"));
        assert!(control.contains("Maintainer: Test User <test@example.com>\n"));
        assert!(control.contains("Installed-Size: 4\n"));
        assert!(control.contains("Section: misc\n"));
        assert!(control.contains("Priority: optional\n"));
        assert!(control.contains("Description: A test package\n"));
        // No Depends line when empty
        assert!(!control.contains("Depends:"));
    }

    #[test]
    fn test_generate_control_with_depends() {
        let mut config = test_config();
        config.package.dependencies.requires = vec!["libc6".to_string()];
        config.package.dependencies.requires_deb = vec!["libssl3".to_string()];
        let sub_pkg = test_sub_package("testpkg", SubPackageRole::Standalone);
        let plan = test_plan();

        let control = generate_control(&sub_pkg, &plan, &config, &[]);

        assert!(control.contains("Depends: libc6, libssl3\n"));
    }

    #[test]
    fn test_generate_control_meta_package_with_extra_depends() {
        let config = test_config();
        let sub_pkg = test_sub_package("testpkg", SubPackageRole::Meta);
        let plan = test_plan();
        let extra = vec![
            "testpkg-part1 (= 1.0-1)".to_string(),
            "testpkg-part2 (= 1.0-1)".to_string(),
        ];

        let control = generate_control(&sub_pkg, &plan, &config, &extra);

        assert!(control.contains("Depends: testpkg-part1 (= 1.0-1), testpkg-part2 (= 1.0-1)\n"));
    }

    #[test]
    fn test_generate_control_conflicts_provides_replaces() {
        let mut config = test_config();
        config.package.dependencies.conflicts = vec!["oldpkg".to_string()];
        config.package.dependencies.provides = vec!["vpkg".to_string()];
        config.package.dependencies.replaces = vec!["oldpkg".to_string()];
        let sub_pkg = test_sub_package("testpkg", SubPackageRole::Standalone);
        let plan = test_plan();

        let control = generate_control(&sub_pkg, &plan, &config, &[]);

        assert!(control.contains("Conflicts: oldpkg\n"));
        assert!(control.contains("Provides: vpkg\n"));
        assert!(control.contains("Replaces: oldpkg\n"));
    }

    #[test]
    fn test_generate_control_section_priority_overrides() {
        let mut config = test_config();
        config.deb = Some(DebOverrides {
            section: Some("science".to_string()),
            priority: Some("extra".to_string()),
            fields: std::collections::HashMap::new(),
            compression: None,
        });
        let sub_pkg = test_sub_package("testpkg", SubPackageRole::Standalone);
        let plan = test_plan();

        let control = generate_control(&sub_pkg, &plan, &config, &[]);

        assert!(control.contains("Section: science\n"));
        assert!(control.contains("Priority: extra\n"));
    }

    #[test]
    fn test_generate_control_homepage() {
        let mut config = test_config();
        config.package.url = Some("https://example.com".to_string());
        let sub_pkg = test_sub_package("testpkg", SubPackageRole::Standalone);
        let plan = test_plan();

        let control = generate_control(&sub_pkg, &plan, &config, &[]);

        assert!(control.contains("Homepage: https://example.com\n"));
    }

    #[test]
    fn test_generate_control_extra_fields() {
        let mut config = test_config();
        let mut fields = std::collections::HashMap::new();
        fields.insert("Bugs".to_string(), "https://bugs.example.com".to_string());
        config.deb = Some(DebOverrides {
            section: None,
            priority: None,
            fields,
            compression: None,
        });
        let sub_pkg = test_sub_package("testpkg", SubPackageRole::Standalone);
        let plan = test_plan();

        let control = generate_control(&sub_pkg, &plan, &config, &[]);

        assert!(control.contains("Bugs: https://bugs.example.com\n"));
    }

    #[test]
    fn test_installed_size_kib_rounding() {
        let config = test_config();
        let plan = test_plan();

        // 1 byte → rounds up to 1 KiB
        let mut sub_pkg = test_sub_package("testpkg", SubPackageRole::Standalone);
        sub_pkg.total_size = 1;
        let control = generate_control(&sub_pkg, &plan, &config, &[]);
        assert!(control.contains("Installed-Size: 1\n"));

        // 1024 bytes → exactly 1 KiB
        sub_pkg.total_size = 1024;
        let control = generate_control(&sub_pkg, &plan, &config, &[]);
        assert!(control.contains("Installed-Size: 1\n"));

        // 1025 bytes → rounds up to 2 KiB
        sub_pkg.total_size = 1025;
        let control = generate_control(&sub_pkg, &plan, &config, &[]);
        assert!(control.contains("Installed-Size: 2\n"));

        // 0 bytes → 0 KiB
        sub_pkg.total_size = 0;
        let control = generate_control(&sub_pkg, &plan, &config, &[]);
        assert!(control.contains("Installed-Size: 0\n"));
    }

    #[test]
    fn test_generate_conffiles_with_config_files() {
        let files = vec![
            FileEntry {
                install_path: PathBuf::from("/etc/myapp.conf"),
                source_path: PathBuf::from("/tmp/myapp.conf"),
                entry_type: EntryType::RegularFile,
                size: 100,
                mode: 0o644,
                user: "root".to_string(),
                group: "root".to_string(),
                is_config: true,
            },
            FileEntry {
                install_path: PathBuf::from("/usr/bin/myapp"),
                source_path: PathBuf::from("/tmp/myapp"),
                entry_type: EntryType::RegularFile,
                size: 1000,
                mode: 0o755,
                user: "root".to_string(),
                group: "root".to_string(),
                is_config: false,
            },
        ];

        let result = generate_conffiles(&files);
        assert_eq!(result, Some("/etc/myapp.conf\n".to_string()));
    }

    #[test]
    fn test_generate_conffiles_no_config_files() {
        let files = vec![FileEntry {
            install_path: PathBuf::from("/usr/bin/myapp"),
            source_path: PathBuf::from("/tmp/myapp"),
            entry_type: EntryType::RegularFile,
            size: 1000,
            mode: 0o755,
            user: "root".to_string(),
            group: "root".to_string(),
            is_config: false,
        }];

        assert_eq!(generate_conffiles(&files), None);
    }

    #[test]
    fn test_generate_conffiles_skips_directories() {
        let files = vec![FileEntry {
            install_path: PathBuf::from("/etc/myapp.d"),
            source_path: PathBuf::new(),
            entry_type: EntryType::Directory,
            size: 0,
            mode: 0o755,
            user: "root".to_string(),
            group: "root".to_string(),
            is_config: true,
        }];

        assert_eq!(generate_conffiles(&files), None);
    }

    #[test]
    fn test_generate_md5sums_format() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("hello.txt");
        std::fs::write(&file_path, "hello\n").unwrap();

        let files = vec![FileEntry {
            install_path: PathBuf::from("/usr/share/doc/hello.txt"),
            source_path: file_path,
            entry_type: EntryType::RegularFile,
            size: 6,
            mode: 0o644,
            user: "root".to_string(),
            group: "root".to_string(),
            is_config: false,
        }];

        let md5sums = generate_md5sums(&files).unwrap();
        // "hello\n" has known MD5
        let expected_md5 = "b1946ac92492d2347c6235b4d2611184";
        assert_eq!(
            md5sums,
            format!("{expected_md5}  usr/share/doc/hello.txt\n")
        );
    }

    #[test]
    fn test_generate_md5sums_strips_leading_slash() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test");
        std::fs::write(&file_path, "x").unwrap();

        let files = vec![FileEntry {
            install_path: PathBuf::from("/opt/test"),
            source_path: file_path,
            entry_type: EntryType::RegularFile,
            size: 1,
            mode: 0o644,
            user: "root".to_string(),
            group: "root".to_string(),
            is_config: false,
        }];

        let md5sums = generate_md5sums(&files).unwrap();
        assert!(md5sums.contains("  opt/test\n"));
        assert!(!md5sums.contains("  /opt/test"));
    }

    #[test]
    fn test_generate_md5sums_skips_non_regular_files() {
        let files = vec![
            FileEntry {
                install_path: PathBuf::from("/usr/lib/dir"),
                source_path: PathBuf::new(),
                entry_type: EntryType::Directory,
                size: 0,
                mode: 0o755,
                user: "root".to_string(),
                group: "root".to_string(),
                is_config: false,
            },
            FileEntry {
                install_path: PathBuf::from("/usr/lib/link"),
                source_path: PathBuf::new(),
                entry_type: EntryType::Symlink {
                    target: PathBuf::from("/usr/lib/real"),
                },
                size: 0,
                mode: 0o777,
                user: "root".to_string(),
                group: "root".to_string(),
                is_config: false,
            },
        ];

        let md5sums = generate_md5sums(&files).unwrap();
        assert_eq!(md5sums, "");
    }

    #[test]
    fn test_control_ends_with_newline() {
        let config = test_config();
        let sub_pkg = test_sub_package("testpkg", SubPackageRole::Standalone);
        let plan = test_plan();

        let control = generate_control(&sub_pkg, &plan, &config, &[]);
        assert!(control.ends_with('\n'));
    }
}
