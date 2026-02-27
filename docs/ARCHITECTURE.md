# Architecture

## Workspace Layout

```
spm/
├── Cargo.toml                  # Workspace root
├── crates/
│   ├── spm-cli/                # Binary crate — CLI frontend
│   │   └── src/main.rs         # clap CLI with validate/init/plan subcommands
│   └── spm-core/               # Config parsing, planning, shared types
│       └── src/
│           ├── lib.rs           # Re-exports modules
│           ├── config.rs        # YAML deserialization & validation
│           ├── error.rs         # Error types (ConfigError, FileTreeError, PlanError)
│           ├── types.rs         # FormatLimits, parse_size, format_size, PackageFileName
│           ├── filetree.rs      # File tree walking, glob expansion, hardlink detection
│           ├── planner.rs       # Package planning, split strategies, meta-package generation
│           └── alternatives.rs  # update-alternatives scriptlet generation
└── tests/
    └── fixtures/               # Test YAML configs
```

## Crate Dependency Graph

```
spm-cli ──► spm-core
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

### spm-cli

- `Cli` / `Commands` — clap-derived CLI structure with `validate`, `init`, and `plan` subcommands.
