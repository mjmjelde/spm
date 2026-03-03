# Configuration Reference

spm (Simple Package Manager) reads its configuration from a YAML file (default: `spm.yaml` in the current directory).

To generate a commented template with all available sections:

```bash
spm init --name myapp --version 1.0.0
```

## Environment Variable Expansion

All `${VAR}` references in the YAML file are expanded before parsing. If a referenced variable is not set, spm exits with an error (no silent empty-string fallback).

```yaml
package:
  version: "${CI_VERSION}"

signing:
  key_file: ${SPM_SIGNING_KEY}
```

## CLI Override Priority

Several settings can be overridden via CLI flags. When the same setting is specified in multiple places, the highest-priority source wins:

1. CLI flag (highest)
2. Environment variable (for `SOURCE_DATE_EPOCH`)
3. Config file value (lowest)

---

## `package`

Package identity and metadata. All fields except `url`, `vendor`, and `dependencies` are required.

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
    requires:
      - libX11
      - "libasound2 >= 1.1"
    requires_rpm:
      - mesa-libGL
    requires_deb:
      - libgl1-mesa-glx
    conflicts: []
    provides:
      - "matlab = 2025a"
    replaces: []
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | String | Yes | | Package name. Allowed characters: alphanumeric, `-`, `.`, `_`, `+`. Must start with alphanumeric. |
| `version` | String | Yes | | Package version. Must start with a digit. No spaces or colons. |
| `release` | String | No | `"1"` | Release number. |
| `arch` | String | Yes | | Target architecture: `x86_64`, `aarch64`, `i686`, `armv7hl`, `noarch`, `all`. |
| `license` | String | Yes | | License identifier (e.g. `MIT`, `Proprietary`, `GPL-3.0`). |
| `maintainer` | String | Yes | | Maintainer name and email. |
| `description` | String | Yes | | Short package description. |
| `url` | String | No | | Project URL. |
| `vendor` | String | No | | Vendor name. |

**Architecture translation for DEB:** `x86_64` becomes `amd64`, `aarch64` becomes `arm64`, `noarch` and `all` become `all`.

### `package.dependencies`

All dependency fields default to empty lists.

| Field | Type | Description |
|-------|------|-------------|
| `requires` | List | Common dependencies included in both RPM and DEB builds. |
| `requires_rpm` | List | RPM-only dependencies (merged with `requires` for RPM builds). |
| `requires_deb` | List | DEB-only dependencies (merged with `requires` for DEB builds). |
| `conflicts` | List | Conflicting packages. |
| `provides` | List | Virtual packages this package provides. |
| `replaces` | List | Packages this replaces. |

Version constraints use the format `"pkgname >= version"` and are passed through to the target format (RPM `Requires:` / DEB `Depends:`).

---

## `content`

File mappings, symlinks, directories, and update-alternatives entries.

```yaml
content:
  defaults:
    user: root
    group: root
    file_mode: "0644"
    dir_mode: "0755"

  files:
    - src: "/opt/staging/myapp/**"
      dst: /opt/myapp/
    - src: myapp.conf
      dst: /etc/myapp/myapp.conf
      type: config

  symlinks:
    - src: /opt/myapp/bin/myapp
      dst: /usr/local/bin/myapp

  alternatives:
    - name: myapp
      link: /usr/bin/myapp
      path: /opt/myapp/bin/myapp
      priority: 100
      followers:
        - name: myapp-man
          link: /usr/share/man/man1/myapp.1
          path: /opt/myapp/share/man/man1/myapp.1

  directories:
    - path: /var/log/myapp
      mode: "0750"
      user: root
      group: myapp
```

### `content.defaults`

Global defaults applied to all file entries unless overridden per-mapping.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `user` | String | `"root"` | Default file owner. |
| `group` | String | `"root"` | Default file group. |
| `file_mode` | String | *None* | Default mode for regular files (e.g. `"0644"`). If omitted, source file permissions are preserved. |
| `dir_mode` | String | *None* | Default mode for directories (e.g. `"0755"`). If omitted, source directory permissions are preserved. |

**Mode resolution order** (first match wins):

1. Per-mapping override (`files[].mode`, `files[].dir_mode`)
2. Global defaults (`content.defaults.file_mode`, `content.defaults.dir_mode`)
3. Source file metadata from disk

### `content.files[]`

File mapping rules. Each entry maps source files to a destination inside the package.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `src` | String | Yes | Source path or glob pattern. |
| `dst` | String | Yes | Destination path inside the package. |
| `mode` | String | No | Override file mode for regular files. |
| `dir_mode` | String | No | Override mode for directories. |
| `user` | String | No | Override owner. |
| `group` | String | No | Override group. |
| `type` | String | No | Set to `"config"` to mark as RPM `%config(noreplace)` / DEB conffile. |

**Glob expansion rules:**

- Standard glob patterns are supported: `*`, `**`, `?`
- Bare directory paths (no glob characters) auto-expand to `dir/**`
- The path prefix before `**` is stripped and replaced with `dst`
- Example: `src: "/opt/staging/**"` with `dst: /opt/myapp/` maps `/opt/staging/bin/tool` to `/opt/myapp/bin/tool`
- Parent directories are implicitly created with proper ownership and mode

### `content.symlinks[]`

Static symlinks to include in the package.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `src` | String | Yes | Symlink target (what the link points to). Must be non-empty. |
| `dst` | String | Yes | Symlink path (where the link is created in the package). |

### `content.alternatives[]`

Declarative `update-alternatives` integration. spm auto-generates `update-alternatives --install` in `post_install` and guarded `update-alternatives --remove` in `post_remove` scripts.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Alternatives group name. |
| `link` | String | Yes | Generic symlink path managed by alternatives (e.g. `/usr/bin/python3`). |
| `path` | String | Yes | This package's real binary path. |
| `priority` | Integer | Yes | Priority value. Higher numbers are preferred. |
| `followers` | List | No | Secondary links that switch atomically with the primary. |

Each follower has the same `name`, `link`, `path` fields as the primary (no `priority`).

When alternatives are defined, spm appends the generated scriptlets to any user-provided scripts. User scripts run first, then the alternatives commands.

### `content.directories[]`

Directories to create with specific ownership and permissions.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `path` | String | Yes | Directory path. |
| `mode` | String | No | Directory mode. |
| `user` | String | No | Owner. |
| `group` | String | No | Group. |

---

## `scripts`

Optional install and remove hook scripts. All fields accept a file path, either absolute or relative to the config file's directory.

```yaml
scripts:
  pre_install: scripts/preinst.sh
  post_install: scripts/postinst.sh
  pre_remove: scripts/prerm.sh
  post_remove: scripts/postrm.sh
  pre_trans: scripts/pretrans.sh
  post_trans: scripts/posttrans.sh
```

| Field | Type | Description |
|-------|------|-------------|
| `pre_install` | Path | Runs before file installation. |
| `post_install` | Path | Runs after file installation. |
| `pre_remove` | Path | Runs before file removal. |
| `post_remove` | Path | Runs after file removal. |
| `pre_trans` | Path | RPM only. Runs before the transaction (`%pretrans`). Ignored for DEB. |
| `post_trans` | Path | RPM only. Runs after the transaction (`%posttrans`). Ignored for DEB. |

Script files must exist at validation time (`spm validate` checks this).

When `content.alternatives` is defined, spm appends auto-generated `update-alternatives` scriptlets to `post_install` and `post_remove`. User script content runs first.

---

## `compression`

Compression settings for the package payload.

```yaml
compression:
  algorithm: zstd
  level: 3
  threads: 0
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `algorithm` | String | `"zstd"` | One of: `zstd`, `gzip`, `xz`, `none`. |
| `level` | Integer | *Algorithm default* | Compression level (see table below). |
| `threads` | Integer | *None* | Thread count. `0` = auto-detect (all CPUs). |

**Algorithm details:**

| Algorithm | Default Level | Level Range | Multi-threaded | Estimated Compression Ratio |
|-----------|--------------|-------------|----------------|---------------------------|
| `zstd` | 3 | 1-22 | Yes | ~35% of original |
| `gzip` | 6 | 0-9 | No | ~40% of original |
| `xz` | 6 | 0-9 | Yes | ~30% of original |
| `none` | N/A | N/A | N/A | 100% (passthrough) |

Multi-threading applies to zstd and xz only. Gzip is always single-threaded.

**CLI overrides:** `--compression`, `--compression-level`, `--threads`.

---

## `splitting`

Package splitting configuration. When a single package would exceed format limits, spm can split it into multiple sub-packages with a meta-package that depends on all parts.

```yaml
splitting:
  enabled: true
  strategy: auto
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | Boolean | `true` | Whether splitting is enabled. |
| `strategy` | String | `"auto"` | One of: `auto`, `size`, `directory`. |
| `max_size` | String | | Required when `strategy: size`. Size threshold with suffix (e.g. `"8GiB"`, `"500MiB"`). |
| `parts` | List | | Required when `strategy: directory`. Named parts with path assignments. |

### Strategy: `auto`

Format-aware splitting. spm estimates the compressed output size and splits if it would exceed the target format's limits:

- **DEB:** Splits when estimated compressed size exceeds 80% of the ar member limit (~9.3 GiB). The actual split point is determined during streaming compression, not from estimates alone.
- **RPM:** The RPM format has no practical payload size limit, so auto-split rarely triggers for RPM.

The 80% safety margin accounts for compression ratio estimation variance.

### Strategy: `size`

Splits when total uncompressed size exceeds `max_size`. Requires the `max_size` field.

```yaml
splitting:
  strategy: size
  max_size: "4GiB"
```

Accepted size suffixes: `B`, `KiB`, `MiB`, `GiB`, `TiB`.

### Strategy: `directory`

User-defined split boundaries based on file paths. Requires a non-empty `parts` list.

```yaml
splitting:
  strategy: directory
  parts:
    - name: core
      paths:
        - /opt/myapp/bin
        - /opt/myapp/lib
    - name: data
      paths:
        - /opt/myapp/share
```

Each part specifies directory prefixes. Files not matching any part's paths are assigned to the last part. Empty parts (no matching files) are automatically filtered out.

### Split output

When splitting occurs, spm produces:

- A **meta-package** (`pkgname`) with no files, which depends on all parts
- **Part sub-packages** (`pkgname-part1`, `pkgname-part2`, ...) containing the actual files
- Install/remove scripts are placed on the meta-package, not the parts

**CLI override:** `--no-split` disables splitting entirely.

---

## `signing`

PGP signing configuration.

```yaml
signing:
  key_file: ${SPM_SIGNING_KEY}
  key_id: ABCD1234
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `key_file` | String | Yes | Path to PGP key file. Supports `${VAR}` expansion. |
| `key_id` | String | No | Specific subkey ID. |

> **Note:** Signing is not yet implemented. The configuration is parsed and validated but has no effect on builds.

---

## `rpm`

RPM-specific overrides.

```yaml
rpm:
  group: Development/Tools
  payload_format: cpio
  compression: xz
```

| Field | Type | Description |
|-------|------|-------------|
| `group` | String | RPM Group tag. Deprecated in modern RPM but still accepted. |
| `payload_format` | String | `"cpio"` (default, auto-selects extended 07070X when needed) or `"cpio-extended"` (force extended format). |
| `compression` | String | Override the global compression algorithm for RPM builds only. |

**Payload format auto-detection:** When any file in the package exceeds 4 GiB, spm automatically uses the RPM extended CPIO format (magic `07070X`) instead of the standard SVR4 newc format (`070701`). Set `payload_format: cpio-extended` to force extended format regardless of file sizes.

---

## `deb`

DEB-specific overrides.

```yaml
deb:
  section: science
  priority: optional
  fields:
    Bugs: https://example.com/bugs
  compression: gzip
```

| Field | Type | Description |
|-------|------|-------------|
| `section` | String | Debian section (e.g. `"science"`, `"utils"`, `"misc"`). |
| `priority` | String | Debian priority (e.g. `"optional"`, `"required"`). |
| `fields` | Map | Additional key-value pairs added to the DEB control file verbatim. |
| `compression` | String | Override the global compression algorithm for DEB builds only. |

The `fields` map is useful for adding Debian control fields like `Bugs`, `Vcs-Git`, `Vcs-Browser`, etc.

---

## `build`

Build reproducibility settings.

```yaml
build:
  source_date_epoch: "1700000000"
```

| Field | Type | Description |
|-------|------|-------------|
| `source_date_epoch` | String | Fixed Unix timestamp (seconds since epoch) for reproducible builds. All file timestamps in the package are set to this value. |

**Priority chain for `source_date_epoch`:**

1. CLI flag: `--source-date-epoch`
2. Environment variable: `SOURCE_DATE_EPOCH`
3. Config file: `build.source_date_epoch`

---

## CLI Override Summary

| CLI Flag | Config Path | Notes |
|----------|-------------|-------|
| `--compression` | `compression.algorithm` | Validated against known algorithms |
| `--compression-level` | `compression.level` | |
| `--threads` | `compression.threads` | |
| `--no-split` | `splitting.enabled = false` | |
| `--source-date-epoch` | `build.source_date_epoch` | Also reads `SOURCE_DATE_EPOCH` env var |
| `--target-distro` | *(not stored in config)* | Used for compatibility warnings only |
| `--format` | *(not stored in config)* | Selects `rpm`, `deb`, or `all` |
| `-o, --output` | *(not stored in config)* | Output directory (default: `./out`) |
