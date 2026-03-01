# `spm` — Large-File-Aware Linux Package Builder

**Version:** 0.1.0-draft
**Author:** Matthew (Texas A&M University)
**Date:** 2026-02-26
**Language:** Rust

---

## 1. Problem Statement

Packaging large vendor software (e.g., MATLAB, Intel compilers, NVIDIA CUDA toolkits, EDA tools) for enterprise Linux deployment is painful. These installations routinely reach 20–50+ GB and hit hard limits in both RPM and DEB package formats:

| Format | Limitation | Root Cause |
|--------|-----------|------------|
| DEB | ~9,536 MiB max per ar member | ar header's 10-digit ASCII decimal size field |
| DEB | ~8 GiB max per tar entry | 11-digit ASCII octal size field in v7/ustar tar |
| RPM (< 4.6) | 2 GB / 4 GB total payload | 32-bit cpio header fields |
| RPM (< 4.12) | 4 GB per individual file | Standard SVR4 cpio `c_filesize` is 8 hex digits (32-bit) |

Existing tools (fpm, nfpm) do not handle these limits gracefully — they either fail silently, produce corrupt packages, or leave it to the user to manually split things up.

### Goals

1. Build valid RPM and DEB packages from a directory tree + config file
2. Handle arbitrarily large payloads by auto-splitting when format limits are reached
3. Use modern, multi-threaded compression (zstd by default)
4. Support RPM's extended cpio format (magic `07070X`) for >4 GB files
5. Pure Rust — no runtime dependency on rpmbuild, dpkg-deb, or cpio
6. Extensible architecture so additional formats (APK, pacman, etc.) can be added later

### Non-Goals (v1)

- Building software from source (this is not rpmbuild/debuild)
- Repository management (that's createrepo/aptly)
- Package format conversion (deb→rpm or vice versa)
- Source packages (SRPM, dsc)

---

## 2. Architecture Overview

```
┌──────────────────────────────────────────────────────┐
│                     CLI Frontend                      │
│              (clap-based argument parsing)             │
└────────────────────────┬─────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────┐
│                   Config Parser                       │
│            (YAML config + CLI overrides)               │
└────────────────────────┬─────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────┐
│                  Package Planner                      │
│  • Walks source directory, calculates sizes           │
│  • Determines if splitting is needed                  │
│  • Produces a PackagePlan (1 or more sub-packages)    │
└────────────────────────┬─────────────────────────────┘
                         │
                    ┌────┴────┐
                    ▼         ▼
         ┌──────────────┐  ┌──────────────┐
         │  RPM Backend  │  │  DEB Backend  │
         │               │  │               │
         │ • RPM Header  │  │ • ar archive  │
         │ • cpio/07070X │  │ • control.tar │
         │ • Compression │  │ • data.tar    │
         │ • Signing     │  │ • Signing     │
         └──────┬───────┘  └──────┬───────┘
                │                 │
                ▼                 ▼
         ┌──────────────────────────┐
         │   Compression Engine     │
         │  (zstd, xz, gzip, none) │
         │  Multi-threaded via      │
         │  zstd crate / xz2 crate │
         └──────────────────────────┘
```

### Crate Structure

```
spm/
├── Cargo.toml                  # workspace root
├── crates/
│   ├── spm-cli/           # Binary crate — CLI frontend
│   ├── spm-core/          # Config parsing, planning, shared types
│   ├── spm-rpm/           # RPM format backend
│   ├── spm-deb/           # DEB format backend
│   ├── spm-compress/      # Compression abstraction layer
│   └── spm-cpio/          # Custom cpio writer (SVR4 + 07070X extended)
└── tests/
    └── integration/            # End-to-end tests against rpm/dpkg
```

This workspace layout keeps each format backend independent. Adding a new format means adding a new `spm-<format>` crate that implements the `PackageBackend` trait.

---

## 3. Configuration Format

YAML is the primary config format (familiar to sysadmins, same as nfpm/Ansible/k8s). The config file is named `spm.yaml` by default.

### 3.1 Full Example

```yaml
package:
  name: matlab
  version: "2025a"
  release: "1"
  arch: x86_64
  license: Proprietary
  maintainer: "HPC Team <hpc-help@tamu.edu>"
  description: "MATLAB R2025a - Technical Computing Environment"
  url: https://www.mathworks.com/products/matlab.html
  vendor: MathWorks

  dependencies:
    # Common dependency syntax — translated per-format
    requires:
      - libX11
      - libXext
      - "libasound2 >= 1.1"
      - libgtk-3-0
    # Format-specific overrides
    requires_rpm:
      - mesa-libGL
    requires_deb:
      - libgl1-mesa-glx
    conflicts: []
    provides:
      - "matlab = 2025a"
    replaces: []

content:
  # Global defaults for all files (can be overridden per-mapping)
  defaults:
    user: root
    group: root
    dir_mode: "0755"
    file_mode: "0644"

  # File mapping rules (applied in order, first match wins)
  #
  # How src → dst path mapping works:
  #   src: /tmp/build/matlab/**    dst: /opt/matlab/
  #   A file at /tmp/build/matlab/bin/matlab becomes /opt/matlab/bin/matlab
  #   The src prefix before the glob is stripped and replaced with dst.
  #
  #   src: /tmp/build/matlab/       dst: /opt/matlab/
  #   A bare directory path auto-expands to /tmp/build/matlab/** — same behavior.
  #
  #   src: /tmp/build/matlab/license.txt    dst: /opt/matlab/license.txt
  #   A literal file is mapped directly to dst.
  #
  files:
    - src: "/opt/matlab-staging/R2025a/**"
      dst: /opt/matlab/R2025a/
      # Per-mapping overrides (these override the global defaults above):
      # mode: "0755"          # override file mode for everything matched
      # user: root            # override owner for everything matched
      # group: root           # override group for everything matched

    - src: matlab.desktop
      dst: /usr/share/applications/matlab-2025a.desktop
      mode: "0644"

    - src: matlab.sh
      dst: /etc/profile.d/matlab-2025a.sh
      type: config           # marks as config file (noreplace for RPM, conffile for DEB)

    # Example: software built in a staging area as non-root
    # The build user owns everything, but the package should install as root:matlab
    # Global defaults handle user/group, so you only need to specify src/dst:
    #
    # - src: "/home/builduser/matlab-build/output/**"
    #   dst: /opt/matlab/R2025a/
    #   group: matlab          # override just the group for this tree
    #   dir_mode: "0750"       # directories get tighter perms

  symlinks:
    # Static symlinks — use sparingly. For multi-version binaries, use alternatives instead.
    # - src: /opt/matlab/R2025a/bin/matlab
    #   dst: /usr/local/bin/matlab     # BAD for multi-version installs

  # update-alternatives / alternatives integration
  # Generates scriptlet snippets automatically — no manual postinst/prerm needed
  alternatives:
    - name: matlab                              # alternatives group name
      link: /usr/bin/matlab                     # the generic symlink managed by alternatives
      path: /opt/matlab/R2025a/bin/matlab       # this version's real binary
      priority: 2025                            # higher = preferred (convention: use year)
      followers:                                # secondary links that switch together
        - name: mex
          link: /usr/bin/mex
          path: /opt/matlab/R2025a/bin/mex
        - name: matlab-help
          link: /usr/bin/matlab-help
          path: /opt/matlab/R2025a/bin/matlab-help

  directories:
    - path: /var/log/matlab
      mode: "0750"
      user: root
      group: matlab
```

### 3.2 File Ownership and Permission Resolution

When building the package, spm resolves the ownership and permissions for each file using this precedence (first wins):

1. **Per-mapping override** — `user`, `group`, `mode`, or `dir_mode` set on an individual `content.files[]` entry
2. **Global defaults** — `content.defaults.user`, `group`, `file_mode`, `dir_mode`
3. **Source file metadata** — the actual uid/gid/mode from disk (only used if no default is set and no override exists)

This means you can build software anywhere, as any user, and control exactly what ownership ends up in the package:

```yaml
content:
  # Everything defaults to root:root with standard permissions
  defaults:
    user: root
    group: root
    file_mode: "0644"
    dir_mode: "0755"

  files:
    # Built by "builduser" in /tmp — doesn't matter, package installs as root:root
    - src: "/tmp/matlab-build/output/**"
      dst: /opt/matlab/R2025a/

    # Binaries need to be executable — override just the file mode
    - src: "/tmp/matlab-build/output/bin/**"
      dst: /opt/matlab/R2025a/bin/
      mode: "0755"

    # Private data directory owned by a service group
    - src: "/tmp/matlab-build/output/data/**"
      dst: /opt/matlab/R2025a/data/
      group: matlab
      dir_mode: "0750"
      mode: "0640"
```

**How `src` → `dst` path mapping works:**

| `src` pattern | File on disk | `dst` | Result install path |
|---------------|-------------|-------|-------------------|
| `/tmp/build/**` | `/tmp/build/bin/tool` | `/opt/app/` | `/opt/app/bin/tool` |
| `/tmp/build/**` | `/tmp/build/lib/libfoo.so` | `/opt/app/` | `/opt/app/lib/libfoo.so` |
| `/tmp/build/` | `/tmp/build/bin/tool` | `/opt/app/` | `/opt/app/bin/tool` |
| `/tmp/build/license.txt` | `/tmp/build/license.txt` | `/opt/app/LICENSE` | `/opt/app/LICENSE` |

For glob patterns, the prefix before the `**` is stripped and replaced with `dst`. If `src` is a bare directory path (no glob characters), it is automatically expanded to `src/**` — so `/tmp/build/` and `/tmp/build/**` are equivalent. For literal file paths, it's a direct 1:1 mapping.

**Per-mapping fields reference:**

| Field | Applies to | Default |
|-------|-----------|---------|
| `mode` | Regular files matched by this mapping | `content.defaults.file_mode`, then source |
| `dir_mode` | Directories matched by this mapping | `content.defaults.dir_mode`, then source |
| `user` | All entries matched by this mapping | `content.defaults.user`, then source |
| `group` | All entries matched by this mapping | `content.defaults.group`, then source |
| `type` | Regular files (`"config"` for conffile/noreplace) | none |
    - path: /var/log/matlab
      mode: "0750"
      user: root
      group: matlab

scripts:
  pre_install: scripts/preinst.sh
  post_install: scripts/postinst.sh
  pre_remove: scripts/prerm.sh
  post_remove: scripts/postrm.sh
  # RPM-specific:
  # pre_trans: scripts/pretrans.sh
  # post_trans: scripts/posttrans.sh

compression:
  # Compression algorithm: "zstd" (default), "xz", "gzip", "none"
  algorithm: zstd
  # Compression level (algorithm-specific)
  level: 19
  # Thread count: 0 = auto-detect (num_cpus), or explicit count
  threads: 0

splitting:
  # Enable auto-splitting for packages that exceed format limits
  enabled: true
  # Strategy: "auto" (respect format limits), "size" (explicit max), "directory" (split by path)
  strategy: auto
  # For strategy = "size": maximum uncompressed size per sub-package (REQUIRED)
  # max_size: 8GiB
  # For strategy = "directory": split boundaries (at least one part REQUIRED)
  # parts:
  #   - name: matlab-core
  #     paths: [/opt/matlab/R2025a/bin, /opt/matlab/R2025a/sys]
  #   - name: matlab-toolboxes
  #     paths: [/opt/matlab/R2025a/toolbox]

signing:
  # PGP signing (optional — can also be handled by repo tools)
  key_file: ${SPM_SIGNING_KEY}
  # key_id: ABCD1234   # optional: specific subkey

rpm:
  # RPM-specific overrides
  group: Development/Tools
  # Payload format: "cpio" (auto-selects 07070X if needed), "cpio-extended" (force 07070X)
  payload_format: cpio
  # Compression override for RPM specifically
  # compression: xz

deb:
  # DEB-specific overrides
  section: science
  priority: optional
  # Additional control fields
  fields:
    Bugs: https://hpc.tamu.edu/bugs
  # Compression override for DEB specifically
  # compression: zstd

build:
  # Reproducible builds: set a fixed timestamp
  # source_date_epoch: 1700000000
  # Or use env var:
  # source_date_epoch: ${SOURCE_DATE_EPOCH}
```

### 3.3 Config Resolution Order

1. `spm.yaml` in current directory (or `--config path`)
2. CLI flags override any config file values
3. Environment variables expand in string values (`${VAR}` syntax)

### 3.4 Name and Version Validation

spm validates `package.name` and `package.version` at config load time:

- **Name:** Must match `[a-zA-Z0-9][a-zA-Z0-9._+-]*` — starts with alphanumeric, then alphanumerics, dots, underscores, hyphens, or plus signs. This is the intersection of valid RPM and DEB package name characters.
- **Version:** Must match `[0-9][^\s:]*` — starts with a digit, contains no whitespace or colons. Colons conflict with the epoch separator in both RPM and DEB version schemes.

Invalid names/versions produce a `ConfigError::InvalidPackageName` or `ConfigError::InvalidPackageVersion` error at parse time, before any planning or building occurs.

### 3.5 Splitting Config Validation

The following constraints are enforced at config validation time (before planning):

- **`splitting.strategy`** must be one of: `auto`, `size`, `directory`
- **`splitting.max_size`** is required when `strategy` is `size` — omitting it is a validation error (no implicit default)
- **`splitting.parts`** must be non-empty when `strategy` is `directory` — an empty parts list is a validation error
- **`splitting.max_size`** format (e.g., `"4GiB"`, `"500MiB"`) is validated at config time via `parse_size()` — invalid size strings like `"8ZB"` or `""` produce an immediate error rather than failing later during planning

---

## 4. Package Planning & Auto-Split

This is the core differentiator. Before any bytes are written, `spm` walks the source tree and builds a plan.

### 4.1 Size Calculation

```
PackagePlanner::plan(config) -> Result<PackagePlan>

1. Walk file mappings, expanding glob patterns
2. For each file: record path, size, mode, ownership, type
3. Sum total uncompressed size
4. Estimate compressed size (sampling-based heuristic or quick zstd level-1 pass)
5. Check against format limits
6. If within limits → single package
7. If exceeds limits → invoke splitter
```

### 4.2 Format Limits Reference

These are the hard constraints the planner must enforce:

**DEB limits:**
- ar member size: 9,999,999,999 bytes (~9,536 MiB) — the ar header's size field is 10 ASCII decimal digits
- Individual tar entry (v7/ustar): 8 GiB — 11 octal digits
- Individual tar entry (GNU extended): effectively unlimited (95-bit binary)
- dpkg support for GNU large file metadata since dpkg 1.18.24
- dpkg support for zstd compression since dpkg 1.21.18

**RPM limits:**
- Packages over 4 GB: requires rpm >= 4.6 (RPMTAG_LONGSIZE, RPMTAG_LONGARCHIVESIZE)
- Individual files over 4 GB: requires rpm >= 4.12 (uses `07070X` extended cpio)
- 64-bit integer header tags required for large values
- zstd compression: requires rpm >= 4.14.0 (RHEL 9+, Fedora 31+)

### 4.3 Split Strategies

#### `auto` — Format-Aware Splitting

The default. spm estimates the compressed payload size and splits if it would exceed 80% of the target format's hard limit. The 20% safety margin (`AUTO_SPLIT_HEADROOM = 0.80`) accounts for the fact that compression ratio estimates can vary ±20% depending on file content.

The split algorithm produces **even-sized parts** rather than greedily filling each part to the maximum:

1. Estimate compressed size: `estimated = total_uncompressed × compression_ratio`
2. Calculate safe limit: `safe_limit = format_limit × 0.80`
3. If `estimated > safe_limit`, compute number of parts: `num_parts = ceil(estimated / safe_limit)`
4. Divide total uncompressed size evenly: `max_per_part = total_uncompressed / num_parts`
5. Sort files by directory path (keeps related files together)
6. Distribute files into parts using the even per-part limit
7. Inject parent directory entries into each part — every part must contain the directory entries for all ancestor paths of its files (e.g., if a part contains `/opt/app/bin/tool`, it must also contain entries for `/opt/app/bin/`, `/opt/app/`, and `/opt/`)
8. Generate a meta-package that depends on all parts

**Borderline warnings:** When `estimated` exceeds the safety threshold but is still under the raw format limit, the plan output warns that splitting was triggered by the safety margin. When a package is not split but estimated size exceeds 60% of the limit, a warning suggests enabling splitting for safety.

**Generated package structure (DEB example):**

```
matlab-2025a_2025a-1_amd64.deb          # meta-package (depends on all parts, ~1KB)
matlab-2025a-part1_2025a-1_amd64.deb    # /opt/matlab/R2025a/bin/...
matlab-2025a-part2_2025a-1_amd64.deb    # /opt/matlab/R2025a/toolbox/...
matlab-2025a-part3_2025a-1_amd64.deb    # /opt/matlab/R2025a/extern/...
```

**Generated package structure (RPM example):**

```
matlab-2025a-1.x86_64.rpm               # meta-package (Requires all parts)
matlab-2025a-part1-1.x86_64.rpm         # /opt/matlab/R2025a/bin/...
matlab-2025a-part2-1.x86_64.rpm         # /opt/matlab/R2025a/toolbox/...
matlab-2025a-part3-1.x86_64.rpm         # /opt/matlab/R2025a/extern/...
```

The meta-package:
- Contains no files (or just a small metadata/version marker file)
- Has `Depends:` / `Requires:` on all part packages with exact version pins
- The user installs `matlab-2025a` and the package manager pulls in all parts
- Scripts (pre/post install) go in the meta-package, NOT the parts
- `Provides: matlab = 2025a` goes on the meta-package

Each part package:
- Named `{name}-part{N}`
- Contains a subset of the files
- Has `Depends: matlab-2025a = 2025a-1` (circular with meta) OR
  uses the pattern where parts have no deps and only the meta depends on them
- Version-locked to the meta-package

#### `size` — Explicit Size Limit

User specifies a maximum per-package size. Useful when you know your repo or transport has size constraints even if the format doesn't. The `max_size` field is **required** when using this strategy — config validation rejects `strategy: size` without it.

```yaml
splitting:
  strategy: size
  max_size: 4GiB    # supports B, KiB, MiB, GiB, TiB (required)
```

The `max_size` value is validated at config load time (not deferred to planning). Invalid size strings produce a clear config validation error.

#### `directory` — Split by Path Boundaries

User specifies directory boundaries for splitting. Each boundary becomes a separate sub-package. The `parts` list is **required** and must be non-empty when using this strategy.

Files that don't match any configured part go into an automatically generated remainder part. Configured parts whose paths match no files are silently filtered out (no empty packages are produced).

Each part receives ancestor directory entries for all of its files — e.g., a part containing `/opt/app/bin/tool` will also include directory entries for `/opt/`, `/opt/app/`, and `/opt/app/bin/`, even if those directories weren't explicitly matched by the part's path prefixes.

```yaml
splitting:
  strategy: directory
  parts:
    - name: matlab-core
      paths:
        - /opt/matlab/R2025a/bin
        - /opt/matlab/R2025a/sys

    - name: matlab-toolboxes
      paths:
        - /opt/matlab/R2025a/toolbox

    - name: matlab-docs
      paths:
        - /opt/matlab/R2025a/help
```

This gives the operator full control, similar to how Debian maintainers manually split packages.

---

## 5. RPM Backend

### 5.1 Package Structure

spm generates RPM V4 format packages with the following structure:

```
┌──────────────────────┐
│ Lead (96 bytes)      │  Magic: 0xEDABEEDB
├──────────────────────┤
│ Signature Header     │  MD5, SHA256, size, PGP signature
├──────────────────────┤
│ Header               │  All metadata tags
├──────────────────────┤
│ Payload              │  Compressed cpio archive
└──────────────────────┘
```

### 5.2 CPIO Payload — Standard and Extended

**Standard SVR4 (magic `070701`):**
Used when all files are < 4 GiB. Header fields are 8-digit hex (32-bit).

```
"070701"           magic (6 bytes)
c_ino              inode number
c_mode             file mode
c_uid              user ID
c_gid              group ID
c_nlink            number of links
c_mtime            modification time
c_filesize         file size (8 hex digits, max 0xFFFFFFFF = 4 GiB)
c_devmajor         device major
c_devminor         device minor
c_rdevmajor        rdev major
c_rdevminor        rdev minor
c_namesize         filename length
c_check            CRC checksum
```

**Extended format (magic `07070X`):**
Used when any file exceeds 4 GiB. This is RPM's custom stripped-down cpio format (not standard cpio — `rpm2cpio` cannot extract these, but `rpm` itself can).

In this format:
- Magic is `07070X` instead of `070701`
- The filename field contains only the **index number** (0-based) of the file in the RPM header, as an 8-byte hex string
- All other file metadata (name, size, mode, etc.) is read from the RPM header tags
- File sizes use `RPMTAG_LONGFILESIZES` (64-bit) in the header

**spm behavior:** Use standard `070701` by default. Automatically switch to `07070X` if any file exceeds 4 GiB. This matches what rpm >= 4.12 does natively.

### 5.3 RPM Header Tags for Large Files

When building packages with large files, spm must use the 64-bit tag variants:

| Tag | Type | Purpose |
|-----|------|---------|
| `RPMTAG_LONGFILESIZES` (5008) | INT64 array | Per-file sizes (64-bit) |
| `RPMTAG_LONGSIZE` (5009) | INT64 | Total installed size |
| `RPMTAG_LONGSIGSIZE` (270) | INT64 | Header + compressed payload size |
| `RPMTAG_LONGARCHIVESIZE` (271) | INT64 | Uncompressed payload size |

For backward compatibility, when values fit in 32 bits, spm also writes the traditional 32-bit tags (`RPMTAG_FILESIZES`, `RPMTAG_SIZE`, etc.).

### 5.4 RPM Compression

Supported payload compression identifiers (stored in `RPMTAG_PAYLOADCOMPRESSOR`):

| Algorithm | Tag Value | Min RPM Version | Notes |
|-----------|----------|-----------------|-------|
| gzip | `"gzip"` | All | Universally compatible |
| xz | `"xz"` | 4.7.1+ | Good ratio, slow compression |
| zstd | `"zstd"` | 4.14.0+ | Best speed/ratio tradeoff, multi-threaded |
| lzma | `"lzma"` | 4.4.6+ | Legacy, prefer xz |
| none | `"identity"` | All | No compression |

**Default:** `zstd` (with a warning/error if the user targets RHEL 8 or older).

### 5.5 Signing

spm supports RPM V4 PGP signatures:
- Header-only signature (`RPMSIGTAG_RSA` / `RPMSIGTAG_DSA`)
- Header+payload signature (`RPMSIGTAG_PGP` / `RPMSIGTAG_GPG`)
- SHA256 digest of header (`RPMSIGTAG_SHA256`)
- MD5 digest of header+payload (`RPMSIGTAG_MD5`)

Implementation: Use the `sequoia-openpgp` or `pgp` crate for PGP operations.

---

## 6. DEB Backend

### 6.1 Package Structure

A `.deb` file is an `ar` archive containing exactly three members in order:

```
!<arch>\n                          ar magic (8 bytes)
┌──────────────────────────────┐
│ debian-binary                │  "2.0\n"
├──────────────────────────────┤
│ control.tar.zst              │  Package metadata
│   ├── control                │  Name, version, deps, description
│   ├── md5sums                │  File checksums
│   ├── conffiles              │  Config file list
│   ├── preinst                │  Pre-install script
│   ├── postinst               │  Post-install script
│   ├── prerm                  │  Pre-remove script
│   └── postrm                 │  Post-remove script
├──────────────────────────────┤
│ data.tar.zst                 │  File payload
│   └── ./opt/matlab/...       │  Actual files
└──────────────────────────────┘
```

### 6.2 ar Format Constraints and Mitigations

The ar member header is fixed-format ASCII:

```
char ar_name[16]    file name
char ar_date[12]    modification time (decimal seconds)
char ar_uid[6]      user ID (decimal)
char ar_gid[6]      group ID (decimal)
char ar_mode[8]     file mode (octal)
char ar_size[10]    file size (decimal) ← THIS IS THE BOTTLENECK
char ar_fmag[2]     "`\n"
```

The 10-digit decimal `ar_size` field limits each member to 9,999,999,999 bytes (~9,536 MiB).

**spm mitigation:** The ar writer validates member sizes at write time and rejects any member exceeding 9,999,999,999 bytes with a clear error message, preventing silent archive corruption. When the estimated compressed `data.tar.*` would exceed 80% of this limit (~8 GiB), spm activates auto-splitting. Unlike proposals to change the ar format (which would break existing dpkg versions), splitting produces multiple standard `.deb` packages that work with any dpkg.

### 6.3 Tar Entry Size Constraints

Inside `data.tar`, individual file entries have their own size limits:

| Tar Format | Max Entry Size | Notes |
|------------|---------------|-------|
| v7/ustar | ~8 GiB | 11 octal digits |
| GNU extended | ~32 ZiB | 95-bit binary encoding |
| POSIX PAX | Unlimited | Arbitrary-precision decimal |

**spm behavior:** Use GNU tar format by default (which dpkg has supported since 1.18.24 for large file metadata). This allows individual files up to 32 ZiB, which is more than sufficient. If targeting very old dpkg versions, fall back to ustar and require splitting at 8 GiB per file.

### 6.4 DEB Compression

The `control.tar` and `data.tar` members can use different compression:

| Algorithm | Extension | Min dpkg Version | Notes |
|-----------|----------|-------------------|-------|
| gzip | `.gz` | All | Universal |
| xz | `.xz` | 1.15.6+ | Good for RHEL/Ubuntu LTS |
| zstd | `.zst` | 1.21.18+ (Debian), 1.19.0.5ubuntu2+ (Ubuntu) | Fast, multi-threaded |
| bzip2 | `.bz2` | 1.10.24+ | Legacy |
| none | (no ext) | 1.10.24+ | No compression |

**Default:** `zstd` for data.tar, `zstd` for control.tar. Emit a warning if the user targets Ubuntu 20.04 or older (dpkg < 1.19.0.5ubuntu2 doesn't support zstd).

### 6.5 Control File Generation

spm generates the `control` file from the YAML config:

```
Package: matlab-2025a
Version: 2025a-1
Architecture: amd64
Maintainer: HPC Team <hpc-help@tamu.edu>
Installed-Size: 31457280
Section: science
Priority: optional
Description: MATLAB R2025a - Technical Computing Environment
 MATLAB combines a desktop environment tuned for iterative analysis
 and design processes with a programming language that expresses
 matrix and array mathematics directly.
Depends: libx11-6, libxext6, libasound2 (>= 1.1), libgtk-3-0, libgl1-mesa-glx
Homepage: https://www.mathworks.com/products/matlab.html
```

### 6.6 Signing

DEB package signing options:
- `dpkg-sig` style (additional ar member with detached PGP signature)
- `debsigs` style (separate signature ar members)
- Repository-level signing (generate `.changes` / `.buildinfo` files for reprepro/aptly)

For v1, focus on generating packages compatible with repository signing (most common enterprise pattern). Package-level signing can be a v2 feature.

---

## 7. Alternatives Integration

Neither RPM nor DEB has a declarative metadata field for `update-alternatives`. Every package that uses alternatives (java, python, editor, gcc, etc.) just injects raw shell commands into its install/remove scripts. This is boilerplate-heavy and error-prone — people forget the `--remove` in prerm, mess up follower links, or use the wrong script phase.

spm makes this declarative. The `content.alternatives` YAML block generates the correct shell snippets for both formats.

### 7.1 How It Works

From the YAML config:

```yaml
content:
  alternatives:
    - name: matlab
      link: /usr/bin/matlab
      path: /opt/matlab/R2025a/bin/matlab
      priority: 2025
      followers:
        - name: mex
          link: /usr/bin/mex
          path: /opt/matlab/R2025a/bin/mex
```

spm generates and injects the following scriptlets:

**Post-install** (prepended before user's `post_install` script):

```bash
# [spm:alternatives] Auto-generated — do not edit
update-alternatives \
  --install '/usr/bin/matlab' 'matlab' '/opt/matlab/R2025a/bin/matlab' 2025 \
  --slave '/usr/bin/mex' 'mex' '/opt/matlab/R2025a/bin/mex'
```

All path and name arguments are single-quoted using POSIX shell escaping (embedded `'` is escaped as `'\''`) to prevent shell injection or breakage from paths containing spaces or metacharacters.

**Pre-remove** (appended after user's `pre_remove` script):

```bash
# [spm:alternatives] Auto-generated — do not edit
if [ "$1" = "0" ] || [ "$1" = "remove" ]; then
  update-alternatives --remove 'matlab' '/opt/matlab/R2025a/bin/matlab'
fi
```

The `$1` guard is important:
- **RPM:** `$1` is the number of remaining package instances. `0` means full removal; `1` means upgrade (don't remove the alternative during upgrade, the new version's postinst will re-register it).
- **DEB:** `$1` is the action string. `"remove"` means removal; `"upgrade"` means upgrade.

spm generates the correct guard syntax per format.

### 7.2 Scriptlet Ordering

When a package has both alternatives and user-provided scripts, the ordering is:

```
┌────────────────────────────────────────────┐
│ postinst / %post                           │
│  1. [spm] alternatives --install ...  │
│  2. [user]     scripts.post_install        │
├────────────────────────────────────────────┤
│ prerm / %preun                             │
│  1. [user]     scripts.pre_remove          │
│  2. [spm] alternatives --remove ...   │
└────────────────────────────────────────────┘
```

Rationale: alternatives are registered first so that user scripts can assume the symlinks exist. On removal, user scripts run first while the alternatives are still in place.

### 7.3 Auto-Injected Dependencies

spm automatically adds the alternatives tool as a package dependency when `content.alternatives` is configured:

| Distro | Package Dependency | Notes |
|--------|-------------------|-------|
| RHEL/EL 8 | `chkconfig` | `alternatives` is provided by `chkconfig` |
| RHEL/EL 9+ / Fedora | `alternatives` | Standalone package since EL9 |
| Debian / Ubuntu | (none needed) | `update-alternatives` ships with `dpkg` |

For RPM, spm detects the right dependency from `--target-distro`. If no target distro is specified, it adds `Requires: /usr/sbin/alternatives` (path-based dependency, works on both EL8 and EL9).

### 7.4 Multi-Version Example

This is the real payoff. Two spm configs for side-by-side MATLAB installs:

**matlab-2024b.yaml:**
```yaml
package:
  name: matlab-2024b
  version: "2024b"
content:
  files:
    - src: "/opt/matlab-staging/R2024b/**"
      dst: /opt/matlab/R2024b/
  alternatives:
    - name: matlab
      link: /usr/bin/matlab
      path: /opt/matlab/R2024b/bin/matlab
      priority: 2024          # lower priority — not preferred
```

**matlab-2025a.yaml:**
```yaml
package:
  name: matlab-2025a
  version: "2025a"
content:
  files:
    - src: "/opt/matlab-staging/R2025a/**"
      dst: /opt/matlab/R2025a/
  alternatives:
    - name: matlab
      link: /usr/bin/matlab
      path: /opt/matlab/R2025a/bin/matlab
      priority: 2025          # higher priority — auto-selected
```

After installing both:

```bash
$ update-alternatives --display matlab
matlab - auto mode
  link best version is /opt/matlab/R2025a/bin/matlab
  link currently points to /opt/matlab/R2025a/bin/matlab
  link matlab is /usr/bin/matlab
/opt/matlab/R2024b/bin/matlab - priority 2024
/opt/matlab/R2025a/bin/matlab - priority 2025

$ update-alternatives --config matlab
There are 2 choices for the alternative matlab:

    Selection    Path                             Priority   Status
------------------------------------------------------------
  * 0            /opt/matlab/R2025a/bin/matlab     2025      auto mode
    1            /opt/matlab/R2024b/bin/matlab     2024      manual mode
    2            /opt/matlab/R2025a/bin/matlab     2025      manual mode

Press <enter> to keep the current choice[*], or type selection number:
```

### 7.5 Config Reference

```yaml
content:
  alternatives:
    - name: <string>          # REQUIRED — alternatives group name
      link: <path>            # REQUIRED — the generic symlink path
      path: <path>            # REQUIRED — this package's real binary/file path
      priority: <integer>     # REQUIRED — higher number = preferred
      followers:              # OPTIONAL — secondary links that switch together
        - name: <string>      #   follower alternative name
          link: <path>        #   follower symlink path
          path: <path>        #   follower real path
```

Note: DEB and older RHEL documentation uses the term "slave" instead of "follower." The `update-alternatives` command itself still uses `--slave` as the flag. spm uses "follower" in the config (the modern terminology) and generates the correct `--slave` flag in the scriptlets for compatibility with all distro versions.

---

## 8. Compression Engine

The compression engine is a shared abstraction used by both backends.

### 8.1 Trait Definition

```rust
pub trait Compressor: Send {
    /// Compress from reader to writer, returning bytes written
    fn compress(&self, reader: &mut dyn Read, writer: &mut dyn Write) -> Result<u64>;

    /// File extension for this compression (e.g., "zst", "xz", "gz")
    fn extension(&self) -> &str;

    /// Identifier string for RPM PAYLOADCOMPRESSOR tag
    fn rpm_identifier(&self) -> &str;

    /// Estimated compression ratio (for planning, 0.0-1.0)
    fn estimated_ratio(&self) -> f64;
}
```

### 8.2 Implementation Notes

| Algorithm | Rust Crate | Multi-threading | Notes |
|-----------|-----------|-----------------|-------|
| zstd | `zstd` (bindings to libzstd) | Native (`zstd::stream::Encoder::set_parameter(CParameter::NbWorkers(N))`) | Preferred default |
| xz | `xz2` or `liblzma-sys` | Via `liblzma` threaded mode | Slower but great ratio |
| gzip | `flate2` with `pigz` backend, or `gzip-encoder` | Via `pigz` or parallel flate2 | Universal compat |
| none | passthrough | N/A | For debugging / custom pipelines |

**Thread count logic:**
```
threads = 0 → num_cpus::get()  (auto)
threads = N → N
```

### 8.3 Streaming Architecture

For large payloads, spm must stream compression — it cannot buffer a 30 GB payload in memory.

```
FileTree Walker
    │
    ▼
cpio/tar Writer (streaming) ──► Compressor (streaming) ──► ar/RPM Writer (streaming)
```

Each stage reads from the previous via `Read`/`Write` trait implementations, potentially using `std::io::copy` with a fixed-size buffer (e.g., 256 KiB).

---

## 9. CLI Interface

```
spm — Large-file-aware Linux package builder

USAGE:
    spm [OPTIONS] <COMMAND>

COMMANDS:
    build       Build package(s) from config
    plan        Show what would be built (dry run)
    inspect     Show metadata for an existing package
    init        Create a template spm.yaml
    validate    Validate a spm.yaml without building

GLOBAL OPTIONS:
    -c, --config <FILE>      Config file path [default: spm.yaml]
    -q, --quiet              Suppress non-error output
    --color <WHEN>           Color output: auto, always, never [default: auto]
```

### `spm build`

```
spm build [OPTIONS]

OPTIONS:
    -f, --format <FORMAT>       Output format: rpm, deb, all [default: all]
    -o, --output <DIR>          Output directory [default: ./out]
    --no-sign                   Skip signing even if key is configured
    --compression <ALG>         Override compression algorithm
    --compression-level <N>     Override compression level
    --threads <N>               Override thread count
    --target-distro <DISTRO>    Target distro for compatibility checks
                                (e.g., "el8", "el9", "ubuntu2004", "ubuntu2204")
    --no-split                  Disable auto-splitting (fail on oversized packages)
    --source-date-epoch <N>     Set timestamp for reproducible builds
```

### `spm plan`

Dry-run mode. Shows:
- Total uncompressed size
- Estimated compressed size per format
- Whether splitting will occur, and how
- Which cpio format will be used (standard vs extended)
- Compatibility warnings (e.g., "zstd requires rpm >= 4.14.0")

```
$ spm plan --format rpm

Package: matlab-2025a-1.x86_64
  Source: /opt/matlab-staging/R2025a
  Files: 142,847
  Uncompressed: 31.4 GiB
  Estimated compressed (zstd -19): ~11.2 GiB

  RPM payload format: 07070X (extended cpio, files > 4 GiB detected)
  Splitting: NOT REQUIRED (RPM supports packages > 4 GiB with rpm >= 4.6)
  Minimum rpm version: 4.14.0 (zstd compression + large files)

  Output: out/matlab-2025a-1.x86_64.rpm

$ spm plan --format deb

Package: matlab-2025a_2025a-1_amd64
  Source: /opt/matlab-staging/R2025a
  Files: 142,847
  Uncompressed: 31.4 GiB
  Estimated compressed (zstd -19): ~11.2 GiB

  ⚠ SPLIT REQUIRED: compressed payload exceeds DEB ar limit (~9.3 GiB)
  Split plan:
    matlab-2025a_2025a-1_amd64.deb          (meta-package, ~2 KiB)
    matlab-2025a-part1_2025a-1_amd64.deb    (~8.9 GiB, 67,423 files)
    matlab-2025a-part2_2025a-1_amd64.deb    (~2.3 GiB, 75,424 files)
  Minimum dpkg version: 1.21.18 (zstd compression)

  Output: out/matlab-2025a_2025a-1_amd64.deb (+ 2 part packages)
```

---

## 10. Target Distro Compatibility

To help users avoid building packages that won't install on their target systems, spm maintains a compatibility matrix:

```yaml
# Internal compatibility database (compiled in, not user-facing YAML)
distro:
  el8:
    rpm_version: "4.14.3"
    supports_zstd: true
    supports_large_files: true   # rpm >= 4.12
    max_rpm_payload_compressors: [gzip, xz, zstd]

  el9:
    rpm_version: "4.16.1"
    supports_zstd: true
    supports_large_files: true

  ubuntu2004:
    dpkg_version: "1.19.7ubuntu3"
    supports_zstd_deb: true       # Ubuntu backported zstd support
    supports_gnu_tar_large: true

  ubuntu2204:
    dpkg_version: "1.21.1ubuntu2.3"
    supports_zstd_deb: true
    supports_gnu_tar_large: true

  ubuntu2404:
    dpkg_version: "1.22.6ubuntu6"
    supports_zstd_deb: true
    supports_gnu_tar_large: true
```

Usage:
```bash
# Warns if config uses features incompatible with RHEL 8
spm build --format rpm --target-distro el8
```

---

## 11. Rust Crate Ecosystem

### 11.1 Existing Crates to Evaluate

| Crate | Purpose | Notes |
|-------|---------|-------|
| `rpm` (v0.18) | RPM building/parsing | Pure Rust, active. May not support 07070X yet — evaluate or fork. |
| `ar` | ar archive creation | Simple, may need extension for edge cases |
| `tar` | tar archive creation | Supports GNU format, large files |
| `zstd` | zstd compression | Bindings to libzstd, multi-threading support |
| `xz2` | xz/lzma compression | Bindings to liblzma |
| `flate2` | gzip compression | Pure Rust or libz backend |
| `sequoia-openpgp` | PGP signing | Full OpenPGP implementation |
| `clap` | CLI parsing | Standard |
| `serde_yaml` | YAML parsing | Standard, mature |
| `walkdir` | Directory traversal | Standard |
| `indicatif` | Progress bars | For large builds |
| `num_cpus` | CPU detection | For auto-threading |

### 11.2 What Needs Custom Implementation

1. **cpio writer with 07070X support** — No existing Rust crate supports RPM's custom extended cpio format. The `spm-cpio` crate must implement:
   - Standard SVR4 newc (`070701`) writer
   - RPM extended (`07070X`) writer
   - Both must handle hardlink grouping correctly

2. **DEB ar writer with exact format compliance** — The `ar` crate may work, but `.deb` files have specific requirements (member ordering, no long filename extensions, exact padding).

3. **RPM header structure** — The `rpm` crate handles this well. Evaluate whether to use it as a dependency or implement directly for more control over large-file tag handling.

---

## 12. Testing Strategy

### 12.1 Unit Tests

- Config parsing (YAML → internal structs)
- Size calculation and split planning
- cpio header generation (both formats)
- ar header generation
- RPM header tag encoding (32-bit and 64-bit)
- Compression round-trip

### 12.2 Integration Tests

**Requires:** `rpm`, `dpkg-deb`, `dpkg` available in CI.

```bash
# Build a test package and verify with native tools
spm build --format rpm -o /tmp/test/
rpm -qpl /tmp/test/package.rpm              # list files
rpm -K /tmp/test/package.rpm                # verify signatures
rpm --checksig /tmp/test/package.rpm        # verify checksums

spm build --format deb -o /tmp/test/
dpkg-deb -c /tmp/test/package.deb           # list contents
dpkg-deb -I /tmp/test/package.deb           # show control info
```

### 12.3 Large File Tests

- Generate a sparse file > 4 GiB, package as RPM, verify `07070X` format is used
- Generate a payload > 9.5 GiB compressed, package as DEB, verify auto-split occurs
- Install split DEB packages on a test system, verify all files present
- Verify packages install cleanly on target distro containers (RHEL 8/9, Ubuntu 20.04/22.04/24.04)

### 12.4 Alternatives Tests

- Build two versions of a package with alternatives, install both, verify `update-alternatives --display` shows both
- Remove one version, verify the other becomes active
- Verify upgrade (install new version, remove old) preserves the alternative registration
- Verify auto-injected dependency on alternatives tool is present in package metadata
- Verify correct `$1` guard in prerm (no removal during upgrade)

### 12.5 CI Matrix

Run integration tests in containers for each target distro:

```yaml
strategy:
  matrix:
    distro: [el8, el9, ubuntu2004, ubuntu2204, ubuntu2404, fedora40]
```

---

## 13. Open Questions & Future Work

### Open Questions for v1

1. **Split naming convention:** `{name}-part{N}` vs `{name}-data{N}` vs `{name}-{subname}` — what feels most natural for enterprise package management?

2. **RPM cpio format selection:** Always use `07070X` (simpler code, but breaks `rpm2cpio` for all packages) vs auto-detect (more complex, better compat)? The recommendation is auto-detect, matching RPM's own behavior.

3. **Dependency version syntax:** Should spm accept a universal syntax and translate per-format, or require format-specific dependency strings? The spec above proposes universal with format-specific overrides.

4. **DEB splitting and conffiles:** When splitting, config files should always go in the meta-package (or a specific part). Need to define this clearly.

5. ~~**Hardlinks across split boundaries:**~~ **Resolved.** `fixup_hardlinks_across_parts()` detects hardlinks whose target is in a different part and promotes them to regular files with their actual size. The part's `total_size` is adjusted upward accordingly (may exceed the original split target, which is acceptable — the alternative is a broken package).

6. **Alternatives in split packages:** When auto-splitting is active, the alternatives scriptlets should be injected into the **meta-package** (not the parts), since the meta-package is what the user installs/removes. The alternative's `path` must point to a file that lives in one of the parts, so the meta-package's dependency on that part ensures the binary exists. Need to validate this ordering works correctly with both dpkg and rpm transaction ordering.

### Future Work (v2+)

- **Additional formats:** Alpine APK, Arch pacman pkg.tar.zst
- **Package conversion:** RPM ↔ DEB (like alien, but correct)
- **Repository generation:** Built-in createrepo_c / apt repository metadata generation
- **Differential updates:** Delta packages for version upgrades
- **FIPS-compliant signing:** Integration with PKCS#11 tokens / HSMs
- **Remote signing:** Sign packages via a remote signing service (useful for CI/CD pipelines)
- **Content-addressable splitting:** Split by file hash to enable deduplication across versions
- **OCI/Docker input:** Extract filesystem layers from container images and package them
- **Lua/Python scriptlet support:** For RPM scriptlets that use embedded interpreters

---

## 14. Reference: Prior Art Comparison

| Feature | fpm | nfpm | spm |
|---------|-----|------|----------|
| Language | Ruby | Go | Rust |
| RPM creation | Via rpmbuild or ruby-rpm | Pure Go | Pure Rust |
| DEB creation | Via dpkg-deb | Pure Go | Pure Rust |
| Config format | CLI flags only | YAML | YAML + CLI |
| Large file support (>4GB per file) | No | No | Yes (07070X cpio) |
| Large package support (>9GB deb) | No (fails) | No (fails) | Yes (auto-split) |
| Auto-split | No | No | Yes |
| Multi-threaded compression | No | No | Yes (zstd native) |
| Compression options | gzip, xz (via system tools) | gzip, zstd, xz | gzip, zstd, xz, none |
| Target distro validation | No | No | Yes |
| Reproducible builds | Partial | Yes (SOURCE_DATE_EPOCH) | Yes |
| Signing | Via external tools | Built-in PGP | Built-in PGP |
| Alternatives support | No | No | Yes (declarative YAML → scriptlet injection) |
| Library usage | No (CLI only) | Yes (Go library) | Yes (Rust crate) |
| Format conversion | Yes (many→many) | No | No (v1) |

---

## Appendix A: Example Workflow

```bash
# 1. Stage the software
mkdir -p /opt/matlab-staging/R2025a
# ... install/extract MATLAB here ...

# 2. Create config
spm init --name matlab --version 2025a

# 3. Edit spm.yaml (set file mappings, deps, etc.)
vim spm.yaml

# 4. Preview what will be built
spm plan

# 5. Build for both formats
spm build --format all --output ./packages/

# 6. Upload to your repo
createrepo_c ./packages/rpm/
reprepro includedeb jammy ./packages/deb/*.deb
```

## Appendix B: Minimum Viable Product (MVP) Scope

For the initial release, focus on:

1. ✅ YAML config parsing
2. ✅ Directory tree walking and size calculation
3. ✅ RPM generation with standard cpio (`070701`)
4. ✅ RPM generation with extended cpio (`07070X`) for large files
5. ✅ DEB generation with standard ar container
6. ✅ Auto-split for DEB when payload exceeds ar limits
7. ✅ zstd and gzip compression (multi-threaded zstd)
8. ✅ Basic CLI (build, plan, init)
9. ✅ `--target-distro` compatibility warnings
10. ✅ Declarative `update-alternatives` support (scriptlet injection)
11. ✅ PGP signing (optional — most repo tools like createrepo_c and reprepro handle this already, but built-in support is straightforward with `sequoia-openpgp` or `pgp` crate)

**Signing note:** Since tools like createrepo_c (RPM repos) and reprepro/aptly (DEB repos) handle repository-level signing, built-in package signing is a convenience rather than a hard requirement. The `sequoia-openpgp` crate makes RPM package signing relatively painless (~200-300 lines for header+payload signatures). DEB package-level signing is less standardized and lower priority — repo signing covers that use case. Recommendation: ship RPM signing in v1, defer DEB package signing.