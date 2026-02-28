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

### Phase 3: CPIO Writer & RPM Backend

- Created `spm-cpio` crate with Newc (`070701`) and Extended (`07070X`) CPIO format writers
- Created `spm-rpm` crate with full RPM v4 package builder
- Implemented RPM lead (96 bytes), header builder (sorted tags, alignment, network byte order),
  signature header (MD5, SHA-256, payload size), and metadata header (package, file, dependency,
  script tags)
- Build pipeline: file digests → cpio|compress → temp file → header → signature → assemble
- Supports hardlink convention (all-but-last size=0, last carries data)
- Extended CPIO auto-selected when any file exceeds 4 GiB standard limit
- Added `spm build --format rpm` to CLI
- Validated with `rpm -qpl`, `rpm -qi -p`, `rpm -K`, and `rpm -ivh` on Fedora 40 container
- 48 unit tests across spm-cpio (13) and spm-rpm (35)

### Phase 4: DEB Backend & Auto-Split

- Created `spm-deb` crate with ar archive writer, control file generation, and DEB build pipeline
- Implemented `ArWriter<W>` with in-memory and streaming APIs, 60-byte member headers,
  even-byte padding
- Implemented control file generation: Package, Version, Architecture, Maintainer, Installed-Size,
  Section, Priority, Depends (common + deb-specific + extra), Conflicts, Provides, Replaces,
  Homepage, Description, custom fields from `DebOverrides.fields`
- Implemented md5sums generation for all regular files
- Implemented conffiles detection for `is_config` entries
- Implemented data.tar generation using `tar` crate with GNU format, compressed via spm-compress
- Implemented control.tar generation with control, md5sums, conffiles, and scripts
  (preinst, postinst, prerm, postrm)
- DEB assembly: `debian-binary` ("2.0\n") + `control.tar.{zst,gz}` + `data.tar.{zst,gz}` in ar
- Auto-split: meta-package with `Depends:` on all parts (version-pinned), each part as separate DEB
- DEB-specific compression override via `deb.compression` config field
- Reproducible builds: `source_date_epoch` applied to ar headers, tar entries, control metadata
- Added `spm build --format deb` to CLI with `FormatLimits::deb()` planning
- Validated with `dpkg-deb -I`, `dpkg-deb -c`, `dpkg -i`, `dpkg -r`, `dpkg --purge` on
  Ubuntu 24.04 container — scripts, conffiles, dependencies, metadata all verified
- 48 unit tests (ar: 12, control: 16, builder: 20)

### Phase 5a: Library Hardening & Distro Compatibility

- Added XZ compression support to `spm-compress` (single-threaded and multi-threaded via `xz2`)
- Created `distro` module in `spm-core` with compile-time database of known Linux distributions
  (EL8, EL9, Ubuntu 20.04/22.04/24.04, Fedora) and their packaging capabilities
- Added `check_compatibility()` for format/compression/large-file compatibility warnings
- Added `minimum_rpm_version()` and `minimum_dpkg_version()` for `spm plan` output
- Filled RPM builder gaps: LONGFILESIZES (tag 5008) for 64-bit per-file sizes, LONGSIZE (5009) for
  total size, PAYLOADFORMAT/PAYLOADCOMPRESSOR/PAYLOADFLAGS tags, rpmlib(PayloadIsZstd) dependency
- Made `Config` and sub-structs `Clone`-able for CLI override pattern
- Added `BuildConfig` for `source_date_epoch` support
- Enhanced config validation: reject negative compression levels, unknown algorithms at parse time
- Improved error messages with source file paths and context
- 28 new tests (204 total)

### Phase 5b: CLI Integration — Build Flags, Inspect, Spinners

- Added `decompress_reader()` to `spm-compress` for unified decompression (Zstd, Gzip, Xz, None)
- Created RPM metadata reader (`spm-rpm/src/reader.rs`): parses lead, signature header (with 8-byte
  alignment padding), and metadata header; extracts all key tags (NAME, VERSION, RELEASE, ARCH, SIZE,
  LONGSIZE, DESCRIPTION, LICENSE, URL, VENDOR, PACKAGER, PAYLOADCOMPRESSOR, BASENAMES, REQUIRENAME)
- Created DEB metadata reader (`spm-deb/src/reader.rs`): parses ar archive, walks members to find
  `control.tar.{zst,gz,xz}`, decompresses with `spm_compress::decompress_reader()`, extracts
  `./control` from tar, parses RFC 822 control fields with continuation line support
- Expanded `spm build` with `--format all|rpm|deb` (default: all), `--no-split`,
  `--source-date-epoch`, `--target-distro`, `--compression`, `--compression-level`, `--threads`
- Expanded `spm plan` with the same override flags, minimum version display, and distro warnings
- `--source-date-epoch` priority: CLI flag > `SOURCE_DATE_EPOCH` env var > config file value
- Added `spm inspect <path>` subcommand: auto-detects `.rpm` or `.deb` by extension, displays
  package metadata using the new readers
- Added per-package progress spinners via `indicatif` with `--quiet` suppression
- 22 new tests (226 total)
