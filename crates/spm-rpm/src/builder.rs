//! RPM build pipeline orchestration.
//!
//! Orchestrates the complete RPM build process: computing file digests,
//! writing the CPIO payload, building metadata and signature headers,
//! and assembling the final RPM file.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use spm_compress::{compress_writer, Algorithm, CompressorConfig};
use spm_core::config::Config;
use spm_core::filetree::{EntryType, FileEntry};
use spm_core::planner::{PackagePlan, SubPackage};
use spm_cpio::{CpioFormat, CpioMetadata, CpioWriter};

use spm_core::distro::{Distro, DistroInfo};

use crate::error::RpmError;
use crate::header::HeaderBuilder;
use crate::lead;
use crate::signature;
use crate::tags::*;

/// Builds RPM packages from package plans.
pub struct RpmBuilder;

impl RpmBuilder {
    /// Build a single RPM file from a SubPackage.
    ///
    /// The build pipeline:
    /// 1. Compute SHA-256 file digests
    /// 2. Write compressed CPIO payload to a temp file
    /// 3. Build the metadata header
    /// 4. Build the signature header (MD5, SHA-256, sizes)
    /// 5. Assemble: Lead + Signature + Header + Payload
    pub fn build(
        sub_package: &SubPackage,
        plan: &PackagePlan,
        config: &Config,
        output_path: &Path,
        target_distro: Option<&Distro>,
    ) -> Result<(), RpmError> {
        let cpio_format = if plan.needs_extended_cpio {
            CpioFormat::Extended
        } else {
            CpioFormat::Newc
        };

        let algorithm = Algorithm::from_str(&config.compression.algorithm)?;
        let compressor_config = CompressorConfig {
            algorithm,
            level: config.compression.level,
            threads: config.compression.threads.unwrap_or(0),
        };

        // 1. Compute file digests (SHA-256 hex) for all regular files.
        let file_digests = compute_file_digests(&sub_package.files)?;

        // 2. Write payload (cpio | compress) to temp file.
        let payload_tmp = tempfile::NamedTempFile::new()?;
        let uncompressed_size: u64;
        {
            let compressor = compress_writer(&compressor_config, &payload_tmp)?;
            let mut cpio = CpioWriter::new(compressor, cpio_format);

            for (index, entry) in sub_package.files.iter().enumerate() {
                let metadata = file_entry_to_cpio_metadata(entry);
                let cpio_name = make_cpio_name(&entry.install_path, cpio_format);

                write_cpio_entry(&mut cpio, index as u32, &cpio_name, &metadata, entry)?;
            }

            let (compressor, bytes) = cpio.finish()?;
            uncompressed_size = bytes;
            // Drop the compressor to flush (auto_finish for zstd).
            drop(compressor);
        }

        // 3. Build metadata header.
        let header_bytes = build_metadata_header(
            sub_package,
            plan,
            config,
            &file_digests,
            &algorithm,
            target_distro,
        )?;

        // 4. Build signature header.
        let sig_bytes =
            signature::build_signature(&header_bytes, payload_tmp.path(), uncompressed_size)?;

        // 5. Assemble final RPM file.
        let mut output = File::create(output_path).map_err(|e| RpmError::SourceFile {
            path: output_path.to_owned(),
            source: e,
        })?;

        // Lead (96 bytes).
        let lead_name = format!("{}-{}-{}", sub_package.name, plan.version, plan.release);
        lead::write_lead(&mut output, &lead_name, &plan.arch)?;

        // Signature header.
        output.write_all(&sig_bytes)?;

        // Pad to 8-byte boundary after signature header.
        let sig_pad = (8 - (sig_bytes.len() % 8)) % 8;
        if sig_pad > 0 {
            output.write_all(&vec![0u8; sig_pad])?;
        }

        // Metadata header.
        output.write_all(&header_bytes)?;

        // Payload (stream from temp file).
        let mut payload_file = File::open(payload_tmp.path())?;
        io::copy(&mut payload_file, &mut output)?;

        Ok(())
    }
}

/// Build the metadata header with all package, file, dependency, and script tags.
fn build_metadata_header(
    sub_package: &SubPackage,
    plan: &PackagePlan,
    config: &Config,
    file_digests: &[String],
    algorithm: &Algorithm,
    target_distro: Option<&Distro>,
) -> Result<Vec<u8>, RpmError> {
    let mut hdr = HeaderBuilder::new();

    add_package_metadata(&mut hdr, plan, sub_package, config, algorithm)?;

    if !sub_package.files.is_empty() {
        add_file_metadata(
            &mut hdr,
            &sub_package.files,
            plan.needs_extended_cpio,
            file_digests,
        )?;
    }

    add_dependencies(&mut hdr, config, algorithm, target_distro)?;
    add_scripts(&mut hdr, &sub_package.scripts)?;

    // Region tag (must be added last — its data goes at end of data section).
    hdr.add_region_tag(RPMTAG_HEADERIMMUTABLE);

    hdr.build()
}

/// Populate package metadata tags.
fn add_package_metadata(
    hdr: &mut HeaderBuilder,
    plan: &PackagePlan,
    sub_package: &SubPackage,
    config: &Config,
    algorithm: &Algorithm,
) -> Result<(), RpmError> {
    hdr.add_string(RPMTAG_NAME, &sub_package.name);
    hdr.add_string(RPMTAG_VERSION, &plan.version);
    hdr.add_string(RPMTAG_RELEASE, &plan.release);
    hdr.add_i18n_string(RPMTAG_SUMMARY, &config.package.description);
    hdr.add_i18n_string(RPMTAG_DESCRIPTION, &config.package.description);

    // Build time.
    let build_time = config
        .build
        .as_ref()
        .and_then(|b| b.source_date_epoch.as_ref())
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i32
        });
    hdr.add_int32(RPMTAG_BUILDTIME, vec![build_time]);

    // Build host.
    let hostname = hostname();
    hdr.add_string(RPMTAG_BUILDHOST, &hostname);

    // Installed size.
    let total_size = sub_package.total_size;
    if total_size <= i32::MAX as u64 {
        hdr.add_int32(RPMTAG_SIZE, vec![total_size as i32]);
    }
    hdr.add_int64(RPMTAG_LONGSIZE, vec![total_size as i64]);

    hdr.add_string(RPMTAG_LICENSE, &config.package.license);
    hdr.add_string(RPMTAG_PACKAGER, &config.package.maintainer);

    // Group from RPM overrides or default.
    let group = config
        .rpm
        .as_ref()
        .and_then(|r| r.group.as_deref())
        .unwrap_or("Unspecified");
    hdr.add_i18n_string(RPMTAG_GROUP, group);

    // Vendor (optional).
    if let Some(vendor) = &config.package.vendor {
        hdr.add_string(RPMTAG_VENDOR, vendor);
    }

    // URL (optional).
    if let Some(url) = &config.package.url {
        hdr.add_string(RPMTAG_URL, url);
    }

    hdr.add_string(RPMTAG_OS, "linux");
    hdr.add_string(RPMTAG_ARCH, &plan.arch);
    hdr.add_string(RPMTAG_SOURCERPM, "");
    hdr.add_string(RPMTAG_RPMVERSION, "spm");
    hdr.add_string(RPMTAG_OPTFLAGS, "");
    hdr.add_string(RPMTAG_PAYLOADFORMAT, "cpio");
    hdr.add_string(RPMTAG_PAYLOADCOMPRESSOR, algorithm.rpm_tag());

    // Payload flags: compression level as string.
    let payload_flags = config
        .compression
        .level
        .map(|l| l.to_string())
        .unwrap_or_else(|| "9".to_owned());
    hdr.add_string(RPMTAG_PAYLOADFLAGS, &payload_flags);

    Ok(())
}

/// Populate file metadata tags.
fn add_file_metadata(
    hdr: &mut HeaderBuilder,
    files: &[FileEntry],
    needs_extended: bool,
    file_digests: &[String],
) -> Result<(), RpmError> {
    let (basenames, dirnames, dirindexes) = decompose_paths(files);

    hdr.add_string_array(RPMTAG_BASENAMES, basenames);
    hdr.add_string_array(RPMTAG_DIRNAMES, dirnames);
    hdr.add_int32(RPMTAG_DIRINDEXES, dirindexes);

    // File sizes: use LONGFILESIZES for extended cpio, FILESIZES for standard.
    if needs_extended {
        let sizes: Vec<i64> = files.iter().map(|f| f.size as i64).collect();
        hdr.add_int64(RPMTAG_LONGFILESIZES, sizes);
    } else {
        let sizes: Vec<i32> = files.iter().map(|f| f.size as i32).collect();
        hdr.add_int32(RPMTAG_FILESIZES, sizes);
    }

    // File modes (INT16 — stored as signed but carries raw mode bits).
    let modes: Vec<i16> = files
        .iter()
        .map(|f| file_mode_with_type(f) as i16)
        .collect();
    hdr.add_int16(RPMTAG_FILEMODES, modes);

    // File rdev (0 for regular files).
    let rdevs: Vec<i16> = vec![0i16; files.len()];
    hdr.add_int16(RPMTAG_FILERDEVS, rdevs);

    // File modification times.
    let mtimes: Vec<i32> = files
        .iter()
        .map(|f| {
            if let Ok(meta) = std::fs::metadata(&f.source_path) {
                meta.modified()
                    .ok()
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i32)
                    .unwrap_or(0)
            } else {
                0
            }
        })
        .collect();
    hdr.add_int32(RPMTAG_FILEMTIMES, mtimes);

    // File digests (SHA-256 hex, empty string for dirs/symlinks/hardlinks-with-no-data).
    hdr.add_string_array(RPMTAG_FILEDIGESTS, file_digests.to_vec());

    // Digest algorithm: SHA-256.
    hdr.add_int32(RPMTAG_FILEDIGESTALGO, vec![PGPHASHALGO_SHA256 as i32]);

    // Symlink targets (empty string for non-symlinks).
    let linktos: Vec<String> = files
        .iter()
        .map(|f| match &f.entry_type {
            EntryType::Symlink { target } => target.to_string_lossy().into_owned(),
            _ => String::new(),
        })
        .collect();
    hdr.add_string_array(RPMTAG_FILELINKTOS, linktos);

    // File flags (config/noreplace).
    let flags: Vec<i32> = files
        .iter()
        .map(|f| {
            if f.is_config {
                (RPMFILE_CONFIG | RPMFILE_NOREPLACE) as i32
            } else {
                0
            }
        })
        .collect();
    hdr.add_int32(RPMTAG_FILEFLAGS, flags);

    // File usernames and group names.
    let users: Vec<String> = files.iter().map(|f| f.user.clone()).collect();
    let groups: Vec<String> = files.iter().map(|f| f.group.clone()).collect();
    hdr.add_string_array(RPMTAG_FILEUSERNAME, users);
    hdr.add_string_array(RPMTAG_FILEGROUPNAME, groups);

    // File devices (all 1 — single device).
    let devices: Vec<i32> = vec![1; files.len()];
    hdr.add_int32(RPMTAG_FILEDEVICES, devices);

    // File inodes (sequential, unique per entry).
    let inodes: Vec<i32> = (1..=files.len() as i32).collect();
    hdr.add_int32(RPMTAG_FILEINODES, inodes);

    // File languages (empty string for all).
    let langs: Vec<String> = vec![String::new(); files.len()];
    hdr.add_string_array(RPMTAG_FILELANGS, langs);

    // File colors (0 for all — we don't do ELF classification).
    let colors: Vec<i32> = vec![0; files.len()];
    hdr.add_int32(RPMTAG_FILECOLORS, colors);

    // File class (0 for all).
    let class: Vec<i32> = vec![0; files.len()];
    hdr.add_int32(RPMTAG_FILECLASS, class);

    // File verification flags (verify everything).
    let verify: Vec<i32> = vec![RPMVERIFY_ALL as i32; files.len()];
    hdr.add_int32(RPMTAG_FILEVERIFYFLAGS, verify);

    Ok(())
}

/// Decompose file install paths into RPM BASENAMES/DIRNAMES/DIRINDEXES format.
///
/// Each path is split into a directory component (ending with `/`) and
/// a basename. Unique directories are collected into DIRNAMES, and each
/// file gets a DIRINDEXES value pointing into that list.
fn decompose_paths(files: &[FileEntry]) -> (Vec<String>, Vec<String>, Vec<i32>) {
    let mut dir_map: HashMap<String, usize> = HashMap::new();
    let mut dirnames: Vec<String> = Vec::new();
    let mut basenames: Vec<String> = Vec::new();
    let mut dirindexes: Vec<i32> = Vec::new();

    for entry in files {
        let path = &entry.install_path;
        let path_str = path.to_string_lossy();

        // Split into directory and basename.
        let (dir, base) = if let Some(parent) = path.parent() {
            let mut dir_str = parent.to_string_lossy().into_owned();
            if !dir_str.ends_with('/') {
                dir_str.push('/');
            }
            let base_str = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (dir_str, base_str)
        } else {
            // Root path — shouldn't normally happen.
            ("/".to_owned(), path_str.into_owned())
        };

        // Look up or insert the directory.
        let dir_idx = if let Some(&idx) = dir_map.get(&dir) {
            idx
        } else {
            let idx = dirnames.len();
            dir_map.insert(dir.clone(), idx);
            dirnames.push(dir);
            idx
        };

        basenames.push(base);
        dirindexes.push(dir_idx as i32);
    }

    (basenames, dirnames, dirindexes)
}

/// Populate dependency tags.
fn add_dependencies(
    hdr: &mut HeaderBuilder,
    config: &Config,
    algorithm: &Algorithm,
    target_distro: Option<&Distro>,
) -> Result<(), RpmError> {
    let mut names: Vec<String> = Vec::new();
    let mut versions: Vec<String> = Vec::new();
    let mut flags: Vec<i32> = Vec::new();

    // Implicit rpmlib dependencies.
    names.push("rpmlib(CompressedFileNames)".into());
    versions.push("3.0.4-1".into());
    flags.push((RPMSENSE_RPMLIB | RPMSENSE_LESS | RPMSENSE_EQUAL) as i32);

    names.push("rpmlib(PayloadFilesHavePrefix)".into());
    versions.push("4.0-1".into());
    flags.push((RPMSENSE_RPMLIB | RPMSENSE_LESS | RPMSENSE_EQUAL) as i32);

    if *algorithm == Algorithm::Zstd {
        names.push("rpmlib(PayloadIsZstd)".into());
        versions.push("5.4.18-1".into());
        flags.push((RPMSENSE_RPMLIB | RPMSENSE_LESS | RPMSENSE_EQUAL) as i32);
    }

    // User-specified dependencies (common + RPM-specific).
    for dep in config
        .package
        .dependencies
        .requires
        .iter()
        .chain(config.package.dependencies.requires_rpm.iter())
    {
        let (name, version, dep_flags) = parse_dependency(dep);
        names.push(name);
        versions.push(version);
        flags.push(dep_flags);
    }

    // Alternatives auto-dependency injection.
    if !config.content.alternatives.is_empty() {
        let alt_dep = match target_distro {
            Some(distro) => match distro.info() {
                DistroInfo::Rpm(info) => info.alternatives_dep,
                DistroInfo::Deb(_) => "/usr/sbin/alternatives",
            },
            None => "/usr/sbin/alternatives",
        };
        names.push(alt_dep.to_owned());
        versions.push(String::new());
        flags.push(RPMSENSE_ANY as i32);
    }

    if !names.is_empty() {
        hdr.add_string_array(RPMTAG_REQUIRENAME, names);
        hdr.add_string_array(RPMTAG_REQUIREVERSION, versions);
        hdr.add_int32(RPMTAG_REQUIREFLAGS, flags);
    }

    // Self-provides.
    let provide_names = vec![config.package.name.clone()];
    let provide_versions = vec![format!(
        "{}-{}",
        config.package.version, config.package.release
    )];
    let provide_flags = vec![RPMSENSE_EQUAL as i32];
    hdr.add_string_array(RPMTAG_PROVIDENAME, provide_names);
    hdr.add_string_array(RPMTAG_PROVIDEVERSION, provide_versions);
    hdr.add_int32(RPMTAG_PROVIDEFLAGS, provide_flags);

    // Conflicts.
    if !config.package.dependencies.conflicts.is_empty() {
        let mut cnames = Vec::new();
        let mut cversions = Vec::new();
        let mut cflags = Vec::new();
        for dep in &config.package.dependencies.conflicts {
            let (name, version, dep_flags) = parse_dependency(dep);
            cnames.push(name);
            cversions.push(version);
            cflags.push(dep_flags);
        }
        hdr.add_string_array(RPMTAG_CONFLICTNAME, cnames);
        hdr.add_string_array(RPMTAG_CONFLICTVERSION, cversions);
        hdr.add_int32(RPMTAG_CONFLICTFLAGS, cflags);
    }

    // Obsoletes (from config's "replaces" field).
    if !config.package.dependencies.replaces.is_empty() {
        let mut onames = Vec::new();
        let mut oversions = Vec::new();
        let mut oflags = Vec::new();
        for dep in &config.package.dependencies.replaces {
            let (name, version, dep_flags) = parse_dependency(dep);
            onames.push(name);
            oversions.push(version);
            oflags.push(dep_flags);
        }
        hdr.add_string_array(RPMTAG_OBSOLETENAME, onames);
        hdr.add_string_array(RPMTAG_OBSOLETEVERSION, oversions);
        hdr.add_int32(RPMTAG_OBSOLETEFLAGS, oflags);
    }

    Ok(())
}

/// Parse a dependency string like `"libfoo >= 1.0"` into (name, version, flags).
fn parse_dependency(dep: &str) -> (String, String, i32) {
    // Try to match patterns like: "name >= version", "name = version", "name"
    let parts: Vec<&str> = dep.splitn(3, ' ').collect();

    if parts.len() >= 3 {
        let name = parts[0].to_owned();
        let op = parts[1];
        let version = parts[2].to_owned();

        let flags = match op {
            ">=" => (RPMSENSE_GREATER | RPMSENSE_EQUAL) as i32,
            "<=" => (RPMSENSE_LESS | RPMSENSE_EQUAL) as i32,
            ">" => RPMSENSE_GREATER as i32,
            "<" => RPMSENSE_LESS as i32,
            "=" => RPMSENSE_EQUAL as i32,
            _ => RPMSENSE_ANY as i32,
        };

        (name, version, flags)
    } else {
        (dep.to_owned(), String::new(), RPMSENSE_ANY as i32)
    }
}

/// Populate script tags.
fn add_scripts(
    hdr: &mut HeaderBuilder,
    scripts: &spm_core::alternatives::ResolvedScripts,
) -> Result<(), RpmError> {
    if let Some(ref s) = scripts.pre_install {
        hdr.add_string(RPMTAG_PREIN, s);
        hdr.add_string(RPMTAG_PREINPROG, "/bin/sh");
    }
    if let Some(ref s) = scripts.post_install {
        hdr.add_string(RPMTAG_POSTIN, s);
        hdr.add_string(RPMTAG_POSTINPROG, "/bin/sh");
    }
    if let Some(ref s) = scripts.pre_remove {
        hdr.add_string(RPMTAG_PREUN, s);
        hdr.add_string(RPMTAG_PREUNPROG, "/bin/sh");
    }
    if let Some(ref s) = scripts.post_remove {
        hdr.add_string(RPMTAG_POSTUN, s);
        hdr.add_string(RPMTAG_POSTUNPROG, "/bin/sh");
    }
    if let Some(ref s) = scripts.pre_trans {
        hdr.add_string(RPMTAG_PRETRANS, s);
        hdr.add_string(RPMTAG_PRETRANSPROG, "/bin/sh");
    }
    if let Some(ref s) = scripts.post_trans {
        hdr.add_string(RPMTAG_POSTTRANS, s);
        hdr.add_string(RPMTAG_POSTTRANSPROG, "/bin/sh");
    }
    Ok(())
}

/// Convert a FileEntry to CpioMetadata.
fn file_entry_to_cpio_metadata(entry: &FileEntry) -> CpioMetadata {
    let filesize = match &entry.entry_type {
        EntryType::RegularFile => entry.size,
        EntryType::Directory => 0,
        EntryType::Symlink { target } => target.to_string_lossy().len() as u64,
        EntryType::Hardlink { .. } => {
            // Caller should have set size=0 for all-but-last links,
            // but we use the entry's size field directly.
            entry.size
        }
    };

    let mode = file_mode_with_type(entry);

    CpioMetadata {
        ino: 0, // Sequential inodes handled by the cpio writer index
        mode,
        uid: 0,
        gid: 0,
        nlink: 1,
        mtime: 0,
        filesize,
        devmajor: 0,
        devminor: 0,
        rdevmajor: 0,
        rdevminor: 0,
    }
}

/// Compute the full mode value including file type bits.
fn file_mode_with_type(entry: &FileEntry) -> u32 {
    let type_bits = match &entry.entry_type {
        EntryType::RegularFile | EntryType::Hardlink { .. } => 0o100000, // S_IFREG
        EntryType::Directory => 0o040000,                                // S_IFDIR
        EntryType::Symlink { .. } => 0o120000,                           // S_IFLNK
    };
    // entry.mode should have permission bits only (e.g. 0o755).
    // If it already has type bits, mask them out and re-apply.
    type_bits | (entry.mode & 0o7777)
}

/// Create the cpio name for an entry.
///
/// For Newc format, paths are prefixed with `./` as RPM convention.
/// For Extended format, the name is ignored (index-based), but we pass it anyway.
fn make_cpio_name(install_path: &Path, format: CpioFormat) -> String {
    let path_str = install_path.to_string_lossy();
    match format {
        CpioFormat::Newc => {
            if path_str.starts_with('/') {
                format!(".{path_str}")
            } else {
                format!("./{path_str}")
            }
        }
        CpioFormat::Extended => path_str.into_owned(),
    }
}

/// Write a single cpio entry, handling different file types.
fn write_cpio_entry<W: Write>(
    cpio: &mut CpioWriter<W>,
    index: u32,
    name: &str,
    metadata: &CpioMetadata,
    entry: &FileEntry,
) -> Result<(), RpmError> {
    match &entry.entry_type {
        EntryType::RegularFile => {
            let mut file = File::open(&entry.source_path).map_err(|e| RpmError::SourceFile {
                path: entry.source_path.clone(),
                source: e,
            })?;
            cpio.write_entry(index, name, metadata, &mut file)?;
        }
        EntryType::Directory => {
            cpio.write_entry(index, name, metadata, &mut io::empty())?;
        }
        EntryType::Symlink { target } => {
            let target_bytes = target.to_string_lossy().into_owned();
            let mut cursor = io::Cursor::new(target_bytes.as_bytes());
            cpio.write_entry(index, name, metadata, &mut cursor)?;
        }
        EntryType::Hardlink { .. } => {
            if metadata.filesize == 0 {
                cpio.write_entry(index, name, metadata, &mut io::empty())?;
            } else {
                let mut file =
                    File::open(&entry.source_path).map_err(|e| RpmError::SourceFile {
                        path: entry.source_path.clone(),
                        source: e,
                    })?;
                cpio.write_entry(index, name, metadata, &mut file)?;
            }
        }
    }
    Ok(())
}

/// Compute SHA-256 hex digests for all files in the entry list.
///
/// Returns one digest per file. Directories, symlinks, and zero-size
/// hardlink entries get an empty string.
fn compute_file_digests(files: &[FileEntry]) -> Result<Vec<String>, RpmError> {
    let mut digests = Vec::with_capacity(files.len());

    for entry in files {
        let digest = match &entry.entry_type {
            EntryType::RegularFile => {
                if entry.source_path.as_os_str().is_empty() {
                    String::new()
                } else {
                    sha256_file(&entry.source_path)?
                }
            }
            EntryType::Hardlink { .. } if entry.size > 0 => sha256_file(&entry.source_path)?,
            _ => String::new(),
        };
        digests.push(digest);
    }

    Ok(digests)
}

/// Compute SHA-256 hex digest of a file.
fn sha256_file(path: &Path) -> Result<String, RpmError> {
    let mut file = File::open(path).map_err(|e| RpmError::SourceFile {
        path: path.to_owned(),
        source: e,
    })?;
    let mut hasher = Sha256::new();
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

/// Get the hostname for BUILDHOST tag.
fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .unwrap_or_else(|_| "localhost".into())
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_decompose_paths_basic() {
        let files = vec![
            make_file_entry("/opt/app/bin/tool"),
            make_file_entry("/opt/app/lib/libfoo.so"),
        ];

        let (basenames, dirnames, dirindexes) = decompose_paths(&files);

        assert_eq!(basenames, vec!["tool", "libfoo.so"]);
        assert!(dirnames.contains(&"/opt/app/bin/".to_string()));
        assert!(dirnames.contains(&"/opt/app/lib/".to_string()));
        assert_eq!(dirindexes.len(), 2);
    }

    #[test]
    fn test_decompose_paths_shared_dir() {
        let files = vec![
            make_file_entry("/opt/app/file1"),
            make_file_entry("/opt/app/file2"),
        ];

        let (basenames, dirnames, dirindexes) = decompose_paths(&files);

        assert_eq!(basenames, vec!["file1", "file2"]);
        assert_eq!(dirnames, vec!["/opt/app/"]);
        assert_eq!(dirindexes, vec![0, 0]);
    }

    #[test]
    fn test_file_mode_with_type_regular() {
        let entry = FileEntry {
            install_path: PathBuf::from("/test"),
            source_path: PathBuf::new(),
            entry_type: EntryType::RegularFile,
            size: 0,
            mode: 0o755,
            user: "root".into(),
            group: "root".into(),
            is_config: false,
        };
        assert_eq!(file_mode_with_type(&entry), 0o100755);
    }

    #[test]
    fn test_file_mode_with_type_directory() {
        let entry = FileEntry {
            install_path: PathBuf::from("/test"),
            source_path: PathBuf::new(),
            entry_type: EntryType::Directory,
            size: 0,
            mode: 0o755,
            user: "root".into(),
            group: "root".into(),
            is_config: false,
        };
        assert_eq!(file_mode_with_type(&entry), 0o040755);
    }

    #[test]
    fn test_file_mode_with_type_symlink() {
        let entry = FileEntry {
            install_path: PathBuf::from("/test"),
            source_path: PathBuf::new(),
            entry_type: EntryType::Symlink {
                target: PathBuf::from("/other"),
            },
            size: 0,
            mode: 0o777,
            user: "root".into(),
            group: "root".into(),
            is_config: false,
        };
        assert_eq!(file_mode_with_type(&entry), 0o120777);
    }

    #[test]
    fn test_make_cpio_name_newc() {
        let name = make_cpio_name(Path::new("/opt/app/bin/tool"), CpioFormat::Newc);
        assert_eq!(name, "./opt/app/bin/tool");
    }

    #[test]
    fn test_make_cpio_name_extended() {
        let name = make_cpio_name(Path::new("/opt/app/bin/tool"), CpioFormat::Extended);
        assert_eq!(name, "/opt/app/bin/tool");
    }

    #[test]
    fn test_parse_dependency_with_version() {
        let (name, version, flags) = parse_dependency("libfoo >= 1.0");
        assert_eq!(name, "libfoo");
        assert_eq!(version, "1.0");
        assert_eq!(flags, (RPMSENSE_GREATER | RPMSENSE_EQUAL) as i32);
    }

    #[test]
    fn test_parse_dependency_no_version() {
        let (name, version, flags) = parse_dependency("libfoo");
        assert_eq!(name, "libfoo");
        assert_eq!(version, "");
        assert_eq!(flags, RPMSENSE_ANY as i32);
    }

    #[test]
    fn test_config_file_flags() {
        let entry = FileEntry {
            install_path: PathBuf::from("/etc/app.conf"),
            source_path: PathBuf::new(),
            entry_type: EntryType::RegularFile,
            size: 100,
            mode: 0o644,
            user: "root".into(),
            group: "root".into(),
            is_config: true,
        };

        let flags = if entry.is_config {
            (RPMFILE_CONFIG | RPMFILE_NOREPLACE) as i32
        } else {
            0
        };
        assert_eq!(flags, 0x81);
    }

    fn make_file_entry(path: &str) -> FileEntry {
        FileEntry {
            install_path: PathBuf::from(path),
            source_path: PathBuf::new(),
            entry_type: EntryType::RegularFile,
            size: 0,
            mode: 0o644,
            user: "root".into(),
            group: "root".into(),
            is_config: false,
        }
    }

    /// Helper: build a minimal Config for dependency/metadata tests.
    fn make_test_config() -> Config {
        use spm_core::config::*;
        Config {
            package: PackageConfig {
                name: "testpkg".into(),
                version: "1.0".into(),
                release: "1".into(),
                arch: "x86_64".into(),
                license: "MIT".into(),
                maintainer: "Test <test@test.com>".into(),
                description: "Test package".into(),
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
            splitting: SplittingConfig::default(),
            signing: None,
            rpm: None,
            deb: None,
            build: None,
        }
    }

    #[test]
    fn test_vendor_tag_emitted() {
        let mut config = make_test_config();
        config.package.vendor = Some("TestVendor Inc.".into());

        let mut hdr = HeaderBuilder::new();
        let plan = PackagePlan {
            name: "testpkg".into(),
            version: "1.0".into(),
            release: "1".into(),
            arch: "x86_64".into(),
            sub_packages: vec![],
            is_split: false,
            needs_extended_cpio: false,
            total_size: 0,
            warnings: vec![],
        };
        let sub_pkg = SubPackage {
            name: "testpkg".into(),
            role: spm_core::planner::SubPackageRole::Standalone,
            files: vec![],
            total_size: 0,
            scripts: spm_core::alternatives::ResolvedScripts::default(),
        };
        add_package_metadata(&mut hdr, &plan, &sub_pkg, &config, &Algorithm::Zstd).unwrap();

        let bytes = hdr.build().unwrap();
        // RPMTAG_VENDOR (1011) should appear in the binary header.
        // The string "TestVendor Inc." should be present in the data section.
        let data_str = String::from_utf8_lossy(&bytes);
        assert!(data_str.contains("TestVendor Inc."));
    }

    #[test]
    fn test_vendor_tag_not_emitted_when_none() {
        let config = make_test_config();

        let mut hdr = HeaderBuilder::new();
        let plan = PackagePlan {
            name: "testpkg".into(),
            version: "1.0".into(),
            release: "1".into(),
            arch: "x86_64".into(),
            sub_packages: vec![],
            is_split: false,
            needs_extended_cpio: false,
            total_size: 0,
            warnings: vec![],
        };
        let sub_pkg = SubPackage {
            name: "testpkg".into(),
            role: spm_core::planner::SubPackageRole::Standalone,
            files: vec![],
            total_size: 0,
            scripts: spm_core::alternatives::ResolvedScripts::default(),
        };
        add_package_metadata(&mut hdr, &plan, &sub_pkg, &config, &Algorithm::Zstd).unwrap();

        let bytes = hdr.build().unwrap();
        let data_str = String::from_utf8_lossy(&bytes);
        assert!(!data_str.contains("TestVendor"));
    }

    #[test]
    fn test_pretrans_posttrans_emitted() {
        use spm_core::alternatives::ResolvedScripts;

        let scripts = ResolvedScripts {
            pre_install: None,
            post_install: None,
            pre_remove: None,
            post_remove: None,
            pre_trans: Some("echo pretrans".into()),
            post_trans: Some("echo posttrans".into()),
        };

        let mut hdr = HeaderBuilder::new();
        add_scripts(&mut hdr, &scripts).unwrap();

        let bytes = hdr.build().unwrap();
        let data_str = String::from_utf8_lossy(&bytes);
        assert!(data_str.contains("echo pretrans"));
        assert!(data_str.contains("echo posttrans"));
    }

    #[test]
    fn test_conflicts_emitted() {
        let mut config = make_test_config();
        config.package.dependencies.conflicts = vec!["otherpkg >= 2.0".into()];

        let mut hdr = HeaderBuilder::new();
        add_dependencies(&mut hdr, &config, &Algorithm::Zstd, None).unwrap();

        let bytes = hdr.build().unwrap();
        let data_str = String::from_utf8_lossy(&bytes);
        assert!(data_str.contains("otherpkg"));
    }

    #[test]
    fn test_obsoletes_emitted() {
        let mut config = make_test_config();
        config.package.dependencies.replaces = vec!["oldpkg < 1.0".into()];

        let mut hdr = HeaderBuilder::new();
        add_dependencies(&mut hdr, &config, &Algorithm::Zstd, None).unwrap();

        let bytes = hdr.build().unwrap();
        let data_str = String::from_utf8_lossy(&bytes);
        assert!(data_str.contains("oldpkg"));
    }

    #[test]
    fn test_alternatives_dep_el8() {
        let mut config = make_test_config();
        config.content.alternatives = vec![spm_core::config::AlternativeConfig {
            name: "editor".into(),
            link: "/usr/bin/editor".into(),
            path: "/opt/app/bin/editor".into(),
            priority: 100,
            followers: vec![],
        }];

        let mut hdr = HeaderBuilder::new();
        add_dependencies(&mut hdr, &config, &Algorithm::Zstd, Some(&Distro::El8)).unwrap();

        let bytes = hdr.build().unwrap();
        let data_str = String::from_utf8_lossy(&bytes);
        assert!(data_str.contains("chkconfig"));
    }

    #[test]
    fn test_alternatives_dep_el9() {
        let mut config = make_test_config();
        config.content.alternatives = vec![spm_core::config::AlternativeConfig {
            name: "editor".into(),
            link: "/usr/bin/editor".into(),
            path: "/opt/app/bin/editor".into(),
            priority: 100,
            followers: vec![],
        }];

        let mut hdr = HeaderBuilder::new();
        add_dependencies(&mut hdr, &config, &Algorithm::Zstd, Some(&Distro::El9)).unwrap();

        let bytes = hdr.build().unwrap();
        let data_str = String::from_utf8_lossy(&bytes);
        assert!(data_str.contains("alternatives"));
    }

    #[test]
    fn test_alternatives_dep_default() {
        let mut config = make_test_config();
        config.content.alternatives = vec![spm_core::config::AlternativeConfig {
            name: "editor".into(),
            link: "/usr/bin/editor".into(),
            path: "/opt/app/bin/editor".into(),
            priority: 100,
            followers: vec![],
        }];

        let mut hdr = HeaderBuilder::new();
        add_dependencies(&mut hdr, &config, &Algorithm::Zstd, None).unwrap();

        let bytes = hdr.build().unwrap();
        let data_str = String::from_utf8_lossy(&bytes);
        assert!(data_str.contains("/usr/sbin/alternatives"));
    }

    #[test]
    fn test_no_alternatives_no_dep() {
        let config = make_test_config();

        let mut hdr = HeaderBuilder::new();
        add_dependencies(&mut hdr, &config, &Algorithm::Zstd, None).unwrap();

        let bytes = hdr.build().unwrap();
        let data_str = String::from_utf8_lossy(&bytes);
        assert!(!data_str.contains("/usr/sbin/alternatives"));
        assert!(!data_str.contains("chkconfig"));
    }
}
