/// YAML config deserialization and validation.
///
/// These structs map 1:1 to the YAML schema defined in spec.md Section 3.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::ConfigError;

/// Top-level spm configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Package metadata (name, version, arch, etc.).
    pub package: PackageConfig,
    /// Content mappings (files, symlinks, directories, alternatives).
    pub content: ContentConfig,
    /// Optional install/remove scripts.
    #[serde(default)]
    pub scripts: ScriptsConfig,
    /// Compression settings.
    #[serde(default)]
    pub compression: CompressionConfig,
    /// Auto-splitting settings.
    #[serde(default)]
    pub splitting: SplittingConfig,
    /// PGP signing configuration.
    #[serde(default)]
    pub signing: Option<SigningConfig>,
    /// RPM-specific overrides.
    #[serde(default)]
    pub rpm: Option<RpmOverrides>,
    /// DEB-specific overrides.
    #[serde(default)]
    pub deb: Option<DebOverrides>,
    /// Build reproducibility settings.
    #[serde(default)]
    pub build: Option<BuildConfig>,
}

/// Package identity and metadata.
#[derive(Debug, Clone, Deserialize)]
pub struct PackageConfig {
    /// Package name (e.g. "matlab").
    pub name: String,
    /// Package version (e.g. "2025a").
    pub version: String,
    /// Release number (default "1").
    #[serde(default = "default_release")]
    pub release: String,
    /// Target architecture (e.g. "x86_64", "aarch64", "noarch").
    pub arch: String,
    /// License identifier.
    pub license: String,
    /// Package maintainer (name and email).
    pub maintainer: String,
    /// Short package description.
    pub description: String,
    /// Project URL.
    #[serde(default)]
    pub url: Option<String>,
    /// Vendor name.
    #[serde(default)]
    pub vendor: Option<String>,
    /// Package dependency declarations.
    #[serde(default)]
    pub dependencies: DependencyConfig,
}

fn default_release() -> String {
    "1".to_string()
}

/// Package dependency declarations (common + format-specific).
#[derive(Debug, Default, Clone, Deserialize)]
pub struct DependencyConfig {
    /// Common requires (translated per format).
    #[serde(default)]
    pub requires: Vec<String>,
    /// RPM-specific requires.
    #[serde(default)]
    pub requires_rpm: Vec<String>,
    /// DEB-specific requires.
    #[serde(default)]
    pub requires_deb: Vec<String>,
    /// Packages this conflicts with.
    #[serde(default)]
    pub conflicts: Vec<String>,
    /// Virtual packages this provides.
    #[serde(default)]
    pub provides: Vec<String>,
    /// Packages this replaces.
    #[serde(default)]
    pub replaces: Vec<String>,
}

/// Content mapping: file rules, symlinks, directories, alternatives.
#[derive(Debug, Clone, Deserialize)]
pub struct ContentConfig {
    /// Global defaults applied to all files unless overridden per-mapping.
    #[serde(default)]
    pub defaults: ContentDefaults,
    /// File mapping rules (glob patterns, destinations, overrides).
    #[serde(default)]
    pub files: Vec<FileMapping>,
    /// Static symlinks to create in the package.
    #[serde(default)]
    pub symlinks: Vec<SymlinkMapping>,
    /// Directories to create with specific ownership/mode.
    #[serde(default)]
    pub directories: Vec<DirectoryMapping>,
    /// update-alternatives integration entries.
    #[serde(default)]
    pub alternatives: Vec<AlternativeConfig>,
}

/// Global defaults applied to all files unless overridden per-mapping.
///
/// Resolution order (first wins):
/// 1. Per-mapping override (content.files[].user/group/mode/dir_mode)
/// 2. Global defaults (content.defaults.user/group/file_mode/dir_mode)
/// 3. Source file metadata on disk
#[derive(Debug, Clone, Deserialize)]
pub struct ContentDefaults {
    /// Default owner for all entries.
    #[serde(default = "default_root")]
    pub user: String,
    /// Default group for all entries.
    #[serde(default = "default_root")]
    pub group: String,
    /// Default mode for regular files (e.g. "0644"). If None, preserve from source.
    #[serde(default)]
    pub file_mode: Option<String>,
    /// Default mode for directories (e.g. "0755"). If None, preserve from source.
    #[serde(default)]
    pub dir_mode: Option<String>,
}

fn default_root() -> String {
    "root".to_string()
}

impl Default for ContentDefaults {
    fn default() -> Self {
        Self {
            user: default_root(),
            group: default_root(),
            file_mode: None,
            dir_mode: None,
        }
    }
}

/// A single file mapping rule (source glob/path to destination).
#[derive(Debug, Clone, Deserialize)]
pub struct FileMapping {
    /// Source path or glob pattern.
    pub src: String,
    /// Destination path inside the package.
    pub dst: String,
    /// Optional file mode override (applies to regular files matched).
    #[serde(default)]
    pub mode: Option<String>,
    /// Optional directory mode override (applies to directories matched).
    #[serde(default)]
    pub dir_mode: Option<String>,
    /// Optional owner override.
    #[serde(default)]
    pub user: Option<String>,
    /// Optional group override.
    #[serde(default)]
    pub group: Option<String>,
    /// File type marker (e.g. "config" for noreplace/conffile).
    #[serde(default)]
    pub r#type: Option<String>,
}

/// A static symlink to include in the package.
#[derive(Debug, Clone, Deserialize)]
pub struct SymlinkMapping {
    /// Symlink target (what the symlink points to).
    pub src: String,
    /// Symlink path (where it is created).
    pub dst: String,
}

/// A directory to create with specific ownership/permissions.
#[derive(Debug, Clone, Deserialize)]
pub struct DirectoryMapping {
    /// Directory path.
    pub path: String,
    /// Optional mode override.
    #[serde(default)]
    pub mode: Option<String>,
    /// Optional owner override.
    #[serde(default)]
    pub user: Option<String>,
    /// Optional group override.
    #[serde(default)]
    pub group: Option<String>,
}

/// An update-alternatives entry for declarative alternatives management.
#[derive(Debug, Clone, Deserialize)]
pub struct AlternativeConfig {
    /// Alternatives group name (e.g. "matlab").
    pub name: String,
    /// Generic symlink path managed by alternatives (e.g. /usr/bin/matlab).
    pub link: String,
    /// This package's real binary path.
    pub path: String,
    /// Priority (higher = preferred).
    pub priority: i32,
    /// Secondary links that switch together with the primary.
    #[serde(default)]
    pub followers: Vec<AlternativeFollower>,
}

/// A follower (secondary) link in an alternatives group.
#[derive(Debug, Clone, Deserialize)]
pub struct AlternativeFollower {
    /// Follower alternative name.
    pub name: String,
    /// Follower symlink path.
    pub link: String,
    /// Follower real path.
    pub path: String,
}

/// Install/remove script paths.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct ScriptsConfig {
    /// Pre-install script.
    pub pre_install: Option<PathBuf>,
    /// Post-install script.
    pub post_install: Option<PathBuf>,
    /// Pre-remove script.
    pub pre_remove: Option<PathBuf>,
    /// Post-remove script.
    pub post_remove: Option<PathBuf>,
    /// Pre-transaction script (RPM only).
    pub pre_trans: Option<PathBuf>,
    /// Post-transaction script (RPM only).
    pub post_trans: Option<PathBuf>,
}

/// Compression configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct CompressionConfig {
    /// Compression algorithm: "zstd", "xz", "gzip", "none".
    #[serde(default = "default_algorithm")]
    pub algorithm: String,
    /// Algorithm-specific compression level.
    #[serde(default)]
    pub level: Option<i32>,
    /// Thread count (0 = auto-detect).
    #[serde(default)]
    pub threads: Option<usize>,
}

fn default_algorithm() -> String {
    "zstd".to_string()
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            algorithm: default_algorithm(),
            level: None,
            threads: None,
        }
    }
}

/// Auto-splitting configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct SplittingConfig {
    /// Whether auto-splitting is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Split strategy: "auto", "size", "directory".
    #[serde(default = "default_strategy")]
    pub strategy: String,
    /// Maximum size per sub-package (for strategy "size"), e.g. "8GiB".
    pub max_size: Option<String>,
    /// Explicit split parts (for strategy "directory").
    #[serde(default)]
    pub parts: Vec<SplitPart>,
}

fn default_true() -> bool {
    true
}

fn default_strategy() -> String {
    "auto".to_string()
}

impl Default for SplittingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            strategy: default_strategy(),
            max_size: None,
            parts: Vec::new(),
        }
    }
}

/// A named split part for directory-based splitting.
#[derive(Debug, Clone, Deserialize)]
pub struct SplitPart {
    /// Sub-package name suffix (e.g. "core", "toolboxes").
    pub name: String,
    /// Directory paths assigned to this part.
    pub paths: Vec<String>,
}

/// PGP signing configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct SigningConfig {
    /// Path to the PGP key file (supports `${VAR}` expansion).
    pub key_file: String,
    /// Optional specific subkey ID.
    pub key_id: Option<String>,
}

/// RPM-specific overrides.
#[derive(Debug, Clone, Deserialize)]
pub struct RpmOverrides {
    /// RPM group tag.
    pub group: Option<String>,
    /// Payload format: "cpio" or "cpio-extended".
    pub payload_format: Option<String>,
    /// Override compression for RPM specifically.
    pub compression: Option<String>,
}

/// DEB-specific overrides.
#[derive(Debug, Clone, Deserialize)]
pub struct DebOverrides {
    /// Debian section (e.g. "science").
    pub section: Option<String>,
    /// Debian priority (e.g. "optional").
    pub priority: Option<String>,
    /// Additional control file fields.
    #[serde(default)]
    pub fields: HashMap<String, String>,
    /// Override compression for DEB specifically.
    pub compression: Option<String>,
}

/// Build reproducibility settings.
#[derive(Debug, Clone, Deserialize)]
pub struct BuildConfig {
    /// Fixed timestamp for reproducible builds.
    pub source_date_epoch: Option<String>,
}

impl Config {
    /// Load config from a YAML file, expanding environment variables.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ConfigError::NotFound(path.to_owned())
            } else {
                ConfigError::Io {
                    path: path.to_owned(),
                    source: e,
                }
            }
        })?;
        // Expand ${VAR} references before parsing; unknown vars pass through
        // so that inline shell scripts with $VAR references are preserved.
        let expanded =
            shellexpand::env_with_context_no_errors(&raw, |var_name| std::env::var(var_name).ok());
        let config: Config = serde_yaml::from_str(&expanded)?;
        config.validate()?;
        let config_dir = path.parent().unwrap_or(Path::new("."));
        config.validate_with_dir(config_dir)?;
        Ok(config)
    }

    /// Validate config after parsing.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.package.name.is_empty() {
            return Err(ConfigError::Validation(
                "package.name is required".to_string(),
            ));
        }
        // Package name: alphanumeric, hyphens, dots, underscores, plus signs.
        if !self
            .package
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_' || c == '+')
        {
            return Err(ConfigError::Validation(format!(
                "package.name '{}' contains invalid characters; \
                 only alphanumeric, hyphens, dots, underscores, and plus signs are allowed",
                self.package.name
            )));
        }
        if self.package.version.is_empty() {
            return Err(ConfigError::Validation(
                "package.version is required".to_string(),
            ));
        }
        // Version: must start with a digit, no spaces or colons.
        if !self
            .package
            .version
            .starts_with(|c: char| c.is_ascii_digit())
        {
            return Err(ConfigError::Validation(format!(
                "package.version '{}' must start with a digit",
                self.package.version
            )));
        }
        if self
            .package
            .version
            .contains(|c: char| c.is_whitespace() || c == ':')
        {
            return Err(ConfigError::Validation(format!(
                "package.version '{}' contains invalid characters (no spaces or colons allowed)",
                self.package.version
            )));
        }
        let valid_arches = ["x86_64", "aarch64", "i686", "armv7hl", "noarch", "all"];
        if !valid_arches.contains(&self.package.arch.as_str()) {
            return Err(ConfigError::Validation(format!(
                "unsupported arch '{}', expected one of: {}",
                self.package.arch,
                valid_arches.join(", ")
            )));
        }
        let valid_algos = ["zstd", "xz", "gzip", "none"];
        if !valid_algos.contains(&self.compression.algorithm.as_str()) {
            return Err(ConfigError::Validation(format!(
                "unsupported compression '{}', expected one of: {}",
                self.compression.algorithm,
                valid_algos.join(", ")
            )));
        }
        let valid_strategies = ["auto", "size", "directory"];
        if !valid_strategies.contains(&self.splitting.strategy.as_str()) {
            return Err(ConfigError::Validation(format!(
                "unsupported splitting strategy '{}'",
                self.splitting.strategy
            )));
        }
        if self.splitting.strategy == "size" && self.splitting.max_size.is_none() {
            return Err(ConfigError::Validation(
                "splitting.max_size is required when strategy is 'size'".to_string(),
            ));
        }
        if self.splitting.strategy == "directory" && self.splitting.parts.is_empty() {
            return Err(ConfigError::Validation(
                "splitting.parts must be non-empty when strategy is 'directory'".to_string(),
            ));
        }
        if let Some(ref max_size) = self.splitting.max_size {
            let parsed = crate::types::parse_size(max_size).map_err(|reason| {
                ConfigError::Validation(format!(
                    "splitting.max_size '{}' is invalid: {}",
                    max_size, reason
                ))
            })?;
            if parsed == 0 {
                return Err(ConfigError::Validation(
                    "splitting.max_size must be greater than 0".to_string(),
                ));
            }
        }
        Ok(())
    }

    /// Validate filesystem-dependent checks (script files).
    ///
    /// Called after structural validation with the config file's parent directory
    /// so relative script paths can be resolved correctly.
    pub fn validate_with_dir(&self, config_dir: &Path) -> Result<(), ConfigError> {
        // Check script files exist.
        let script_fields = [
            ("scripts.pre_install", &self.scripts.pre_install),
            ("scripts.post_install", &self.scripts.post_install),
            ("scripts.pre_remove", &self.scripts.pre_remove),
            ("scripts.post_remove", &self.scripts.post_remove),
            ("scripts.pre_trans", &self.scripts.pre_trans),
            ("scripts.post_trans", &self.scripts.post_trans),
        ];
        for (field_name, script_path) in &script_fields {
            if let Some(p) = script_path {
                let resolved = if p.is_absolute() {
                    p.clone()
                } else {
                    config_dir.join(p)
                };
                if !resolved.exists() {
                    return Err(ConfigError::Validation(format!(
                        "{field_name}: script file '{}' does not exist",
                        resolved.display()
                    )));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("fixtures")
    }

    #[test]
    fn test_parse_minimal_config() {
        let path = fixtures_dir().join("minimal.yaml");
        let config = Config::load(&path).expect("should parse minimal config");
        assert_eq!(config.package.name, "testpkg");
        assert_eq!(config.package.version, "1.0.0");
        assert_eq!(config.package.release, "1");
        assert_eq!(config.package.arch, "x86_64");
        assert_eq!(config.compression.algorithm, "zstd");
        assert!(config.splitting.enabled);
    }

    #[test]
    fn test_parse_full_config() {
        // The full.yaml references ${SPM_SIGNING_KEY}, so we set it.
        std::env::set_var("SPM_SIGNING_KEY", "/tmp/test-key.gpg");
        // Read and parse the YAML directly to test parsing without filesystem
        // validation (/opt/matlab-staging/R2025a may not exist in CI).
        let path = fixtures_dir().join("full.yaml");
        let raw = std::fs::read_to_string(&path).expect("should read full.yaml");
        let expanded =
            shellexpand::env_with_context_no_errors(&raw, |var_name| std::env::var(var_name).ok());
        let config: Config = serde_yaml::from_str(&expanded).expect("should parse full config");
        config
            .validate()
            .expect("structural validation should pass");
        assert_eq!(config.package.name, "matlab");
        assert_eq!(config.package.version, "2025a");
        assert_eq!(config.package.arch, "x86_64");
        assert_eq!(config.package.maintainer, "HPC Team <hpc-help@tamu.edu>");
        assert!(config.signing.is_some());
        assert_eq!(
            config.signing.as_ref().unwrap().key_file,
            "/tmp/test-key.gpg"
        );
        assert!(!config.content.alternatives.is_empty());
        assert_eq!(config.content.alternatives[0].name, "matlab");
        assert_eq!(config.content.alternatives[0].priority, 2025);
        assert_eq!(config.content.alternatives[0].followers.len(), 2);
    }

    #[test]
    fn test_reject_missing_name() {
        let path = fixtures_dir().join("invalid").join("missing_name.yaml");
        let err = Config::load(&path).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("package.name is required"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn test_reject_bad_arch() {
        let path = fixtures_dir().join("invalid").join("bad_arch.yaml");
        let err = Config::load(&path).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unsupported arch"), "unexpected error: {msg}");
    }

    #[test]
    fn test_reject_empty_file() {
        let path = fixtures_dir().join("invalid").join("empty.yaml");
        let err = Config::load(&path).unwrap_err();
        // An empty file will fail YAML deserialization
        assert!(matches!(err, ConfigError::ParseError(_)));
    }

    #[test]
    fn test_env_var_expansion() {
        std::env::set_var("SPM_TEST_VERSION", "42.0");
        let yaml = r#"
package:
  name: envtest
  version: "${SPM_TEST_VERSION}"
  arch: x86_64
  license: MIT
  maintainer: test
  description: test env expansion
content: {}
"#;
        let expanded =
            shellexpand::env_with_context_no_errors(yaml, |var_name| std::env::var(var_name).ok());
        let config: Config = serde_yaml::from_str(&expanded).expect("should parse");
        config.validate().expect("should validate");
        assert_eq!(config.package.version, "42.0");
    }

    #[test]
    fn test_env_var_missing_passes_through() {
        let yaml = r#"
package:
  name: envtest
  version: "${SPM_NONEXISTENT_VAR_12345}"
  arch: x86_64
  license: MIT
  maintainer: test
  description: test
content: {}
"#;
        let expanded =
            shellexpand::env_with_context_no_errors(yaml, |var_name| std::env::var(var_name).ok());
        let config: Config = serde_yaml::from_str(&expanded).expect("should parse");
        assert_eq!(config.package.version, "${SPM_NONEXISTENT_VAR_12345}");
    }

    #[test]
    fn test_inline_script_shell_vars_not_expanded() {
        let yaml = r#"
package:
  name: scripttest
  version: "1.0"
  arch: x86_64
  license: MIT
  maintainer: test
  description: test

content:
  files:
    - src: /opt/app
      dst: /opt/app

scripts:
  post_install: |
    #!/bin/bash
    LINK_DIR="/usr/local/bin"
    ln -sf "${MATLAB_ROOT}/bin/matlab" "${LINK_DIR}/matlab"
"#;
        let expanded =
            shellexpand::env_with_context_no_errors(yaml, |var_name| std::env::var(var_name).ok());
        let config: Config = serde_yaml::from_str(&expanded).expect("should parse");
        assert_eq!(config.package.name, "scripttest");
        let script = config.scripts.post_install.as_ref().unwrap();
        let script_str = script.to_str().unwrap();
        assert!(script_str.contains("${MATLAB_ROOT}"));
        assert!(script_str.contains("${LINK_DIR}"));
    }

    #[test]
    fn test_default_values() {
        let yaml = r#"
package:
  name: defaults
  version: "1.0"
  arch: noarch
  license: MIT
  maintainer: test
  description: test defaults
content: {}
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("should parse");
        config.validate().expect("should validate");
        assert_eq!(config.package.release, "1");
        assert_eq!(config.compression.algorithm, "zstd");
        assert!(config.splitting.enabled);
        assert_eq!(config.splitting.strategy, "auto");
        assert!(config.signing.is_none());
        assert!(config.rpm.is_none());
        assert!(config.deb.is_none());
    }

    #[test]
    fn test_reject_bad_compression() {
        let yaml = r#"
package:
  name: badcompress
  version: "1.0"
  arch: x86_64
  license: MIT
  maintainer: test
  description: test
content: {}
compression:
  algorithm: brotli
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("should parse");
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("unsupported compression"));
    }

    #[test]
    fn test_reject_bad_strategy() {
        let yaml = r#"
package:
  name: badstrat
  version: "1.0"
  arch: x86_64
  license: MIT
  maintainer: test
  description: test
content: {}
splitting:
  strategy: random
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("should parse");
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("unsupported splitting strategy"));
    }

    #[test]
    fn test_validate_missing_script_file() {
        let yaml = r#"
package:
  name: test
  version: "1.0"
  arch: x86_64
  license: MIT
  maintainer: test
  description: test
content: {}
scripts:
  post_install: nonexistent-script.sh
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("should parse");
        config.validate().expect("structural should pass");
        let err = config.validate_with_dir(Path::new("/tmp")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("scripts.post_install"), "got: {msg}");
        assert!(msg.contains("does not exist"), "got: {msg}");
    }

    #[test]
    fn test_config_clone() {
        let yaml = r#"
package:
  name: clonetest
  version: "1.0"
  arch: x86_64
  license: MIT
  maintainer: test
  description: test
content: {}
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("should parse");
        let cloned = config.clone();
        assert_eq!(cloned.package.name, "clonetest");
    }
}
