# Phase 4: DEB Backend & Auto-Split

## What Was Implemented

### spm-deb crate
- **ar writer** (`ar.rs`): `ArWriter<W: Write>` with in-memory and streaming APIs, 60-byte member headers (`!<arch>\n` global magic, `\x60\n` per-member magic), even-byte padding, reproducible timestamps via `source_date_epoch`
- **Control file generator** (`control.rs`): RFC 822 format output with Package, Version, Architecture (x86_64→amd64 translation), Maintainer, Installed-Size, Section, Priority, Depends (common + deb-specific + extra), Conflicts, Provides, Replaces, Homepage, Description, and custom fields from `DebOverrides.fields`
- **md5sums generation**: SHA-256 digests replaced with MD5 for DEB-standard `md5sums` control file
- **conffiles detection**: Files with `is_config: true` written to `conffiles` control file
- **data.tar generation**: Uses `tar` crate with GNU format, compressed via `spm_compress`
- **control.tar generation**: Contains `control`, `md5sums`, `conffiles`, and scripts (`preinst`, `postinst`, `prerm`, `postrm`)
- **DEB assembly**: `debian-binary` (`"2.0\n"`) + `control.tar.{zst,gz}` + `data.tar.{zst,gz}` packaged in ar archive
- **Auto-split**: Meta-package with `Depends:` on all parts (version-pinned), each part as separate DEB
- **DEB-specific compression override**: `deb.compression` config field allows different compression than RPM
- **Reproducible builds**: `source_date_epoch` applied to ar headers, tar entry mtimes, control metadata

### spm-cli integration
- `spm build --format deb` subcommand with `FormatLimits::deb()` planning
- `spm plan --format deb` for DEB-specific planning output

## Design Decisions

- **`tar` crate for data.tar.** Unlike RPM (which needs custom cpio), DEB's data.tar is a standard tar archive. The `tar` crate handles GNU format correctly, including long paths.
- **ar writer from scratch.** The ar format is simple enough (8-byte global magic + 60-byte member headers) that a custom writer avoids an unnecessary dependency and gives full control over reproducible timestamps.
- **md5sums, not SHA-256.** DEB policy requires MD5 checksums in the `md5sums` file. This is a legacy format requirement, not a security boundary.
- **Architecture translation.** RPM uses `x86_64`, DEB uses `amd64`. Translation happens in `PackageFileName::deb()` and control file generation.
- **Meta-package for splits.** When splitting, the parent package becomes a meta-package with only `Depends:` lines pointing to versioned sub-packages, matching Debian conventions.

## Testing

- **spm-deb**: 48 tests (ar: 12, control: 16, builder: 20)
- **End-to-end DEB validation**: Built DEB passes `dpkg-deb -I` (control info), `dpkg-deb -c` (file listing), `dpkg -i` (install), `dpkg -r` (remove), `dpkg --purge` (purge). Scripts, conffiles, dependencies, and metadata all verified.

```bash
cargo test --workspace          # 176 tests pass
cargo fmt --all -- --check      # no formatting issues
cargo clippy --all-targets      # no warnings in phase-4 crates
```
