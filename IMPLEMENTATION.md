# spm — Implementation Guide

**Companion to:** `spec.md`
**Language:** Rust (2021 edition, workspace)
**Target:** Implemented phase-by-phase, each phase produces working, tested code.

---

## How to Use This Document

Each phase has:
- **Goal** — what capability you have at the end
- **Crates touched** — which workspace members are involved
- **Key types** — Rust type definitions to implement
- **Steps** — ordered implementation tasks
- **Acceptance criteria** — specific tests that must pass before moving on

Implement phases in order. Do not skip ahead. Each phase builds on the previous.

---

## Phase 0: Workspace Scaffolding & Config Parsing

**Goal:** A `spm` binary that reads a `spm.yaml` file, validates it, and prints a summary. No package building yet.

**Crates touched:** `spm-cli`, `spm-core`

### Workspace Layout

```
spm/
├── Cargo.toml                      # workspace definition
├── crates/
│   ├── spm-cli/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── main.rs
│   └── spm-core/
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── config.rs           # YAML deserialization
│           ├── error.rs            # Error types
│           └── types.rs            # Shared types
└── tests/
    └── fixtures/
        ├── minimal.yaml            # minimal valid config
        ├── full.yaml               # all fields populated (use MATLAB example from spec)
        └── invalid/
            ├── missing_name.yaml
            ├── bad_arch.yaml
            └── empty.yaml
```

### Dependencies (Phase 0)

```toml
# spm-core/Cargo.toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_yaml = "0.9"
thiserror = "2"
shellexpand = "3"           # for ${VAR} expansion

# spm-cli/Cargo.toml
[dependencies]
spm-core = { path = "../spm-core" }
clap = { version = "4", features = ["derive"] }
anyhow = "1"
```

### Error Strategy

Use `thiserror` in library crates for typed errors. Use `anyhow` in the CLI binary only.

```rust
// spm-core/src/error.rs

use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config file not found: {0}")]
    NotFound(PathBuf),

    #[error("failed to parse YAML: {0}")]
    ParseError(#[from] serde_yaml::Error),

    #[error("validation error: {0}")]
    Validation(String),

    #[error("environment variable not set: {0}")]
    EnvVar(String),

    #[error("I/O error reading {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}
```

### Key Types

```rust
// spm-core/src/config.rs
// These structs map 1:1 to the YAML schema in spec.md Section 3.

use serde::Deserialize;
use std::path::PathBuf;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub package: PackageConfig,
    pub content: ContentConfig,
    #[serde(default)]
    pub scripts: ScriptsConfig,
    #[serde(default)]
    pub compression: CompressionConfig,
    #[serde(default)]
    pub splitting: SplittingConfig,
    #[serde(default)]
    pub signing: Option<SigningConfig>,
    #[serde(default)]
    pub rpm: Option<RpmOverrides>,
    #[serde(default)]
    pub deb: Option<DebOverrides>,
    #[serde(default)]
    pub build: Option<BuildConfig>,
}

#[derive(Debug, Deserialize)]
pub struct PackageConfig {
    pub name: String,
    pub version: String,
    #[serde(default = "default_release")]
    pub release: String,
    pub arch: String,
    pub license: String,
    pub maintainer: String,
    pub description: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub dependencies: DependencyConfig,
}

fn default_release() -> String { "1".to_string() }

#[derive(Debug, Default, Deserialize)]
pub struct DependencyConfig {
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub requires_rpm: Vec<String>,
    #[serde(default)]
    pub requires_deb: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(default)]
    pub replaces: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ContentConfig {
    pub source_dir: PathBuf,
    #[serde(default)]
    pub defaults: ContentDefaults,
    #[serde(default)]
    pub files: Vec<FileMapping>,
    #[serde(default)]
    pub symlinks: Vec<SymlinkMapping>,
    #[serde(default)]
    pub directories: Vec<DirectoryMapping>,
    #[serde(default)]
    pub alternatives: Vec<AlternativeConfig>,
}

/// Global defaults applied to all files unless overridden per-mapping
#[derive(Debug, Deserialize)]
pub struct ContentDefaults {
    #[serde(default = "default_root")]
    pub user: String,
    #[serde(default = "default_root")]
    pub group: String,
    #[serde(default)]
    pub file_mode: Option<String>,    // e.g. "0644" — if None, preserve from source
    #[serde(default)]
    pub dir_mode: Option<String>,     // e.g. "0755" — if None, preserve from source
}

fn default_root() -> String { "root".to_string() }

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

#[derive(Debug, Deserialize)]
pub struct FileMapping {
    pub src: String,        // glob pattern or path
    pub dst: String,        // destination path
    #[serde(default)]
    pub mode: Option<String>,       // override file mode (applies to files matched)
    #[serde(default)]
    pub dir_mode: Option<String>,   // override dir mode (applies to dirs matched)
    #[serde(default)]
    pub user: Option<String>,       // override owner
    #[serde(default)]
    pub group: Option<String>,      // override group
    #[serde(default)]
    pub r#type: Option<String>,     // "config", etc.
}

#[derive(Debug, Deserialize)]
pub struct SymlinkMapping {
    pub src: String,        // symlink target
    pub dst: String,        // symlink path
}

#[derive(Debug, Deserialize)]
pub struct DirectoryMapping {
    pub path: String,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AlternativeConfig {
    pub name: String,           // alternatives group name
    pub link: String,           // generic symlink path
    pub path: String,           // this version's real binary
    pub priority: i32,          // higher = preferred
    #[serde(default)]
    pub followers: Vec<AlternativeFollower>,
}

#[derive(Debug, Deserialize)]
pub struct AlternativeFollower {
    pub name: String,
    pub link: String,
    pub path: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct ScriptsConfig {
    pub pre_install: Option<PathBuf>,
    pub post_install: Option<PathBuf>,
    pub pre_remove: Option<PathBuf>,
    pub post_remove: Option<PathBuf>,
    pub pre_trans: Option<PathBuf>,     // RPM only
    pub post_trans: Option<PathBuf>,    // RPM only
}

#[derive(Debug, Deserialize)]
pub struct CompressionConfig {
    #[serde(default = "default_algorithm")]
    pub algorithm: String,
    #[serde(default)]
    pub level: Option<i32>,
    #[serde(default)]
    pub threads: Option<usize>,     // 0 = auto
}

fn default_algorithm() -> String { "zstd".to_string() }

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            algorithm: default_algorithm(),
            level: None,
            threads: None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SplittingConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_strategy")]
    pub strategy: String,
    pub max_size: Option<String>,       // e.g. "8GiB"
    #[serde(default)]
    pub parts: Vec<SplitPart>,
}

fn default_true() -> bool { true }
fn default_strategy() -> String { "auto".to_string() }

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

#[derive(Debug, Deserialize)]
pub struct SplitPart {
    pub name: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct SigningConfig {
    pub key_file: String,
    pub key_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RpmOverrides {
    pub group: Option<String>,
    pub payload_format: Option<String>,
    pub compression: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DebOverrides {
    pub section: Option<String>,
    pub priority: Option<String>,
    #[serde(default)]
    pub fields: HashMap<String, String>,
    pub compression: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BuildConfig {
    pub source_date_epoch: Option<String>,
}
```

```rust
// spm-core/src/config.rs (continued)
// Config loading and validation

impl Config {
    /// Load config from a YAML file, expanding environment variables
    pub fn load(path: &std::path::Path) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
            path: path.to_owned(),
            source: e,
        })?;
        // Expand ${VAR} references before parsing
        let expanded = shellexpand::env(&raw)
            .map_err(|e| ConfigError::EnvVar(e.var_name))?;
        let config: Config = serde_yaml::from_str(&expanded)?;
        config.validate()?;
        Ok(config)
    }

    /// Validate config after parsing
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.package.name.is_empty() {
            return Err(ConfigError::Validation("package.name is required".into()));
        }
        if self.package.version.is_empty() {
            return Err(ConfigError::Validation("package.version is required".into()));
        }
        let valid_arches = ["x86_64", "aarch64", "i686", "armv7hl", "noarch", "all"];
        if !valid_arches.contains(&self.package.arch.as_str()) {
            return Err(ConfigError::Validation(
                format!("unsupported arch '{}', expected one of: {}", 
                    self.package.arch, valid_arches.join(", "))
            ));
        }
        let valid_algos = ["zstd", "xz", "gzip", "none"];
        if !valid_algos.contains(&self.compression.algorithm.as_str()) {
            return Err(ConfigError::Validation(
                format!("unsupported compression '{}', expected one of: {}",
                    self.compression.algorithm, valid_algos.join(", "))
            ));
        }
        let valid_strategies = ["auto", "size", "directory"];
        if !valid_strategies.contains(&self.splitting.strategy.as_str()) {
            return Err(ConfigError::Validation(
                format!("unsupported splitting strategy '{}'", self.splitting.strategy)
            ));
        }
        Ok(())
    }
}
```

### Steps

1. Create workspace `Cargo.toml` with members
2. Implement `spm-core` with config types, deserialization, validation
3. Implement environment variable expansion (`${VAR}` syntax)
4. Create test fixture YAML files (minimal, full, invalid variants)
5. Implement `spm-cli` with `clap` — subcommands: `validate`, `init`
6. `spm validate` — loads config, prints validation result
7. `spm init` — writes a template `spm.yaml` to current directory

### Acceptance Criteria

```bash
# Parses the full MATLAB example config without error
spm validate --config tests/fixtures/full.yaml
# Exit 0, prints "Config valid: matlab 2025a-1 (x86_64)"

# Rejects missing required fields
spm validate --config tests/fixtures/invalid/missing_name.yaml
# Exit 1, prints "validation error: package.name is required"

# Environment variable expansion works
SPM_SIGNING_KEY=/tmp/key.gpg spm validate --config tests/fixtures/full.yaml
# Expands ${SPM_SIGNING_KEY} in signing.key_file

# Init creates a template
spm init --name myapp --version 1.0.0
# Creates spm.yaml with sensible defaults

# Unit tests pass
cargo test -p spm-core
```

---

## Phase 1: File Tree Walking & Package Planning

**Goal:** `spm plan` walks a source directory, calculates sizes, applies file mappings, and reports whether splitting is needed. Still no package building.

**Crates touched:** `spm-core` (new module: `planner.rs`, `filetree.rs`)

### Key Types

```rust
// spm-core/src/filetree.rs

use std::path::PathBuf;

/// Represents a single file/dir/symlink to include in the package.
///
/// Ownership and permissions are resolved in this order (first wins):
///   1. Per-mapping override (content.files[].user/group/mode/dir_mode)
///   2. Global defaults (content.defaults.user/group/file_mode/dir_mode)
///   3. Source file metadata on disk (only if no defaults set and no override)
///
/// This means: if you build as "builduser" but set content.defaults.user = "root",
/// all files in the package will be owned by root regardless of who owns them on disk.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Path as it will appear inside the package (absolute, e.g. /opt/matlab/bin/matlab)
    pub install_path: PathBuf,
    /// Path to the source file on disk (for reading contents)
    pub source_path: PathBuf,
    /// File type
    pub entry_type: EntryType,
    /// File size in bytes (0 for dirs/symlinks)
    pub size: u64,
    /// Unix mode (e.g. 0o755) — resolved from override > defaults > source
    pub mode: u32,
    /// Owner — resolved from override > defaults > source
    pub user: String,
    /// Group — resolved from override > defaults > source
    pub group: String,
    /// Whether this is a config file (noreplace/conffile)
    pub is_config: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EntryType {
    RegularFile,
    Directory,
    Symlink { target: PathBuf },
    Hardlink { target: PathBuf },
}
```

```rust
// spm-core/src/planner.rs

use crate::filetree::FileEntry;

/// The output of the planning phase — everything needed to build package(s)
#[derive(Debug)]
pub struct PackagePlan {
    /// The primary package name and metadata
    pub name: String,
    pub version: String,
    pub release: String,
    pub arch: String,

    /// If no splitting needed: one SubPackage with all files.
    /// If splitting: a meta-package + N part packages.
    pub sub_packages: Vec<SubPackage>,

    /// True if the meta-package pattern is being used
    pub is_split: bool,

    /// RPM-specific: whether any file exceeds 4 GiB (triggers 07070X cpio)
    pub needs_extended_cpio: bool,

    /// Total uncompressed size across all sub-packages
    pub total_size: u64,
}

/// A single buildable package (either the whole thing, or one part of a split)
#[derive(Debug)]
pub struct SubPackage {
    /// Package name (e.g. "matlab-2025a" or "matlab-2025a-part1")
    pub name: String,

    /// Role of this sub-package
    pub role: SubPackageRole,

    /// Files included in this sub-package
    pub files: Vec<FileEntry>,

    /// Total uncompressed size of files in this sub-package
    pub total_size: u64,

    /// Scripts to include (only populated for meta-package or non-split)
    pub scripts: ResolvedScripts,
}

#[derive(Debug, PartialEq)]
pub enum SubPackageRole {
    /// Standalone (no splitting)
    Standalone,
    /// Meta-package: no files, depends on all parts
    Meta,
    /// Part N of a split package
    Part(u32),
}

/// Scripts with content loaded from disk and alternatives injected
#[derive(Debug, Default)]
pub struct ResolvedScripts {
    pub pre_install: Option<String>,
    pub post_install: Option<String>,
    pub pre_remove: Option<String>,
    pub post_remove: Option<String>,
    pub pre_trans: Option<String>,
    pub post_trans: Option<String>,
}

/// Format-specific size limits used by the planner
pub struct FormatLimits {
    /// Max compressed payload size per package (for auto-split)
    pub max_compressed_payload: u64,
    /// Max individual file size before extended format needed
    pub max_file_size_standard: u64,
    /// Name of the format (for messages)
    pub format_name: &'static str,
}

impl FormatLimits {
    pub fn rpm() -> Self {
        Self {
            // RPM doesn't have a practical package size limit (64-bit tags since 4.6)
            max_compressed_payload: u64::MAX,
            // Standard cpio limit: 4 GiB (8 hex digits)
            max_file_size_standard: 0xFFFF_FFFF,
            format_name: "rpm",
        }
    }

    pub fn deb() -> Self {
        Self {
            // ar member size limit: 10 decimal digits
            max_compressed_payload: 9_999_999_999,
            // GNU tar: effectively unlimited per entry
            max_file_size_standard: u64::MAX,
            format_name: "deb",
        }
    }
}
```

### Steps

1. Implement `FileTree::walk(source_dir, file_mappings)` — walks source dir, applies glob patterns from config `content.files`, returns `Vec<FileEntry>`
2. Implement mode/user/group override logic from config
3. Implement symlink and directory entries from config
4. Implement `Planner::plan(config, format_limits)` → `PackagePlan`
   - Calculates total size
   - Checks `needs_extended_cpio` (any file > 4 GiB)
   - Runs split logic if needed
5. Implement auto-split algorithm:
   - Sort files by install path
   - Accumulate into sub-packages, respecting `max_compressed_payload` with safety margin
   - Generate meta-package entry
6. Implement alternatives → scriptlet generation (see spec.md §7)
7. Implement `spm plan` CLI command — runs planner, prints summary matching the format shown in spec.md §9
8. Implement size-based and directory-based split strategies

### Acceptance Criteria

```bash
# Create a test directory with known structure
mkdir -p /tmp/test-pkg/{bin,lib,share}
dd if=/dev/zero of=/tmp/test-pkg/lib/bigfile bs=1M count=100
echo '#!/bin/bash' > /tmp/test-pkg/bin/hello
chmod 755 /tmp/test-pkg/bin/hello

# Plan shows correct file count and sizes
spm plan --config tests/fixtures/minimal.yaml --format rpm
# Output includes file count, total size, and "Splitting: NOT REQUIRED"

# Plan detects need for splitting with DEB format on large payload
spm plan --config tests/fixtures/large.yaml --format deb
# Output includes "SPLIT REQUIRED" and shows part breakdown

# Unit tests for planner
cargo test -p spm-core -- planner
# Tests cover: no-split case, auto-split case, directory-split case,
# extended cpio detection, alternatives scriptlet generation
```

```rust
// Key unit tests to implement:

#[test]
fn test_no_split_small_package() {
    // Create a plan with total size well under limits
    // Assert: is_split == false, sub_packages.len() == 1, role == Standalone
}

#[test]
fn test_auto_split_deb_over_limit() {
    // Create entries totaling > 9.5 GiB
    // Plan with FormatLimits::deb()
    // Assert: is_split == true, first sub_package.role == Meta,
    //         remaining are Part(1), Part(2), etc.
}

#[test]
fn test_extended_cpio_detection() {
    // Create one entry with size > 4 GiB
    // Plan with FormatLimits::rpm()
    // Assert: needs_extended_cpio == true
}

#[test]
fn test_alternatives_scriptlet_generation() {
    // Config with alternatives block
    // Assert: resolved scripts contain update-alternatives --install in post_install
    // Assert: resolved scripts contain update-alternatives --remove in pre_remove
    // Assert: $1 guard is present in pre_remove
}

#[test]
fn test_alternatives_follower_syntax() {
    // Config with followers
    // Assert: --slave flags appear in generated scriptlet
}

#[test]
fn test_ownership_global_defaults() {
    // Config with content.defaults.user = "root", content.defaults.group = "appgroup"
    // Source files on disk owned by "builduser:builduser"
    // Assert: all FileEntry instances have user="root", group="appgroup"
}

#[test]
fn test_ownership_per_mapping_override() {
    // Config with content.defaults.user = "root"
    // One file mapping with user = "nobody"
    // Assert: files from that mapping have user="nobody"
    // Assert: files from other mappings still have user="root"
}

#[test]
fn test_ownership_dir_mode_vs_file_mode() {
    // Config with content.defaults.file_mode = "0644", content.defaults.dir_mode = "0755"
    // Source tree has mixed files and directories
    // Assert: regular files get mode 0o644
    // Assert: directories get mode 0o755
}

#[test]
fn test_src_dst_path_stripping() {
    // src: /tmp/build/output/**   dst: /opt/app/
    // File at /tmp/build/output/bin/tool
    // Assert: install_path == /opt/app/bin/tool
    // (src prefix before glob is stripped, replaced with dst)
}

#[test]
fn test_src_dst_literal_file() {
    // src: /tmp/build/license.txt   dst: /opt/app/LICENSE
    // Assert: install_path == /opt/app/LICENSE
    // (direct 1:1 mapping, no stripping)
}
```

---

## Phase 2: Compression Engine

**Goal:** A working compression abstraction that can compress a stream with zstd (multi-threaded) and gzip. Tested independently before integrating with package backends.

**Crates touched:** `spm-compress` (new crate)

### Dependencies

```toml
# spm-compress/Cargo.toml
[dependencies]
zstd = "0.13"
flate2 = "1"
thiserror = "2"
num_cpus = "1"
```

### Key Types

```rust
// spm-compress/src/lib.rs

use std::io::{Read, Write};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompressError {
    #[error("compression I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("unsupported algorithm: {0}")]
    Unsupported(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Algorithm {
    Zstd,
    Gzip,
    Xz,
    None,
}

impl Algorithm {
    pub fn from_str(s: &str) -> Result<Self, CompressError> {
        match s {
            "zstd" => Ok(Self::Zstd),
            "gzip" => Ok(Self::Gzip),
            "xz" => Ok(Self::Xz),
            "none" => Ok(Self::None),
            other => Err(CompressError::Unsupported(other.into())),
        }
    }

    /// File extension for archive members (e.g., "zst", "gz", "xz")
    pub fn extension(&self) -> &str {
        match self {
            Self::Zstd => "zst",
            Self::Gzip => "gz",
            Self::Xz => "xz",
            Self::None => "",
        }
    }

    /// RPM PAYLOADCOMPRESSOR tag value
    pub fn rpm_tag(&self) -> &str {
        match self {
            Self::Zstd => "zstd",
            Self::Gzip => "gzip",
            Self::Xz => "xz",
            Self::None => "identity",
        }
    }
}

pub struct CompressorConfig {
    pub algorithm: Algorithm,
    pub level: Option<i32>,
    pub threads: usize,     // 0 = auto
}

impl CompressorConfig {
    fn effective_threads(&self) -> usize {
        if self.threads == 0 { num_cpus::get() } else { self.threads }
    }

    fn effective_level(&self) -> i32 {
        match (self.algorithm, self.level) {
            (Algorithm::Zstd, Some(l)) => l,
            (Algorithm::Zstd, None) => 3,       // zstd default
            (Algorithm::Gzip, Some(l)) => l,
            (Algorithm::Gzip, None) => 6,       // gzip default
            (Algorithm::Xz, Some(l)) => l,
            (Algorithm::Xz, None) => 6,         // xz default
            (Algorithm::None, _) => 0,
        }
    }
}

/// Create a compressing writer that wraps an output writer.
/// Returns a boxed Write that compresses data written to it.
/// Caller must drop/finish the writer to flush compression state.
pub fn compress_writer<'a, W: Write + 'a>(
    config: &CompressorConfig,
    output: W,
) -> Result<Box<dyn Write + 'a>, CompressError> {
    match config.algorithm {
        Algorithm::Zstd => {
            let mut encoder = zstd::stream::Encoder::new(output, config.effective_level())?;
            encoder.multithread(config.effective_threads() as u32)?;
            Ok(Box::new(encoder.auto_finish()))
        }
        Algorithm::Gzip => {
            let encoder = flate2::write::GzEncoder::new(
                output,
                flate2::Compression::new(config.effective_level() as u32),
            );
            Ok(Box::new(encoder))
        }
        Algorithm::Xz => {
            // xz support added in Phase 5 (requires liblzma bindings)
            Err(CompressError::Unsupported("xz not yet implemented".into()))
        }
        Algorithm::None => {
            Ok(Box::new(output))
        }
    }
}
```

### Steps

1. Create `spm-compress` crate
2. Implement `compress_writer` for zstd with multi-threading
3. Implement `compress_writer` for gzip
4. Implement passthrough (none)
5. Write round-trip tests: compress → decompress → verify identical
6. Write benchmark test: compress a 100 MB buffer, verify multi-threading is faster than single-threaded
7. Stub xz (return `Unsupported` error — add in Phase 5)

### Acceptance Criteria

```rust
#[test]
fn test_zstd_roundtrip() {
    let original = vec![42u8; 1_000_000]; // 1 MB of repeated data
    let mut compressed = Vec::new();
    {
        let config = CompressorConfig { algorithm: Algorithm::Zstd, level: Some(3), threads: 1 };
        let mut writer = compress_writer(&config, &mut compressed).unwrap();
        writer.write_all(&original).unwrap();
        // writer dropped here, flushing
    }
    // Compressed should be much smaller than original
    assert!(compressed.len() < original.len() / 10);
    // Decompress and verify
    let mut decompressed = Vec::new();
    zstd::stream::copy_decode(&compressed[..], &mut decompressed).unwrap();
    assert_eq!(original, decompressed);
}

#[test]
fn test_gzip_roundtrip() {
    // Same pattern as above with flate2 decoder
}

#[test]
fn test_zstd_multithreaded() {
    let config = CompressorConfig { algorithm: Algorithm::Zstd, level: Some(3), threads: 4 };
    let mut writer = compress_writer(&config, std::io::sink()).unwrap();
    // Should not panic — verifies multithread setup works
    writer.write_all(&vec![0u8; 10_000_000]).unwrap();
}
```

---

## Phase 3: CPIO Writer & RPM Backend

**Goal:** `spm build --format rpm` produces a valid RPM that `rpm -qpl` can read and `rpm -K` verifies checksums for. No signing yet.

**Crates touched:** `spm-cpio` (new), `spm-rpm` (new)

### Dependencies

```toml
# spm-cpio/Cargo.toml
[dependencies]
thiserror = "2"

# spm-rpm/Cargo.toml
[dependencies]
spm-core = { path = "../spm-core" }
spm-cpio = { path = "../spm-cpio" }
spm-compress = { path = "../spm-compress" }
sha2 = "0.10"
md-5 = "0.10"
thiserror = "2"
```

### Key Types

```rust
// spm-cpio/src/lib.rs

use std::io::Write;

/// CPIO archive format variant
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CpioFormat {
    /// Standard SVR4 newc (magic "070701"), max file size 4 GiB
    Newc,
    /// RPM extended (magic "07070X"), file index only, unlimited size
    Extended,
}

/// Builder for a CPIO archive. Writes entries sequentially to an underlying writer.
pub struct CpioWriter<W: Write> {
    writer: W,
    format: CpioFormat,
    bytes_written: u64,
    entry_index: u32,
}

impl<W: Write> CpioWriter<W> {
    pub fn new(writer: W, format: CpioFormat) -> Self { /* ... */ }

    /// Write a file entry. For Newc format, metadata comes from FileEntry.
    /// For Extended format, only the index is written in the header.
    pub fn write_entry(
        &mut self,
        index: u32,
        name: &str,       // ignored for Extended format
        metadata: &CpioMetadata,
        data: &mut dyn std::io::Read,
    ) -> Result<u64, CpioError> { /* ... */ }

    /// Write the TRAILER!!! entry to terminate the archive
    pub fn finish(self) -> Result<W, CpioError> { /* ... */ }
}

pub struct CpioMetadata {
    pub ino: u32,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub mtime: u32,
    pub filesize: u64,      // u64 for Extended format support
    pub devmajor: u32,
    pub devminor: u32,
    pub rdevmajor: u32,
    pub rdevminor: u32,
}
```

```rust
// spm-rpm/src/lib.rs

use spm_core::planner::{PackagePlan, SubPackage};

/// Builds an RPM file from a SubPackage
pub struct RpmBuilder {
    // Configuration for this build
}

impl RpmBuilder {
    pub fn new() -> Self { /* ... */ }

    /// Build a single RPM file from a SubPackage plan
    pub fn build(
        &self,
        sub_package: &SubPackage,
        plan: &PackagePlan,
        config: &spm_core::config::Config,
        output_path: &std::path::Path,
    ) -> Result<(), RpmError> { /* ... */ }
}
```

### Steps

**CPIO Writer:**
1. Implement `CpioWriter` for `Newc` format (`070701`)
   - Header: 6-byte magic + 13 fields × 8 hex chars each = 110 bytes
   - Filename follows header, padded to 4-byte boundary
   - File data follows filename, padded to 4-byte boundary
   - TRAILER!!! entry at the end
2. Implement `CpioWriter` for `Extended` format (`07070X`)
   - Header: 6-byte magic + file index as 8-byte hex string
   - No filename, no other metadata in header
   - File data follows, padded to 4-byte boundary
3. Test both formats produce output that `cpio` (for Newc) or `rpm2cpio | cpio` (for Extended) can read

**RPM Builder:**
4. Implement RPM Lead (96 bytes, hardcoded structure)
5. Implement RPM Header Structure writer:
   - Magic bytes, index entry count, data section size
   - Index entries: tag, type, offset, count
   - Data section: tag values
6. Implement Header tags for package metadata (name, version, release, arch, description, etc.)
7. Implement Header tags for file metadata (RPMTAG_BASENAMES, RPMTAG_DIRNAMES, RPMTAG_DIRINDEXES, RPMTAG_FILESIZES / LONGFILESIZES, RPMTAG_FILEMODES, etc.)
8. Implement Header tags for dependencies (RPMTAG_REQUIRENAME, REQUIREVERSION, REQUIREFLAGS)
9. Implement Signature Header (MD5 of header+payload, SHA256 of header, sizes)
10. Wire it together: Lead → Signature → Header → (cpio | compress) → payload
11. Implement the streaming pipeline: file entries → CpioWriter → CompressWriter → file

**Evaluate vs existing `rpm` crate:** Before implementing the RPM header from scratch, evaluate whether the `rpm` crate (v0.18) on crates.io can be used or forked. It handles header construction but may not support `07070X` cpio. If it's close enough, use it and add the extended cpio. If not, implement directly. Document the decision.

### Acceptance Criteria

```bash
# Build a simple RPM with a few small files
spm build --format rpm --config tests/fixtures/minimal.yaml -o /tmp/out/

# RPM tools can read it
rpm -qpl /tmp/out/testpkg-1.0-1.x86_64.rpm
# Lists expected files

rpm -qi -p /tmp/out/testpkg-1.0-1.x86_64.rpm
# Shows name, version, description, etc.

rpm -K /tmp/out/testpkg-1.0-1.x86_64.rpm
# Shows: digests OK (no signature yet, but checksums pass)

# Install on a test RHEL/Fedora container
podman run --rm -v /tmp/out:/pkg:ro fedora:40 rpm -ivh /pkg/testpkg-1.0-1.x86_64.rpm
# Installs successfully, files appear at expected paths

# Build with extended cpio (mock a >4GB file with sparse)
# Unit test: verify 07070X magic appears in payload when large file present
```

```rust
#[test]
fn test_cpio_newc_magic() {
    let mut buf = Vec::new();
    let mut writer = CpioWriter::new(&mut buf, CpioFormat::Newc);
    // ... write an entry ...
    writer.finish().unwrap();
    assert_eq!(&buf[0..6], b"070701");
}

#[test]
fn test_cpio_extended_magic() {
    let mut buf = Vec::new();
    let mut writer = CpioWriter::new(&mut buf, CpioFormat::Extended);
    // ... write an entry ...
    writer.finish().unwrap();
    assert_eq!(&buf[0..6], b"07070X");
}

#[test]
fn test_cpio_newc_padding() {
    // Verify filename and data are padded to 4-byte boundaries
}

#[test]
fn test_rpm_header_byte_order() {
    // All integers in RPM headers are network byte order (big-endian)
}
```

---

## Phase 4: DEB Backend & Auto-Split

**Goal:** `spm build --format deb` produces a valid DEB that `dpkg-deb -I` and `dpkg-deb -c` can read. Auto-splitting works for large packages.

**Crates touched:** `spm-deb` (new)

### Dependencies

```toml
# spm-deb/Cargo.toml
[dependencies]
spm-core = { path = "../spm-core" }
spm-compress = { path = "../spm-compress" }
tar = "0.4"
md-5 = "0.10"
thiserror = "2"
```

### Key Types

```rust
// spm-deb/src/lib.rs

pub struct DebBuilder {
    // Configuration for this build
}

impl DebBuilder {
    pub fn new() -> Self { /* ... */ }

    /// Build one or more DEB files from a PackagePlan.
    /// Returns paths to all generated .deb files.
    pub fn build(
        &self,
        plan: &PackagePlan,
        config: &spm_core::config::Config,
        output_dir: &std::path::Path,
    ) -> Result<Vec<std::path::PathBuf>, DebError> { /* ... */ }
}

/// Writes a minimal ar archive (DEB-specific: no long filenames, no symbol table)
struct ArWriter<W: std::io::Write> {
    writer: W,
    wrote_magic: bool,
}

impl<W: std::io::Write> ArWriter<W> {
    fn new(writer: W) -> Self { /* ... */ }
    fn write_magic(&mut self) -> Result<(), std::io::Error> { /* writes "!<arch>\n" */ }
    fn write_member(&mut self, name: &str, data: &[u8], mtime: u64, mode: u32) -> Result<(), std::io::Error> { /* ... */ }
    /// Streaming variant: write header first, then caller writes data, then call finish_member
    fn begin_member(&mut self, name: &str, size: u64, mtime: u64, mode: u32) -> Result<(), std::io::Error> { /* ... */ }
    fn finish_member(&mut self) -> Result<(), std::io::Error> { /* padding */ }
    fn finish(self) -> Result<W, std::io::Error> { /* ... */ }
}
```

### Steps

1. Implement `ArWriter` — minimal ar archive writer
   - `!<arch>\n` magic header
   - Member headers: 16-byte name, 12-byte mtime, 6-byte uid, 6-byte gid, 8-byte mode, 10-byte size, 2-byte fmag
   - Padding to even byte boundary between members
2. Implement control file generation from config (spec.md §6.5)
3. Implement `control.tar.zst` generation:
   - Create tar archive containing: `control`, `md5sums`, `conffiles` (if any), scripts
   - Compress with configured algorithm
4. Implement `data.tar.zst` generation:
   - Use the `tar` crate to write files from `SubPackage.files`
   - Use GNU tar format (for large file support)
   - Compress with configured algorithm
   - **Stream the data**: tar → compress → temp file (need to know final size for ar header)
5. Assemble the DEB: `debian-binary` + `control.tar.zst` + `data.tar.zst` into ar archive
6. Implement auto-split DEB flow:
   - When `plan.is_split == true`, build each `SubPackage` as a separate DEB
   - Meta-package DEB has empty `data.tar.zst` but control file with `Depends:` on all parts
   - Each part DEB has its subset of files
7. Implement md5sums generation for all files
8. Wire alternatives auto-dependency injection for DEB (no extra package needed — `update-alternatives` ships with dpkg)

### Acceptance Criteria

```bash
# Build a simple DEB
spm build --format deb --config tests/fixtures/minimal.yaml -o /tmp/out/

# DEB tools can read it
dpkg-deb -I /tmp/out/testpkg_1.0-1_amd64.deb
# Shows package metadata

dpkg-deb -c /tmp/out/testpkg_1.0-1_amd64.deb
# Lists files with correct permissions

# Install on test Ubuntu container
podman run --rm -v /tmp/out:/pkg:ro ubuntu:24.04 dpkg -i /pkg/testpkg_1.0-1_amd64.deb
# Installs successfully

# Auto-split test (mock large payload)
spm build --format deb --config tests/fixtures/large.yaml -o /tmp/out/
ls /tmp/out/
# Shows: testpkg_1.0-1_amd64.deb (meta), testpkg-part1_1.0-1_amd64.deb, testpkg-part2_1.0-1_amd64.deb

dpkg-deb -I /tmp/out/testpkg_1.0-1_amd64.deb
# Meta-package Depends: field lists all parts

# Install all parts on test container
podman run --rm -v /tmp/out:/pkg:ro ubuntu:24.04 bash -c \
    "dpkg -i /pkg/testpkg-part*_1.0-1_amd64.deb /pkg/testpkg_1.0-1_amd64.deb"
# All files present from all parts
```

---

## Phase 5: Full CLI, Distro Compat, Polish

**Goal:** Complete CLI with all subcommands, target distro validation, xz compression, `spm plan` with full output, progress bars for large builds. Tool is usable for real packaging.

**Crates touched:** All crates — this is the integration and polish phase.

### Steps

1. **`spm build` full implementation:**
   - `--format all` builds both RPM and DEB
   - `--format rpm` and `--format deb` individually
   - `--output` directory creation
   - Progress bars via `indicatif` for large file operations
   - `--no-split` flag (fail with error if package exceeds limits)
   - `--source-date-epoch` for reproducible builds

2. **`spm plan` full implementation:**
   - Match the output format shown in spec.md §9
   - Show file counts, sizes, compression estimates, split plans
   - Show compatibility warnings for target distro
   - Show which cpio format will be used

3. **`spm inspect`:**
   - Read an existing .rpm or .deb and print its metadata
   - Useful for verifying built packages

4. **Target distro compatibility (spec.md §10):**
   - Compile-time distro database (RHEL 8/9, Ubuntu 20.04/22.04/24.04, Fedora)
   - `--target-distro el9` checks compression compat, feature support
   - Emit warnings, not errors (user can override with `--force`)

5. **xz compression:**
   - Add `xz2` or `liblzma-sys` dependency to `spm-compress`
   - Implement `compress_writer` for xz
   - Multi-threading via liblzma's threaded mode

6. **Alternatives auto-dependency injection:**
   - RPM: detect target distro, add `Requires: /usr/sbin/alternatives` (or `chkconfig` for EL8)
   - DEB: no injection needed
   - Test on both EL8 and EL9 containers

7. **Config file validation improvements:**
   - Check that source_dir exists
   - Check that script files exist
   - Check that glob patterns match at least one file (warning)
   - Check that alternatives paths exist in the file tree

8. **Error messages:**
   - Every error should include context: which file, which config field, what was expected
   - Use `anyhow` context chains in CLI

### Acceptance Criteria

```bash
# Full build for both formats
spm build --config spm.yaml --format all --output ./packages/
ls ./packages/
# matlab-2025a-1.x86_64.rpm
# matlab-2025a_2025a-1_amd64.deb

# Plan with target distro
spm plan --format rpm --target-distro el8
# Shows compatibility notes

# Inspect a built package
spm inspect ./packages/matlab-2025a-1.x86_64.rpm
# Package: matlab-2025a
# Version: 2025a
# Release: 1
# ...

# Reproducible builds
SOURCE_DATE_EPOCH=1700000000 spm build --format rpm -o /tmp/a/
SOURCE_DATE_EPOCH=1700000000 spm build --format rpm -o /tmp/b/
sha256sum /tmp/a/*.rpm /tmp/b/*.rpm
# Hashes match
```

---

## Phase 6: Signing (Optional)

**Goal:** Built-in RPM PGP signing. DEB signing deferred (repo tools handle it).

**Crates touched:** `spm-rpm`

### Dependencies

```toml
# Add to spm-rpm/Cargo.toml
sequoia-openpgp = "1"
```

### Steps

1. Implement RPM V4 PGP signature generation:
   - Load PGP secret key from file (ASCII-armored or binary)
   - Sign the Header (RPMSIGTAG_RSA for RSA keys)
   - Sign the Header+Payload (RPMSIGTAG_PGP)
   - Insert signatures into the Signature Header
2. Implement `--no-sign` CLI flag
3. Implement `SPM_SIGNING_KEY` and `SPM_PASSPHRASE` env vars
4. Test: verify signed RPMs with `rpm -K` and `rpm --checksig`

### Acceptance Criteria

```bash
# Build and sign
SPM_SIGNING_KEY=./test-key.asc spm build --format rpm -o /tmp/out/

# Verify signature
rpm -K /tmp/out/testpkg-1.0-1.x86_64.rpm
# testpkg-1.0-1.x86_64.rpm: digests signatures OK

# Import key and verify
rpm --import ./test-key.pub.asc
rpm --checksig /tmp/out/testpkg-1.0-1.x86_64.rpm
# testpkg-1.0-1.x86_64.rpm: ... pgp ... OK

# Build without signing
spm build --format rpm --no-sign -o /tmp/out/
rpm -K /tmp/out/testpkg-1.0-1.x86_64.rpm
# digests OK (no signature)
```

---

## General Implementation Notes

### Streaming I/O Pattern

For large payloads, never buffer the entire archive in memory. Use this pattern:

```rust
// Pseudocode for the RPM build pipeline
let output_file = File::create("package.rpm")?;

// 1. Write Lead (96 bytes, fixed)
write_lead(&mut output_file)?;

// 2. Build payload into a temp file (need to know size for header)
let payload_tmp = tempfile::NamedTempFile::new()?;
{
    let compressor = compress_writer(&config, &payload_tmp)?;
    let mut cpio = CpioWriter::new(compressor, cpio_format);
    for (index, entry) in sub_package.files.iter().enumerate() {
        let mut file = File::open(&entry.source_path)?;
        cpio.write_entry(index as u32, &entry.install_path, &metadata, &mut file)?;
    }
    cpio.finish()?;
    // compressor dropped here → flushes
}

// 3. Now we know the payload size — build Header and Signature
let payload_size = payload_tmp.as_file().metadata()?.len();
let header_bytes = build_header(&sub_package, payload_size)?;
let signature_bytes = build_signature(&header_bytes, &payload_tmp)?;

// 4. Write Signature, then Header, then copy payload
write_header_structure(&mut output_file, &signature_bytes)?;
pad_to_8_bytes(&mut output_file)?;
output_file.write_all(&header_bytes)?;
std::io::copy(&mut payload_tmp.reopen()?, &mut output_file)?;
```

### Hardlink Handling

The cpio format handles hardlinks by writing the file data only once (with the last link). All earlier links have filesize=0. The planner must:

1. Detect hardlinks (same device + inode)
2. Group them together
3. Ensure all links in a group end up in the same sub-package (or break them into copies if split across packages)
4. In the cpio archive, write all-but-last with size 0, last with full data

### Config File Handling

Files marked `type: config` in the YAML get special treatment:
- **RPM:** Added to `RPMTAG_FILEFLAGS` with `RPMFILE_CONFIG | RPMFILE_NOREPLACE`
- **DEB:** Listed in `conffiles` inside `control.tar`

### Reproducible Builds

When `source_date_epoch` is set:
- All file timestamps in cpio/tar set to that value
- RPM BUILDTIME tag set to that value
- ar member timestamps set to that value
- File ordering is deterministic (sorted by install path)

### Testing Infrastructure

Create a `tests/integration/` directory with:
- A `conftest.rs` or helper module that builds test directory trees
- Container-based tests using `testcontainers` crate or shell scripts with `podman`
- A test GPG key pair (committed to repo, never used for real packages)
- Sparse file generation for large file tests