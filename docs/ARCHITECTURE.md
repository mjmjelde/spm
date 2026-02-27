# Architecture

## Workspace Layout

```
spm/
├── Cargo.toml                  # Workspace root
├── crates/
│   ├── spm-cli/                # Binary crate — CLI frontend
│   │   └── src/main.rs         # clap CLI with validate/init/plan/build subcommands
│   ├── spm-compress/           # Streaming compression abstraction
│   │   └── src/lib.rs          # Algorithm, CompressorConfig, compress_writer()
│   ├── spm-core/               # Config parsing, planning, shared types
│   │   └── src/
│   │       ├── lib.rs           # Re-exports modules
│   │       ├── config.rs        # YAML deserialization & validation
│   │       ├── error.rs         # Error types (ConfigError, FileTreeError, PlanError)
│   │       ├── types.rs         # FormatLimits, parse_size, format_size, PackageFileName
│   │       ├── filetree.rs      # File tree walking, glob expansion, hardlink detection
│   │       ├── planner.rs       # Package planning, split strategies, meta-package generation
│   │       └── alternatives.rs  # update-alternatives scriptlet generation
│   ├── spm-cpio/               # CPIO archive writer (Newc + Extended)
│   │   └── src/lib.rs          # CpioWriter, CpioFormat, CpioMetadata
│   └── spm-rpm/                # RPM v4 package builder
│       └── src/
│           ├── lib.rs           # Re-exports modules
│           ├── error.rs         # RpmError (Io, Cpio, Compress, SourceFile, Header)
│           ├── lead.rs          # 96-byte RPM lead writer
│           ├── tags.rs          # All RPM tag constants and flag definitions
│           ├── header.rs        # HeaderBuilder — RPM header binary format
│           ├── signature.rs     # Signature header (MD5, SHA-1, SHA-256, sizes)
│           └── builder.rs       # RpmBuilder — full RPM build pipeline
└── tests/
    └── fixtures/               # Test YAML configs
```

## Crate Dependency Graph

```
spm-cli ──► spm-core
        └─► spm-rpm ──► spm-core
                    └─► spm-cpio
                    └─► spm-compress
spm-compress  (standalone, no spm-core dependency)
spm-cpio      (standalone, only depends on thiserror)
```

## Key Types

### spm-core

**Config layer:**
- `Config` — Top-level config struct, deserializable from YAML. Entry point: `Config::load(path)`.
- `PackageConfig` — Package identity (name, version, arch, etc.).
- `ContentConfig` — File mappings, symlinks, directories, alternatives.
- `CompressionConfig` — Algorithm, level, thread count.
- `SplittingConfig` — Auto-split strategy and parameters.
- `ConfigError` — Typed error enum for config loading/validation failures.

**File tree layer:**
- `FileEntry` — A single file/dir/symlink to include in the package (install path, source path, type, size, mode, user/group).
- `EntryType` — `RegularFile`, `Directory`, `Symlink { target }`, `Hardlink { target }`.
- `FileTree::walk()` — Walks source directory, applies glob mappings, returns sorted/deduplicated entries.
- `FileTreeError` — Errors during file tree walking.

**Planning layer:**
- `PackagePlan` — Complete output of planning: sub-packages, split status, extended cpio flag, total size.
- `SubPackage` — One buildable package (standalone, meta, or part) with its files and scripts.
- `SubPackageRole` — `Standalone`, `Meta`, `Part(u32)`.
- `ResolvedScripts` — Script contents with alternatives scriptlets injected.
- `Planner::plan()` — Creates a package plan from config and format limits.
- `PlanError` — Errors during planning.

**Shared types:**
- `FormatLimits` — Format-specific size limits (`rpm()`, `deb()`).
- `parse_size()` / `format_size()` — Human-readable size parsing/formatting.
- `PackageFileName` — RPM/DEB output filename generation with arch translation.

**Alternatives:**
- `generate_install_scriptlet()` — Generates `update-alternatives --install` with follower support.
- `generate_remove_scriptlet()` — Generates guarded `update-alternatives --remove`.
- `resolve_scripts()` — Loads user scripts, injects alternatives scriptlets.

### spm-compress

- `Algorithm` — `Zstd`, `Gzip`, `Xz`, `None`. Methods: `from_str()`, `extension()`, `rpm_tag()`, `estimated_ratio()`.
- `CompressorConfig` — Algorithm, level, thread count. Resolves defaults via `effective_level()` and `effective_threads()`.
- `compress_writer()` — Creates a `Box<dyn Write>` that compresses data written to it. Supports zstd (multi-threaded), gzip, none (passthrough). Xz stubbed.
- `CompressError` — `Io`, `Unsupported`.

### spm-cpio

- `CpioFormat` — `Newc` (070701, standard), `Extended` (07070X, RPM-specific for >4GiB files).
- `CpioWriter<W: Write>` — Sequential archive writer. `write_entry()` writes header + data, `finish()` writes trailer and returns `(W, u64)`.
- `CpioMetadata` — Per-entry metadata: ino, mode, uid, gid, nlink, mtime, filesize, devmajor/minor, rdevmajor/minor.
- `CpioError` — `Io`, `FileTooLarge` (Newc format, >4GiB).

### spm-rpm

- `RpmBuilder::build()` — Full RPM build pipeline: file digests → CPIO payload → metadata header → signature header → assembly.
- `HeaderBuilder` — Builds RPM header binary format with proper alignment and tag-sorted data ordering. Supports region tags.
- `build_signature()` — Creates signature header with MD5, SHA-1, SHA-256, and size tags.
- `write_lead()` — Writes the 96-byte RPM lead structure.
- `RpmError` — `Io`, `Cpio`, `Compress`, `SourceFile`, `Header`.

### spm-cli

- `Cli` / `Commands` — clap-derived CLI structure with `validate`, `init`, `plan`, and `build` subcommands.
- `cmd_build()` — Loads config, creates package plan, builds RPM for each sub-package.
