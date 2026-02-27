# Phase 2: Compression Engine

## What Was Implemented

- New `spm-compress` crate providing a streaming compression abstraction
- `Algorithm` enum: `Zstd`, `Gzip`, `Xz`, `None` with metadata methods (`extension()`, `rpm_tag()`, `estimated_ratio()`)
- `CompressorConfig` struct with algorithm, level, and thread count (0 = auto-detect via `num_cpus`)
- `compress_writer()` function returning `Box<dyn Write>` for streaming compression:
  - **Zstd**: Multi-threaded via `zstd::stream::Encoder` with `multithread()` and `auto_finish()` for drop-based flushing
  - **Gzip**: `flate2::write::GzEncoder` (single-threaded, `threads` config ignored)
  - **Xz**: Stubbed with `CompressError::Unsupported` (deferred to Phase 5)
  - **None**: Passthrough — boxes the output writer directly
- `CompressError` enum with `Io` and `Unsupported` variants

## Design Decisions

- **`spm-compress` has no dependency on `spm-core`.** The `Algorithm` enum and `CompressorConfig` are independent of the YAML-facing `CompressionConfig` in spm-core. Bridging happens in downstream crates (Phase 3+). This keeps the dependency graph clean.
- **`Algorithm::from_str` is an inherent method, not a `FromStr` trait impl.** The error type is `CompressError`, not a parsing-specific error. Matches the IMPLEMENTATION.md specification. A clippy allow attribute suppresses the `should_implement_trait` lint.
- **`auto_finish()` for zstd.** Since `compress_writer` returns `Box<dyn Write>`, callers can't call `finish()`. The `AutoFinishEncoder` flushes on drop, making the streaming pattern work correctly.
- **Gzip ignores thread count.** `flate2` doesn't support parallel compression. This is documented behavior, not an error.
- **`zstdmt` feature flag required.** The `multithread()` method on `zstd::stream::Encoder` is gated behind the `zstdmt` cargo feature.
- **`estimated_ratio()` duplicated from spm-core.** Both spm-core's `estimated_compression_ratio(&str)` and spm-compress's `Algorithm::estimated_ratio()` return the same values. This avoids a circular dependency.

## Testing

12 unit tests + 1 doc-test:

- Round-trip tests: zstd, gzip (compress → decompress → verify identical, assert compression ratio)
- Varied data round-trip: sequential u32s through zstd (non-trivially compressible)
- None passthrough: output equals input byte-for-byte
- Multi-threading: zstd with 4 threads on 10 MB, no panic
- Auto-thread detection: threads=0 resolves to CPU count
- Empty input: zstd and gzip produce valid streams for zero-length input
- Xz unsupported: returns `CompressError::Unsupported`
- Algorithm enum: `from_str`, `extension`, `rpm_tag` methods

```bash
cargo test -p spm-compress
```
