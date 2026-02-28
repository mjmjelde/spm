//! DEB file metadata reader.
//!
//! Parses an existing DEB file to extract control metadata.
//! Used by `spm inspect` to display information about built packages.

use std::io::Read;
use std::path::Path;

use crate::error::DebError;

/// ar global magic header.
const AR_MAGIC: &[u8; 8] = b"!<arch>\n";

/// Size of an ar member header.
const AR_HEADER_SIZE: usize = 60;

/// Metadata extracted from a DEB file.
#[derive(Debug)]
pub struct DebMetadata {
    /// All control fields as (key, value) pairs, preserving order.
    pub fields: Vec<(String, String)>,
}

impl DebMetadata {
    /// Look up a control field by key (case-insensitive).
    pub fn get(&self, key: &str) -> Option<&str> {
        let key_lower = key.to_lowercase();
        self.fields
            .iter()
            .find(|(k, _)| k.to_lowercase() == key_lower)
            .map(|(_, v)| v.as_str())
    }
}

/// Read metadata from an existing DEB file.
pub fn read_deb_metadata(path: &Path) -> Result<DebMetadata, DebError> {
    let data = std::fs::read(path).map_err(|e| DebError::SourceFile {
        path: path.to_owned(),
        source: e,
    })?;

    if data.len() < AR_MAGIC.len() {
        return Err(DebError::InvalidDeb("file too small".into()));
    }
    if &data[..8] != AR_MAGIC.as_slice() {
        return Err(DebError::InvalidDeb("not a DEB file (bad ar magic)".into()));
    }

    // Find the control.tar member.
    let (member_name, member_data) = find_control_tar(&data)?;

    // Decompress and extract the control file.
    let control_content = extract_control_file(member_name, member_data)?;

    Ok(parse_control_file(&control_content))
}

/// Walk ar members to find the control.tar member.
/// Returns the member name and its raw data slice.
fn find_control_tar(data: &[u8]) -> Result<(&str, &[u8]), DebError> {
    let mut offset = 8; // Skip ar magic.

    while offset + AR_HEADER_SIZE <= data.len() {
        let (name, data_offset, data_size, next_offset) = parse_ar_header(data, offset)?;

        if name.starts_with("control.tar") {
            if data_offset + data_size > data.len() {
                return Err(DebError::InvalidDeb("control.tar member truncated".into()));
            }
            // Return name from header and the data slice.
            let name_str = std::str::from_utf8(&data[offset..offset + 16])
                .unwrap_or("")
                .trim_end_matches(|c: char| c == '/' || c == ' ');
            return Ok((name_str, &data[data_offset..data_offset + data_size]));
        }

        offset = next_offset;
    }

    Err(DebError::InvalidDeb("no control.tar member found".into()))
}

/// Parse an ar member header at the given offset.
/// Returns (name, data_offset, data_size, next_member_offset).
fn parse_ar_header(data: &[u8], offset: usize) -> Result<(String, usize, usize, usize), DebError> {
    if offset + AR_HEADER_SIZE > data.len() {
        return Err(DebError::InvalidDeb("truncated ar header".into()));
    }

    let header = &data[offset..offset + AR_HEADER_SIZE];

    // Validate file magic (last 2 bytes of header).
    if &header[58..60] != b"`\n" {
        return Err(DebError::InvalidDeb(
            "invalid ar member header magic".into(),
        ));
    }

    // Name is first 16 bytes, right-padded with spaces.
    let name = std::str::from_utf8(&header[0..16])
        .unwrap_or("")
        .trim_end_matches(|c: char| c == '/' || c == ' ')
        .to_string();

    // Size is bytes 48..58, ASCII decimal, space-padded.
    let size_str = std::str::from_utf8(&header[48..58]).unwrap_or("0").trim();
    let data_size: usize = size_str
        .parse()
        .map_err(|_| DebError::InvalidDeb(format!("invalid ar member size: '{size_str}'")))?;

    let data_offset = offset + AR_HEADER_SIZE;
    // Next member is aligned to even boundary.
    let next_offset = data_offset + data_size + (data_size % 2);

    Ok((name, data_offset, data_size, next_offset))
}

/// Detect compression from the control.tar member name and decompress.
/// Then extract the `./control` or `control` file from the tar archive.
fn extract_control_file(member_name: &str, compressed_data: &[u8]) -> Result<String, DebError> {
    let algorithm = if member_name.ends_with(".zst") {
        spm_compress::Algorithm::Zstd
    } else if member_name.ends_with(".gz") {
        spm_compress::Algorithm::Gzip
    } else if member_name.ends_with(".xz") {
        spm_compress::Algorithm::Xz
    } else {
        spm_compress::Algorithm::None
    };

    let reader = spm_compress::decompress_reader(algorithm, compressed_data)?;
    let mut archive = tar::Archive::new(reader);

    for entry in archive
        .entries()
        .map_err(|e| DebError::Tar(e.to_string()))?
    {
        let mut entry = entry.map_err(|e| DebError::Tar(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| DebError::Tar(e.to_string()))?
            .to_string_lossy()
            .into_owned();

        if path == "./control" || path == "control" {
            let mut content = String::new();
            entry
                .read_to_string(&mut content)
                .map_err(|e| DebError::Tar(e.to_string()))?;
            return Ok(content);
        }
    }

    Err(DebError::InvalidDeb(
        "no control file found in control.tar".into(),
    ))
}

/// Parse a Debian control file (RFC 822 format) into key-value pairs.
///
/// Fields are `Key: value\n`. Continuation lines start with a space or tab.
pub fn parse_control_file(content: &str) -> DebMetadata {
    let mut fields: Vec<(String, String)> = Vec::new();

    for line in content.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            // Continuation line: append to last field's value.
            if let Some(last) = fields.last_mut() {
                last.1.push('\n');
                last.1.push_str(line);
            }
        } else if let Some(colon_pos) = line.find(':') {
            let key = line[..colon_pos].to_string();
            let value = line[colon_pos + 1..].trim_start().to_string();
            fields.push((key, value));
        }
        // Skip empty lines and lines without colons.
    }

    DebMetadata { fields }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::DebBuilder;
    use spm_core::alternatives::ResolvedScripts;
    use spm_core::config::*;
    use spm_core::filetree::FileEntry;
    use spm_core::planner::{PackagePlan, SubPackage, SubPackageRole};
    use std::path::PathBuf;

    fn test_config() -> Config {
        Config {
            package: PackageConfig {
                name: "testpkg".into(),
                version: "1.0".into(),
                release: "1".into(),
                arch: "x86_64".into(),
                license: "MIT".into(),
                maintainer: "Test <test@example.com>".into(),
                description: "A test package".into(),
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

    fn test_plan(config: &Config) -> PackagePlan {
        PackagePlan {
            name: config.package.name.clone(),
            version: config.package.version.clone(),
            release: config.package.release.clone(),
            arch: config.package.arch.clone(),
            sub_packages: vec![SubPackage {
                name: config.package.name.clone(),
                role: SubPackageRole::Standalone,
                files: vec![],
                total_size: 0,
                scripts: ResolvedScripts::default(),
            }],
            is_split: false,
            needs_extended_cpio: false,
            total_size: 0,
            warnings: vec![],
        }
    }

    /// Build a DEB and return the path to it + owning tempdir.
    fn build_test_deb(config: &Config, files: Vec<FileEntry>) -> (PathBuf, tempfile::TempDir) {
        let mut plan = test_plan(config);
        plan.sub_packages[0].files = files.clone();
        plan.sub_packages[0].total_size = files.iter().map(|f| f.size).sum();
        plan.total_size = plan.sub_packages[0].total_size;

        let dir = tempfile::tempdir().unwrap();
        let paths = DebBuilder::build(&plan, config, dir.path()).unwrap();
        (paths[0].clone(), dir)
    }

    #[test]
    fn test_read_invalid_magic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.deb");
        std::fs::write(&path, b"not a deb file").unwrap();
        let result = read_deb_metadata(&path);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("bad ar magic"), "error was: {err}");
    }

    #[test]
    fn test_read_truncated_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tiny.deb");
        std::fs::write(&path, b"!<arch").unwrap();
        let result = read_deb_metadata(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_roundtrip_minimal() {
        let config = test_config();
        let (deb_path, _dir) = build_test_deb(&config, vec![]);
        let meta = read_deb_metadata(&deb_path).unwrap();

        assert_eq!(meta.get("Package"), Some("testpkg"));
        assert_eq!(meta.get("Version"), Some("1.0-1"));
        assert_eq!(meta.get("Architecture"), Some("amd64"));
    }

    #[test]
    fn test_roundtrip_with_depends() {
        let mut config = test_config();
        config.package.dependencies.requires = vec!["libfoo (>= 1.0)".into()];

        let (deb_path, _dir) = build_test_deb(&config, vec![]);
        let meta = read_deb_metadata(&deb_path).unwrap();

        let depends = meta.get("Depends").unwrap_or("");
        assert!(depends.contains("libfoo"), "depends: {depends}");
    }

    #[test]
    fn test_roundtrip_with_homepage() {
        let mut config = test_config();
        config.package.url = Some("https://example.com".into());

        let (deb_path, _dir) = build_test_deb(&config, vec![]);
        let meta = read_deb_metadata(&deb_path).unwrap();

        assert_eq!(meta.get("Homepage"), Some("https://example.com"));
    }

    #[test]
    fn test_parse_control_basic() {
        let content = "Package: testpkg\nVersion: 1.0\nArchitecture: amd64\n";
        let meta = parse_control_file(content);

        assert_eq!(meta.get("Package"), Some("testpkg"));
        assert_eq!(meta.get("Version"), Some("1.0"));
        assert_eq!(meta.get("Architecture"), Some("amd64"));
    }

    #[test]
    fn test_parse_control_continuation() {
        let content =
            "Package: testpkg\nDescription: Short desc\n Long description\n continues here\n";
        let meta = parse_control_file(content);

        let desc = meta.get("Description").unwrap();
        assert!(desc.contains("Short desc"));
        assert!(desc.contains("Long description"));
        assert!(desc.contains("continues here"));
    }

    #[test]
    fn test_parse_control_empty_value() {
        let content = "Package: testpkg\nDepends:\nVersion: 1.0\n";
        let meta = parse_control_file(content);

        assert_eq!(meta.get("Depends"), Some(""));
        assert_eq!(meta.get("Version"), Some("1.0"));
    }
}
