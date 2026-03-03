//! RPM file metadata reader.
//!
//! Parses an existing RPM file to extract package metadata from the header.
//! Used by `spm inspect` to display information about built packages.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::RpmError;
use crate::tags::*;

/// RPM lead magic number.
const RPM_MAGIC: [u8; 4] = [0xED, 0xAB, 0xEE, 0xDB];

/// RPM header magic bytes.
const HEADER_MAGIC: [u8; 4] = [0x8E, 0xAD, 0xE8, 0x01];

/// Size of the RPM lead in bytes.
const LEAD_SIZE: u64 = 96;

/// Metadata extracted from an RPM file.
#[derive(Debug)]
pub struct RpmMetadata {
    pub name: String,
    pub version: String,
    pub release: String,
    pub arch: String,
    pub size: u64,
    pub description: String,
    pub license: String,
    pub url: Option<String>,
    pub vendor: Option<String>,
    pub packager: Option<String>,
    pub compressor: Option<String>,
    pub file_count: usize,
    pub requires: Vec<String>,
}

/// A parsed tag value from an RPM header.
#[derive(Debug, Clone)]
enum ParsedTagValue {
    String(String),
    StringArray(Vec<String>),
    Int32(Vec<i32>),
    Int64(Vec<i64>),
    #[allow(dead_code)]
    Int16(Vec<i16>),
    #[allow(dead_code)]
    Bin(Vec<u8>),
}

/// Read metadata from an existing RPM file.
pub fn read_rpm_metadata(path: &Path) -> Result<RpmMetadata, RpmError> {
    let mut file = std::fs::File::open(path).map_err(|e| RpmError::SourceFile {
        path: path.to_owned(),
        source: e,
    })?;

    // 1. Validate RPM magic and skip lead.
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)?;
    if magic != RPM_MAGIC {
        return Err(RpmError::InvalidRpm("not an RPM file (bad magic)".into()));
    }
    file.seek(SeekFrom::Start(LEAD_SIZE))?;

    // 2. Skip the signature header.
    let sig_size = skip_header_section(&mut file)?;
    // Pad to 8-byte boundary after signature header.
    let total_sig = 16 + sig_size; // 16 = magic(4) + reserved(4) + index_count(4) + data_size(4)
    let sig_pad = (8 - (total_sig % 8)) % 8;
    if sig_pad > 0 {
        file.seek(SeekFrom::Current(sig_pad as i64))?;
    }

    // 3. Parse the metadata header.
    let tags = parse_header_section(&mut file)?;

    // 4. Extract fields from parsed tags.
    let name = extract_string(&tags, RPMTAG_NAME).unwrap_or_else(|| "<unknown>".into());
    let version = extract_string(&tags, RPMTAG_VERSION).unwrap_or_else(|| "<unknown>".into());
    let release = extract_string(&tags, RPMTAG_RELEASE).unwrap_or_else(|| "<unknown>".into());
    let arch = extract_string(&tags, RPMTAG_ARCH).unwrap_or_else(|| "<unknown>".into());

    // Prefer LONGSIZE (64-bit) over SIZE (32-bit).
    let size = extract_i64(&tags, RPMTAG_LONGSIZE)
        .map(|v| v as u64)
        .or_else(|| extract_i32(&tags, RPMTAG_SIZE).map(|v| v as u64))
        .unwrap_or(0);

    let description = extract_string(&tags, RPMTAG_DESCRIPTION).unwrap_or_default();
    let license = extract_string(&tags, RPMTAG_LICENSE).unwrap_or_default();
    let url = extract_string(&tags, RPMTAG_URL);
    let vendor = extract_string(&tags, RPMTAG_VENDOR);
    let packager = extract_string(&tags, RPMTAG_PACKAGER);
    let compressor = extract_string(&tags, RPMTAG_PAYLOADCOMPRESSOR);

    // File count from BASENAMES array length.
    let file_count = extract_string_array(&tags, RPMTAG_BASENAMES)
        .map(|v| v.len())
        .unwrap_or(0);

    // Dependencies from REQUIRENAME.
    let requires = extract_string_array(&tags, RPMTAG_REQUIRENAME).unwrap_or_default();

    Ok(RpmMetadata {
        name,
        version,
        release,
        arch,
        size,
        description,
        license,
        url,
        vendor,
        packager,
        compressor,
        file_count,
        requires,
    })
}

/// Skip a header section (signature or metadata), returning the byte count
/// of the index entries + data section (excludes the 16-byte preamble).
fn skip_header_section<R: Read + Seek>(reader: &mut R) -> Result<u64, RpmError> {
    let mut preamble = [0u8; 16];
    reader.read_exact(&mut preamble)?;

    if preamble[0..4] != HEADER_MAGIC {
        return Err(RpmError::InvalidRpm("invalid header magic".into()));
    }

    let index_count = u32::from_be_bytes(preamble[8..12].try_into().unwrap()) as u64;
    let data_size = u32::from_be_bytes(preamble[12..16].try_into().unwrap()) as u64;

    // Same bounds as parse_header_section to reject malformed/malicious RPMs.
    const MAX_INDEX_COUNT: u64 = 100_000;
    const MAX_DATA_SIZE: u64 = 64 * 1024 * 1024; // 64 MiB
    if index_count > MAX_INDEX_COUNT {
        return Err(RpmError::InvalidRpm(format!(
            "header index_count {index_count} exceeds maximum {MAX_INDEX_COUNT}"
        )));
    }
    if data_size > MAX_DATA_SIZE {
        return Err(RpmError::InvalidRpm(format!(
            "header data_size {data_size} exceeds maximum {MAX_DATA_SIZE}"
        )));
    }

    let skip_bytes = index_count * 16 + data_size;
    reader.seek(SeekFrom::Current(skip_bytes as i64))?;

    Ok(skip_bytes)
}

/// Parse a header section, returning all tag values.
fn parse_header_section<R: Read>(reader: &mut R) -> Result<Vec<(u32, ParsedTagValue)>, RpmError> {
    let mut preamble = [0u8; 16];
    reader.read_exact(&mut preamble)?;

    if preamble[0..4] != HEADER_MAGIC {
        return Err(RpmError::InvalidRpm("invalid header magic".into()));
    }

    let index_count = u32::from_be_bytes(preamble[8..12].try_into().unwrap()) as usize;
    let data_size = u32::from_be_bytes(preamble[12..16].try_into().unwrap()) as usize;

    // Guard against excessively large headers from malformed/malicious RPMs.
    const MAX_INDEX_COUNT: usize = 100_000;
    const MAX_DATA_SIZE: usize = 64 * 1024 * 1024; // 64 MiB
    if index_count > MAX_INDEX_COUNT {
        return Err(RpmError::InvalidRpm(format!(
            "header index_count {index_count} exceeds maximum {MAX_INDEX_COUNT}"
        )));
    }
    if data_size > MAX_DATA_SIZE {
        return Err(RpmError::InvalidRpm(format!(
            "header data_size {data_size} exceeds maximum {MAX_DATA_SIZE}"
        )));
    }

    // Read all index entries.
    let mut index_buf = vec![0u8; index_count * 16];
    reader.read_exact(&mut index_buf)?;

    // Read the data section.
    let mut data = vec![0u8; data_size];
    reader.read_exact(&mut data)?;

    // Parse each index entry and extract its value from the data section.
    let mut tags = Vec::with_capacity(index_count);
    for i in 0..index_count {
        let base = i * 16;
        let tag = u32::from_be_bytes(index_buf[base..base + 4].try_into().unwrap());
        let tag_type = u32::from_be_bytes(index_buf[base + 4..base + 8].try_into().unwrap());
        let offset_i32 =
            i32::from_be_bytes(index_buf[base + 8..base + 12].try_into().unwrap());
        let count =
            u32::from_be_bytes(index_buf[base + 12..base + 16].try_into().unwrap()) as usize;

        // Region tag entries (62, 63) have negative offsets — skip them since
        // they are format metadata, not package data we need to extract.
        if offset_i32 < 0 {
            continue;
        }
        let offset = offset_i32 as usize;

        if let Some(value) = extract_tag_value(tag_type, offset, count, &data) {
            tags.push((tag, value));
        }
    }

    Ok(tags)
}

/// Extract a tag value from the data section based on type, offset, and count.
fn extract_tag_value(
    tag_type: u32,
    offset: usize,
    count: usize,
    data: &[u8],
) -> Option<ParsedTagValue> {
    match tag_type {
        // String (type 6) or I18NString (type 9)
        6 | 9 => {
            let s = read_nul_string(data, offset)?;
            Some(ParsedTagValue::String(s))
        }
        // StringArray (type 8)
        8 => {
            let mut strings = Vec::with_capacity(count);
            let mut pos = offset;
            for _ in 0..count {
                let s = read_nul_string(data, pos)?;
                pos += s.len() + 1; // +1 for NUL
                strings.push(s);
            }
            Some(ParsedTagValue::StringArray(strings))
        }
        // Int16 (type 3)
        3 => {
            let mut values = Vec::with_capacity(count);
            for i in 0..count {
                let o = offset + i * 2;
                if o + 2 > data.len() {
                    return None;
                }
                values.push(i16::from_be_bytes(data[o..o + 2].try_into().unwrap()));
            }
            Some(ParsedTagValue::Int16(values))
        }
        // Int32 (type 4)
        4 => {
            let mut values = Vec::with_capacity(count);
            for i in 0..count {
                let o = offset + i * 4;
                if o + 4 > data.len() {
                    return None;
                }
                values.push(i32::from_be_bytes(data[o..o + 4].try_into().unwrap()));
            }
            Some(ParsedTagValue::Int32(values))
        }
        // Int64 (type 5)
        5 => {
            let mut values = Vec::with_capacity(count);
            for i in 0..count {
                let o = offset + i * 8;
                if o + 8 > data.len() {
                    return None;
                }
                values.push(i64::from_be_bytes(data[o..o + 8].try_into().unwrap()));
            }
            Some(ParsedTagValue::Int64(values))
        }
        // Bin (type 7)
        7 => {
            if offset + count > data.len() {
                return None;
            }
            Some(ParsedTagValue::Bin(data[offset..offset + count].to_vec()))
        }
        _ => None,
    }
}

/// Read a NUL-terminated string from a byte slice at the given offset.
fn read_nul_string(data: &[u8], offset: usize) -> Option<String> {
    if offset >= data.len() {
        return None;
    }
    let remaining = &data[offset..];
    let nul_pos = remaining.iter().position(|&b| b == 0)?;
    String::from_utf8(remaining[..nul_pos].to_vec()).ok()
}

/// Extract a string tag value.
fn extract_string(tags: &[(u32, ParsedTagValue)], tag_num: u32) -> Option<String> {
    for (tag, value) in tags {
        if *tag == tag_num {
            return match value {
                ParsedTagValue::String(s) => Some(s.clone()),
                ParsedTagValue::StringArray(v) if !v.is_empty() => Some(v[0].clone()),
                _ => None,
            };
        }
    }
    None
}

/// Extract the first i32 value from a tag.
fn extract_i32(tags: &[(u32, ParsedTagValue)], tag_num: u32) -> Option<i32> {
    for (tag, value) in tags {
        if *tag == tag_num {
            return match value {
                ParsedTagValue::Int32(v) if !v.is_empty() => Some(v[0]),
                _ => None,
            };
        }
    }
    None
}

/// Extract the first i64 value from a tag.
fn extract_i64(tags: &[(u32, ParsedTagValue)], tag_num: u32) -> Option<i64> {
    for (tag, value) in tags {
        if *tag == tag_num {
            return match value {
                ParsedTagValue::Int64(v) if !v.is_empty() => Some(v[0]),
                _ => None,
            };
        }
    }
    None
}

/// Extract a string array tag value.
fn extract_string_array(tags: &[(u32, ParsedTagValue)], tag_num: u32) -> Option<Vec<String>> {
    for (tag, value) in tags {
        if *tag == tag_num {
            return match value {
                ParsedTagValue::StringArray(v) => Some(v.clone()),
                _ => None,
            };
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::RpmBuilder;
    use spm_core::alternatives::ResolvedScripts;
    use spm_core::config::*;
    use spm_core::filetree::{EntryType, FileEntry};
    use spm_core::planner::{PackagePlan, SubPackage, SubPackageRole};
    use std::io::Write;
    use std::path::PathBuf;

    /// Build a minimal RPM to a temp file and return its path + owning tempdir.
    /// The tempdir must be kept alive so the RPM file is not deleted.
    fn build_test_rpm(config: &Config, files: Vec<FileEntry>) -> (PathBuf, tempfile::TempDir) {
        let plan = PackagePlan {
            name: config.package.name.clone(),
            version: config.package.version.clone(),
            release: config.package.release.clone(),
            arch: config.package.arch.clone(),
            sub_packages: vec![],
            is_split: false,
            needs_extended_cpio: false,
            total_size: files.iter().map(|f| f.size).sum(),
            warnings: vec![],
            deferred_split: false,
        };
        let sub_pkg = SubPackage {
            name: config.package.name.clone(),
            role: SubPackageRole::Standalone,
            files,
            total_size: plan.total_size,
            scripts: ResolvedScripts::default(),
        };
        let dir = tempfile::tempdir().unwrap();
        let rpm_path = dir.path().join("test.rpm");
        RpmBuilder::build(&sub_pkg, &plan, config, &rpm_path, None, None).unwrap();
        (rpm_path, dir)
    }

    fn test_config() -> Config {
        Config {
            package: PackageConfig {
                name: "testpkg".into(),
                version: "1.0".into(),
                release: "1".into(),
                arch: "x86_64".into(),
                license: "MIT".into(),
                maintainer: "Test <test@test.com>".into(),
                description: "A test package".into(),
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
            splitting: SplittingConfig::default(),
            signing: None,
            rpm: None,
            deb: None,
            build: None,
        }
    }

    /// Create a temp file with given content. Returns (path, handle).
    /// The handle must be kept alive so the file is not deleted.
    fn make_temp_file(content: &[u8]) -> (PathBuf, tempfile::NamedTempFile) {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(content).unwrap();
        let path = tmp.path().to_path_buf();
        (path, tmp)
    }

    #[test]
    fn test_read_invalid_magic() {
        let (path, _keep) = make_temp_file(b"not an rpm file at all!!");
        let result = read_rpm_metadata(&path);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("bad magic"), "error was: {err}");
    }

    #[test]
    fn test_read_truncated_file() {
        // Just the RPM magic, nothing else — should fail on lead/header read.
        let (path, _keep) = make_temp_file(&[0xED, 0xAB, 0xEE, 0xDB]);
        let result = read_rpm_metadata(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_roundtrip_minimal() {
        let config = test_config();
        let (source, _s) = make_temp_file(b"hello world");
        let files = vec![FileEntry {
            install_path: PathBuf::from("/opt/testpkg/hello.txt"),
            source_path: source,
            entry_type: EntryType::RegularFile,
            size: 11,
            mode: 0o644,
            user: "root".into(),
            group: "root".into(),
            is_config: false,
        }];

        let (rpm_path, _dir) = build_test_rpm(&config, files);
        let meta = read_rpm_metadata(&rpm_path).unwrap();

        assert_eq!(meta.name, "testpkg");
        assert_eq!(meta.version, "1.0");
        assert_eq!(meta.release, "1");
        assert_eq!(meta.arch, "x86_64");
        assert_eq!(meta.license, "MIT");
    }

    #[test]
    fn test_roundtrip_with_vendor() {
        let mut config = test_config();
        config.package.vendor = Some("TestVendor Inc.".into());

        let (rpm_path, _dir) = build_test_rpm(&config, vec![]);
        let meta = read_rpm_metadata(&rpm_path).unwrap();

        assert_eq!(meta.vendor.as_deref(), Some("TestVendor Inc."));
    }

    #[test]
    fn test_roundtrip_with_url() {
        let mut config = test_config();
        config.package.url = Some("https://example.com".into());

        let (rpm_path, _dir) = build_test_rpm(&config, vec![]);
        let meta = read_rpm_metadata(&rpm_path).unwrap();

        assert_eq!(meta.url.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn test_roundtrip_file_count() {
        let config = test_config();
        let (source1, _s1) = make_temp_file(b"file1");
        let (source2, _s2) = make_temp_file(b"file2");
        let files = vec![
            FileEntry {
                install_path: PathBuf::from("/opt/testpkg/file1"),
                source_path: source1,
                entry_type: EntryType::RegularFile,
                size: 5,
                mode: 0o644,
                user: "root".into(),
                group: "root".into(),
                is_config: false,
            },
            FileEntry {
                install_path: PathBuf::from("/opt/testpkg/file2"),
                source_path: source2,
                entry_type: EntryType::RegularFile,
                size: 5,
                mode: 0o644,
                user: "root".into(),
                group: "root".into(),
                is_config: false,
            },
        ];

        let (rpm_path, _dir) = build_test_rpm(&config, files);
        let meta = read_rpm_metadata(&rpm_path).unwrap();

        assert_eq!(meta.file_count, 2);
    }

    #[test]
    fn test_roundtrip_requires() {
        let mut config = test_config();
        config.package.dependencies.requires = vec!["libfoo >= 1.0".into()];

        let (rpm_path, _dir) = build_test_rpm(&config, vec![]);
        let meta = read_rpm_metadata(&rpm_path).unwrap();

        // Should contain user dep plus rpmlib implicit deps.
        assert!(
            meta.requires.iter().any(|r| r == "libfoo"),
            "requires: {:?}",
            meta.requires
        );
    }

    #[test]
    fn test_roundtrip_compressor() {
        let config = test_config();
        let (rpm_path, _dir) = build_test_rpm(&config, vec![]);
        let meta = read_rpm_metadata(&rpm_path).unwrap();

        // Default algorithm is zstd.
        assert_eq!(meta.compressor.as_deref(), Some("zstd"));
    }

    #[test]
    fn test_roundtrip_description() {
        let config = test_config();
        let (rpm_path, _dir) = build_test_rpm(&config, vec![]);
        let meta = read_rpm_metadata(&rpm_path).unwrap();

        assert_eq!(meta.description, "A test package");
    }

    #[test]
    fn test_roundtrip_size() {
        let config = test_config();
        let (source, _s) = make_temp_file(&vec![0u8; 4096]);
        let files = vec![FileEntry {
            install_path: PathBuf::from("/opt/testpkg/data.bin"),
            source_path: source,
            entry_type: EntryType::RegularFile,
            size: 4096,
            mode: 0o644,
            user: "root".into(),
            group: "root".into(),
            is_config: false,
        }];

        let (rpm_path, _dir) = build_test_rpm(&config, files);
        let meta = read_rpm_metadata(&rpm_path).unwrap();

        assert_eq!(meta.size, 4096);
    }
}
