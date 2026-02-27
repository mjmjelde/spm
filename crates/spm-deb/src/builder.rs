//! DEB build pipeline orchestration.
//!
//! Orchestrates the complete DEB build process: generating control metadata,
//! writing compressed data and control tars to temp files, and assembling
//! the final ar archive.

use std::fs::File;
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tar::{Builder as TarBuilder, EntryType as TarEntryType, Header as TarHeader};

use spm_compress::{compress_writer, Algorithm, CompressorConfig};
use spm_core::config::Config;
use spm_core::filetree::{EntryType, FileEntry};
use spm_core::planner::{PackagePlan, SubPackage, SubPackageRole};
use spm_core::types::PackageFileName;

use crate::ar::ArWriter;
use crate::control;
use crate::error::DebError;

/// Builds DEB packages from package plans.
pub struct DebBuilder;

impl DebBuilder {
    /// Build all DEB files from a PackagePlan.
    ///
    /// Returns paths to all generated `.deb` files. When the plan is split,
    /// this produces a meta-package and one DEB per part.
    pub fn build(
        plan: &PackagePlan,
        config: &Config,
        output_dir: &Path,
    ) -> Result<Vec<PathBuf>, DebError> {
        std::fs::create_dir_all(output_dir)?;
        let mut output_paths = Vec::new();

        for sub_pkg in &plan.sub_packages {
            let filename = PackageFileName::deb(
                &sub_pkg.name,
                &plan.version,
                &plan.release,
                &plan.arch,
            );
            let output_path = output_dir.join(&filename);

            // For meta-packages, compute Depends on all parts.
            let extra_depends = if sub_pkg.role == SubPackageRole::Meta {
                plan.sub_packages
                    .iter()
                    .filter(|sp| matches!(sp.role, SubPackageRole::Part(_)))
                    .map(|sp| {
                        format!(
                            "{} (= {}-{})",
                            sp.name, plan.version, plan.release
                        )
                    })
                    .collect()
            } else {
                Vec::new()
            };

            build_single_deb(sub_pkg, plan, config, &output_path, &extra_depends)?;
            output_paths.push(output_path);
        }

        Ok(output_paths)
    }
}

/// Build a single `.deb` file from a SubPackage.
fn build_single_deb(
    sub_package: &SubPackage,
    plan: &PackagePlan,
    config: &Config,
    output_path: &Path,
    extra_depends: &[String],
) -> Result<(), DebError> {
    let algorithm = resolve_algorithm(config)?;
    let compressor_config = make_compressor_config(&algorithm, config);
    let compress_ext = match algorithm {
        Algorithm::None => String::new(),
        _ => format!(".{}", algorithm.extension()),
    };
    let mtime = resolve_mtime(config);

    // 1. Build data.tar.{ext} to temp file.
    let (data_tmp, data_size) = write_data_tar(&sub_package.files, &compressor_config, mtime)?;

    // 2. Build control.tar.{ext} to temp file.
    let (control_tmp, control_size) = write_control_tar(
        sub_package,
        plan,
        config,
        extra_depends,
        &compressor_config,
        mtime,
    )?;

    // 3. Assemble the ar archive.
    let output_file = File::create(output_path).map_err(|e| DebError::SourceFile {
        path: output_path.to_owned(),
        source: e,
    })?;
    let mut ar = ArWriter::new(output_file);

    // debian-binary member.
    ar.write_member("debian-binary", b"2.0\n", mtime, 0o100644)?;

    // control.tar.{ext} member.
    let control_name = format!("control.tar{compress_ext}");
    ar.begin_member(&control_name, control_size, mtime, 0o100644)?;
    let mut control_file = File::open(control_tmp.path())?;
    io::copy(&mut control_file, ar.writer_mut())?;
    ar.finish_member()?;

    // data.tar.{ext} member.
    let data_name = format!("data.tar{compress_ext}");
    ar.begin_member(&data_name, data_size, mtime, 0o100644)?;
    let mut data_file = File::open(data_tmp.path())?;
    io::copy(&mut data_file, ar.writer_mut())?;
    ar.finish_member()?;

    ar.finish()?;
    Ok(())
}

/// Write a compressed data tar to a temp file. Returns the temp file and its size.
fn write_data_tar(
    files: &[FileEntry],
    compressor_config: &CompressorConfig,
    mtime: u64,
) -> Result<(tempfile::NamedTempFile, u64), DebError> {
    let tmp = tempfile::NamedTempFile::new()?;
    {
        let compressor = compress_writer(compressor_config, &tmp)?;
        let mut tar = TarBuilder::new(compressor);

        for entry in files {
            write_tar_entry(&mut tar, entry, mtime)?;
        }

        // Finalize the tar (writes two 512-byte zero blocks).
        let compressor = tar.into_inner().map_err(|e| DebError::Tar(e.to_string()))?;
        // Drop compressor to flush compression.
        drop(compressor);
    }

    let size = std::fs::metadata(tmp.path())?.len();
    Ok((tmp, size))
}

/// Write a compressed control tar to a temp file. Returns the temp file and its size.
fn write_control_tar(
    sub_package: &SubPackage,
    plan: &PackagePlan,
    config: &Config,
    extra_depends: &[String],
    compressor_config: &CompressorConfig,
    mtime: u64,
) -> Result<(tempfile::NamedTempFile, u64), DebError> {
    let tmp = tempfile::NamedTempFile::new()?;
    {
        let compressor = compress_writer(compressor_config, &tmp)?;
        let mut tar = TarBuilder::new(compressor);

        // control file.
        let control_text = control::generate_control(sub_package, plan, config, extra_depends);
        append_tar_bytes(&mut tar, "./control", control_text.as_bytes(), 0o644, mtime)?;

        // md5sums.
        let md5sums = control::generate_md5sums(&sub_package.files)?;
        if !md5sums.is_empty() {
            append_tar_bytes(&mut tar, "./md5sums", md5sums.as_bytes(), 0o644, mtime)?;
        }

        // conffiles.
        if let Some(conffiles) = control::generate_conffiles(&sub_package.files) {
            append_tar_bytes(&mut tar, "./conffiles", conffiles.as_bytes(), 0o644, mtime)?;
        }

        // Scripts.
        if let Some(ref s) = sub_package.scripts.pre_install {
            append_tar_bytes(&mut tar, "./preinst", s.as_bytes(), 0o755, mtime)?;
        }
        if let Some(ref s) = sub_package.scripts.post_install {
            append_tar_bytes(&mut tar, "./postinst", s.as_bytes(), 0o755, mtime)?;
        }
        if let Some(ref s) = sub_package.scripts.pre_remove {
            append_tar_bytes(&mut tar, "./prerm", s.as_bytes(), 0o755, mtime)?;
        }
        if let Some(ref s) = sub_package.scripts.post_remove {
            append_tar_bytes(&mut tar, "./postrm", s.as_bytes(), 0o755, mtime)?;
        }

        let compressor = tar.into_inner().map_err(|e| DebError::Tar(e.to_string()))?;
        drop(compressor);
    }

    let size = std::fs::metadata(tmp.path())?.len();
    Ok((tmp, size))
}

/// Write a single file entry into a tar archive.
fn write_tar_entry<W: Write>(
    tar: &mut TarBuilder<W>,
    entry: &FileEntry,
    mtime: u64,
) -> Result<(), DebError> {
    let install_path = entry.install_path.to_string_lossy();
    // DEB convention: paths are prefixed with "./"
    let tar_path = if install_path.starts_with('/') {
        format!(".{install_path}")
    } else {
        format!("./{install_path}")
    };

    let mut header = TarHeader::new_gnu();
    header.set_mtime(mtime);
    header.set_uid(0);
    header.set_gid(0);
    header
        .set_username("root")
        .map_err(|e| DebError::Tar(e.to_string()))?;
    header
        .set_groupname("root")
        .map_err(|e| DebError::Tar(e.to_string()))?;

    match &entry.entry_type {
        EntryType::RegularFile => {
            header.set_entry_type(TarEntryType::Regular);
            header.set_mode(entry.mode);
            header.set_size(entry.size);

            let mut file =
                File::open(&entry.source_path).map_err(|e| DebError::SourceFile {
                    path: entry.source_path.clone(),
                    source: e,
                })?;
            tar.append_data(&mut header, &tar_path, &mut file)
                .map_err(|e| DebError::Tar(e.to_string()))?;
        }
        EntryType::Directory => {
            header.set_entry_type(TarEntryType::Directory);
            header.set_mode(entry.mode);
            header.set_size(0);
            let dir_path = if tar_path.ends_with('/') {
                tar_path
            } else {
                format!("{tar_path}/")
            };
            tar.append_data(&mut header, &dir_path, &mut io::empty())
                .map_err(|e| DebError::Tar(e.to_string()))?;
        }
        EntryType::Symlink { target } => {
            header.set_entry_type(TarEntryType::Symlink);
            header.set_mode(entry.mode);
            header.set_size(0);
            tar.append_link(&mut header, &tar_path, target)
                .map_err(|e| DebError::Tar(e.to_string()))?;
        }
        EntryType::Hardlink { target } => {
            header.set_entry_type(TarEntryType::Link);
            header.set_mode(entry.mode);
            header.set_size(0);
            let target_str = target.to_string_lossy();
            let tar_target = if target_str.starts_with('/') {
                format!(".{target_str}")
            } else {
                format!("./{target_str}")
            };
            tar.append_link(&mut header, &tar_path, &tar_target)
                .map_err(|e| DebError::Tar(e.to_string()))?;
        }
    }

    Ok(())
}

/// Append an in-memory file to a tar archive.
fn append_tar_bytes<W: Write>(
    tar: &mut TarBuilder<W>,
    name: &str,
    data: &[u8],
    mode: u32,
    mtime: u64,
) -> Result<(), DebError> {
    let mut header = TarHeader::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(mtime);
    header.set_entry_type(TarEntryType::Regular);
    header
        .set_username("root")
        .map_err(|e| DebError::Tar(e.to_string()))?;
    header
        .set_groupname("root")
        .map_err(|e| DebError::Tar(e.to_string()))?;
    tar.append_data(&mut header, name, &mut Cursor::new(data))
        .map_err(|e| DebError::Tar(e.to_string()))?;
    Ok(())
}

/// Resolve the compression algorithm, respecting DEB-specific overrides.
fn resolve_algorithm(config: &Config) -> Result<Algorithm, DebError> {
    let algo_str = config
        .deb
        .as_ref()
        .and_then(|d| d.compression.as_deref())
        .unwrap_or(&config.compression.algorithm);
    Algorithm::from_str(algo_str).map_err(DebError::Compress)
}

/// Build a CompressorConfig from the resolved algorithm and config.
fn make_compressor_config(algorithm: &Algorithm, config: &Config) -> CompressorConfig {
    CompressorConfig {
        algorithm: *algorithm,
        level: config.compression.level,
        threads: config.compression.threads.unwrap_or(0),
    }
}

/// Resolve the build timestamp, using `source_date_epoch` for reproducible builds.
fn resolve_mtime(config: &Config) -> u64 {
    config
        .build
        .as_ref()
        .and_then(|b| b.source_date_epoch.as_ref())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use spm_core::alternatives::ResolvedScripts;
    use spm_core::config::*;

    /// Helper to create a minimal Config for testing.
    fn test_config() -> Config {
        Config {
            package: PackageConfig {
                name: "testpkg".to_string(),
                version: "1.0".to_string(),
                release: "1".to_string(),
                arch: "x86_64".to_string(),
                license: "MIT".to_string(),
                maintainer: "Test <test@example.com>".to_string(),
                description: "A test package".to_string(),
                url: None,
                vendor: None,
                dependencies: DependencyConfig::default(),
            },
            content: ContentConfig {
                source_dir: PathBuf::from("/tmp"),
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

    fn test_plan() -> PackagePlan {
        PackagePlan {
            name: "testpkg".to_string(),
            version: "1.0".to_string(),
            release: "1".to_string(),
            arch: "x86_64".to_string(),
            sub_packages: Vec::new(),
            is_split: false,
            needs_extended_cpio: false,
            total_size: 0,
        }
    }

    fn test_sub_package(name: &str, role: SubPackageRole) -> SubPackage {
        SubPackage {
            name: name.to_string(),
            role,
            files: Vec::new(),
            total_size: 0,
            scripts: ResolvedScripts::default(),
        }
    }

    /// Create a temporary source file with given content. Returns path.
    fn create_temp_file(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    // --- Data tar tests ---

    #[test]
    fn test_write_data_tar_empty() {
        let config = CompressorConfig {
            algorithm: Algorithm::None,
            level: None,
            threads: 0,
        };
        let (tmp, size) = write_data_tar(&[], &config, 0).unwrap();
        // Even an empty tar has the two 512-byte zero blocks = 1024 bytes.
        assert!(size >= 1024);
        assert!(tmp.path().exists());
    }

    #[test]
    fn test_write_data_tar_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = create_temp_file(dir.path(), "hello.txt", b"hello world\n");

        let files = vec![FileEntry {
            install_path: PathBuf::from("/usr/share/doc/hello.txt"),
            source_path: src,
            entry_type: EntryType::RegularFile,
            size: 12,
            mode: 0o644,
            user: "root".to_string(),
            group: "root".to_string(),
            is_config: false,
        }];

        let config = CompressorConfig {
            algorithm: Algorithm::None,
            level: None,
            threads: 0,
        };
        let (tmp, _size) = write_data_tar(&files, &config, 1000).unwrap();

        // Read back and verify the tar contains our file.
        // Note: the tar crate's path() strips the "./" prefix on readback.
        let file = File::open(tmp.path()).unwrap();
        let mut archive = tar::Archive::new(file);
        let entries: Vec<_> = archive.entries().unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(entries.len(), 1);
        let path = entries[0].path().unwrap();
        assert_eq!(path.to_str().unwrap(), "usr/share/doc/hello.txt");
    }

    #[test]
    fn test_write_data_tar_directory() {
        let files = vec![FileEntry {
            install_path: PathBuf::from("/usr/share/doc"),
            source_path: PathBuf::new(),
            entry_type: EntryType::Directory,
            size: 0,
            mode: 0o755,
            user: "root".to_string(),
            group: "root".to_string(),
            is_config: false,
        }];

        let config = CompressorConfig {
            algorithm: Algorithm::None,
            level: None,
            threads: 0,
        };
        let (tmp, _) = write_data_tar(&files, &config, 0).unwrap();

        let file = File::open(tmp.path()).unwrap();
        let mut archive = tar::Archive::new(file);
        let entries: Vec<_> = archive.entries().unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(entries.len(), 1);
        let path = entries[0].path().unwrap();
        let path_str = path.to_str().unwrap();
        // tar crate strips "./" prefix on readback.
        assert!(
            path_str == "usr/share/doc" || path_str == "usr/share/doc/",
            "unexpected path: {path_str}"
        );
    }

    #[test]
    fn test_write_data_tar_symlink() {
        let files = vec![FileEntry {
            install_path: PathBuf::from("/usr/bin/link"),
            source_path: PathBuf::new(),
            entry_type: EntryType::Symlink {
                target: PathBuf::from("/usr/bin/real"),
            },
            size: 0,
            mode: 0o777,
            user: "root".to_string(),
            group: "root".to_string(),
            is_config: false,
        }];

        let config = CompressorConfig {
            algorithm: Algorithm::None,
            level: None,
            threads: 0,
        };
        let (tmp, _) = write_data_tar(&files, &config, 0).unwrap();

        let file = File::open(tmp.path()).unwrap();
        let mut archive = tar::Archive::new(file);
        let entries: Vec<_> = archive.entries().unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].header().entry_type(), TarEntryType::Symlink);
        let link_name = entries[0].link_name().unwrap().unwrap();
        assert_eq!(link_name.to_str().unwrap(), "/usr/bin/real");
    }

    #[test]
    fn test_write_data_tar_compressed() {
        let dir = tempfile::tempdir().unwrap();
        let src = create_temp_file(dir.path(), "file.txt", b"test content");

        let files = vec![FileEntry {
            install_path: PathBuf::from("/opt/file.txt"),
            source_path: src,
            entry_type: EntryType::RegularFile,
            size: 12,
            mode: 0o644,
            user: "root".to_string(),
            group: "root".to_string(),
            is_config: false,
        }];

        let config = CompressorConfig {
            algorithm: Algorithm::Gzip,
            level: Some(1),
            threads: 0,
        };
        let (tmp, size) = write_data_tar(&files, &config, 0).unwrap();
        assert!(size > 0);

        // Decompress and verify.
        let compressed = std::fs::read(tmp.path()).unwrap();
        let decoder = flate2::read::GzDecoder::new(&compressed[..]);
        let mut archive = tar::Archive::new(decoder);
        let entries: Vec<_> = archive.entries().unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_tar_path_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let src = create_temp_file(dir.path(), "a", b"x");

        let files = vec![FileEntry {
            install_path: PathBuf::from("/opt/a"),
            source_path: src,
            entry_type: EntryType::RegularFile,
            size: 1,
            mode: 0o644,
            user: "root".to_string(),
            group: "root".to_string(),
            is_config: false,
        }];

        let config = CompressorConfig {
            algorithm: Algorithm::None,
            level: None,
            threads: 0,
        };
        let (tmp, _) = write_data_tar(&files, &config, 0).unwrap();

        let file = File::open(tmp.path()).unwrap();
        let mut archive = tar::Archive::new(file);
        let entries: Vec<_> = archive.entries().unwrap().collect::<Result<_, _>>().unwrap();
        // Verify the path is relative (no leading "/").
        let path_str = entries[0].path().unwrap().to_string_lossy().to_string();
        assert!(
            !path_str.starts_with('/'),
            "path should be relative (no leading /): {path_str}"
        );
        assert!(
            path_str.contains("opt/a"),
            "path should contain opt/a: {path_str}"
        );
    }

    // --- Control tar tests ---

    #[test]
    fn test_write_control_tar_contains_control() {
        let config = test_config();
        let plan = test_plan();
        let sub_pkg = test_sub_package("testpkg", SubPackageRole::Standalone);
        let compressor_config = CompressorConfig {
            algorithm: Algorithm::None,
            level: None,
            threads: 0,
        };

        let (tmp, _) =
            write_control_tar(&sub_pkg, &plan, &config, &[], &compressor_config, 0).unwrap();

        let file = File::open(tmp.path()).unwrap();
        let mut archive = tar::Archive::new(file);
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|e| {
                e.unwrap()
                    .path()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        // tar crate strips "./" prefix on readback.
        assert!(names.contains(&"control".to_string()));
    }

    #[test]
    fn test_write_control_tar_contains_scripts() {
        let config = test_config();
        let plan = test_plan();
        let mut sub_pkg = test_sub_package("testpkg", SubPackageRole::Standalone);
        sub_pkg.scripts.pre_install = Some("#!/bin/sh\necho pre".to_string());
        sub_pkg.scripts.post_install = Some("#!/bin/sh\necho post".to_string());
        sub_pkg.scripts.pre_remove = Some("#!/bin/sh\necho prerm".to_string());
        sub_pkg.scripts.post_remove = Some("#!/bin/sh\necho postrm".to_string());

        let compressor_config = CompressorConfig {
            algorithm: Algorithm::None,
            level: None,
            threads: 0,
        };

        let (tmp, _) =
            write_control_tar(&sub_pkg, &plan, &config, &[], &compressor_config, 0).unwrap();

        let file = File::open(tmp.path()).unwrap();
        let mut archive = tar::Archive::new(file);
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|e| {
                e.unwrap()
                    .path()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        // tar crate strips "./" prefix on readback.
        assert!(names.contains(&"preinst".to_string()));
        assert!(names.contains(&"postinst".to_string()));
        assert!(names.contains(&"prerm".to_string()));
        assert!(names.contains(&"postrm".to_string()));
    }

    #[test]
    fn test_write_control_tar_scripts_executable() {
        let config = test_config();
        let plan = test_plan();
        let mut sub_pkg = test_sub_package("testpkg", SubPackageRole::Standalone);
        sub_pkg.scripts.post_install = Some("#!/bin/sh\necho ok".to_string());

        let compressor_config = CompressorConfig {
            algorithm: Algorithm::None,
            level: None,
            threads: 0,
        };

        let (tmp, _) =
            write_control_tar(&sub_pkg, &plan, &config, &[], &compressor_config, 0).unwrap();

        let file = File::open(tmp.path()).unwrap();
        let mut archive = tar::Archive::new(file);
        for entry in archive.entries().unwrap() {
            let entry = entry.unwrap();
            let name = entry.path().unwrap().to_string_lossy().to_string();
            if name == "postinst" {
                assert_eq!(entry.header().mode().unwrap(), 0o755);
            }
        }
    }

    // --- Full DEB assembly tests ---

    #[test]
    fn test_build_single_deb_structure() {
        let dir = tempfile::tempdir().unwrap();
        let src = create_temp_file(dir.path(), "hello", b"hello world");

        let config = test_config();
        let plan = test_plan();
        let mut sub_pkg = test_sub_package("testpkg", SubPackageRole::Standalone);
        sub_pkg.files.push(FileEntry {
            install_path: PathBuf::from("/usr/share/hello"),
            source_path: src,
            entry_type: EntryType::RegularFile,
            size: 11,
            mode: 0o644,
            user: "root".to_string(),
            group: "root".to_string(),
            is_config: false,
        });
        sub_pkg.total_size = 11;

        let output_path = dir.path().join("testpkg_1.0-1_amd64.deb");
        build_single_deb(&sub_pkg, &plan, &config, &output_path, &[]).unwrap();

        // Read the ar archive and verify structure.
        let data = std::fs::read(&output_path).unwrap();
        // Starts with ar magic.
        assert_eq!(&data[..8], b"!<arch>\n");
        // First member name is "debian-binary".
        let first_name = std::str::from_utf8(&data[8..24]).unwrap();
        assert!(first_name.starts_with("debian-binary/"));
    }

    #[test]
    fn test_build_single_deb_debian_binary_content() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config();
        let plan = test_plan();
        let sub_pkg = test_sub_package("testpkg", SubPackageRole::Standalone);

        let output_path = dir.path().join("test.deb");
        build_single_deb(&sub_pkg, &plan, &config, &output_path, &[]).unwrap();

        let data = std::fs::read(&output_path).unwrap();
        // debian-binary data starts at offset 68 (8 magic + 60 header).
        assert_eq!(&data[68..72], b"2.0\n");
    }

    #[test]
    fn test_build_single_deb_member_ordering() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config();
        let plan = test_plan();
        let sub_pkg = test_sub_package("testpkg", SubPackageRole::Standalone);

        let output_path = dir.path().join("test.deb");
        build_single_deb(&sub_pkg, &plan, &config, &output_path, &[]).unwrap();

        let data = std::fs::read(&output_path).unwrap();
        // Parse member names from the ar archive.
        let mut offset = 8; // after magic
        let mut names = Vec::new();
        while offset + 60 <= data.len() {
            let name_field = std::str::from_utf8(&data[offset..offset + 16]).unwrap();
            let name = name_field.trim_end().trim_end_matches('/');
            if name.is_empty() {
                break;
            }
            names.push(name.to_string());

            // Parse size field.
            let size_str = std::str::from_utf8(&data[offset + 48..offset + 58])
                .unwrap()
                .trim();
            let size: u64 = size_str.parse().unwrap_or(0);
            let padded_size = size + (size % 2); // even-byte padding
            offset += 60 + padded_size as usize;
        }

        assert_eq!(names.len(), 3);
        assert_eq!(names[0], "debian-binary");
        assert!(names[1].starts_with("control.tar"));
        assert!(names[2].starts_with("data.tar"));
    }

    #[test]
    fn test_deb_filename_format() {
        let dir = tempfile::tempdir().unwrap();
        let output_dir = dir.path().join("out");

        let config = test_config();
        let mut plan = test_plan();
        let sub_pkg = test_sub_package("testpkg", SubPackageRole::Standalone);
        plan.sub_packages.push(sub_pkg);

        let paths = DebBuilder::build(&plan, &config, &output_dir).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(
            paths[0].file_name().unwrap().to_str().unwrap(),
            "testpkg_1.0-1_amd64.deb"
        );
    }

    #[test]
    fn test_resolve_mtime_with_epoch() {
        let mut config = test_config();
        config.build = Some(BuildConfig {
            source_date_epoch: Some("1700000000".to_string()),
        });
        assert_eq!(resolve_mtime(&config), 1700000000);
    }

    #[test]
    fn test_resolve_mtime_without_epoch() {
        let config = test_config();
        let mtime = resolve_mtime(&config);
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!((now - mtime) < 10);
    }

    #[test]
    fn test_resolve_algorithm_default() {
        let config = test_config();
        let algo = resolve_algorithm(&config).unwrap();
        assert_eq!(algo, Algorithm::Zstd);
    }

    #[test]
    fn test_resolve_algorithm_deb_override() {
        let mut config = test_config();
        config.deb = Some(DebOverrides {
            section: None,
            priority: None,
            fields: std::collections::HashMap::new(),
            compression: Some("gzip".to_string()),
        });
        let algo = resolve_algorithm(&config).unwrap();
        assert_eq!(algo, Algorithm::Gzip);
    }

    // --- Auto-split tests ---

    #[test]
    fn test_split_build_produces_meta_and_parts() {
        let dir = tempfile::tempdir().unwrap();
        let src1 = create_temp_file(dir.path(), "f1", b"file one");
        let src2 = create_temp_file(dir.path(), "f2", b"file two");
        let output_dir = dir.path().join("out");

        let config = test_config();

        let mut part1 = test_sub_package("testpkg-part1", SubPackageRole::Part(1));
        part1.files.push(FileEntry {
            install_path: PathBuf::from("/opt/f1"),
            source_path: src1,
            entry_type: EntryType::RegularFile,
            size: 8,
            mode: 0o644,
            user: "root".to_string(),
            group: "root".to_string(),
            is_config: false,
        });
        part1.total_size = 8;

        let mut part2 = test_sub_package("testpkg-part2", SubPackageRole::Part(2));
        part2.files.push(FileEntry {
            install_path: PathBuf::from("/opt/f2"),
            source_path: src2,
            entry_type: EntryType::RegularFile,
            size: 8,
            mode: 0o644,
            user: "root".to_string(),
            group: "root".to_string(),
            is_config: false,
        });
        part2.total_size = 8;

        let meta = test_sub_package("testpkg", SubPackageRole::Meta);

        let mut plan = test_plan();
        plan.is_split = true;
        plan.sub_packages = vec![meta, part1, part2];
        plan.total_size = 16;

        let paths = DebBuilder::build(&plan, &config, &output_dir).unwrap();
        assert_eq!(paths.len(), 3);

        // Verify filenames.
        let names: Vec<String> = paths
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"testpkg_1.0-1_amd64.deb".to_string()));
        assert!(names.contains(&"testpkg-part1_1.0-1_amd64.deb".to_string()));
        assert!(names.contains(&"testpkg-part2_1.0-1_amd64.deb".to_string()));
    }

    #[test]
    fn test_meta_package_depends_on_parts() {
        let dir = tempfile::tempdir().unwrap();
        let output_dir = dir.path().join("out");

        let config = test_config();

        let meta = test_sub_package("testpkg", SubPackageRole::Meta);
        let part1 = test_sub_package("testpkg-part1", SubPackageRole::Part(1));
        let part2 = test_sub_package("testpkg-part2", SubPackageRole::Part(2));

        let mut plan = test_plan();
        plan.is_split = true;
        plan.sub_packages = vec![meta, part1, part2];

        let paths = DebBuilder::build(&plan, &config, &output_dir).unwrap();

        // Find the meta-package and read its control file.
        let meta_path = paths
            .iter()
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("testpkg_")
            })
            .unwrap();

        let data = std::fs::read(meta_path).unwrap();
        // Parse ar to find control.tar member and extract control file.
        let control_text = extract_control_from_deb(&data);
        assert!(
            control_text.contains("testpkg-part1 (= 1.0-1)"),
            "control: {control_text}"
        );
        assert!(
            control_text.contains("testpkg-part2 (= 1.0-1)"),
            "control: {control_text}"
        );
    }

    #[test]
    fn test_meta_package_empty_data_tar() {
        let dir = tempfile::tempdir().unwrap();
        let output_dir = dir.path().join("out");

        let config = test_config();
        let meta = test_sub_package("testpkg", SubPackageRole::Meta);

        let mut plan = test_plan();
        plan.sub_packages = vec![meta];

        let paths = DebBuilder::build(&plan, &config, &output_dir).unwrap();
        let data = std::fs::read(&paths[0]).unwrap();

        // Extract data.tar from the ar archive and verify it's a valid but empty tar.
        let data_tar = extract_data_tar_from_deb(&data);
        let mut archive = tar::Archive::new(&data_tar[..]);
        let entries: Vec<_> = archive.entries().unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(entries.len(), 0);
    }

    /// Helper: extract the control file text from a DEB (ar archive).
    fn extract_control_from_deb(deb_data: &[u8]) -> String {
        let members = parse_ar_members(deb_data);
        for (name, data) in &members {
            if name.starts_with("control.tar") {
                let decompressed = decompress_tar_member(name, data);
                let mut archive = tar::Archive::new(&decompressed[..]);
                for entry in archive.entries().unwrap() {
                    let mut entry = entry.unwrap();
                    let path = entry.path().unwrap().to_string_lossy().to_string();
                    if path == "./control" || path == "control" {
                        let mut content = String::new();
                        io::Read::read_to_string(&mut entry, &mut content).unwrap();
                        return content;
                    }
                }
            }
        }
        panic!("control file not found in DEB");
    }

    /// Helper: extract the raw data tar content from a DEB.
    fn extract_data_tar_from_deb(deb_data: &[u8]) -> Vec<u8> {
        let members = parse_ar_members(deb_data);
        for (name, data) in &members {
            if name.starts_with("data.tar") {
                return decompress_tar_member(name, data);
            }
        }
        panic!("data.tar not found in DEB");
    }

    /// Parse ar archive into (name, data) pairs.
    fn parse_ar_members(data: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut members = Vec::new();
        let mut offset = 8; // skip magic
        while offset + 60 <= data.len() {
            let name_field = std::str::from_utf8(&data[offset..offset + 16]).unwrap();
            let name = name_field.trim_end().trim_end_matches('/').to_string();
            if name.is_empty() {
                break;
            }
            let size_str = std::str::from_utf8(&data[offset + 48..offset + 58])
                .unwrap()
                .trim();
            let size: usize = size_str.parse().unwrap_or(0);
            let data_start = offset + 60;
            let member_data = data[data_start..data_start + size].to_vec();
            members.push((name, member_data));
            offset = data_start + size + (size % 2);
        }
        members
    }

    /// Decompress a tar member based on its extension.
    fn decompress_tar_member(name: &str, data: &[u8]) -> Vec<u8> {
        if name.ends_with(".zst") {
            zstd::decode_all(data).unwrap()
        } else if name.ends_with(".gz") {
            let mut decoder = flate2::read::GzDecoder::new(data);
            let mut out = Vec::new();
            io::Read::read_to_end(&mut decoder, &mut out).unwrap();
            out
        } else {
            // Uncompressed or unknown — return as-is.
            data.to_vec()
        }
    }
}
