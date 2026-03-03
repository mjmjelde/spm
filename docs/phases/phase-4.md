# Phase 4: DEB Backend & Auto-Split

## What Was Implemented

### spm-deb crate
- **ar writer** (`ar.rs`): `ArWriter<W: Write>` with in-memory and streaming APIs, 60-byte member headers (`!<arch>\n` global magic, `\x60\n` per-member magic), even-byte padding, reproducible timestamps via `source_date_epoch`. Validates member sizes at write time — rejects members exceeding 9,999,999,999 bytes (~9.3 GiB) with a clear error, preventing silent archive corruption from the 10-digit decimal size field overflowing
- **Control file generator** (`control.rs`): RFC 822 format output with Package, Version, Architecture (x86_64→amd64 translation), Maintainer, Installed-Size, Section, Priority, Depends (common + deb-specific + extra), Conflicts, Provides, Replaces, Homepage, Description, and custom fields from `DebOverrides.fields`
- **md5sums generation**: Two paths — `generate_md5sums()` reads source files (fallback), `generate_md5sums_precomputed()` formats pre-computed hashes from the data tar write pass (primary, no file I/O)
- **Inline MD5 hashing**: `HashingReader<R: Read>` wrapper computes MD5 digests as data flows through to the tar builder during `write_tar_entry()`, eliminating a separate file-reading pass for md5sums. For large packages (e.g. 24.3 GiB / 628K files), this saves ~400s that would otherwise be spent re-reading files from cold page cache after streaming compression evicts them
- **conffiles detection**: Files with `is_config: true` written to `conffiles` control file
- **data.tar generation**: Uses `tar` crate with GNU format, compressed via `spm_compress`. `write_data_tar()` returns pre-computed MD5 hashes alongside the temp file and size
- **control.tar generation**: Contains `control`, `md5sums`, `conffiles`, and scripts (`preinst`, `postinst`, `prerm`, `postrm`). Accepts optional `md5_map` — when provided, uses pre-computed hashes; falls back to file-reading when `None`
- **DEB assembly**: `debian-binary` (`"2.0\n"`) + `control.tar.{zst,gz}` + `data.tar.{zst,gz}` packaged in ar archive
- **Auto-split (estimated)**: Meta-package with `Depends:` on all parts (version-pinned), each part as separate DEB. Uses even-parts sizing (divides total size equally across parts) with 80% safety factor on format limits
- **Auto-split (deferred/streaming)**: `build_streaming_split()` streams all files through tar → compressor → `CountingWriter` → temp file, splitting when actual compressed output reaches 95% of the ar member limit. `CountingWriter<W: Write>` tracks compressed bytes via `AtomicU64` for real-time threshold monitoring without flushing the compressor. MD5 hashes collected inline per-part via `HashingReader`
- **DEB-specific compression override**: `deb.compression` config field allows different compression than RPM
- **Reproducible builds**: `source_date_epoch` applied to ar headers, tar entry mtimes, control metadata

### spm-cli integration
- `spm build --format deb` subcommand with `FormatLimits::deb()` planning
- `spm plan --format deb` for DEB-specific planning output

## Design Decisions

- **`tar` crate for data.tar.** Unlike RPM (which needs custom cpio), DEB's data.tar is a standard tar archive. The `tar` crate handles GNU format correctly, including long paths.
- **ar writer from scratch.** The ar format is simple enough (8-byte global magic + 60-byte member headers) that a custom writer avoids an unnecessary dependency and gives full control over reproducible timestamps.
- **md5sums, not SHA-256.** DEB policy requires MD5 checksums in the `md5sums` file. This is a legacy format requirement, not a security boundary.
- **Inline MD5 via `HashingReader`.** Computing MD5 during the tar write pass (as data is already being read for the archive) eliminates a second full read of all source files. This is critical for large packages where the initial streaming pass evicts files from the OS page cache.
- **Architecture translation.** RPM uses `x86_64`, DEB uses `amd64`. Translation happens in `PackageFileName::deb()` and control file generation.
- **Meta-package for splits.** When splitting, the parent package becomes a meta-package with only `Depends:` lines pointing to versioned sub-packages, matching Debian conventions.
- **Deferred split with compressed-size monitoring.** Rather than estimating compressed sizes from ratios, the streaming split path monitors actual compressed output in real time via `CountingWriter` and splits at 95% of the ar member limit, producing accurately-sized parts.

## Testing

- **spm-deb**: 63 tests (ar: 14, control: 17, builder: 24, reader: 8)
- **End-to-end DEB validation**: Built DEB passes `dpkg-deb -I` (control info), `dpkg-deb -c` (file listing), `dpkg -i` (install), `dpkg -r` (remove), `dpkg --purge` (purge). Scripts, conffiles, dependencies, and metadata all verified.

```bash
cargo test --workspace          # 259 tests pass
cargo fmt --all -- --check      # no formatting issues
cargo clippy --all-targets      # no warnings
```
