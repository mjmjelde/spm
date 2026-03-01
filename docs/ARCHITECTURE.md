# Architecture

## Workspace Layout

```
spm/
├── Cargo.toml                  # Workspace root
├── crates/
│   ├── spm-cli/                # Binary crate — CLI frontend
│   │   └── src/main.rs         # clap CLI with validate/init/plan/build/inspect subcommands
│   ├── spm-compress/           # Streaming compression & decompression abstraction
│   │   └── src/lib.rs          # Algorithm, CompressorConfig, compress_writer(), decompress_reader()
│   ├── spm-core/               # Config parsing, planning, shared types
│   │   └── src/
│   │       ├── lib.rs           # Re-exports modules
│   │       ├── config.rs        # YAML deserialization & validation
│   │       ├── error.rs         # Error types (ConfigError, FileTreeError, PlanError)
│   │       ├── types.rs         # FormatLimits, parse_size, format_size, PackageFileName
│   │       ├── filetree.rs      # File tree walking, glob expansion, hardlink detection
│   │       ├── planner.rs       # Package planning, split strategies, meta-package generation
│   │       ├── alternatives.rs  # update-alternatives scriptlet generation
│   │       └── distro.rs        # Target distribution compatibility database
│   ├── spm-cpio/               # CPIO archive writer (Newc + Extended)
│   │   └── src/lib.rs          # CpioWriter, CpioFormat, CpioMetadata
│   ├── spm-deb/                # DEB package builder & reader
│   │   └── src/
│   │       ├── lib.rs           # Re-exports modules
│   │       ├── ar.rs            # ar archive writer
│   │       ├── builder.rs       # DebBuilder — full DEB build pipeline
│   │       ├── control.rs       # Control file generation (RFC 822 format)
│   │       ├── error.rs         # DebError
│   │       └── reader.rs        # DEB metadata reader (ar → control.tar → control)
│   └── spm-rpm/                # RPM v4 package builder & reader
│       └── src/
│           ├── lib.rs           # Re-exports modules
│           ├── error.rs         # RpmError
│           ├── lead.rs          # 96-byte RPM lead writer
│           ├── tags.rs          # All RPM tag constants and flag definitions
│           ├── header.rs        # HeaderBuilder — RPM header binary format
│           ├── signature.rs     # Signature header (MD5, SHA-1, SHA-256, sizes)
│           ├── builder.rs       # RpmBuilder — full RPM build pipeline
│           └── reader.rs        # RPM metadata reader (lead → sig header → metadata header)
└── tests/
    └── fixtures/               # Test YAML configs
```

## Crate Dependency Graph

```
spm-cli ──► spm-core
        ├─► spm-rpm ──► spm-core
        │            ├─► spm-cpio
        │            └─► spm-compress
        └─► spm-deb ──► spm-core
                     └─► spm-compress
spm-compress  (standalone, no spm-core dependency)
spm-cpio      (standalone, only depends on thiserror)
```

## Key Types

### spm-core

**Config layer:**
- `Config` — Top-level config struct, deserializable from YAML. Entry point: `Config::load(path)`. Implements `Clone` for CLI override pattern.
- `PackageConfig` — Package identity (name, version, arch, etc.).
- `ContentConfig` — File mappings, symlinks, directories, alternatives.
- `CompressionConfig` — Algorithm, level, thread count.
- `SplittingConfig` — Auto-split strategy and parameters.
- `BuildConfig` — Build-time settings (`source_date_epoch`).
- `ConfigError` — Typed error enum for config loading/validation failures.

**File tree layer:**
- `FileEntry` — A single file/dir/symlink to include in the package (install path, source path, type, size, mode, user/group).
- `EntryType` — `RegularFile`, `Directory`, `Symlink { target }`, `Hardlink { target }`.
- `FileTree::walk()` — Walks file mappings, expands glob patterns (bare directory paths auto-expand to `dir/**`), returns sorted/deduplicated entries.
- `FileTreeError` — Errors during file tree walking.

**Planning layer:**
- `PackagePlan` — Complete output of planning: sub-packages, split status, extended cpio flag, total size, warnings.
- `SubPackage` — One buildable package (standalone, meta, or part) with its files and scripts.
- `SubPackageRole` — `Standalone`, `Meta`, `Part(u32)`.
- `ResolvedScripts` — Script contents with alternatives scriptlets injected.
- `Planner::plan()` — Creates a package plan from config and format limits. Auto-split uses 80% safety factor and even-parts sizing.
- `PlanError` — Errors during planning.

**Shared types:**
- `FormatLimits` — Format-specific size limits (`rpm()`, `deb()`).
- `parse_size()` / `format_size()` — Human-readable size parsing/formatting.
- `PackageFileName` — RPM/DEB output filename generation with arch translation.

**Alternatives:**
- `generate_install_scriptlet()` — Generates `update-alternatives --install` with follower support.
- `generate_remove_scriptlet()` — Generates guarded `update-alternatives --remove`.
- `resolve_scripts()` — Loads user scripts, injects alternatives scriptlets.

**Distro compatibility:**
- `Distro` — Enum of known target distributions (El8, El9, Ubuntu2004, Ubuntu2204, Ubuntu2404, Fedora).
- `RpmDistroInfo` / `DebDistroInfo` — Per-distro capability info (version, zstd support, large file support).
- `check_compatibility()` — Validates config against a target distro, returns warnings.
- `minimum_rpm_version()` / `minimum_dpkg_version()` — Returns minimum tool version required for a given configuration.

### spm-compress

- `Algorithm` — `Zstd`, `Gzip`, `Xz`, `None`. Methods: `from_str()`, `extension()`, `rpm_tag()`, `estimated_ratio()`.
- `CompressorConfig` — Algorithm, level, thread count. Resolves defaults via `effective_level()` and `effective_threads()`.
- `FinishableWriter` — Wrapper around compressor streams with an explicit `finish()` method that finalizes compression and returns `io::Result<()>`. Implements `Write` for transparent use in streaming pipelines.
- `compress_writer()` — Creates a `FinishableWriter` that compresses data written to it. Supports zstd (multi-threaded), gzip, xz (multi-threaded), none (passthrough). Callers **must** call `.finish()` to flush and finalize the compressor — relying on `Drop` silently discards gzip/xz finalization errors.
- `decompress_reader()` — Creates a `Box<dyn Read>` that decompresses data read from it. Supports zstd, gzip, xz, none (passthrough).
- `CompressError` — `Io`, `Unsupported`.

### spm-cpio

- `CpioFormat` — `Newc` (070701, standard), `Extended` (07070X, RPM-specific for >4GiB files).
- `CpioWriter<W: Write>` — Sequential archive writer. `write_entry()` writes header + data, `finish()` writes trailer and returns `(W, u64)`.
- `CpioMetadata` — Per-entry metadata: ino, mode, uid, gid, nlink, mtime, filesize, devmajor/minor, rdevmajor/minor.
- `CpioError` — `Io`, `FileTooLarge` (Newc format, >4GiB).

### spm-rpm

- `RpmBuilder::build()` — Full RPM build pipeline: file digests → CPIO payload → metadata header → signature header → assembly. Accepts optional `&Distro` for distro-aware builds.
- `HeaderBuilder` — Builds RPM header binary format with proper alignment and tag-sorted data ordering. Supports region tags.
- `build_signature()` — Creates signature header with MD5, SHA-1, SHA-256, and size tags.
- `write_lead()` — Writes the 96-byte RPM lead structure.
- `read_rpm_metadata()` — Reads and parses an existing RPM file, returning `RpmMetadata` with all key fields.
- `RpmMetadata` — Extracted metadata: name, version, release, arch, size, description, license, url, vendor, packager, compressor, file_count, requires.
- `RpmError` — `Io`, `Cpio`, `Compress`, `SourceFile`, `Header`, `InvalidRpm`.

### spm-deb

- `DebBuilder::build()` — Full DEB build pipeline: control file → control.tar → data.tar → ar assembly. Handles split packages.
- `ArWriter<W: Write>` — ar archive writer with reproducible timestamps and member size validation (rejects > 9,999,999,999 bytes).
- `generate_control()` — Creates RFC 822 control file content.
- `read_deb_metadata()` — Reads and parses an existing DEB file, returning `DebMetadata` with all control fields.
- `DebMetadata` — Extracted metadata: ordered `fields: Vec<(String, String)>` with case-insensitive `get()`.
- `DebError` — `Io`, `Tar`, `Compress`, `SourceFile`, `InvalidDeb`.

### spm-cli

- `Cli` / `Commands` — clap-derived CLI structure with `validate`, `init`, `plan`, `build`, and `inspect` subcommands.
- `cmd_build()` — Loads config, applies overrides, creates package plan, builds RPM/DEB/both for each sub-package with progress spinners.
- `cmd_plan()` — Shows build plan with minimum version requirements and distro compatibility warnings.
- `cmd_inspect()` — Reads and displays metadata from existing .rpm or .deb files.
- `apply_overrides()` — Shared helper for CLI flag → config mutation (splitting, compression, source_date_epoch).
