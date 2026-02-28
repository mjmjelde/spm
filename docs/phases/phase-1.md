# Phase 1: File Tree Walking & Package Planning

## What Was Implemented

- File tree walker (`filetree.rs`) that walks file mappings, applies glob patterns from config `content.files`, and produces a sorted, deduplicated list of `FileEntry` values
- Glob expansion using `walkdir` + `glob::Pattern` for recursive `**` patterns (the `glob::glob()` function only matches directories with trailing `/**`, not files)
- Bare directory `src` paths auto-expand to `dir/**` for recursive inclusion
- Hardlink detection via `(dev, ino)` tracking with `MetadataExt`
- Implicit parent directory generation for all file entries
- Mode/user/group override from file mapping config
- Config file flag (`type: config`) support
- Symlink and directory entries from config sections

- Package planner (`planner.rs`) that produces a `PackagePlan` from config + format limits:
  - Calculates total uncompressed size
  - Detects extended cpio need (any file > 4 GiB for RPM)
  - Three split strategies: `auto` (format-aware), `size` (explicit limit), `directory` (path-based)
  - Meta-package generation for split packages (scripts go on meta, not parts)
  - Hardlink fixup across split boundaries (converts to regular files)

- Alternatives scriptlet generation (`alternatives.rs`):
  - `update-alternatives --install` with `--slave` follower support
  - `update-alternatives --remove` with `$1` guard (`"0"` for RPM, `"remove"` for DEB)
  - Script resolution: loads user scripts from disk, injects alternatives before user's `post_install` and after user's `pre_remove`

- Shared types (`types.rs`):
  - `FormatLimits` with `rpm()` and `deb()` constructors
  - `parse_size()` for human-readable sizes ("8GiB", "500MiB", etc.)
  - `format_size()` for display
  - `PackageFileName` for RPM/DEB output filenames with arch translation

- CLI `spm plan` command:
  - `--format rpm|deb` flag
  - Shows file count, sizes, compression estimate, cpio format, split status
  - Comma-formatted file counts, human-readable sizes

## Design Decisions

- **`walkdir` + `glob::Pattern` for recursive globs.** The `glob::glob()` function with `dir/**` only matches directories, not files within them. Using `walkdir` for traversal and `glob::Pattern::matches_path_with()` for filtering gives correct recursive behavior.
- **Compression ratio estimation uses a fixed heuristic (0.35 for zstd).** Actual compression isn't available in Phase 1; the planner estimates compressed sizes for split decisions. A safety factor of 0.80 (`AUTO_SPLIT_HEADROOM`) is applied to format limits to account for estimation error up to ±20%. When splitting is triggered, parts are sized evenly rather than greedily filled.
- **Meta-package gets all scripts; parts get none.** Per spec.md Section 4.3, this ensures alternatives registration and user scripts run once during install/remove, not per-part.
- **Combined `$1` guard in pre-remove.** Both RPM (`"0"`) and DEB (`"remove"`) conditions are checked in a single `if` statement for simplicity.
- **`plan_from_entries()` for testability.** A separate method accepts pre-built file entries, allowing planner tests to run without a real filesystem.

## Testing

62 unit tests covering:
- `types.rs`: size parsing/formatting, format limits, filename generation
- `filetree.rs`: directory walking, glob expansion, mode overrides, config files, symlinks, directories, hardlinks, implicit parents, deterministic ordering
- `alternatives.rs`: install/remove scriptlets, follower syntax, empty cases, script resolution ordering
- `planner.rs`: no-split, auto-split (even-parts sizing, borderline detection), size-split, directory-split, extended cpio detection, splitting disabled, meta-package scripts, total size calculation, build warnings

```bash
# Run all tests
cargo test -p spm-core

# CLI smoke test
mkdir -p /tmp/test-pkg/bin /tmp/test-pkg/lib
echo '#!/bin/bash' > /tmp/test-pkg/bin/hello && chmod 755 /tmp/test-pkg/bin/hello
dd if=/dev/zero of=/tmp/test-pkg/lib/bigfile bs=1M count=100 2>/dev/null

spm plan --config tests/fixtures/minimal.yaml --format rpm
spm plan --config tests/fixtures/minimal.yaml --format deb
```
