//! DEB file metadata reader.
//!
//! Parses an existing DEB file to extract control metadata.
//! Used by `spm inspect` to display information about built packages.

use std::io::{BufReader, Read, Seek, SeekFrom};
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
///
/// Uses streaming I/O — only reads ar headers and the control.tar member,
/// avoiding loading multi-GiB data.tar members into memory.
pub fn read_deb_metadata(path: &Path) -> Result<DebMetadata, DebError> {
    let file = std::fs::File::open(path).map_err(|e| DebError::SourceFile {
        path: path.to_owned(),
        source: e,
    })?;
    let mut reader = BufReader::new(file);

    // Read and verify ar magic.
    let mut magic = [0u8; 8];
    reader
        .read_exact(&mut magic)
        .map_err(|_| DebError::InvalidDeb("file too small".into()))?;
    if magic != *AR_MAGIC {
        return Err(DebError::InvalidDeb("not a DEB file (bad ar magic)".into()));
    }

    // Find the control.tar member by streaming through ar headers.
    let (member_name, member_data) = find_control_tar_streaming(&mut reader)?;

    // Decompress and extract the control file.
    let control_content = extract_control_file(&member_name, &member_data)?;

    Ok(parse_control_file(&control_content))
}

/// Stream through ar members to find and read the control.tar member.
/// Only reads header bytes and the (small) control.tar data; skips over
/// large data.tar members using seek.
fn find_control_tar_streaming<R: Read + Seek>(
    reader: &mut R,
) -> Result<(String, Vec<u8>), DebError> {
    let mut header_buf = [0u8; AR_HEADER_SIZE];

    loop {
        // Try to read the next ar member header.
        match reader.read_exact(&mut header_buf) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(DebError::InvalidDeb("no control.tar member found".into()));
            }
            Err(e) => return Err(DebError::Tar(e.to_string())),
        }

        // Validate file magic (last 2 bytes of header).
        if &header_buf[58..60] != b"`\n" {
            return Err(DebError::InvalidDeb(
                "invalid ar member header magic".into(),
            ));
        }

        // Name is first 16 bytes, right-padded with spaces.
        let name = std::str::from_utf8(&header_buf[0..16])
            .map_err(|_| DebError::InvalidDeb("non-UTF8 ar member name".into()))?
            .trim_end_matches(['/', ' '])
            .to_string();

        // Size is bytes 48..58, ASCII decimal, space-padded.
        let size_str = std::str::from_utf8(&header_buf[48..58])
            .unwrap_or("0")
            .trim();
        let data_size: u64 = size_str
            .parse()
            .map_err(|_| DebError::InvalidDeb(format!("invalid ar member size: '{size_str}'")))?;

        if name.starts_with("control.tar") {
            // control.tar is small (typically a few KB) — read it entirely.
            let mut data = vec![0u8; data_size as usize];
            reader
                .read_exact(&mut data)
                .map_err(|_| DebError::InvalidDeb("control.tar member truncated".into()))?;
            return Ok((name, data));
        }

        // Skip this member's data (+ padding to even boundary).
        let skip = data_size + (data_size % 2);
        reader
            .seek(SeekFrom::Current(skip as i64))
            .map_err(|e| DebError::Tar(e.to_string()))?;
    }
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
            deferred_split: false,
        }
    }

    /// Build a DEB and return the path to it + owning tempdir.
    fn build_test_deb(config: &Config, files: Vec<FileEntry>) -> (PathBuf, tempfile::TempDir) {
        let mut plan = test_plan(config);
        plan.sub_packages[0].files = files.clone();
        plan.sub_packages[0].total_size = files.iter().map(|f| f.size).sum();
        plan.total_size = plan.sub_packages[0].total_size;

        let dir = tempfile::tempdir().unwrap();
        let paths = DebBuilder::build(&plan, config, dir.path(), None).unwrap();
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
