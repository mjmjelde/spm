# Changelog

## 0.1.0 (unreleased)

### Phase 0: Workspace Scaffolding & Config Parsing

- Created workspace with `spm-core` and `spm-cli` crates
- Implemented YAML config parsing with full schema from spec.md
- Implemented environment variable expansion (`${VAR}` syntax)
- Implemented config validation (arch, compression, splitting strategy)
- Added `spm validate` subcommand
- Added `spm init` subcommand to generate template configs
- Added test fixtures (minimal, full MATLAB example, invalid configs)

### Phase 1: File Tree Walking & Package Planning

- Implemented file tree walker (`filetree.rs`) with glob expansion, hardlink detection, and implicit parent directories
- Implemented package planner (`planner.rs`) with auto/size/directory split strategies and meta-package generation
- Implemented alternatives scriptlet generation (`alternatives.rs`) with `--slave` follower support and `$1` guard
- Added shared types: `FormatLimits`, `parse_size()`, `format_size()`, `PackageFileName`
- Added `spm plan` subcommand with `--format rpm|deb` flag
- Added `walkdir` and `glob` dependencies for file system traversal
- 62 unit tests across all new modules

### Phase 2: Compression Engine

- Created `spm-compress` crate with streaming compression abstraction
- Implemented `compress_writer()` for zstd (multi-threaded), gzip, and none (passthrough)
- Stubbed xz support (returns `Unsupported` error, deferred to Phase 5)
- `Algorithm` enum with `from_str()`, `extension()`, `rpm_tag()`, `estimated_ratio()` methods
- Auto-detect thread count via `num_cpus` when `threads = 0`
- 12 unit tests + 1 doc-test
