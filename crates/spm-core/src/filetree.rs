/// File tree walking and entry collection.
///
/// Applies file mapping rules from config and produces a sorted, deduplicated
/// list of `FileEntry` values representing every file, directory, and symlink
/// to include in the package.
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::config::{ContentConfig, ContentDefaults};
use crate::error::FileTreeError;

/// Validate that an install path is absolute and contains no `..` components.
fn validate_install_path(path: &str, context: &str) -> Result<(), FileTreeError> {
    if !path.starts_with('/') {
        return Err(FileTreeError::InvalidMapping {
            src: String::new(),
            dst: path.to_string(),
            reason: format!("{context} must be an absolute path (starting with '/')"),
        });
    }
    if Path::new(path)
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(FileTreeError::InvalidMapping {
            src: String::new(),
            dst: path.to_string(),
            reason: format!("{context} must not contain '..' components"),
        });
    }
    Ok(())
}

/// A single file, directory, or symlink to include in the package.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Absolute path as it will appear inside the package.
    pub install_path: PathBuf,
    /// Path to the source file on disk (empty for synthetic entries like symlinks/dirs from config).
    pub source_path: PathBuf,
    /// Type of this entry.
    pub entry_type: EntryType,
    /// File size in bytes (0 for directories and symlinks).
    pub size: u64,
    /// Unix file mode (e.g., 0o755).
    pub mode: u32,
    /// Owner username.
    pub user: String,
    /// Group name.
    pub group: String,
    /// Whether this file is a config file (noreplace/conffile).
    pub is_config: bool,
}

/// The type of a file entry.
#[derive(Debug, Clone, PartialEq)]
pub enum EntryType {
    RegularFile,
    Directory,
    Symlink { target: PathBuf },
    Hardlink { target: PathBuf },
}

/// Builds a file tree from a source directory and config mappings.
pub struct FileTree;

impl FileTree {
    /// Walk file mappings, apply rules, and return all entries.
    ///
    /// The returned entries are sorted by `install_path` and deduplicated
    /// (first mapping match wins). Implicit parent directories are included.
    pub fn walk(content: &ContentConfig) -> Result<Vec<FileEntry>, FileTreeError> {
        let mut entries: Vec<FileEntry> = Vec::new();
        let mut seen_install_paths: HashSet<PathBuf> = HashSet::new();
        // Track (dev, ino) -> first install_path for hardlink detection.
        let mut inode_map: HashMap<(u64, u64), PathBuf> = HashMap::new();

        let defaults = &content.defaults;

        // Process file mappings (first match wins).
        for mapping in &content.files {
            let new_entries = process_file_mapping(mapping, &mut inode_map, defaults)?;
            for entry in new_entries {
                if seen_install_paths.insert(entry.install_path.clone()) {
                    entries.push(entry);
                }
            }
        }

        // Process symlinks from config.
        for sym in &content.symlinks {
            if sym.src.is_empty() {
                return Err(FileTreeError::InvalidMapping {
                    src: sym.src.clone(),
                    dst: sym.dst.clone(),
                    reason: "symlink target must not be empty".into(),
                });
            }
            if sym.dst.is_empty() {
                return Err(FileTreeError::InvalidMapping {
                    src: sym.src.clone(),
                    dst: sym.dst.clone(),
                    reason: "symlink install path must not be empty".into(),
                });
            }
            validate_install_path(&sym.dst, "symlink dst")?;
            let install_path = PathBuf::from(&sym.dst);
            if seen_install_paths.insert(install_path.clone()) {
                entries.push(FileEntry {
                    install_path,
                    source_path: PathBuf::new(),
                    entry_type: EntryType::Symlink {
                        target: PathBuf::from(&sym.src),
                    },
                    size: 0,
                    mode: 0o120777,
                    user: defaults.user.clone(),
                    group: defaults.group.clone(),
                    is_config: false,
                });
            }
        }

        // Process directories from config.
        for dir in &content.directories {
            validate_install_path(&dir.path, "directory path")?;
            let install_path = PathBuf::from(&dir.path);
            if seen_install_paths.insert(install_path.clone()) {
                let mode = dir
                    .mode
                    .as_ref()
                    .map(|m| parse_mode(m))
                    .transpose()?
                    .or_else(|| defaults.dir_mode.as_ref().and_then(|m| parse_mode(m).ok()))
                    .unwrap_or(0o755);
                entries.push(FileEntry {
                    install_path,
                    source_path: PathBuf::new(),
                    entry_type: EntryType::Directory,
                    size: 0,
                    mode,
                    user: dir.user.clone().unwrap_or_else(|| defaults.user.clone()),
                    group: dir.group.clone().unwrap_or_else(|| defaults.group.clone()),
                    is_config: false,
                });
            }
        }

        // Add implicit parent directories.
        add_implicit_directories(&mut entries, &mut seen_install_paths, defaults);

        // Sort by install_path for deterministic ordering.
        entries.sort_by(|a, b| a.install_path.cmp(&b.install_path));

        Ok(entries)
    }
}

/// Process a single file mapping rule, expanding globs and computing install paths.
///
/// For patterns containing `**`, we use `walkdir` for recursive traversal and
/// `glob::Pattern` for matching (the `glob::glob()` function doesn't match files
/// with a trailing `/**` pattern). For simpler globs, `glob::glob()` is used directly.
fn process_file_mapping(
    mapping: &crate::config::FileMapping,
    inode_map: &mut HashMap<(u64, u64), PathBuf>,
    defaults: &ContentDefaults,
) -> Result<Vec<FileEntry>, FileTreeError> {
    let src_pattern = &mapping.src;
    let dst = &mapping.dst;

    validate_install_path(dst.trim_end_matches('/'), "file mapping dst")?;

    // Determine if src is a glob pattern or a plain path.
    let is_glob =
        src_pattern.contains('*') || src_pattern.contains('?') || src_pattern.contains('[');

    // Resolve the pattern: if relative, make it relative to the current working directory.
    let resolved_pattern = if Path::new(src_pattern).is_absolute() {
        src_pattern.to_string()
    } else {
        std::env::current_dir()
            .map_err(|e| FileTreeError::Metadata {
                path: PathBuf::from(src_pattern),
                source: e,
            })?
            .join(src_pattern)
            .to_string_lossy()
            .to_string()
    };

    // If src is a plain directory path (no glob chars), auto-expand to dir/**
    // and ensure dst is treated as a directory too.
    let (resolved_pattern, is_glob, dst) = if !is_glob && Path::new(&resolved_pattern).is_dir() {
        let dst = if dst.ends_with('/') {
            dst.to_string()
        } else {
            format!("{dst}/")
        };
        (
            format!("{}/**", resolved_pattern.trim_end_matches('/')),
            true,
            dst,
        )
    } else {
        (resolved_pattern, is_glob, dst.to_string())
    };

    // Expand the pattern to a list of matching paths.
    let matched_paths = expand_glob_pattern(&resolved_pattern)?;

    if matched_paths.is_empty() {
        return Err(FileTreeError::NoMatches {
            pattern: src_pattern.clone(),
        });
    }

    // For multi-file globs, dst must end with '/'.
    let dst_is_dir = dst.ends_with('/');
    if matched_paths.len() > 1 && !dst_is_dir && is_glob {
        return Err(FileTreeError::InvalidMapping {
            src: src_pattern.clone(),
            dst: dst.clone(),
            reason: "glob matches multiple files but dst does not end with '/'".to_string(),
        });
    }

    // Compute the glob base (longest non-glob prefix) for relative path calculation.
    let glob_base = if is_glob {
        glob_base_path(&resolved_pattern)
    } else if Path::new(src_pattern).is_absolute() {
        // Single absolute file: the parent is the base.
        Path::new(src_pattern)
            .parent()
            .unwrap_or(Path::new("/"))
            .to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };

    let file_mode_override = mapping.mode.as_ref().map(|m| parse_mode(m)).transpose()?;
    let dir_mode_override = mapping
        .dir_mode
        .as_ref()
        .map(|m| parse_mode(m))
        .transpose()?;
    let default_file_mode = defaults
        .file_mode
        .as_ref()
        .map(|m| parse_mode(m))
        .transpose()?;
    let default_dir_mode = defaults
        .dir_mode
        .as_ref()
        .map(|m| parse_mode(m))
        .transpose()?;
    let user_override = mapping.user.clone();
    let group_override = mapping.group.clone();
    let is_config = mapping
        .r#type
        .as_ref()
        .map(|t| t == "config")
        .unwrap_or(false);

    let mut entries = Vec::new();

    for source_path in matched_paths {
        // Get metadata (using symlink_metadata to not follow symlinks).
        let metadata =
            std::fs::symlink_metadata(&source_path).map_err(|e| FileTreeError::Metadata {
                path: source_path.clone(),
                source: e,
            })?;

        // Compute the install path.
        let install_path = if dst_is_dir {
            // Directory destination: append relative path from glob base.
            let relative = source_path.strip_prefix(&glob_base).unwrap_or(&source_path);
            PathBuf::from(&dst).join(relative)
        } else {
            // Direct file-to-file mapping.
            PathBuf::from(&dst)
        };

        // Determine entry type and collect metadata.
        let file_type = metadata.file_type();

        // Resolve user/group: per-mapping > defaults > "root"
        let resolved_user = user_override
            .clone()
            .unwrap_or_else(|| defaults.user.clone());
        let resolved_group = group_override
            .clone()
            .unwrap_or_else(|| defaults.group.clone());

        if file_type.is_symlink() {
            let target = std::fs::read_link(&source_path).map_err(|e| FileTreeError::Metadata {
                path: source_path.clone(),
                source: e,
            })?;
            entries.push(FileEntry {
                install_path,
                source_path,
                entry_type: EntryType::Symlink { target },
                size: 0,
                mode: 0o120777,
                user: resolved_user,
                group: resolved_group,
                is_config,
            });
        } else if file_type.is_dir() {
            // Directory mode: per-mapping dir_mode > per-mapping mode > defaults dir_mode > source
            let mode = dir_mode_override
                .or(file_mode_override)
                .or(default_dir_mode)
                .unwrap_or(metadata.mode() & 0o7777);
            entries.push(FileEntry {
                install_path,
                source_path,
                entry_type: EntryType::Directory,
                size: 0,
                mode,
                user: resolved_user,
                group: resolved_group,
                is_config: false,
            });
        } else if file_type.is_file() {
            // File mode: per-mapping mode > defaults file_mode > source
            let mode = file_mode_override
                .or(default_file_mode)
                .unwrap_or(metadata.mode() & 0o7777);
            let size = metadata.len();
            let dev = metadata.dev();
            let ino = metadata.ino();
            let nlink = metadata.nlink();

            // Hardlink detection: if nlink > 1 and we've seen this inode before.
            let entry_type = if nlink > 1 {
                if let Some(first_path) = inode_map.get(&(dev, ino)) {
                    EntryType::Hardlink {
                        target: first_path.clone(),
                    }
                } else {
                    inode_map.insert((dev, ino), install_path.clone());
                    EntryType::RegularFile
                }
            } else {
                EntryType::RegularFile
            };

            // Hardlinks have size 0 in the archive (data is written with the last link).
            let effective_size = match &entry_type {
                EntryType::Hardlink { .. } => 0,
                _ => size,
            };

            entries.push(FileEntry {
                install_path,
                source_path,
                entry_type,
                size: effective_size,
                mode,
                user: resolved_user,
                group: resolved_group,
                is_config,
            });
        } else {
            // Special file (block/char device, FIFO, socket) — reject with
            // a clear error instead of silently skipping.
            return Err(FileTreeError::InvalidMapping {
                src: source_path.to_string_lossy().into_owned(),
                dst: install_path.to_string_lossy().into_owned(),
                reason: "special files (devices, FIFOs, sockets) are not supported".into(),
            });
        }
    }

    Ok(entries)
}

/// Expand a glob pattern to a list of matching filesystem paths.
///
/// For patterns containing `**`, uses `walkdir` for recursive traversal
/// combined with `glob::Pattern` for matching, because `glob::glob()` with
/// a trailing `/**` only matches directories, not files within them.
fn expand_glob_pattern(pattern: &str) -> Result<Vec<PathBuf>, FileTreeError> {
    if pattern.contains("**") {
        // Use walkdir + glob::Pattern for recursive patterns.
        let base = glob_base_path(pattern);
        let glob_pattern = glob::Pattern::new(pattern).map_err(|e| FileTreeError::InvalidGlob {
            pattern: pattern.to_string(),
            source: e,
        })?;

        let match_opts = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        };

        let mut results = Vec::new();
        if base.is_dir() {
            for entry in walkdir::WalkDir::new(&base).min_depth(1) {
                let entry = entry?;
                let path = entry.path().to_path_buf();
                if glob_pattern.matches_path_with(&path, match_opts) {
                    results.push(path);
                }
            }
        }
        Ok(results)
    } else {
        // Simple glob without ** — use glob::glob() directly.
        let paths: Vec<PathBuf> = glob::glob(pattern)
            .map_err(|e| FileTreeError::InvalidGlob {
                pattern: pattern.to_string(),
                source: e,
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| FileTreeError::Metadata {
                path: PathBuf::from(pattern),
                source: e.into_error(),
            })?;
        Ok(paths)
    }
}

/// Extract the longest non-glob prefix from a glob pattern.
///
/// For "/opt/staging/R2025a/**", returns "/opt/staging/R2025a/".
/// For "src/*.rs", returns "src/".
fn glob_base_path(pattern: &str) -> PathBuf {
    let path = Path::new(pattern);
    let mut base = PathBuf::new();
    for component in path.components() {
        let s = component.as_os_str().to_string_lossy();
        if s.contains('*') || s.contains('?') || s.contains('[') {
            break;
        }
        base.push(component);
    }
    // Ensure the base is a directory path.
    if base.as_os_str().is_empty() {
        base.push(".");
    }
    base
}

/// Parse an octal mode string like "0755" into a u32.
fn parse_mode(mode_str: &str) -> Result<u32, FileTreeError> {
    let stripped = mode_str.trim_start_matches('0');
    let s = if stripped.is_empty() { "0" } else { stripped };
    u32::from_str_radix(s, 8).map_err(|_| FileTreeError::InvalidMapping {
        src: String::new(),
        dst: String::new(),
        reason: format!("invalid mode '{mode_str}': expected octal like '0755'"),
    })
}

/// Well-known system directories that should never be owned by a package.
///
/// These are standard FHS paths owned by the `filesystem` package on RPM distros
/// and `base-files` on DEB distros. Owning them causes install conflicts.
fn is_system_directory(path: &Path) -> bool {
    static SYSTEM_DIRS: &[&str] = &[
        "/usr",
        "/usr/bin",
        "/usr/sbin",
        "/usr/lib",
        "/usr/lib64",
        "/usr/libexec",
        "/usr/share",
        "/usr/share/man",
        "/usr/share/doc",
        "/usr/share/info",
        "/usr/include",
        "/usr/local",
        "/usr/local/bin",
        "/usr/local/lib",
        "/usr/local/share",
        "/usr/local/include",
        "/etc",
        "/var",
        "/var/lib",
        "/var/log",
        "/var/run",
        "/var/cache",
        "/var/tmp",
        "/opt",
        "/bin",
        "/sbin",
        "/lib",
        "/lib64",
        "/tmp",
        "/run",
        "/srv",
        "/home",
        "/root",
        "/mnt",
        "/media",
        "/boot",
        "/dev",
        "/proc",
        "/sys",
    ];
    SYSTEM_DIRS.iter().any(|d| path == Path::new(d))
}

/// Add implicit parent directory entries for all files.
///
/// For a file at /opt/app/bin/tool, ensures /opt/app/ and /opt/app/bin/
/// exist as directory entries. Well-known system directories (e.g. /opt,
/// /usr, /usr/bin) are excluded to avoid conflicts with the filesystem
/// package. Uses global defaults for mode/user/group.
fn add_implicit_directories(
    entries: &mut Vec<FileEntry>,
    seen: &mut HashSet<PathBuf>,
    defaults: &ContentDefaults,
) {
    let mut dirs_to_add: Vec<PathBuf> = Vec::new();

    for entry in entries.iter() {
        let mut current = entry.install_path.parent();
        while let Some(parent) = current {
            if parent == Path::new("/") || parent == Path::new("") {
                break;
            }
            let parent_buf = parent.to_path_buf();
            if is_system_directory(&parent_buf) {
                break; // System dir — don't own it or any of its parents.
            }
            if seen.contains(&parent_buf) {
                break; // Already have this dir (and therefore all its parents).
            }
            dirs_to_add.push(parent_buf.clone());
            seen.insert(parent_buf);
            current = parent.parent();
        }
    }

    let dir_mode = defaults
        .dir_mode
        .as_ref()
        .and_then(|m| parse_mode(m).ok())
        .unwrap_or(0o755);

    for dir_path in dirs_to_add {
        entries.push(FileEntry {
            install_path: dir_path,
            source_path: PathBuf::new(),
            entry_type: EntryType::Directory,
            size: 0,
            mode: dir_mode,
            user: defaults.user.clone(),
            group: defaults.group.clone(),
            is_config: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ContentConfig, ContentDefaults, DirectoryMapping, FileMapping, SymlinkMapping,
    };
    use std::fs;
    use tempfile::TempDir;

    /// Create a minimal ContentConfig for testing.
    fn test_content(files: Vec<FileMapping>) -> ContentConfig {
        ContentConfig {
            defaults: ContentDefaults::default(),
            files,
            symlinks: vec![],
            directories: vec![],
            alternatives: vec![],
        }
    }

    /// Create a ContentConfig with custom defaults.
    fn test_content_with_defaults(
        files: Vec<FileMapping>,
        defaults: ContentDefaults,
    ) -> ContentConfig {
        ContentConfig {
            defaults,
            files,
            symlinks: vec![],
            directories: vec![],
            alternatives: vec![],
        }
    }

    #[test]
    fn test_walk_simple_directory() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();

        // Create test files.
        fs::create_dir_all(base.join("bin")).unwrap();
        fs::write(base.join("bin/hello"), "#!/bin/bash\necho hello").unwrap();
        fs::write(base.join("bin/world"), "#!/bin/bash\necho world").unwrap();

        let content = test_content(vec![FileMapping {
            src: format!("{}/**", base.display()),
            dst: "/opt/testpkg/".to_string(),
            mode: None,
            dir_mode: None,
            user: None,
            group: None,
            r#type: None,
        }]);

        let entries = FileTree::walk(&content).unwrap();

        // Should have the bin dir + 2 files + implicit parent /opt/testpkg.
        let file_entries: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e.entry_type, EntryType::RegularFile))
            .collect();
        assert_eq!(file_entries.len(), 2);

        let dir_entries: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e.entry_type, EntryType::Directory))
            .collect();
        assert!(!dir_entries.is_empty()); // At least bin/ dir

        // Verify install paths.
        let install_paths: Vec<String> = entries
            .iter()
            .map(|e| e.install_path.to_string_lossy().to_string())
            .collect();
        assert!(install_paths.iter().any(|p| p.contains("bin/hello")));
        assert!(install_paths.iter().any(|p| p.contains("bin/world")));
    }

    #[test]
    fn test_walk_directory_src_auto_expand() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();

        // Create test files.
        fs::create_dir_all(base.join("bin")).unwrap();
        fs::write(base.join("bin/hello"), "#!/bin/bash\necho hello").unwrap();
        fs::write(base.join("bin/world"), "#!/bin/bash\necho world").unwrap();

        // Use a bare directory path (no **) — should auto-expand.
        let content = test_content(vec![FileMapping {
            src: format!("{}", base.display()),
            dst: "/opt/testpkg/".to_string(),
            mode: None,
            dir_mode: None,
            user: None,
            group: None,
            r#type: None,
        }]);

        let entries = FileTree::walk(&content).unwrap();

        let file_entries: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e.entry_type, EntryType::RegularFile))
            .collect();
        assert_eq!(file_entries.len(), 2);

        let install_paths: Vec<String> = entries
            .iter()
            .map(|e| e.install_path.to_string_lossy().to_string())
            .collect();
        assert!(install_paths.iter().any(|p| p.contains("bin/hello")));
        assert!(install_paths.iter().any(|p| p.contains("bin/world")));
    }

    #[test]
    fn test_walk_directory_src_auto_expand_dst_no_trailing_slash() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();

        // Create test files.
        fs::create_dir_all(base.join("bin")).unwrap();
        fs::write(base.join("bin/hello"), "#!/bin/bash\necho hello").unwrap();
        fs::write(base.join("bin/world"), "#!/bin/bash\necho world").unwrap();

        // Use a bare directory src AND dst without trailing slash — should still work.
        let content = test_content(vec![FileMapping {
            src: format!("{}", base.display()),
            dst: "/opt/testpkg".to_string(),
            mode: None,
            dir_mode: None,
            user: None,
            group: None,
            r#type: None,
        }]);

        let entries = FileTree::walk(&content).unwrap();

        let file_entries: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e.entry_type, EntryType::RegularFile))
            .collect();
        assert_eq!(file_entries.len(), 2);

        let install_paths: Vec<String> = entries
            .iter()
            .map(|e| e.install_path.to_string_lossy().to_string())
            .collect();
        assert!(install_paths.iter().any(|p| p.contains("bin/hello")));
        assert!(install_paths.iter().any(|p| p.contains("bin/world")));
    }

    #[test]
    fn test_walk_mode_override() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        fs::write(base.join("script.sh"), "#!/bin/bash").unwrap();

        let content = test_content(vec![FileMapping {
            src: format!("{}/script.sh", base.display()),
            dst: "/usr/bin/script.sh".to_string(),
            mode: Some("0755".to_string()),
            dir_mode: None,
            user: None,
            group: None,
            r#type: None,
        }]);

        let entries = FileTree::walk(&content).unwrap();
        let file_entry = entries
            .iter()
            .find(|e| matches!(e.entry_type, EntryType::RegularFile))
            .unwrap();
        assert_eq!(file_entry.mode, 0o755);
    }

    #[test]
    fn test_walk_config_file_flag() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        fs::write(base.join("app.conf"), "setting=value").unwrap();

        let content = test_content(vec![FileMapping {
            src: format!("{}/app.conf", base.display()),
            dst: "/etc/app.conf".to_string(),
            mode: None,
            dir_mode: None,
            user: None,
            group: None,
            r#type: Some("config".to_string()),
        }]);

        let entries = FileTree::walk(&content).unwrap();
        let file_entry = entries
            .iter()
            .find(|e| matches!(e.entry_type, EntryType::RegularFile))
            .unwrap();
        assert!(file_entry.is_config);
    }

    #[test]
    fn test_walk_symlink_entries() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        // Need at least one file mapping for the walk to succeed.
        fs::write(base.join("dummy"), "").unwrap();

        let content = ContentConfig {
            defaults: ContentDefaults::default(),
            files: vec![FileMapping {
                src: format!("{}/dummy", base.display()),
                dst: "/opt/app/dummy".to_string(),
                mode: None,
                dir_mode: None,
                user: None,
                group: None,
                r#type: None,
            }],
            symlinks: vec![SymlinkMapping {
                src: "/opt/app/bin/real".to_string(),
                dst: "/usr/bin/app".to_string(),
            }],
            directories: vec![],
            alternatives: vec![],
        };

        let entries = FileTree::walk(&content).unwrap();
        let sym = entries
            .iter()
            .find(|e| matches!(e.entry_type, EntryType::Symlink { .. }))
            .unwrap();
        assert_eq!(sym.install_path, PathBuf::from("/usr/bin/app"));
        if let EntryType::Symlink { target } = &sym.entry_type {
            assert_eq!(target, &PathBuf::from("/opt/app/bin/real"));
        }
    }

    #[test]
    fn test_walk_directory_entries() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        fs::write(base.join("dummy"), "").unwrap();

        let content = ContentConfig {
            defaults: ContentDefaults::default(),
            files: vec![FileMapping {
                src: format!("{}/dummy", base.display()),
                dst: "/opt/app/dummy".to_string(),
                mode: None,
                dir_mode: None,
                user: None,
                group: None,
                r#type: None,
            }],
            symlinks: vec![],
            directories: vec![DirectoryMapping {
                path: "/var/log/app".to_string(),
                mode: Some("0750".to_string()),
                user: Some("app".to_string()),
                group: Some("app".to_string()),
            }],
            alternatives: vec![],
        };

        let entries = FileTree::walk(&content).unwrap();
        let dir = entries
            .iter()
            .find(|e| e.install_path == Path::new("/var/log/app"))
            .unwrap();
        assert!(matches!(dir.entry_type, EntryType::Directory));
        assert_eq!(dir.mode, 0o750);
        assert_eq!(dir.user, "app");
        assert_eq!(dir.group, "app");
    }

    #[test]
    fn test_walk_deterministic_ordering() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        fs::create_dir_all(base.join("c")).unwrap();
        fs::create_dir_all(base.join("a")).unwrap();
        fs::write(base.join("c/z.txt"), "z").unwrap();
        fs::write(base.join("a/m.txt"), "m").unwrap();
        fs::write(base.join("b.txt"), "b").unwrap();

        let content = test_content(vec![FileMapping {
            src: format!("{}/**", base.display()),
            dst: "/opt/pkg/".to_string(),
            mode: None,
            dir_mode: None,
            user: None,
            group: None,
            r#type: None,
        }]);

        let entries1 = FileTree::walk(&content).unwrap();
        let entries2 = FileTree::walk(&content).unwrap();

        let paths1: Vec<_> = entries1.iter().map(|e| &e.install_path).collect();
        let paths2: Vec<_> = entries2.iter().map(|e| &e.install_path).collect();
        assert_eq!(paths1, paths2);

        // Verify sorted order.
        for window in paths1.windows(2) {
            assert!(window[0] <= window[1]);
        }
    }

    #[test]
    fn test_walk_implicit_parent_directories() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        fs::write(base.join("tool"), "binary").unwrap();

        let content = test_content(vec![FileMapping {
            src: format!("{}/tool", base.display()),
            dst: "/opt/app/bin/tool".to_string(),
            mode: None,
            dir_mode: None,
            user: None,
            group: None,
            r#type: None,
        }]);

        let entries = FileTree::walk(&content).unwrap();
        let dir_paths: HashSet<PathBuf> = entries
            .iter()
            .filter(|e| matches!(e.entry_type, EntryType::Directory))
            .map(|e| e.install_path.clone())
            .collect();

        assert!(dir_paths.contains(&PathBuf::from("/opt/app/bin")));
        assert!(dir_paths.contains(&PathBuf::from("/opt/app")));
        assert!(
            !dir_paths.contains(&PathBuf::from("/opt")),
            "system dir /opt should not be owned"
        );
    }

    #[test]
    fn test_walk_system_dirs_not_owned() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        fs::write(base.join("spm"), "binary").unwrap();

        let content = test_content(vec![FileMapping {
            src: format!("{}/spm", base.display()),
            dst: "/usr/bin/spm".to_string(),
            mode: None,
            dir_mode: None,
            user: None,
            group: None,
            r#type: None,
        }]);

        let entries = FileTree::walk(&content).unwrap();
        let dir_paths: HashSet<PathBuf> = entries
            .iter()
            .filter(|e| matches!(e.entry_type, EntryType::Directory))
            .map(|e| e.install_path.clone())
            .collect();

        // /usr and /usr/bin are system directories — should not be owned.
        assert!(dir_paths.is_empty(), "no directories should be owned for /usr/bin/spm, got: {dir_paths:?}");
    }

    #[test]
    fn test_walk_user_group_override() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        fs::write(base.join("file"), "data").unwrap();

        let content = test_content(vec![FileMapping {
            src: format!("{}/file", base.display()),
            dst: "/opt/app/file".to_string(),
            mode: None,
            dir_mode: None,
            user: Some("appuser".to_string()),
            group: Some("appgroup".to_string()),
            r#type: None,
        }]);

        let entries = FileTree::walk(&content).unwrap();
        let file = entries
            .iter()
            .find(|e| matches!(e.entry_type, EntryType::RegularFile))
            .unwrap();
        assert_eq!(file.user, "appuser");
        assert_eq!(file.group, "appgroup");
    }

    #[test]
    fn test_hardlink_detection() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();

        let file_a = base.join("file_a");
        let file_b = base.join("file_b");
        fs::write(&file_a, "shared content").unwrap();
        fs::hard_link(&file_a, &file_b).unwrap();

        let content = test_content(vec![FileMapping {
            src: format!("{}/*", base.display()),
            dst: "/opt/pkg/".to_string(),
            mode: None,
            dir_mode: None,
            user: None,
            group: None,
            r#type: None,
        }]);

        let entries = FileTree::walk(&content).unwrap();
        let file_entries: Vec<_> = entries
            .iter()
            .filter(|e| {
                matches!(
                    e.entry_type,
                    EntryType::RegularFile | EntryType::Hardlink { .. }
                )
            })
            .collect();

        assert_eq!(file_entries.len(), 2);

        let regular_count = file_entries
            .iter()
            .filter(|e| matches!(e.entry_type, EntryType::RegularFile))
            .count();
        let hardlink_count = file_entries
            .iter()
            .filter(|e| matches!(e.entry_type, EntryType::Hardlink { .. }))
            .count();

        assert_eq!(regular_count, 1);
        assert_eq!(hardlink_count, 1);
    }

    #[test]
    fn test_glob_base_path() {
        assert_eq!(
            glob_base_path("/opt/staging/R2025a/**"),
            PathBuf::from("/opt/staging/R2025a")
        );
        assert_eq!(glob_base_path("src/*.rs"), PathBuf::from("src"));
        assert_eq!(glob_base_path("**/*.rs"), PathBuf::from("."));
        assert_eq!(glob_base_path("/opt/*/foo/**"), PathBuf::from("/opt"));
    }

    #[test]
    fn test_parse_mode() {
        assert_eq!(parse_mode("0755").unwrap(), 0o755);
        assert_eq!(parse_mode("0644").unwrap(), 0o644);
        assert_eq!(parse_mode("755").unwrap(), 0o755);
        assert_eq!(parse_mode("0750").unwrap(), 0o750);
        assert_eq!(parse_mode("0000").unwrap(), 0);
        assert_eq!(parse_mode("0").unwrap(), 0);
        assert_eq!(parse_mode("00644").unwrap(), 0o644);
    }

    #[test]
    fn test_ownership_global_defaults() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        fs::create_dir_all(base.join("bin")).unwrap();
        fs::write(base.join("bin/tool"), "binary").unwrap();

        let defaults = ContentDefaults {
            user: "root".to_string(),
            group: "appgroup".to_string(),
            file_mode: None,
            dir_mode: None,
        };
        let content = test_content_with_defaults(
            vec![FileMapping {
                src: format!("{}/**", base.display()),
                dst: "/opt/app/".to_string(),
                mode: None,
                dir_mode: None,
                user: None,
                group: None,
                r#type: None,
            }],
            defaults,
        );

        let entries = FileTree::walk(&content).unwrap();

        // All entries should have group="appgroup" from global defaults.
        for entry in &entries {
            assert_eq!(entry.user, "root", "entry {:?} user", entry.install_path);
            assert_eq!(
                entry.group, "appgroup",
                "entry {:?} group",
                entry.install_path
            );
        }
    }

    #[test]
    fn test_ownership_per_mapping_override() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        fs::write(base.join("file_a"), "data").unwrap();
        fs::write(base.join("file_b"), "data").unwrap();

        let defaults = ContentDefaults {
            user: "root".to_string(),
            group: "root".to_string(),
            file_mode: None,
            dir_mode: None,
        };
        let content = test_content_with_defaults(
            vec![
                FileMapping {
                    src: format!("{}/file_a", base.display()),
                    dst: "/opt/app/file_a".to_string(),
                    mode: None,
                    dir_mode: None,
                    user: Some("nobody".to_string()),
                    group: None,
                    r#type: None,
                },
                FileMapping {
                    src: format!("{}/file_b", base.display()),
                    dst: "/opt/app/file_b".to_string(),
                    mode: None,
                    dir_mode: None,
                    user: None,
                    group: None,
                    r#type: None,
                },
            ],
            defaults,
        );

        let entries = FileTree::walk(&content).unwrap();

        let file_a = entries
            .iter()
            .find(|e| e.install_path == Path::new("/opt/app/file_a"))
            .unwrap();
        assert_eq!(file_a.user, "nobody");

        let file_b = entries
            .iter()
            .find(|e| e.install_path == Path::new("/opt/app/file_b"))
            .unwrap();
        assert_eq!(file_b.user, "root");
    }

    #[test]
    fn test_ownership_dir_mode_vs_file_mode() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        fs::create_dir_all(base.join("subdir")).unwrap();
        fs::write(base.join("subdir/file.txt"), "data").unwrap();

        let defaults = ContentDefaults {
            user: "root".to_string(),
            group: "root".to_string(),
            file_mode: Some("0644".to_string()),
            dir_mode: Some("0755".to_string()),
        };
        let content = test_content_with_defaults(
            vec![FileMapping {
                src: format!("{}/**", base.display()),
                dst: "/opt/app/".to_string(),
                mode: None,
                dir_mode: None,
                user: None,
                group: None,
                r#type: None,
            }],
            defaults,
        );

        let entries = FileTree::walk(&content).unwrap();

        // Regular files should get file_mode 0o644.
        let files: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e.entry_type, EntryType::RegularFile))
            .collect();
        for f in &files {
            assert_eq!(f.mode, 0o644, "file {:?} mode", f.install_path);
        }

        // Directories should get dir_mode 0o755.
        let dirs: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e.entry_type, EntryType::Directory))
            .collect();
        for d in &dirs {
            assert_eq!(d.mode, 0o755, "dir {:?} mode", d.install_path);
        }
    }

    #[test]
    fn test_src_dst_path_stripping() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        fs::create_dir_all(base.join("bin")).unwrap();
        fs::write(base.join("bin/tool"), "binary").unwrap();

        let content = test_content(vec![FileMapping {
            src: format!("{}/**", base.display()),
            dst: "/opt/app/".to_string(),
            mode: None,
            dir_mode: None,
            user: None,
            group: None,
            r#type: None,
        }]);

        let entries = FileTree::walk(&content).unwrap();

        // The src prefix before the glob should be stripped and replaced with dst.
        let tool = entries
            .iter()
            .find(|e| matches!(e.entry_type, EntryType::RegularFile))
            .unwrap();
        assert_eq!(tool.install_path, PathBuf::from("/opt/app/bin/tool"));
    }

    #[test]
    fn test_src_dst_literal_file() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        fs::write(base.join("license.txt"), "MIT License").unwrap();

        let content = test_content(vec![FileMapping {
            src: format!("{}/license.txt", base.display()),
            dst: "/opt/app/LICENSE".to_string(),
            mode: None,
            dir_mode: None,
            user: None,
            group: None,
            r#type: None,
        }]);

        let entries = FileTree::walk(&content).unwrap();

        // Direct 1:1 mapping — no stripping.
        let license = entries
            .iter()
            .find(|e| matches!(e.entry_type, EntryType::RegularFile))
            .unwrap();
        assert_eq!(license.install_path, PathBuf::from("/opt/app/LICENSE"));
    }
}
