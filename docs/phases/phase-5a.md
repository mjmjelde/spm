# Phase 5a: Library Hardening & Distro Compatibility

## What Was Implemented

### spm-compress: XZ support
- Implemented XZ compression via `xz2` crate (single-threaded with `XzEncoder::new`, multi-threaded with `MtStreamBuilder`)
- Added `xz2 = "0.1"` dependency
- 3 new tests: single-thread roundtrip, multi-threaded roundtrip, empty input

### spm-core: Distro compatibility database
- New `distro.rs` module with compile-time database of known Linux distributions
- `Distro` enum: `El8`, `El9`, `Ubuntu2004`, `Ubuntu2204`, `Ubuntu2404`, `Fedora`
- `RpmDistroInfo` / `DebDistroInfo` structs with version, zstd support, large file support, alternatives dependency package name
- `Distro::from_str()` — parse CLI identifiers (`"el8"`, `"rhel8"`, `"ubuntu2204"`, etc.)
- `check_compatibility()` — validates format/compression/large-file compatibility, returns warning messages
- `minimum_rpm_version()` — returns `(version, reason)` based on compression algorithm and large file presence
- `minimum_dpkg_version()` — returns `(version, reason)` based on compression algorithm
- 13 new tests for parsing, info lookup, compatibility checks, minimum version logic

### spm-rpm: Builder gaps filled
- Added LONGFILESIZES tag (5008) for 64-bit per-file sizes alongside existing FILESIZES
- Added LONGSIZE tag (5009) for 64-bit total installed size alongside existing SIZE
- Added PAYLOADFORMAT (`"cpio"`), PAYLOADCOMPRESSOR, PAYLOADFLAGS tags to metadata header
- Added `rpmlib(PayloadIsZstd)` implicit dependency when compression is zstd
- `RpmBuilder::build()` now accepts `Option<&Distro>` for future distro-aware dependency injection

### spm-core: Config improvements
- Derived `Clone` on `Config` and all sub-structs (needed for CLI override pattern)
- Added `BuildConfig` struct with `source_date_epoch: Option<String>` field
- Enhanced config validation: reject negative compression levels, validate algorithm at parse time
- Improved error messages with source file paths and context

## Design Decisions

- **XZ multi-threading via `MtStreamBuilder`.** When `threads > 1`, XZ compression uses liblzma's built-in multi-threaded encoder. Falls back to single-threaded for `threads = 1`.
- **Compile-time distro database rather than runtime detection.** The `--target-distro` flag explicitly selects a target. This avoids autodetection ambiguity and works correctly when cross-building packages on a different OS.
- **`Clone` on `Config`.** CLI override flags need to mutate a config copy. Deriving Clone is simpler than threading mutable references through the entire call chain.
- **LONGFILESIZES + FILESIZES dual output.** Both tags are written to maximize compatibility — older rpm reads FILESIZES, newer rpm reads LONGFILESIZES.

## Testing

- **spm-compress**: 14 tests (12 existing + 2 new XZ tests, noting empty input is the 3rd)
- **spm-core**: 84 tests (67 existing + 17 new distro/config tests)
- **spm-rpm**: 44 tests (35 existing + 9 new builder gap tests)
- **Total**: 204 tests (up from 176)

```bash
cargo test --workspace          # 204 tests pass
cargo fmt --all -- --check      # no formatting issues
cargo clippy --all-targets      # no warnings
```
