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

### Deep Analysis: Correctness, Security, and Compliance Fixes

Comprehensive codebase audit identified ~60 issues; 25 fixes implemented with 10 new tests (226 → 236 total).

**Correctness-critical:**
- Fixed `split_by_size` omitting directory entries from parts 2+ — all parts now include parent
  directory entries for their files
- Fixed `parse_mode("0000")` producing an error (empty string after zero-trim)
- Fixed aarch64 architecture number in RPM lead (was 12/armv7, now 19)
- Added `FinishableWriter` to spm-compress with explicit `finish()` — gzip/xz finalization
  errors were silently discarded by `Drop`
- Fixed hardlink inode/nlink handling in CPIO — entries now share inodes and have correct nlink
  via `InodeMap`

**Security & robustness:**
- Added POSIX shell escaping (`shell_escape()`) for all path arguments in alternatives
  scriptlets — prevents shell injection from paths with spaces or metacharacters
- Added ar header name field overflow protection (name + "/" must fit in 16 bytes)
- Added Year-2038 timestamp clamping (`clamp_timestamp()`) for RPM INT32 fields
- Added RPM reader allocation limits (100K index entries, 64 MiB data) to prevent DoS from
  crafted packages

**Format compliance:**
- Fixed self-provides to use `sub_package.name` and plan version/release (was using config name)
- Fixed multi-line DEB Description formatting per Debian policy (continuation lines with space
  prefix, empty lines as ` .`)
- Added conffiles path validation (must start with `/`)
- Added `rpmlib(PayloadIsXz)` auto-dependency when using XZ compression
- Changed SOURCERPM from empty string to `"(none)"` per RPM convention
- Changed symlink target handling to error on non-UTF-8 instead of `to_string_lossy()`

**Code quality:**
- Fixed glob errors silently swallowed (now propagated as `FileTreeError`)
- Deduplicated DEB arch mapping to `spm_core::types::deb_arch()`
- Added zero-size safety in `fixup_hardlinks_across_parts`
- Added header data offset overflow check (`> i32::MAX`)
- Added package name/version format validation at config load time

### Deep Analysis: Correctness and Quality Improvements (Round 2)

Codebase-wide audit across all 6 crates; 12 fixes applied (236 total tests, all passing).

**Correctness-critical:**
- Fixed directory-split path prefix matching — was using string `starts_with()` which caused
  `/opt/pkg` to incorrectly match `/opt/pkg2/file`; now uses `Path::starts_with()` for
  component-aware matching
- Added empty symlink target validation in `filetree.rs` — symlinks with empty `src` now
  produce a clear `InvalidMapping` error instead of silently creating broken entries

**CLI improvements:**
- Removed unused `--verbose` (`-v`) flag that was accepted but ignored
- `apply_overrides()` now validates compression algorithm early via `Algorithm::from_str()`,
  so `spm plan` catches invalid `--compression` values (not just `spm build`)
- Added `spm-compress` as direct dependency of `spm-cli` for early validation
- Fixed plan output path from `out/` to `./out/` for consistency with build default

**Code quality:**
- Replaced `Mutex` with `RefCell` in `IndicatifProgress` — single-threaded code doesn't need
  mutex overhead or poisoning risk
- Removed `eprintln!()` from RPM library code (`clamp_timestamp()`) — library crates should
  not write to stderr
- Replaced `panic!()` with `unreachable!()` in test helpers for clearer intent
- Narrowed `#[allow(dead_code)]` to specific unused `ParsedTagValue` variants
- Added clarifying comment on hardlink size adjustment in `fixup_hardlinks_across_parts()`
- Eliminated unnecessary `.clone()` in `split_by_directory()` by restructuring to move entries

### Fix: Auto-split trailing runt part

- Fixed `split_by_size` producing a trivially small trailing part when greedy bin-packing
  with floor-divided per-part budgets leaves a tiny remainder (e.g. a single 495-byte file
  in its own multi-GiB package). Trailing parts under 2% of the per-part target are now
  merged back into the previous part.
- 4 new tests (248 total)

### Deep Analysis: Package Splitting Logic (Round 3)

Focused audit of the package splitting subsystem across planner, builders, and config validation;
3 bugs fixed, 3 config validation gaps closed, 18 new tests (244 total).

**Bugs fixed:**
- Fixed `split_by_directory` not injecting ancestor directory entries into parts — parts could
  be missing `/opt`, `/opt/pkg`, etc., causing package manager warnings or incorrect permissions.
  Extracted shared `inject_ancestor_dirs()` helper used by both `split_by_size` and
  `split_by_directory`.
- Fixed RPM meta-package missing `Requires` on part sub-packages — DEB correctly injected
  `Depends` for meta-packages, but RPM's `add_dependencies()` did not, so installing an RPM
  meta-package would not pull in its parts. Now injects `Requires: {part} = {version}-{release}`
  with `RPMSENSE_EQUAL` flags.
- Fixed `split_by_directory` producing empty sub-packages when configured paths matched no files —
  empty parts are now filtered out before building sub-packages.

**Config validation improvements:**
- `strategy: size` now requires `max_size` — previously silently defaulted to `"4GiB"`
- `strategy: directory` now requires non-empty `parts` — previously silently treated as standalone
- `max_size` format validated at config time via `parse_size()` — previously deferred to planning

**Test improvements:**
- Strengthened 3 weak assertions (changed `>=` to exact part counts)
- Added tests: cross-part hardlink promotion, same-part hardlink preservation, file distribution
  integrity, directory-based split ancestor directory injection, empty part filtering, 3 config
  validation tests

### DEB Streaming Split & Performance

- Implemented monitored streaming split for DEB auto-split (`build_streaming_split`):
  streams all files through tar → compressor → `CountingWriter` → temp file, splitting when
  actual compressed output reaches 95% of the ar member limit. Replaces the old estimated
  compression ratio approach with exact compressed-size monitoring via `AtomicU64` counter.
- Added `HashingReader<R>` wrapper that computes MD5 hashes inline during the tar write pass,
  eliminating a second full read of all source files for `md5sums` generation. For the 24.3 GiB
  MATLAB package (628K files), this saves ~400s that was previously spent re-reading files from
  cold page cache after streaming compression evicted them.
- Added `generate_md5sums_precomputed()` in `control.rs` — formats pre-computed hashes without
  any file I/O, used when `md5_map` is available from the data tar write pass.
- `write_tar_entry()` now returns `Option<String>` (MD5 hex digest for regular files) and
  `write_data_tar()` returns a `HashMap<PathBuf, String>` of pre-computed hashes alongside the
  temp file and size.
- `write_control_tar()` accepts optional `md5_map` parameter — when provided, uses pre-computed
  hashes; falls back to `generate_md5sums()` (file-reading) when `None`.

### Deep Analysis Round 3: Correctness, Security, and Compliance Fixes

Comprehensive audit across all 6 crates; 20 fixes applied across 8 files.

**Critical:**
- Fixed RPM header region tag index sort — region tags (62/63) now sorted after data tags to
  maintain monotonically non-decreasing offset order required by `hdrblobVerifyInfo()`

**High:**
- Fixed zstd `finish()` — changed from `AutoFinishEncoder` (which silently discards errors on
  drop) to raw `Encoder` with explicit `finish()` for proper error propagation
- Added bounds check for `sub_packages[0]` access in `build_streaming_split`
- Handle empty `parts` in `build_streaming_split` — all-directory packages now produce a valid
  single .deb instead of a broken empty meta-package
- Added bounds checks to RPM `skip_header_section` (matching `parse_header_section` limits)

**Medium:**
- Moved DEB extra fields before Description (Debian policy §5.6.13 compliance)
- DEB `write_tar_entry` now uses `entry.user`/`entry.group` instead of hardcoded "root"
- DEB reader uses streaming `BufReader` with `Seek` instead of loading entire file into memory
- RPM reader parses index offset as `i32` (region tags have negative offsets per spec)
- Added path traversal validation — `dst` fields must be absolute with no `..` components
- Added duplicate tag detection in RPM `HeaderBuilder`
- Fixed `build_warnings` for deferred-split case (no longer warns "consider splitting" when
  splitting is already active)
- RPM `parse_dependency` now handles `==` operator (was silently treating as "any version")
- Compression level validation per algorithm (negative levels no longer wrap to huge `u32`)
- RPM builder uses `symlink_metadata` for symlink mtime (was following symlinks)
- Special files (devices, sockets, pipes) in glob results now emit warnings

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
