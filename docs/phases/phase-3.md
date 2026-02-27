# Phase 3: CPIO Writer & RPM Backend

## What Was Implemented

### spm-cpio crate
- `CpioFormat` enum: `Newc` (070701), `Extended` (07070X)
- `CpioWriter<W: Write>` — sequential entry writer tracking bytes_written and entry_count
- `CpioMetadata` struct with ino, mode, uid, gid, nlink, mtime, filesize (u64), devmajor/minor, rdevmajor/minor
- `CpioError` enum: `Io`, `FileTooLarge`
- Newc format: 110-byte header, "070701" magic, 8-char hex fields, 4-byte padding, TRAILER!!! sentinel
- Extended format: 14-byte header, "07070X" magic + 8-char hex index, no filesize limit
- Hardlink handling via caller convention (all-but-last link has filesize=0)
- `finish()` returns `(W, u64)` — inner writer + uncompressed bytes written

### spm-rpm crate
- **Lead** (`lead.rs`): 96-byte RPM lead writer with `write_lead()` and `arch_to_num()`
- **Tags** (`tags.rs`): All RPM tag constants — package metadata, file metadata, dependencies, scripts, signature, region tags, flag constants
- **Header** (`header.rs`): `HeaderBuilder` with `add_string`, `add_string_array`, `add_i18n_string`, `add_int32`, `add_int64`, `add_int16`, `add_bin`, `add_region_tag`. Data section ordered by tag number with region tags at end. Alignment rules enforced (INT16→2, INT32→4, INT64→8).
- **Signature** (`signature.rs`): `build_signature()` computes MD5 (header+payload), SHA-1 (header-only), SHA-256 (header-only), size tags, and region tag
- **Builder** (`builder.rs`): `RpmBuilder::build()` orchestrates the full pipeline — file digests, CPIO payload, metadata header, signature header, RPM assembly. Includes path decomposition (BASENAMES/DIRNAMES/DIRINDEXES), dependency handling, script tags, and `./` prefix for Newc cpio.

### spm-cli integration
- `spm build --config <path> --format rpm --output <dir>` subcommand
- Loads config, runs planner, builds each sub-package as a separate RPM

## Design Decisions

- **RPM built from scratch** (not using the `rpm` crate v0.18). The `rpm` crate doesn't support 07070X extended cpio, and we need precise control over 64-bit tag variants (LONGFILESIZES, LONGSIZE, etc.).
- **Data section ordered by tag number.** RPM's `hdrblobVerifyInfo()` iterates sorted index entries checking that data offsets are monotonically increasing. Region tags (62, 63) are an exception — their trailer data goes at the end.
- **Region tags included.** Tags 62 (HEADERSIGNATURES) and 63 (HEADERIMMUTABLE) mark the header as RPM v4, avoiding the "v3 packages are deprecated" warning.
- **SHA-1 included alongside SHA-256.** RPM 4.19 verifies both `Header SHA256 digest` and `Header SHA1 digest`. The sha1 crate is added as a dependency.
- **Two-pass file reading.** Files are read once for SHA-256 digest computation and once for CPIO payload writing. Simple and correct; optimization deferred.
- **Temp file for payload.** Compressed payload is written to a `NamedTempFile` so we can measure its size before building the signature header.

## Testing

- **spm-cpio**: 13 tests (magic bytes, padding, trailer, roundtrip, large file rejection, hardlinks, extended format)
- **spm-rpm**: 35 tests (lead structure, header builder alignment/sorting/types, path decomposition, file mode bits, dependencies, signature digests, end-to-end build)
- **End-to-end RPM validation**: Built RPM passes `rpm -K` (digests OK), `rpm -qpl` (file listing), `rpm -qi -p` (metadata), `rpm2cpio | cpio` (file extraction)

```bash
cargo test --workspace          # 128 tests pass
cargo fmt --all -- --check      # no formatting issues
cargo clippy --all-targets      # no warnings in phase-3 crates
```
