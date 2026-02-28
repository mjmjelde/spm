# Phase 5b: CLI Integration — Build Flags, Inspect, Spinners

## What Was Implemented

### spm-compress: Decompression reader
- Added `decompress_reader()` alongside the existing `compress_writer()`
- Supports Zstd (`zstd::stream::Decoder`), Gzip (`flate2::read::GzDecoder`), Xz (`xz2::read::XzDecoder`), and None (passthrough)
- Used by both RPM and DEB readers for decompression
- 4 new tests: roundtrip tests for each algorithm + passthrough

### spm-rpm: Metadata reader
- New `reader.rs` module with `read_rpm_metadata(path) -> Result<RpmMetadata>`
- Parses RPM binary format in order: lead (96 bytes, validates `0xEDABEEDB` magic) → signature header (skip + 8-byte alignment padding) → metadata header (full parse)
- `RpmMetadata` struct: name, version, release, arch, size, description, license, url, vendor, packager, compressor, file_count, requires
- `ParsedTagValue` enum for typed extraction: String, StringArray, Int32, Int64, Int16, Bin
- Helper functions: `skip_header_section()`, `parse_header_section()`, `extract_tag_value()`, `read_nul_string()`, typed extractors
- Prefers LONGSIZE (tag 5009) over SIZE (tag 1009) for 64-bit package sizes
- File count derived from BASENAMES array length, dependencies from REQUIRENAME
- 10 new tests including roundtrip tests (build with `RpmBuilder`, read back, verify)

### spm-deb: Metadata reader
- New `reader.rs` module with `read_deb_metadata(path) -> Result<DebMetadata>`
- Parses ar archive: validates `!<arch>\n` magic, walks 60-byte member headers
- Finds `control.tar.{zst,gz,xz}` member, detects compression from extension
- Decompresses using `spm_compress::decompress_reader()`, extracts `./control` or `control` from tar
- `parse_control_file()` — RFC 822 parser handling continuation lines (leading space/tab)
- `DebMetadata` struct with ordered `fields: Vec<(String, String)>` and case-insensitive `get()` lookup
- 8 new tests including roundtrip tests (build with `DebBuilder`, read back, verify)

### spm-cli: Enhanced build & plan commands
- **`--format all|rpm|deb`** (default: `all`): builds/plans both formats when `all`
- **`--no-split`**: disables package splitting (`config.splitting.enabled = false`)
- **`--source-date-epoch`**: CLI flag > `SOURCE_DATE_EPOCH` env var > config file value
- **`--target-distro`**: parses with `Distro::from_str()`, prints compatibility warnings
- **`--compression`**, **`--compression-level`**, **`--threads`**: override compression config
- Shared `apply_overrides()` helper applies all overrides to a cloned `Config`
- Enhanced `cmd_plan()`: minimum version display (`minimum_rpm_version()` / `minimum_dpkg_version()`), compatibility warnings, `--format all` shows both formats with section headers
- Enhanced `cmd_build()`: `--format all` builds RPM then DEB sequentially with separate plans (different `FormatLimits`)

### spm-cli: `spm inspect` command
- New `Commands::Inspect { path: PathBuf }` subcommand
- Auto-detects format from file extension (`.rpm` / `.deb`)
- RPM: displays structured output (Package, Version, Release, Architecture, Installed Size, Description, License, URL, Vendor, Packager, Compression, Files, Requires)
- DEB: displays all control fields in order as `Key: Value`
- Unknown extensions produce a clear error

### spm-cli: Progress spinners
- Added `indicatif = "0.17"` dependency
- `make_spinner(quiet)` helper: returns `ProgressBar::hidden()` when quiet, otherwise a green spinner with 100ms tick
- RPM builds: per-sub-package spinner with filename, finishes with file size
- DEB builds: single spinner for all DEB packages
- `--quiet` flag suppresses all spinner output

## Design Decisions

- **Decompression centralized in spm-compress.** Both RPM and DEB readers need to decompress payloads. Rather than each crate depending on `zstd`/`flate2`/`xz2` directly, decompression is routed through `spm_compress::decompress_reader()`.
- **RPM reader parses headers from scratch.** The existing `header.rs` (builder) documents the exact binary format. The reader mirrors this layout in reverse rather than using an external crate.
- **8-byte alignment after signature header.** RPM requires the metadata header to start on an 8-byte boundary. The reader computes `(8 - (total_sig_bytes % 8)) % 8` padding, matching the builder's pattern.
- **`--format all` as default.** Most users want both RPM and DEB. Each format gets its own `Planner::plan()` call since RPM and DEB have different `FormatLimits`.
- **DEB reader uses `tar` crate for control.tar parsing.** The control.tar inside a DEB is a standard tar archive, so the `tar` crate (already a dependency of spm-deb) is reused.

## Testing

- **spm-compress**: 18 tests (14 + 4 decompress reader tests)
- **spm-core**: 84 tests (unchanged)
- **spm-cpio**: 13 tests (unchanged)
- **spm-deb**: 56 tests (48 + 8 reader tests)
- **spm-rpm**: 54 tests (44 + 10 reader tests)
- **Total**: 226 tests (up from 204)

```bash
cargo test --workspace          # 226 tests pass
cargo fmt --all -- --check      # no formatting issues
cargo clippy --all-targets      # no warnings
```
